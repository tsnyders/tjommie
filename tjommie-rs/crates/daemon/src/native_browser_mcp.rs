//! `native-browser-mcp.ts`: `/mcp/browser`, a token-scoped MCP proxy for the 26 native browser tools. The tools run in
//! the desktop; here a bot's token is checked, arguments are validated, calls are queued one at a time per bot (a
//! dialog answer may skip the queue so it can unblock an action), and every result is bounded and checked before a
//! model sees it. Closing a bot's tool session bumps its epoch, so queued and late results answer "no longer
//! authorized".
//!
//! The tool list and input schemas are `tjommie-rs/protocol/browser-tools.json`, generated from the desktop's zod
//! definitions by `scripts/browser-tools-manifest.ts`; the rules zod's export drops are ported from
//! `validateNativeBrowserTool` below.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::future::join_all;
use serde_json::{Map, Value, json};

use crate::host_poll;
use crate::native_browser_broker::Authorized;
use crate::native_browser_handlers::NativeRequest;
use crate::server::Route;

const BODY_BYTES: usize = 1024 * 1024;
const MAX_BATCH: usize = 20;
const REPLY_BYTES: usize = 8 * 1024 * 1024;
const RESULT_BYTES: usize = 4 * 1024 * 1024;
const TEXT_CHARS: usize = 128 * 1024 + 100;
/// Base64 of 2 MiB.
const IMAGE_CHARS: usize = (2 * 1024 * 1024usize).div_ceil(3) * 4;

struct Tool {
    listed: Value,
    validator: jsonschema::Validator,
}

static TOOLS: LazyLock<Vec<(String, Tool)>> = LazyLock::new(|| {
    let listed: Vec<Value> =
        serde_json::from_str(include_str!("../../../protocol/browser-tools.json"))
            .expect("browser-tools.json is JSON");
    listed
        .into_iter()
        .map(|entry| {
            let name = entry["name"].as_str().expect("a tool has a name").to_owned();
            let validator = jsonschema::validator_for(&entry["argumentsSchema"]).expect("a tool's arguments schema compiles");
            let listed = json!({ "name": name, "description": entry["description"], "inputSchema": entry["inputSchema"] });
            (name, Tool { listed, validator })
        })
        .collect()
});

fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS
        .iter()
        .find(|(tool, _)| tool == name)
        .map(|(_, tool)| tool)
}

/// `target()`: one of target and ref, or both the same.
fn target(args: &Map<String, Value>, target: &str, reference: &str) -> Result<(), String> {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    match (text(target), text(reference)) {
        (Some(a), Some(b)) if a != b => {
            Err("Use one target or matching ref, not conflicting targets.".into())
        }
        (None, None) => Err("An exact snapshot ref or unique CSS target is required.".into()),
        _ => Ok(()),
    }
}

/// `validateNativeBrowserTool`: the schema, then the cross-field rules. Returns the arguments with schema defaults filled
/// in, as zod's parse leaves them.
pub fn validate_tool(name: &str, args: &Value) -> Result<Map<String, Value>, String> {
    let Some(definition) = tool(name) else {
        return Err(format!(
            "Unsupported native browser tool: {}.",
            name.chars().take(100).collect::<String>()
        ));
    };
    let issues: Vec<String> = definition
        .validator
        .iter_errors(args)
        .map(|error| {
            let path = error.instance_path().to_string();
            let path = if path.is_empty() {
                "arguments".to_owned()
            } else {
                path.trim_start_matches('/').replace('/', ".")
            };
            format!("{path}: {error}")
        })
        .collect();
    if !issues.is_empty() {
        return Err(format!(
            "Invalid arguments for {name}: {}",
            issues.join("; ")
        ));
    }
    let mut value = args.as_object().cloned().unwrap_or_default();
    if let Some(properties) = definition.listed["inputSchema"]["properties"].as_object() {
        for (key, schema) in properties {
            if let (None, Some(default)) = (value.get(key), schema.get("default")) {
                value.insert(key.clone(), default.clone());
            }
        }
    }
    if [
        "browser_click",
        "browser_hover",
        "browser_type",
        "browser_select_option",
        "browser_drop",
    ]
    .contains(&name)
    {
        target(&value, "target", "ref")?;
    }
    if name == "browser_drag" {
        target(&value, "startTarget", "startRef")?;
        target(&value, "endTarget", "endRef")?;
    }
    let present = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };
    if name == "browser_find" && present("text") == present("regex") {
        return Err("Supply text or regex, not both.".into());
    }
    if name == "browser_wait_for"
        && value.get("time").is_none()
        && !present("text")
        && !present("textGone")
    {
        return Err("Supply time, text or textGone.".into());
    }
    if name == "browser_tabs"
        && present("url")
        && value.get("action").and_then(Value::as_str) != Some("new")
    {
        return Err("A URL is supported only when creating a new tab.".into());
    }
    Ok(value)
}

