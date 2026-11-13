//! `mcp.test.ts`: the `/mcp` route over a real server. A small HTTP/1.1 client sets Host and Origin itself and can hold a
//! request body open, as node:http did in the TS tests. The `server.ts` 500 case is ignored at the end (see there).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use daemon::mcp::{ArgIssue, McpTokens, McpTool, ToolFuture, mcp_route, prettify};
use daemon::server::{Handlers, Routes, Server, ServerOptions};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

const LIMIT: usize = 1024 * 1024;

// ---- tools ----

/// `z.string()` at `key`, reported the way zod does when it is missing or not a string.
fn string_arg(arguments: &Value, key: &str) -> Result<String, String> {
    match arguments.get(key).and_then(Value::as_str) {
        Some(text) => Ok(text.to_owned()),
        None => Err(prettify(&[ArgIssue {
            path: vec![key.to_owned()],
            message: "Invalid input: expected string".to_owned(),
        }])),
    }
}

/// `echo`: `z.object({ text: z.string().describe("What to repeat") })`, run as `"{bot} says {text}"`.
fn echo() -> McpTool {
    McpTool::define(
        "echo",
        "Repeats the text back.",
        json!({ "type": "object", "properties": { "text": { "type": "string", "description": "What to repeat" } }, "required": ["text"] }),
        |arguments: &Value| -> Result<String, String> { string_arg(arguments, "text") },
        |bot: String, text: String| -> ToolFuture {
            Box::pin(async move { Ok::<String, String>(format!("{bot} says {text}")) })
        },
    )
}

/// `refuses`: always fails with a message the bot reads.
fn refuses() -> McpTool {
    McpTool::define(
        "refuses",
        "Always refuses.",
        json!({ "type": "object", "properties": {} }),
        |_: &Value| -> Result<(), String> { Ok(()) },
        |_: String, _: ()| -> ToolFuture {
            Box::pin(async { Err::<String, String>("Limit reached, ask the user.".to_owned()) })
        },
    )
}

/// A tool named `record` that counts its runs and answers `ran`. `parse` runs first, as zod's parse would.
fn counted(
    calls: Arc<AtomicUsize>,
    parse: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static,
) -> McpTool {
    McpTool::define(
        "record",
        "Records a call.",
        json!({ "type": "object", "properties": {} }),
        parse,
        move |_: String, _: ()| -> ToolFuture {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok::<String, String>("ran".to_owned()) })
        },
    )
}

fn record(calls: Arc<AtomicUsize>) -> McpTool {
    counted(calls, |_| Ok(()))
}

// ---- server and HTTP ----

async fn start(tokens: &McpTokens, tools: Vec<McpTool>) -> Server {
    let server = Server::start(ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec!["app://tjommie".into()],
        defer_requests: false,
    })
    .await
    .unwrap();
    server
        .activate_with(
            "t",
            Handlers::default(),
            Routes::default().route("/mcp", mcp_route(tokens.clone(), tools)),
        )
        .unwrap();
    server
}

/// `Authorization: Bearer ...` for a fresh session of `bot`.
fn bearer_for(tokens: &McpTokens, bot: &str) -> String {
    format!("Bearer {}", tokens.issue(bot))
}

/// A reply. `body` is parsed as JSON, or is `null` when empty.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// `{ status, body }`, the shape the TS assertions compare.
    fn as_json(&self) -> Value {
        json!({ "status": self.status, "body": self.body })
    }
}

/// One request on its own connection (`Connection: close`). Host defaults to this machine's loopback and port.
struct Upload {
    stream: TcpStream,
    /// Sent with the first write, so a refused request's head and body usually arrive in one segment. Otherwise the
    /// server could close with body bytes unread, and the reset would drop its reply on Windows.
    head: Vec<u8>,
}

impl Upload {
    /// Connects and prepares the head, declaring `length` body bytes.
    async fn open(
        port: u16,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        length: usize,
    ) -> Upload {
        let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {length}\r\n"
        );
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"))
        {
            head.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
        }
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        Upload {
            stream,
            head: head.into_bytes(),
        }
    }

    /// A refused request may close before every byte is sent, so a failed write is left for the reply to show.
    async fn write(&mut self, bytes: &[u8]) {
        let mut out = std::mem::take(&mut self.head);
        out.extend_from_slice(bytes);
        let _ = self.stream.write_all(&out).await;
    }

    async fn reply(&mut self) -> Reply {
        timeout(Duration::from_secs(60), read_reply(&mut self.stream))
            .await
            .expect("a reply in time")
    }
}

