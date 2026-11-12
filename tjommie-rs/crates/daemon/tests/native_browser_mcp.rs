//! Port of `packages/daemon/src/native-browser-mcp.test.ts`: `/mcp/browser`, a token-scoped MCP proxy for the 26 native
//! browser tools. The desktop runs them; this side validates, serializes per bot and fences late results.

mod host_support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use daemon::native_browser_handlers::NativeRequest;
use daemon::native_browser_mcp::{NativeBrowserMcp, NativeBrowserMcpOptions};
use daemon::server::{Handlers, Routes, RpcError, Server, ServerOptions};
use futures_util::future::BoxFuture;
use host_support::{Reply, http};
use serde_json::{Value, json};
use tokio::sync::oneshot;

const BOT: &str = "bot-one";

fn ok() -> Value {
    json!({ "content": [{ "type": "text", "text": "native result" }] })
}

type Calls = Arc<
    Mutex<
        Vec<(
            String,
            Value,
            Option<daemon::native_browser_broker::Authorized>,
        )>,
    >,
>;
type Answer = Arc<dyn Fn(&Value) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync>;

struct Fixture {
    port: u16,
    token: String,
    tokens: Arc<Mutex<HashMap<String, String>>>,
    browser: Arc<AtomicBool>,
    calls: Calls,
    cancelled: Arc<Mutex<Vec<String>>>,
    mcp: Arc<NativeBrowserMcp>,
    _server: Server,
}

/// Results the test releases by hand, in order.
#[derive(Clone, Default)]
struct Held(Arc<Mutex<Vec<oneshot::Sender<Value>>>>);

impl Held {
    fn answer(&self) -> BoxFuture<'static, Result<Value, RpcError>> {
        let (send, receive) = oneshot::channel();
        self.0.lock().unwrap().push(send);
        Box::pin(async move {
            receive
                .await
                .map_err(|_| RpcError::new(protocol::ErrorCode::Internal, "dropped"))
        })
    }

    fn release(&self, value: Value) {
        let _ = self.0.lock().unwrap().remove(0).send(value);
    }
}

