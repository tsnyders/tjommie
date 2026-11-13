//! `daemon::m6::install`: the one call that wires attachments, Logins, the native browser and the worker boundary,
//! as `daemon.ts` does. With the desktop host token every host route and method is there; without it (`--dev`) none
//! are, and browser tools answer that the desktop app is needed: there is no Playwright fallback.

mod host_support;

use std::path::Path;
use std::sync::{Arc, Mutex};

use daemon::bots::create_bot;
use daemon::context::Db;
use daemon::m6::{self, M6, M6Deps};
use daemon::server::{Handlers, Principal, RequestContext, Routes, Server, ServerOptions};
use daemon::worker_boundary_host::{WorkerBoundary, WorkerLease};
use futures_util::future::BoxFuture;
use host_support::{get, post};
use protocol::ErrorCode;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

struct Isolated;

impl WorkerBoundary for Isolated {
    fn unavailable_reason(&self) -> BoxFuture<'static, Option<String>> {
        Box::pin(async { None })
    }
    fn acquire(&self) -> BoxFuture<'static, Option<Box<dyn WorkerLease>>> {
        Box::pin(async { None })
    }
    fn invalidate(&self) {}
}

fn host_token() -> String {
    "h".repeat(43)
}

struct Fixture {
    server: Server,
    m6: M6,
    db: Db,
    bot: String,
    _dir: tempfile::TempDir,
}

fn database(root: &Path) -> (Db, String) {
    std::fs::create_dir_all(root.join("shared-files")).unwrap();
    let connection = daemon::db::open(Path::new(":memory:")).unwrap();
    let input = serde_json::from_value(json!({
        "name": "Browser", "color": "blue", "shape": "circle", "instructions": "", "provider": "codex", "model": "synthetic"
    }))
    .unwrap();
    let bot = create_bot(&connection, &root.join("bots"), &input).unwrap();
    connection
        .execute("UPDATE bots SET browser = 1 WHERE id = ?1", [&bot.id])
        .unwrap();
    (Arc::new(Mutex::new(connection)), bot.id)
}

async fn fixture(desktop_host_token: Option<String>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let (db, bot) = database(dir.path());
    let session = bot.clone();
    let mut handlers = Handlers::default();
    let mut routes = Routes::default();
    let m6 = m6::install(
        &mut handlers,
        &mut routes,
        M6Deps {
            db: db.clone(),
            data_root: dir.path().to_path_buf(),
            desktop_host_token,
            bot_for: Arc::new(move |token: &str| {
                (token == "synthetic-session").then(|| session.clone())
            }),
            worker: Some(Arc::new(Isolated)),
        },
    )
    .unwrap();
    let server = Server::start(ServerOptions {
        port: 0,
        token: "rpc-token".into(),
        handlers: Handlers::default(),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap();
    server.activate_with("rpc-token", handlers, routes).unwrap();
    Fixture {
        server,
        m6,
        db,
        bot,
        _dir: dir,
    }
}

fn local() -> RequestContext {
    RequestContext {
        connection_id: "local-one".into(),
        principal: Principal::Local,
        signal: CancellationToken::new(),
    }
}

impl Fixture {
    async fn code(&self, method: &str, params: Value) -> ErrorCode {
        self.server
            .dispatch(method, params, Some(local()))
            .await
            .unwrap_err()
            .code
    }

    async fn status(&self, path: &str) -> u16 {
        get(
            self.server.port(),
            path,
            &[("Authorization", "Bearer wrong-token")],
        )
        .await
        .status
    }

    async fn close(self) {
        self.m6.close();
        self.server.close().await;
    }
}

#[tokio::test]
async fn production_mounts_every_host_route_and_method_and_keeps_one_attachment_host_id() {
    let f = fixture(Some(host_token())).await;
    for path in [
        "/browser-host",
        "/vault-host",
        "/worker-host",
        "/mcp/browser",
    ] {
        assert_eq!(f.status(path).await, 401, "{path}");
    }
    let worker = post(
        f.server.port(),
        "/worker-host",
        &[("Authorization", &format!("Bearer {}", host_token()))],
        br#"{"kind":"status"}"#,
    )
    .await
    .json();
    assert_eq!(worker["available"], true);
    let missing = json!({ "attachmentId": "missing" });
    assert_ne!(
        f.code("attachments.metadata", missing).await,
        ErrorCode::UnknownMethod
    );
    assert_eq!(
        f.server
            .dispatch("vault.status", json!({}), None)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    let open = json!({ "botId": "missing" });
    assert_eq!(f.code("browser.open", open).await, ErrorCode::NotFound);
    let host_id = |db: &Db| -> String {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT value FROM settings WHERE key = 'attachmentHostId'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let first = host_id(&f.db);
    let (db, root) = (f.db.clone(), f._dir.path().to_path_buf());
    let again = m6::install(
        &mut Handlers::default(),
        &mut Routes::default(),
        M6Deps {
            db: db.clone(),
            data_root: root,
            desktop_host_token: None,
            bot_for: Arc::new(|_: &str| None),
            worker: None,
        },
    )
    .unwrap();
    assert_eq!(host_id(&db), first);
    again.close();
    f.close().await;
}

#[tokio::test]
async fn development_has_no_host_routes_and_browser_tools_answer_that_the_desktop_app_is_needed() {
    let f = fixture(None).await;
    for path in ["/browser-host", "/vault-host", "/worker-host"] {
        assert_eq!(f.status(path).await, 404, "{path}");
    }
    assert_eq!(
        f.code("vault.status", json!({})).await,
        ErrorCode::UnknownMethod
    );
    let open = json!({ "botId": f.bot });
    assert_eq!(f.code("browser.open", open).await, ErrorCode::UnknownMethod);
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "browser_snapshot", "arguments": {} }
    });
    let reply = post(
        f.server.port(),
        "/mcp/browser",
        &[("Authorization", "Bearer synthetic-session")],
        call.to_string().as_bytes(),
    )
    .await
    .json();
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("needs the Tjommie desktop app"), "{text}");
    let shown = f.m6.show_browser(&f.bot).await.unwrap_err();
    assert_eq!(shown.code, ErrorCode::Unavailable);
    f.close().await;
}

/// `NativeRequest`'s rule: never lock at call time. The runner makes its browser fence (and any hook that closes a
/// session) with the database locked, and that mutex is not reentrant, so a request that read the database as it was
/// made would wait forever on its own caller's lock. The close runs on its own thread so a hang fails this test
/// instead of stopping the run.
#[tokio::test]
async fn closing_a_bots_browser_tools_does_not_wait_on_a_database_lock_its_caller_holds() {
    let f = fixture(Some(host_token())).await;
    let (db, tools, bot) = (f.db.clone(), f.m6.browser_mcp.clone(), f.bot.clone());
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _held = db.lock().unwrap();
        tools.close(&bot);
        let _ = done.send(());
    });
    finished
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("close returned while its caller held the database lock");
    f.close().await;
}
