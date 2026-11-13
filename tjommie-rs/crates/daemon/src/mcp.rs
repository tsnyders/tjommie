//! `mcp.ts`: the `/mcp` route, a stateless MCP endpoint that answers plain JSON (no streaming). A CLI session holds a
//! bearer token that names its bot. A tool call is parsed first, then run as that bot, and only while the token still
//! names it. Notifications get 202 with no body.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use futures_util::future::{BoxFuture, join_all};
use serde_json::{Value, json};

use crate::server::Route;

const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_BATCH: usize = 20;
const FALLBACK_PROTOCOL: &str = "2025-06-18";

/// `Ok` is the text the bot reads. `Err` is a failed call, with the message the bot reads.
pub type ToolFuture = BoxFuture<'static, Result<String, String>>;

/// A tool whose arguments parsed, waiting for the calling bot's id.
type Prepared = Box<dyn FnOnce(String) -> ToolFuture + Send>;
/// Parses a call's `arguments`. `Err` is already prettified.
type Prepare = Arc<dyn Fn(&Value) -> Result<Prepared, String> + Send + Sync>;

/// `defineTool`: a tool the CLIs can call.
pub struct McpTool {
    pub name: String,
    pub description: String,
    /// The JSON Schema `tools/list` shows, verbatim.
    pub input_schema: Value,
    prepare: Prepare,
}

impl McpTool {
    /// `parse` is zod's `safeParse` with `prettifyError`: an `Err` becomes a failed call the bot can read. `run` gets the
    /// calling bot's id and the parsed arguments.
    pub fn define<A: Send + 'static>(
        name: &str,
        description: &str,
        input_schema: Value,
        parse: impl Fn(&Value) -> Result<A, String> + Send + Sync + 'static,
        run: impl Fn(String, A) -> ToolFuture + Send + Sync + 'static,
    ) -> McpTool {
        let run = Arc::new(run);
        let prepare: Prepare = Arc::new(move |arguments: &Value| -> Result<Prepared, String> {
            let parsed = parse(arguments)?;
            let run = run.clone();
            let prepared: Prepared = Box::new(move |bot: String| run(bot, parsed));
            Ok(prepared)
        });
        McpTool {
            name: name.to_owned(),
            description: description.to_owned(),
            input_schema,
            prepare,
        }
    }

    /// Parses `arguments` and runs the tool as `bot_id`, without the endpoint's token checks: for a test, or a caller
    /// inside the daemon. A parse failure is an `Err` with the text a bot would read.
    pub fn call(&self, bot_id: &str, arguments: &Value) -> ToolFuture {
        match (self.prepare)(arguments) {
            Ok(run) => run(bot_id.to_owned()),
            Err(message) => Box::pin(std::future::ready(Err(message))),
        }
    }
}

/// One validation problem, as zod reports it.
pub struct ArgIssue {
    pub path: Vec<String>,
    pub message: String,
}