/// `resultSchema`: at most 8 parts of bounded text or a small PNG, JPEG or WebP image, nothing else.
fn valid_result(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let known = object
        .keys()
        .all(|key| key == "content" || key == "isError");
    let error_flag = object.get("isError").is_none_or(Value::is_boolean);
    let Some(content) = object.get("content").and_then(Value::as_array) else {
        return false;
    };
    let base64 = |data: &str| {
        let body = data.trim_end_matches('=');
        data.len() - body.len() <= 2
            && body
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
    };
    let part = |part: &Value| {
        let Some(fields) = part.as_object() else {
            return false;
        };
        match part["type"].as_str() {
            Some("text") => {
                fields.len() == 2
                    && part["text"]
                        .as_str()
                        .is_some_and(|text| text.encode_utf16().count() <= TEXT_CHARS)
            }
            Some("image") => {
                fields.len() == 3
                    && matches!(
                        part["mimeType"].as_str(),
                        Some("image/png" | "image/jpeg" | "image/webp")
                    )
                    && part["data"]
                        .as_str()
                        .is_some_and(|data| data.len() <= IMAGE_CHARS && base64(data))
            }
            _ => false,
        }
    };
    known && error_flag && content.len() <= 8 && content.iter().all(part)
}

fn failed(message: &str) -> Value {
    json!({ "isError": true, "content": [{ "type": "text", "text": message.chars().take(4000).collect::<String>() }] })
}

/// A question about one bot (or token), asked by id.
pub type ForBot<T> = Arc<dyn Fn(&str) -> T + Send + Sync>;

pub struct NativeBrowserMcpOptions {
    /// `McpTokens.botFor`: the bot a session token was issued to.
    pub bot_for: ForBot<Option<String>>,
    /// `Some(browser on)` for a bot that exists, `None` otherwise.
    pub browser_enabled: ForBot<Option<bool>>,
    pub request: NativeRequest,
    pub cancel_tools: Option<ForBot<()>>,
    /// False while the bot is paused.
    pub can_run_tools: Option<ForBot<bool>>,
}

pub struct NativeBrowserMcp {
    options: NativeBrowserMcpOptions,
    epochs: Mutex<HashMap<String, u64>>,
    /// One call at a time per bot, in arrival order.
    // ponytail: entries stay for the daemon's life; one small lock per bot that ever used its browser.
    queues: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    closed: AtomicBool,
}

impl NativeBrowserMcp {
    pub fn new(options: NativeBrowserMcpOptions) -> Arc<NativeBrowserMcp> {
        Arc::new(NativeBrowserMcp {
            options,
            epochs: Mutex::default(),
            queues: Mutex::default(),
            closed: AtomicBool::new(false),
        })
    }

    fn epoch(&self, bot: &str) -> u64 {
        self.epochs.lock().unwrap().get(bot).copied().unwrap_or(0)
    }

    fn authorized(&self, token: &str, bot: &str, epoch: u64) -> bool {
        !self.closed.load(Ordering::Acquire)
            && self
                .options
                .can_run_tools
                .as_ref()
                .is_none_or(|can| can(bot))
            && (self.options.bot_for)(token).as_deref() == Some(bot)
            && (self.options.browser_enabled)(bot) == Some(true)
            && self.epoch(bot) == epoch
    }

    fn authority(self: &Arc<Self>, token: &str, bot: &str, epoch: u64) -> Authorized {
        let (mcp, token, bot) = (self.clone(), token.to_owned(), bot.to_owned());
        Arc::new(move || mcp.authorized(&token, &bot, epoch))
    }