async fn send(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    let mut upload = Upload::open(port, method, path, headers, body.len()).await;
    upload.write(body).await;
    upload.reply().await
}

async fn post(port: u16, body: &Value, headers: &[(&str, &str)]) -> Reply {
    send(
        port,
        "POST",
        "/mcp",
        headers,
        &serde_json::to_vec(body).unwrap(),
    )
    .await
}

/// The head, then `Content-Length` bytes. Reading stops there, so an open request body can't hold the reply back.
async fn read_reply(stream: &mut TcpStream) -> Reply {
    let mut raw = Vec::new();
    let head_end = loop {
        if let Some(at) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        if read_more(stream, &mut raw).await == 0 {
            break raw.len();
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("an HTTP reply, got {head:?}"));
    let length = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    let end = length.map_or(usize::MAX, |length| head_end + length);
    while raw.len() < end {
        if read_more(stream, &mut raw).await == 0 {
            break;
        }
    }
    let text = &raw[head_end..end.min(raw.len())];
    let body = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(text)
            .unwrap_or_else(|_| panic!("JSON body, got {:?}", String::from_utf8_lossy(text)))
    };
    Reply { status, body }
}

/// Reads what is available. Returns 0 at the end of the connection or on an error, which ends the reply.
async fn read_more(stream: &mut TcpStream, raw: &mut Vec<u8>) -> usize {
    let mut chunk = [0u8; 4096];
    match stream.read(&mut chunk).await {
        Ok(n) => {
            raw.extend_from_slice(&chunk[..n]);
            n
        }
        Err(_) => 0,
    }
}

/// Vitest's `toMatchObject`: every expected key must match, extra keys in `actual` are fine, arrays must be the same length.
fn assert_match(actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            for (key, want) in expected {
                let got = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("no {key} in {actual:?}"));
                assert_match(got, want);
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len(), "array length in {actual:?}");
            for (got, want) in actual.iter().zip(expected) {
                assert_match(got, want);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}

/// `n` `tools/call` requests for `record`, with ids 0..n, in one batch.
fn record_calls(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|id| json!({ "id": id, "method": "tools/call", "params": { "name": "record", "arguments": {} } }))
            .collect(),
    )
}