/// zod's `prettifyError`: one block per issue, shallowest path first (a stable sort, like JavaScript's), joined with
/// newlines. A path segment is written as zod writes it, such as `days[0]`; segments are joined with ".".
pub fn prettify(issues: &[ArgIssue]) -> String {
    let mut ordered: Vec<&ArgIssue> = issues.iter().collect();
    ordered.sort_by_key(|issue| issue.path.len());
    ordered
        .into_iter()
        .map(|issue| {
            if issue.path.is_empty() {
                format!("✖ {}", issue.message)
            } else {
                format!("✖ {}\n  → at {}", issue.message, issue.path.join("."))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `McpTokens`: one bearer token per running CLI session. Clones share the same map.
#[derive(Clone, Default)]
pub struct McpTokens(Arc<Mutex<HashMap<String, String>>>); // token -> bot id

impl McpTokens {
    /// Revokes the bot's older tokens, then issues a new one: 24 random bytes as base64url without padding.
    pub fn issue(&self, bot_id: &str) -> String {
        self.revoke(bot_id);
        let mut bytes = [0u8; 24];
        getrandom::fill(&mut bytes).expect("the operating system's random source works");
        let token = URL_SAFE_NO_PAD.encode(bytes);
        self.lock().insert(token.clone(), bot_id.to_owned());
        token
    }

    pub fn revoke(&self, bot_id: &str) {
        self.lock().retain(|_, id| id.as_str() != bot_id);
    }

    pub fn bot_for(&self, token: &str) -> Option<String> {
        self.lock().get(token).cloned()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Endpoint {
    tokens: McpTokens,
    tools: Vec<McpTool>,
}

impl Endpoint {
    /// One message's answer, or `None` for a notification (an object without an `id` key).
    async fn answer(&self, bot: &str, token: &str, message: Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(invalid_request(Value::Null));
        };
        let id = object.get("id")?;
        let method = object.get("method").and_then(Value::as_str);
        if !is_id(id) {
            return Some(invalid_request(Value::Null));
        }
        let Some(method) = method else {
            return Some(invalid_request(id.clone()));
        };
        let params = object.get("params").and_then(Value::as_object);
        let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let error = |code: i64, detail: String| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": detail } });
        let answer = match method {
            "initialize" => ok(json!({
                "protocolVersion": params
                    .and_then(|params| params.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or(FALLBACK_PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tjommie", "version": "0.1.0" },
            })),
            "ping" => ok(json!({})),
            "tools/list" => {
                let listed: Vec<Value> = self
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({ "name": tool.name, "description": tool.description, "inputSchema": tool.input_schema })
                    })
                    .collect();
                ok(json!({ "tools": listed }))
            }
            "tools/call" => {
                let name = params
                    .and_then(|params| params.get("name"))
                    .and_then(Value::as_str);
                let Some(tool) =
                    name.and_then(|name| self.tools.iter().find(|tool| tool.name == name))
                else {
                    return Some(error(
                        -32602,
                        format!("Unknown tool {}", name.unwrap_or("undefined")),
                    ));
                };
                let empty = json!({});
                let arguments = params
                    .and_then(|params| params.get("arguments"))
                    .filter(|value| !value.is_null())
                    .unwrap_or(&empty);
                let run = match (tool.prepare)(arguments) {
                    Ok(run) => run,
                    Err(message) => return Some(ok(text(message, true))),
                };
                // Checked again after parsing, so a session revoked meanwhile never runs the tool.
                if self.tokens.bot_for(token).as_deref() != Some(bot) {
                    return Some(error(-32001, "Session token is no longer valid".to_owned()));
                }
                match run(bot.to_owned()).await {
                    Ok(output) => ok(text(output, false)),
                    Err(message) => ok(text(message, true)),
                }
            }
            _ => error(-32601, format!("Method not found: {method}")),
        };
        Some(answer)
    }

    async fn handle(self: Arc<Self>, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        if parts.method != Method::POST {
            return (
                StatusCode::METHOD_NOT_ALLOWED,
                [(header::ALLOW, "POST")],
                "",
            )
                .into_response();
        }
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();
        let Some(bot) = self.tokens.bot_for(token) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        // Counted in bytes. The refusal comes as soon as the limit is passed, not when the client finishes.
        let mut stream = body.into_data_stream();
        let mut raw = Vec::new();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                return parse_error();
            };
            if raw.len() + chunk.len() > MAX_BODY_BYTES {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
            raw.extend_from_slice(&chunk);
        }
        // Lossy, as Node's utf8 decoding is.
        let Ok(input) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&raw)) else {
            return parse_error();
        };
        let batch = input.is_array();
        let messages = match input {
            Value::Array(messages) => messages,
            single => vec![single],
        };
        if messages.len() > MAX_BATCH {
            return json_reply(
                StatusCode::OK,
                &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "A batch can contain at most 20 calls." } }),
            );
        }
        let mut answers: Vec<Value> = join_all(
            messages
                .into_iter()
                .map(|message| self.answer(&bot, token, message)),
        )
        .await
        .into_iter()
        .flatten()
        .collect();
        if answers.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        // A single message has at most one answer, so `remove(0)` is in range here.
        let reply = if batch {
            Value::Array(answers)
        } else {
            answers.remove(0)
        };
        json_reply(StatusCode::OK, &reply)
    }
}

/// `mcpHandler`: the `/mcp` route. Both CLIs need only initialize, ping, tools/list and tools/call.
pub fn mcp_route(tokens: McpTokens, tools: Vec<McpTool>) -> Route {
    let endpoint = Arc::new(Endpoint { tokens, tools });
    Arc::new(move |request: Request| -> BoxFuture<'static, Response> {
        let endpoint = endpoint.clone();
        Box::pin(async move { endpoint.handle(request).await })
    })
}

fn text(value: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": value }], "isError": is_error })
}

fn is_id(value: &Value) -> bool {
    matches!(value, Value::Null | Value::String(_) | Value::Number(_))
}

fn invalid_request(id: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32600, "message": "Invalid request" } })
}

fn parse_error() -> Response {
    json_reply(
        StatusCode::BAD_REQUEST,
        &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }),
    )
}

fn json_reply(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}