async fn fixture(answer: Answer, can_run: Option<Arc<AtomicBool>>) -> Fixture {
    let tokens: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let token = "synthetic-mcp-token".to_owned();
    tokens.lock().unwrap().insert(token.clone(), BOT.into());
    let browser = Arc::new(AtomicBool::new(true));
    let calls: Calls = Arc::default();
    let cancelled: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = calls.clone();
    let request: NativeRequest = Arc::new(move |bot: String, command: Value, authorized| {
        seen.lock()
            .unwrap()
            .push((bot, command.clone(), authorized));
        answer(&command)
    });
    let (lookup, enabled, stopped) = (tokens.clone(), browser.clone(), cancelled.clone());
    let mcp = NativeBrowserMcp::new(NativeBrowserMcpOptions {
        bot_for: Arc::new(move |token: &str| lookup.lock().unwrap().get(token).cloned()),
        browser_enabled: Arc::new(move |bot: &str| {
            (bot == BOT).then(|| enabled.load(Ordering::Relaxed))
        }),
        request,
        cancel_tools: Some(Arc::new(move |bot: &str| {
            stopped.lock().unwrap().push(bot.into())
        })),
        can_run_tools: can_run.map(|flag| {
            Arc::new(move |_: &str| flag.load(Ordering::Relaxed))
                as Arc<dyn Fn(&str) -> bool + Send + Sync>
        }),
    });
    let server = Server::start(ServerOptions {
        port: 0,
        token: "renderer".into(),
        handlers: Handlers::default(),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap();
    server
        .activate_with(
            "renderer",
            Handlers::default(),
            Routes::default().route("/mcp/browser", mcp.route()),
        )
        .unwrap();
    Fixture {
        port: server.port(),
        token,
        tokens,
        browser,
        calls,
        cancelled,
        mcp,
        _server: server,
    }
}

fn always(value: Value) -> Answer {
    Arc::new(move |_| {
        let value = value.clone();
        Box::pin(async move { Ok(value) })
    })
}

fn held(holder: &Held) -> Answer {
    let holder = holder.clone();
    Arc::new(move |_| holder.answer())
}

impl Fixture {
    async fn post(&self, body: &str) -> Reply {
        http(
            self.port,
            "POST",
            "/mcp/browser",
            &[("Authorization", &format!("Bearer {}", self.token))],
            body.as_bytes(),
        )
        .await
    }

    fn call(&self, name: &str, args: Value, id: i64) -> tokio::task::JoinHandle<Value> {
        let (port, token) = (self.port, self.token.clone());
        let body = json!({ "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } }).to_string();
        tokio::spawn(async move {
            http(
                port,
                "POST",
                "/mcp/browser",
                &[("Authorization", &format!("Bearer {token}"))],
                body.as_bytes(),
            )
            .await
            .json()
        })
    }

    async fn calls_made(&self, count: usize) {
        for _ in 0..400 {
            if self.calls.lock().unwrap().len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("expected {count} host calls");
    }

    fn authorized_now(&self, index: usize) -> bool {
        self.calls.lock().unwrap()[index]
            .2
            .as_ref()
            .is_some_and(|authorized| authorized())
    }
}

fn error_code(reply: &Value) -> Option<i64> {
    reply["error"]["code"].as_i64()
}

#[tokio::test]
async fn fences_queued_and_late_results_when_the_bot_is_paused() {
    let holder = Held::default();
    let allowed = Arc::new(AtomicBool::new(true));
    let f = fixture(held(&holder), Some(allowed.clone())).await;
    let first = f.call("browser_snapshot", json!({}), 1);
    f.calls_made(1).await;
    let second = f.call("browser_snapshot", json!({}), 2);
    tokio::time::sleep(Duration::from_millis(30)).await;
    allowed.store(false, Ordering::Relaxed);
    holder.release(ok());
    assert_eq!(error_code(&first.await.unwrap()), Some(-32001));
    assert_eq!(error_code(&second.await.unwrap()), Some(-32001));
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn initializes_and_lists_without_opening_a_browser_and_only_forwards_validated_tool_commands()
{
    let f = fixture(always(ok()), None).await;
    let initialized = f
        .post(
            &json!({ "id": 1, "method": "initialize", "params": { "protocolVersion": "future" } })
                .to_string(),
        )
        .await;
    assert_eq!(initialized.status, 200);
    assert_eq!(initialized.json()["result"]["protocolVersion"], "future");
    let listed = f
        .post(&json!({ "id": 2, "method": "tools/list" }).to_string())
        .await
        .json();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 26);
    assert!(names.contains(&"browser_take_screenshot"));
    assert!(!names.contains(&"browser_run_code"));
    let mut keys: Vec<&String> = listed["result"]["tools"][0]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["description", "inputSchema", "name"],
        "exactly what native-browser-mcp.ts lists"
    );
    let messages = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "browser_console_messages")
        .unwrap();
    assert_eq!(
        messages["inputSchema"]["required"],
        json!(["level"]),
        "listed with zod's output view"
    );
    assert!(f.calls.lock().unwrap().is_empty());
    assert_eq!(
        f.call("browser_snapshot", json!({}), 3).await.unwrap()["result"],
        ok()
    );
    assert_eq!(f.calls.lock().unwrap()[0].0, BOT);
    assert_eq!(
        f.calls.lock().unwrap()[0].1,
        json!({ "kind": "tool", "name": "browser_snapshot", "args": {} })
    );
    assert!(f.authorized_now(0));
    // Schema defaults fill in, as zod's parse does.
    f.call("browser_console_messages", json!({}), 4)
        .await
        .unwrap();
    assert_eq!(
        f.calls.lock().unwrap()[1].1["args"],
        json!({ "level": "info" })
    );
}

#[tokio::test]
async fn rejects_renderer_or_bad_tokens_disabled_bots_and_non_post_requests() {
    let f = fixture(always(ok()), None).await;
    let renderer = http(
        f.port,
        "POST",
        "/mcp/browser",
        &[("Authorization", "Bearer renderer")],
        b"{}",
    )
    .await;
    assert_eq!(renderer.status, 401);
    let get = http(
        f.port,
        "GET",
        "/mcp/browser",
        &[("Authorization", &format!("Bearer {}", f.token))],
        b"",
    )
    .await;
    assert_eq!(get.status, 405);
    f.browser.store(false, Ordering::Relaxed);
    assert_eq!(f.post("{}").await.status, 403);
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_oversized_or_bad_input_before_dispatch_and_handles_notifications() {
    let f = fixture(always(ok()), None).await;
    assert_eq!(f.post(&"x".repeat(1024 * 1024 + 1)).await.status, 413);
    assert_eq!(f.post("not-json").await.status, 400);
    let many: Vec<Value> = (0..21)
        .map(|id| json!({ "id": id, "method": "ping" }))
        .collect();
    assert_eq!(f.post(&Value::Array(many).to_string()).await.status, 400);
    let note = f
        .post(&json!({ "method": "notifications/initialized" }).to_string())
        .await;
    assert_eq!((note.status, note.body.is_empty()), (202, true));
    assert_eq!(
        error_code(
            &f.call("browser_run_code", json!({ "code": "process.env" }), 1)
                .await
                .unwrap()
        ),
        Some(-32602)
    );
    let injected = f
        .call(
            "browser_click",
            json!({ "target": "#save", "injected": "bad" }),
            2,
        )
        .await
        .unwrap();
    assert_eq!(injected["result"]["isError"], true);
    for (name, args) in [
        ("browser_click", json!({})),
        ("browser_click", json!({ "target": "#a", "ref": "e2" })),
        ("browser_find", json!({ "text": "a", "regex": "b" })),
        ("browser_wait_for", json!({})),
        (
            "browser_tabs",
            json!({ "action": "list", "url": "https://example.com/" }),
        ),
    ] {
        assert_eq!(
            f.call(name, args.clone(), 3).await.unwrap()["result"]["isError"],
            true,
            "{name} {args}"
        );
    }
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn serializes_ordinary_calls_checks_revoked_tokens_when_queued_work_starts_and_drops_late_output()
 {
    let holder = Held::default();
    let f = fixture(held(&holder), None).await;
    let first = f.call("browser_snapshot", json!({}), 1);
    f.calls_made(1).await;
    let second = f.call("browser_snapshot", json!({}), 2);
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.tokens.lock().unwrap().clear();
    assert!(!f.authorized_now(0));
    holder.release(ok());
    assert_eq!(error_code(&first.await.unwrap()), Some(-32001));
    assert_eq!(error_code(&second.await.unwrap()), Some(-32001));
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn close_invalidates_queued_and_in_flight_calls_and_cancels_tools_without_closing_the_native_session()
 {
    let holder = Held::default();
    let f = fixture(held(&holder), None).await;
    let first = f.call("browser_snapshot", json!({}), 1);
    f.calls_made(1).await;
    let second = f.call("browser_click", json!({ "target": "#save" }), 2);
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.mcp.close(BOT);
    assert_eq!(*f.cancelled.lock().unwrap(), [BOT]);
    holder.release(ok());
    assert_eq!(error_code(&first.await.unwrap()), Some(-32001));
    assert_eq!(error_code(&second.await.unwrap()), Some(-32001));
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert!(
        !f.calls
            .lock()
            .unwrap()
            .iter()
            .any(|(_, command, _)| command["kind"] == "close")
    );
    let after = f.call("browser_snapshot", json!({}), 3);
    f.calls_made(2).await;
    holder.release(ok());
    assert_eq!(after.await.unwrap()["result"], ok());
}

#[tokio::test]
async fn lets_dialog_dismissal_bypass_a_pending_tool_queue() {
    let holder = Held::default();
    let parked = holder.clone();
    let answer: Answer = Arc::new(move |command| {
        if command["name"] == "browser_handle_dialog" {
            Box::pin(async { Ok(ok()) })
        } else {
            parked.answer()
        }
    });
    let f = fixture(answer, None).await;
    let first = f.call(
        "browser_evaluate",
        json!({ "function": "() => alert('local')" }),
        1,
    );
    f.calls_made(1).await;
    assert_eq!(
        f.call("browser_handle_dialog", json!({ "accept": false }), 2)
            .await
            .unwrap()["result"],
        ok()
    );
    assert_eq!(f.calls.lock().unwrap().len(), 2);
    holder.release(ok());
    first.await.unwrap();
}

#[tokio::test]
async fn bounds_the_combined_reply_when_a_batch_requests_several_large_screenshots() {
    let image = json!({ "content": [{ "type": "image", "mimeType": "image/png", "data": STANDARD.encode(vec![0u8; 2 * 1024 * 1024]) }] });
    let f = fixture(always(image), None).await;
    let batch: Vec<Value> = (0..4)
        .map(|id| json!({ "id": id, "method": "tools/call", "params": { "name": "browser_take_screenshot", "arguments": {} } }))
        .collect();
    let reply = f.post(&Value::Array(batch).to_string()).await;
    assert!(reply.body.len() < 8 * 1024 * 1024, "{}", reply.body.len());
    assert!(String::from_utf8_lossy(&reply.body).contains("Request large results individually"));
}

#[tokio::test]
async fn bounds_and_validates_bridge_results() {
    for raw in [
        json!({ "arbitrary": "CDP object" }),
        json!({ "content": [{ "type": "text", "text": "x".repeat(128 * 1024 + 101) }] }),
        json!({ "content": [{ "type": "image", "mimeType": "image/svg+xml", "data": "abc" }] }),
    ] {
        let f = fixture(always(raw.clone()), None).await;
        let reply = f.call("browser_snapshot", json!({}), 1).await.unwrap();
        assert_eq!(reply["result"]["isError"], true);
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("invalid or oversized"),
            "{reply}"
        );
    }
}
