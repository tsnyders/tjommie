//! A `browser` fixture event calls one of the agent's browser tools over its own `browser` MCP server, as the Claude CLI
//! would. The call goes to the real `/mcp/browser` proxy, which hands it to the real `NativeBrowserBroker` as a tool job.
//! The desktop is faked here: a loop long-polls `/browser-host` with the host token, records each job and answers it.
//! Nothing else is started.
#![cfg(feature = "provider-fixtures")]

mod common;
mod host_support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::TempRoot;
use daemon::bots::StoredBot;
use daemon::native_browser_broker::{Authorized, BotIdentity, NativeBrowserBroker};
use daemon::native_browser_handlers::NativeRequest;
use daemon::native_browser_mcp::{NativeBrowserMcp, NativeBrowserMcpOptions};
use daemon::provider_fixtures::{Fixtures, fixture_session_factory};
use daemon::providers::{McpServer, ProviderEvent, ProviderSession, StartOptions};
use daemon::server::{Handlers, Routes, RpcError, Server, ServerOptions};
use futures_util::future::BoxFuture;
use host_support::{get, post};
use protocol::ErrorCode;
use protocol::model::{Autonomy, BotColor, BotShape, Provider};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(20);
const BOT: &str = "bot-ada";
/// The bearer token the session's `browser` server carries (in a daemon, `McpTokens` issues it).
const SESSION_TOKEN: &str = "synthetic-browser-session-token";

/// The desktop host's private token for `/browser-host`.
fn host_secret() -> String {
    "h".repeat(43)
}

/// Two turns: the first navigates (the desktop answers it), the second snapshots (the desktop refuses it).
const ADA: &str = r#"{ "version": 1, "turns": [
  { "events": [
      { "type": "browser", "id": "b1", "tool": "browser_navigate", "arguments": { "url": "http://127.0.0.1:8080/" } },
      { "type": "text", "delta": "Opened it." }, { "type": "text_end" } ] },
  { "events": [ { "type": "browser", "id": "b2", "tool": "browser_snapshot" } ] }
] }"#;

fn fixtures_with(ada: &str) -> (TempRoot, Fixtures) {
    let root = TempRoot::new();
    let dir = root.0.join("fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Ada.json"), ada).unwrap();
    let fixtures = Fixtures::open(&dir).unwrap();
    (root, fixtures)
}

fn bot(browser: bool) -> StoredBot {
    StoredBot {
        id: BOT.into(),
        name: "Ada".into(),
        color: BotColor::Blue,
        shape: BotShape::Circle,
        instructions: String::new(),
        provider: Provider::Claude,
        model: "sonnet".into(),
        account_id: None,
        autonomy: Autonomy::Ask,
        home_folder: String::new(),
        project_folder: None,
        browser,
        paused: false,
        can_message: Vec::new(),
        limited_until: None,
        created_at: "2026-01-01T00:00:00.000Z".into(),
    }
}

fn options(mcp_servers: Vec<McpServer>) -> StartOptions {
    StartOptions {
        cwd: String::new(),
        extra_dirs: Vec::new(),
        model: "sonnet".into(),
        instructions: String::new(),
        autonomy: Autonomy::Ask,
        codex_sandbox_ready: false,
        allow_rules: Vec::new(),
        resume_id: None,
        resume_cost_usd: None,
        env: Default::default(),
        api_key: None,
        max_budget_usd: None,
        mcp_servers,
    }
}

/// The `browser` server the runner gives a bot whose browser is on: its URL and its bearer token.
fn browser_server(port: u16) -> McpServer {
    McpServer::Http {
        name: "browser".into(),
        url: format!("http://127.0.0.1:{port}/mcp/browser"),
        headers: BTreeMap::from([(
            "Authorization".to_owned(),
            format!("Bearer {SESSION_TOKEN}"),
        )]),
        auto_allow: true,
    }
}

async fn play(
    fixtures: Fixtures,
    servers: Vec<McpServer>,
    browser: bool,
) -> (Arc<dyn ProviderSession>, UnboundedReceiver<ProviderEvent>) {
    let session = fixture_session_factory(fixtures)(&bot(browser), options(servers))
        .expect("the factory makes a session");
    let events = session.subscribe();
    session.start().await.unwrap();
    (session, events)
}

/// Every event up to and including the turn's end.
async fn turn(events: &mut UnboundedReceiver<ProviderEvent>) -> Vec<ProviderEvent> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("an event arrives in time")
            .expect("the session still sends events");
        let done = matches!(event, ProviderEvent::TurnDone(_));
        seen.push(event);
        if done {
            return seen;
        }
    }
}

/// The events a browser turn shows, as short lines: its activity, its text and the turn's end.
fn steps(events: &[ProviderEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Tool { id, name, summary } => {
                Some(format!("tool {id} {name} {summary}"))
            }
            ProviderEvent::ToolDone { id, ok } => Some(format!("done {id} {ok}")),
            ProviderEvent::Text { delta } => Some(format!("text {delta}")),
            ProviderEvent::TurnDone(result) => Some(format!("turn {}", result.ok)),
            _ => None,
        })
        .collect()
}

fn turn_error(events: &[ProviderEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        ProviderEvent::TurnDone(result) => result.error.clone(),
        _ => None,
    })
}