    async fn answer(
        self: Arc<Self>,
        token: String,
        bot: String,
        epoch: u64,
        raw: Value,
    ) -> Option<Value> {
        let message = raw.as_object().cloned().unwrap_or_default();
        let invalid = || json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "Invalid request" } });
        if message.is_empty() {
            return Some(invalid());
        }
        let id = message.get("id")?.clone();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Some(invalid());
        };
        if !(id.is_string() || id.is_number()) {
            return Some(invalid());
        }
        let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let unauthorized = || json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32001, "message": "Browser session is no longer authorized" } });
        let error = |code: i64, text: &str| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": text } });
        if !self.authorized(&token, &bot, epoch) {
            return Some(unauthorized());
        }
        let params = message
            .get("params")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        match method {
            "initialize" => {
                let version = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2025-06-18");
                return Some(ok(
                    json!({ "protocolVersion": version, "capabilities": { "tools": {} }, "serverInfo": { "name": "browser", "version": "0.1.0" } }),
                ));
            }
            "ping" => return Some(ok(json!({}))),
            "tools/list" => {
                return Some(ok(
                    json!({ "tools": TOOLS.iter().map(|(_, tool)| &tool.listed).collect::<Vec<_>>() }),
                ));
            }
            "tools/call" => {}
            _ => return Some(error(-32601, "Method not found")),
        }
        let Some(name) = params
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| tool(name).is_some())
        else {
            return Some(error(-32602, "Unknown native browser tool"));
        };
        let args = match validate_tool(name, params.get("arguments").unwrap_or(&json!({}))) {
            Ok(args) => args,
            Err(message) => {
                return Some(if self.authorized(&token, &bot, epoch) {
                    ok(failed(&message))
                } else {
                    unauthorized()
                });
            }
        };
        if !self.authorized(&token, &bot, epoch) {
            return Some(unauthorized());
        }
        let command = json!({ "kind": "tool", "name": name, "args": args });
        let result = {
            // A dialog answer skips the queue: it may be what a queued action is waiting on.
            let queue = (name != "browser_handle_dialog").then(|| {
                self.queues
                    .lock()
                    .unwrap()
                    .entry(bot.clone())
                    .or_default()
                    .clone()
            });
            let _turn = match &queue {
                Some(queue) => Some(queue.lock().await),
                None => None,
            };
            if self.authorized(&token, &bot, epoch) {
                (self.options.request)(
                    bot.clone(),
                    command,
                    Some(self.authority(&token, &bot, epoch)),
                )
                .await
                .map_err(|error| error.message)
            } else {
                Err("Browser session is no longer authorized".into())
            }
        };
        if !self.authorized(&token, &bot, epoch) {
            return Some(unauthorized());
        }
        Some(match result {
            // Never pass on arbitrary bridge output or raw CDP objects.
            Ok(raw) if valid_result(&raw) && raw.to_string().len() <= RESULT_BYTES => ok(raw),
            Ok(_) => ok(failed(
                "The desktop returned an invalid or oversized browser result.",
            )),
            Err(message) => ok(failed(&message)),
        })
    }

    pub fn route(self: &Arc<Self>) -> Route {
        let mcp = self.clone();
        Arc::new(move |request| {
            let mcp = mcp.clone();
            Box::pin(async move { mcp.handle(request).await })
        })
    }

    async fn handle(self: Arc<Self>, request: axum::extract::Request) -> Response {
        let (parts, body) = request.into_parts();
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_owned();
        let Some(bot) =
            (self.options.bot_for)(&token).filter(|_| !self.closed.load(Ordering::Acquire))
        else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        match (self.options.browser_enabled)(&bot) {
            None => return StatusCode::UNAUTHORIZED.into_response(),
            Some(false) => return StatusCode::FORBIDDEN.into_response(),
            Some(true) => {}
        }
        if parts.method != Method::POST {
            return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response();
        }
        let epoch = *self.epochs.lock().unwrap().entry(bot.clone()).or_insert(0);
        let bytes = match host_poll::body(body, BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(refused) => return refused,
        };
        let Ok(input) = serde_json::from_slice::<Value>(&bytes) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let batch = input.is_array();
        let messages = match input {
            Value::Array(messages) => messages,
            single => vec![single],
        };
        if messages.is_empty() || messages.len() > MAX_BATCH {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let answers = join_all(messages.into_iter().map(|message| {
            self.clone()
                .answer(token.clone(), bot.clone(), epoch, message)
        }))
        .await;
        let mut reply_bytes = 0;
        let answers: Vec<Value> = answers
            .into_iter()
            .flatten()
            .map(|answer| {
                reply_bytes += answer.to_string().len();
                if reply_bytes > REPLY_BYTES {
                    let message = "This batch exceeds the 8 MiB browser reply limit. Request large results individually.";
                    json!({ "jsonrpc": "2.0", "id": answer["id"], "result": failed(message) })
                } else {
                    answer
                }
            })
            .collect();
        if answers.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        let reply = if batch {
            Value::Array(answers)
        } else {
            answers.into_iter().next().expect("one answer")
        };
        host_poll::job(&reply)
    }

    /// The bot's tool session closed: queued and late results are refused, and its running tools are cancelled.
    pub fn close(&self, bot: &str) {
        *self
            .epochs
            .lock()
            .unwrap()
            .entry(bot.to_owned())
            .or_insert(0) += 1;
        if let Some(cancel) = &self.options.cancel_tools {
            cancel(bot);
        }
    }

    pub fn close_all(&self) {
        self.closed.store(true, Ordering::Release);
        let mut bots: Vec<String> = self.epochs.lock().unwrap().keys().cloned().collect();
        bots.extend(self.queues.lock().unwrap().keys().cloned());
        bots.sort();
        bots.dedup();
        if let Some(cancel) = &self.options.cancel_tools {
            for bot in bots {
                cancel(&bot);
            }
        }
    }
}