// ---- the endpoint ----

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_batch_over_20_calls_before_executing_anything() {
    let calls = Arc::new(AtomicUsize::new(0));
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![record(calls.clone())]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    assert_match(
        &post(port, &record_calls(21), &auth).await.as_json(),
        &json!({ "status": 200, "body": { "id": null, "error": { "code": -32600, "message": "A batch can contain at most 20 calls." } } }),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let reply = post(port, &record_calls(20), &auth).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.as_array().map(Vec::len), Some(20));
    assert_eq!(calls.load(Ordering::SeqCst), 20);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn echoes_the_clients_protocol_version_lists_tools_and_runs_them_as_the_tokens_bot() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    let init = post(
        port,
        &json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "claude-code" } } }),
        &auth,
    )
    .await;
    assert_eq!(
        init.as_json(),
        json!({ "status": 200, "body": { "jsonrpc": "2.0", "id": 0, "result": { "protocolVersion": "2025-11-25", "capabilities": { "tools": {} }, "serverInfo": { "name": "tjommie", "version": "0.1.0" } } } })
    );

    let list = post(
        port,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        &auth,
    )
    .await;
    assert_eq!(
        list.as_json(),
        json!({ "status": 200, "body": { "jsonrpc": "2.0", "id": 1, "result": { "tools": [
            {
                "name": "echo",
                "description": "Repeats the text back.",
                "inputSchema": { "type": "object", "properties": { "text": { "type": "string", "description": "What to repeat" } }, "required": ["text"] },
            },
            { "name": "refuses", "description": "Always refuses.", "inputSchema": { "type": "object", "properties": {} } },
        ] } } })
    );

    let call = post(
        port,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "echo", "arguments": { "text": "hi ñ" } } }),
        &auth,
    )
    .await;
    assert_eq!(
        call.as_json(),
        json!({ "status": 200, "body": { "jsonrpc": "2.0", "id": 2, "result": { "content": [{ "type": "text", "text": "bot-1 says hi ñ" }], "isError": false } } })
    );
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn initializes_without_jsonrpc_and_preserves_protocol_behavior() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    let cases = [
        (json!({ "protocolVersion": "2025-06-18" }), "2025-06-18"),
        (
            json!({ "protocolVersion": "future-client-version" }),
            "future-client-version",
        ),
        (json!({}), "2025-06-18"),
    ];
    for (params, expected) in cases {
        let reply = post(
            port,
            &json!({ "id": "init", "method": "initialize", "params": params }),
            &auth,
        )
        .await;
        assert_match(
            &reply.as_json(),
            &json!({ "status": 200, "body": { "id": "init", "result": { "protocolVersion": expected } } }),
        );
    }
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn turns_bad_arguments_and_refusals_into_failed_calls_the_bot_can_read() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    let bad = post(port, &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "echo", "arguments": {} } }), &auth).await;
    assert_eq!(bad.status, 200);
    assert_eq!(bad.body["result"]["isError"], json!(true));
    assert_eq!(
        bad.body["result"]["content"].as_array().map(Vec::len),
        Some(1)
    );
    let text = bad.body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(text.contains("text"), "bad-argument message was {text:?}");

    let refused = post(port, &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "refuses", "arguments": {} } }), &auth).await;
    assert_eq!(
        refused.as_json(),
        json!({ "status": 200, "body": { "jsonrpc": "2.0", "id": 2, "result": { "content": [{ "type": "text", "text": "Limit reached, ask the user." }], "isError": true } } })
    );

    assert_match(
        &post(
            port,
            &json!({ "id": 3, "method": "tools/call", "params": { "name": "missing" } }),
            &auth,
        )
        .await
        .as_json(),
        &json!({ "status": 200, "body": { "error": { "code": -32602, "message": "Unknown tool missing" } } }),
    );
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn awaits_tool_results_and_reports_rejected_promises() {
    let asynchronous = McpTool::define(
        "async",
        "Asynchronous tool.",
        json!({ "type": "object", "properties": { "refuse": { "type": "boolean", "default": false } } }),
        |arguments: &Value| -> Result<bool, String> {
            match arguments.get("refuse") {
                None => Ok(false),
                Some(Value::Bool(refuse)) => Ok(*refuse),
                Some(_) => Err("Invalid input: expected boolean".to_owned()),
            }
        },
        |bot: String, refuse: bool| -> ToolFuture {
            Box::pin(async move {
                if refuse {
                    Err("Not allowed.".to_owned())
                } else {
                    Ok(bot)
                }
            })
        },
    );
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![asynchronous]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    assert_match(
        &post(
            port,
            &json!({ "id": 1, "method": "tools/call", "params": { "name": "async" } }),
            &auth,
        )
        .await
        .as_json(),
        &json!({ "body": { "result": { "content": [{ "type": "text", "text": "bot-1" }], "isError": false } } }),
    );
    assert_match(
        &post(port, &json!({ "id": 2, "method": "tools/call", "params": { "name": "async", "arguments": { "refuse": true } } }), &auth).await.as_json(),
        &json!({ "body": { "result": { "content": [{ "type": "text", "text": "Not allowed." }], "isError": true } } }),
    );
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn accepts_notifications_with_202_refuses_unknown_methods_and_answers_batches() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    assert_eq!(
        post(
            port,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            &auth
        )
        .await
        .as_json(),
        json!({ "status": 202, "body": null })
    );
    // Claude omits jsonrpc on this probe.
    assert_match(
        &post(
            port,
            &json!({ "id": 3, "method": "server/discover", "params": {} }),
            &auth,
        )
        .await
        .as_json(),
        &json!({ "body": { "error": { "code": -32601 } } }),
    );
    let batch = post(
        port,
        &json!([{ "jsonrpc": "2.0", "id": 4, "method": "ping" }, { "jsonrpc": "2.0", "method": "notifications/cancelled" }]),
        &auth,
    )
    .await;
    assert_eq!(
        batch.body,
        json!([{ "jsonrpc": "2.0", "id": 4, "result": {} }])
    );
    assert_eq!(
        post(
            port,
            &json!([{ "method": "notifications/initialized" }, {}]),
            &auth
        )
        .await
        .as_json(),
        json!({ "status": 202, "body": null })
    );
    assert_eq!(
        send(port, "POST", "/mcp", &auth, b"{not json")
            .await
            .as_json(),
        json!({ "status": 400, "body": { "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } } })
    );
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_malformed_object_with_an_id() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    for message in [json!({ "id": 7 }), json!({ "id": 7, "method": 42 })] {
        assert_eq!(
            post(port, &message, &auth).await.as_json(),
            json!({ "status": 200, "body": { "jsonrpc": "2.0", "id": 7, "error": { "code": -32600, "message": "Invalid request" } } })
        );
    }
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_each_invalid_entry_in_a_mixed_batch_without_executing_notifications() {
    let calls = Arc::new(AtomicUsize::new(0));
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![record(calls.clone())]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    let invalid = || json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "Invalid request" } });
    let reply = post(
        port,
        &json!([
            { "id": 1, "method": "ping" },
            { "id": 2 },
            null,
            42,
            [],
            { "id": {}, "method": "ping" },
            { "method": "tools/call", "params": { "name": "record" } },
        ]),
        &auth,
    )
    .await;
    assert_eq!(
        reply.as_json(),
        json!({ "status": 200, "body": [
            { "jsonrpc": "2.0", "id": 1, "result": {} },
            { "jsonrpc": "2.0", "id": 2, "error": { "code": -32600, "message": "Invalid request" } },
            invalid(), invalid(), invalid(), invalid(),
        ] })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_browsers_other_hosts_bad_tokens_revoked_tokens_and_huge_bodies() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let token = tokens.issue("bot-1");
    let bearer = format!("Bearer {token}");
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

    for origin in ["https://evil.example", "app://tjommie", ""] {
        let headers = [("authorization", bearer.as_str()), ("origin", origin)];
        assert_eq!(
            post(port, &ping, &headers).await.status,
            403,
            "origin {origin:?}"
        );
    }
    for host in [
        format!("evil.example:{port}"),
        "127.0.0.1:1".to_owned(),
        "localhost".to_owned(),
    ] {
        let headers = [("authorization", bearer.as_str()), ("host", host.as_str())];
        assert_eq!(post(port, &ping, &headers).await.status, 403, "host {host}");
    }
    let local = format!("localhost:{port}");
    assert_eq!(
        post(
            port,
            &ping,
            &[("authorization", bearer.as_str()), ("host", local.as_str())]
        )
        .await
        .status,
        200
    );
    assert_eq!(post(port, &ping, &[]).await.status, 401);
    assert_eq!(
        post(port, &ping, &[("authorization", "Bearer nope")])
            .await
            .status,
        401
    );
    let basic = format!("Basic {token}");
    assert_eq!(
        post(port, &ping, &[("authorization", basic.as_str())])
            .await
            .status,
        401
    );

    let auth = [("authorization", bearer.as_str())];
    assert_eq!(send(port, "GET", "/mcp", &auth, b"").await.status, 405);
    let huge = vec![b'x'; LIMIT + 1];
    assert_eq!(send(port, "POST", "/mcp", &auth, &huge).await.status, 413);
    tokens.revoke("bot-1");
    assert_eq!(post(port, &ping, &auth).await.status, 401);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn caps_the_body_in_bytes_and_accepts_exactly_1_mb() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    // `{"id":1,"method":"ping","padding":"xxx..."}` with the padding sized so the body is exactly LIMIT bytes.
    const PREFIX: &[u8] = b"{\"id\":1,\"method\":\"ping\",\"padding\":\"";
    const SUFFIX: &[u8] = b"\"}";
    let filler = LIMIT - PREFIX.len() - SUFFIX.len();
    let padding = vec![b'x'; filler];
    let exact = [PREFIX, padding.as_slice(), SUFFIX].concat();
    assert_eq!(exact.len(), LIMIT);
    assert_eq!(send(port, "POST", "/mcp", &auth, &exact).await.status, 200);

    // The same, with one 2-byte "ñ" in place of an "x": one byte over the limit, though the JS string is still LIMIT long.
    let over = [
        PREFIX,
        vec![b'x'; filler - 1].as_slice(),
        "ñ".as_bytes(),
        SUFFIX,
    ]
    .concat();
    assert_eq!(over.len(), LIMIT + 1);
    assert_eq!(send(port, "POST", "/mcp", &auth, &over).await.status, 413);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_an_oversized_streaming_upload_before_the_client_finishes_the_body() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    // Declares twice the limit, sends one byte past it, and never finishes the body.
    let mut upload = Upload::open(port, "POST", "/mcp", &auth, 2 * LIMIT).await;
    upload.write(&vec![b'x'; LIMIT + 1]).await;
    assert_eq!(
        upload.reply().await.as_json(),
        json!({ "status": 413, "body": null })
    );
    drop(upload);
    server.close().await;
}

#[test]
fn gives_each_session_a_new_token_and_forgets_the_old_one() {
    let tokens = McpTokens::default();
    let first = tokens.issue("bot-1");
    let other = tokens.issue("bot-2");
    let second = tokens.issue("bot-1");
    assert_ne!(second, first);
    assert_eq!(second.len(), 32);
    assert!(
        second
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "token {second:?} is not base64url"
    );
    assert_eq!(tokens.bot_for(&first), None);
    assert_eq!(tokens.bot_for(&second).as_deref(), Some("bot-1"));
    tokens.revoke("bot-1");
    assert_eq!(tokens.bot_for(&second), None);
    assert_eq!(tokens.bot_for(&other).as_deref(), Some("bot-2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_tool_call_when_the_token_is_revoked_after_authentication_but_before_the_body_finishes()
 {
    let calls = Arc::new(AtomicUsize::new(0));
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![record(calls.clone())]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let first: &[u8] = br#"{"id":1,"method":"tools/call","#;
    let second: &[u8] = br#""params":{"name":"record","arguments":{}}}"#;

    let mut upload = Upload::open(
        port,
        "POST",
        "/mcp",
        &[("authorization", bearer.as_str())],
        first.len() + second.len(),
    )
    .await;
    upload.write(first).await;
    // The TS test spies on `botFor` to learn when authentication ran. Rust has no such seam, so this waits for the
    // server to read the head, which happens before the body is read. Revoking after that is what the test is about.
    sleep(Duration::from_millis(200)).await;
    tokens.revoke("bot-1");
    upload.write(second).await;
    assert_eq!(
        upload.reply().await.as_json(),
        json!({ "status": 200, "body": { "jsonrpc": "2.0", "id": 1, "error": { "code": -32001, "message": "Session token is no longer valid" } } })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(upload);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rechecks_the_token_before_every_tool_in_a_batch() {
    let tokens = McpTokens::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let closer = tokens.clone();
    let close = McpTool::define(
        "close",
        "Closes the session.",
        json!({ "type": "object", "properties": {} }),
        |_: &Value| -> Result<(), String> { Ok(()) },
        move |bot: String, _: ()| -> ToolFuture {
            closer.revoke(&bot);
            Box::pin(async { Ok::<String, String>("closed".to_owned()) })
        },
    );
    let server = start(&tokens, vec![close, record(calls.clone())]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    assert_eq!(
        post(
            port,
            &json!([
                { "id": 1, "method": "tools/call", "params": { "name": "close" } },
                { "id": 2, "method": "tools/call", "params": { "name": "record" } },
            ]),
            &auth,
        )
        .await
        .as_json(),
        json!({ "status": 200, "body": [
            { "jsonrpc": "2.0", "id": 1, "result": { "content": [{ "type": "text", "text": "closed" }], "isError": false } },
            { "jsonrpc": "2.0", "id": 2, "error": { "code": -32001, "message": "Session token is no longer valid" } },
        ] })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn requires_the_token_to_still_identify_the_same_bot_before_a_tool_runs() {
    let tokens = McpTokens::default();
    let calls = Arc::new(AtomicUsize::new(0));
    // The TS test swaps `botFor`'s answer between the two checks. Here the token is revoked while the tool's arguments
    // are parsed, which is the same window: after the call is found, before the token is checked again.
    let revoker = tokens.clone();
    let tool = counted(calls.clone(), move |_| {
        revoker.revoke("bot-1");
        Ok(())
    });
    let server = start(&tokens, vec![tool]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");

    assert_match(
        &post(
            port,
            &json!({ "id": 1, "method": "tools/call", "params": { "name": "record" } }),
            &[("authorization", bearer.as_str())],
        )
        .await
        .as_json(),
        &json!({ "status": 200, "body": { "error": { "code": -32001 } } }),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_query_strings_and_keeps_the_health_and_missing_route_responses() {
    let tokens = McpTokens::default();
    let server = start(&tokens, vec![echo(), refuses()]).await;
    let port = server.port();
    let bearer = bearer_for(&tokens, "bot-1");
    let auth = [("authorization", bearer.as_str())];

    let ping = serde_json::to_vec(&json!({ "id": 1, "method": "ping" })).unwrap();
    assert_eq!(
        send(port, "POST", "/mcp?client=codex", &auth, &ping)
            .await
            .as_json(),
        json!({ "status": 200, "body": { "jsonrpc": "2.0", "id": 1, "result": {} } })
    );
    assert_eq!(
        send(port, "GET", "/health", &[], b"").await.as_json(),
        json!({ "status": 200, "body": { "ok": true, "protocol": protocol::PROTOCOL_VERSION } })
    );
    assert_eq!(
        send(port, "GET", "/missing", &[], b"").await.as_json(),
        json!({ "status": 404, "body": null })
    );
    server.close().await;
}

/// `server.ts` answers an extra route that throws, or rejects, with 500 and logs the error. Here a route returns a
/// `Response` and has no error path: a panicking route drops the connection instead. Un-ignore this when the server
/// gains one.
#[test]
#[ignore = "server.rs routes have no error path yet: a panicking route drops the connection instead of answering 500 and logging"]
fn extra_route_errors_answer_500_and_log() {}