/// What the daemon's `NativeRequest` does with a command: the broker takes it as a job for the bot.
fn route_request(broker: Arc<NativeBrowserBroker>) -> NativeRequest {
    Arc::new(
        move |bot: String, command: Value, authorized: Option<Authorized>| {
            let broker = broker.clone();
            let future: BoxFuture<'static, Result<Value, RpcError>> = Box::pin(async move {
                broker
                    .request(
                        BotIdentity {
                            id: bot,
                            name: "Ada".into(),
                            home_folder: String::new(),
                            project_folder: None,
                        },
                        command,
                        authorized,
                    )
                    .await
                    .map_err(|error| RpcError::new(ErrorCode::Internal, error.0))
            });
            future
        },
    )
}

/// The daemon's `/mcp/browser` proxy and `/browser-host` broker, and a fake desktop that answers each job with `answer`
/// (by the tool's name): `Ok` is the result, `Err` the desktop's error text.
struct Desktop {
    port: u16,
    /// The jobs the broker handed the desktop, in order.
    jobs: Arc<Mutex<Vec<Value>>>,
    poll: tokio::task::JoinHandle<()>,
    broker: Arc<NativeBrowserBroker>,
    server: Server,
}

async fn desktop(answer: fn(&str) -> Result<Value, String>) -> Desktop {
    let secret = host_secret();
    let broker = NativeBrowserBroker::new(secret.clone(), Duration::from_secs(30));
    let mcp = NativeBrowserMcp::new(NativeBrowserMcpOptions {
        bot_for: Arc::new(|token: &str| (token == SESSION_TOKEN).then(|| BOT.to_owned())),
        browser_enabled: Arc::new(|bot: &str| (bot == BOT).then_some(true)),
        request: route_request(broker.clone()),
        cancel_tools: None,
        can_run_tools: None,
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
            Routes::default()
                .route("/mcp/browser", mcp.route())
                .route("/browser-host", broker.route()),
        )
        .unwrap();
    let port = server.port();

    let jobs: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = jobs.clone();
    let auth = format!("Bearer {secret}");
    let poll = tokio::spawn(async move {
        loop {
            let polled = get(port, "/browser-host", &[("Authorization", auth.as_str())]).await;
            if polled.status != 200 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            let job = polled.json();
            let name = job["command"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            seen.lock().unwrap().push(job.clone());
            let reply = match answer(&name) {
                Ok(result) => json!({ "id": job["id"], "ok": true, "result": result }),
                Err(message) => json!({ "id": job["id"], "ok": false, "error": message }),
            };
            post(
                port,
                "/browser-host",
                &[("Authorization", auth.as_str())],
                reply.to_string().as_bytes(),
            )
            .await;
        }
    });
    Desktop {
        port,
        jobs,
        poll,
        broker,
        server,
    }
}

impl Desktop {
    async fn close(self) {
        self.poll.abort();
        self.broker.close();
        self.server.close().await;
    }
}

#[tokio::test]
async fn a_browser_event_reaches_the_broker_as_a_tool_job_and_reports_the_answer() {
    let (_root, fixtures) = fixtures_with(ADA);
    let desktop = desktop(|tool| match tool {
        "browser_navigate" => Ok(json!({ "content": [{ "type": "text", "text": "Opened." }] })),
        _ => Err("The page is not open.".to_owned()),
    })
    .await;

    let (session, mut events) = play(fixtures, vec![browser_server(desktop.port)], true).await;
    session.send("open the page").await.unwrap();
    assert_eq!(
        steps(&turn(&mut events).await),
        [
            "tool b1 Using the browser browser_navigate",
            "done b1 true",
            "text Opened it.",
            "turn true",
        ]
    );
    // The desktop refused the snapshot, so its line is failed, and the turn still ends normally.
    session.send("check the page").await.unwrap();
    assert_eq!(
        steps(&turn(&mut events).await),
        [
            "tool b2 Using the browser browser_snapshot",
            "done b2 false",
            "turn true",
        ]
    );

    let jobs = desktop.jobs.lock().unwrap().clone();
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    assert_eq!(jobs[0]["bot"]["id"], BOT);
    assert_eq!(jobs[0]["command"]["kind"], "tool");
    assert_eq!(jobs[0]["command"]["name"], "browser_navigate");
    assert_eq!(jobs[0]["command"]["args"]["url"], "http://127.0.0.1:8080/");
    assert_eq!(jobs[1]["command"]["name"], "browser_snapshot");
    session.close();
    desktop.close().await;
}

#[tokio::test]
async fn a_browser_event_fails_the_turn_when_the_agents_browser_is_off() {
    let (_root, fixtures) = fixtures_with(ADA);
    let (session, mut events) = play(fixtures, Vec::new(), false).await;
    session.send("open the page").await.unwrap();
    let seen = turn(&mut events).await;
    assert_eq!(steps(&seen), ["turn false"]);
    assert_eq!(
        turn_error(&seen).as_deref(),
        Some(
            "Provider fixtures: this agent's browser is off, so the browser event has no tool to call."
        )
    );
    session.close();
}

#[tokio::test]
async fn a_browser_call_that_cannot_reach_the_proxy_is_a_failed_tool_line_and_the_turn_goes_on() {
    let (_root, fixtures) = fixtures_with(ADA);
    // A port with nothing listening on it: the connection is refused.
    let closed = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);

    let (session, mut events) = play(fixtures, vec![browser_server(port)], true).await;
    session.send("open the page").await.unwrap();
    assert_eq!(
        steps(&turn(&mut events).await),
        [
            "tool b1 Using the browser browser_navigate",
            "done b1 false",
            "text Opened it.",
            "turn true",
        ]
    );
    session.close();
}
