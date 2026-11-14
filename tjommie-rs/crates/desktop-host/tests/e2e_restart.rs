//! Review focus 2, the browser half: the daemon restarts while a bot's panel is open. The engine, the bridge and the
//! daemon's real broker run together: jobs flow from the broker through the bridge to the engine and the real WebView2
//! page; then the daemon goes away and comes back with a new port and token.
//!
//! Expected: the panel and its page survive (they are the desktop's, not the daemon's); the dead daemon is reported (the
//! app locks the vault on it); every browser of a bot deleted or switched off meanwhile closes; and a new bridge with the
//! new token serves the same engine. The shield (T3.5), the vault lock (T3.6) and the UI's reconnect (T1.2, the Client
//! lane) add their own assertions to this test when they land.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::Rig;
use daemon::native_browser_broker::{BotIdentity, NativeBrowserBroker};
use daemon::server::{Handlers, Routes, Server, ServerOptions};
use desktop_host::browser::engine::BotEvent;
use desktop_host::browser::host::{JobQueue, serve};
use desktop_host::browser::host_bridge::{BridgeConfig, HostBridge};
use serde_json::{Value, json};

struct Daemon {
    broker: Arc<NativeBrowserBroker>,
    server: Server,
    port: u16,
    token: String,
}

impl Daemon {
    async fn start(token: char) -> Daemon {
        let token = token.to_string().repeat(43);
        let broker = NativeBrowserBroker::new(token.clone(), Duration::from_secs(20));
        let server = Server::start(ServerOptions {
            port: 0,
            token: "renderer-token".into(),
            handlers: Handlers::default(),
            allowed_origins: Vec::new(),
            defer_requests: false,
        })
        .await
        .expect("server");
        server
            .activate_with(
                "renderer-token",
                Handlers::default(),
                Routes::default().route("/browser-host", broker.route()),
            )
            .expect("route");
        let port = server.port();
        Daemon {
            broker,
            server,
            port,
            token,
        }
    }

    fn identity(rig: &Rig, id: &str) -> BotIdentity {
        let bot = rig.bot(id);
        BotIdentity {
            id: bot.id,
            name: bot.name,
            home_folder: bot.home_folder,
            project_folder: None,
        }
    }
}

/// Waits, pumping the UI thread, for a request the broker is serving through the bridge and the engine.
fn finish(
    rig: &mut Rig,
    runtime: &tokio::runtime::Runtime,
    request: tokio::task::JoinHandle<Result<Value, String>>,
) -> Result<Value, String> {
    rig.ui.wait_until(
        "the daemon's request to be answered",
        Duration::from_secs(30),
        || request.is_finished(),
    );
    runtime.block_on(request).expect("the request task")
}

#[test]
fn e2e_daemon_restart_with_panel_open() {
    let mut rig = Rig::new();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let vault_locks = Arc::new(AtomicUsize::new(0));

    // The first daemon, a bridge to it, and the engine serving its jobs on the UI thread.
    let first = runtime.block_on(Daemon::start('a'));
    let (queue, jobs) = JobQueue::new();
    let spawner = rig.ui.spawner();
    rig.ui.spawn(serve(rig.engine.clone(), jobs, move |task| {
        use futures::task::LocalSpawnExt as _;
        let _ = spawner.spawn_local(task);
    }));
    let locks = vault_locks.clone();
    let old_bridge = HostBridge::start(
        BridgeConfig {
            port: first.port,
            token: first.token.clone(),
        },
        Arc::new(queue),
        Arc::new(move || {
            locks.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("bridge");

    // Two bots with their browsers open; alpha's panel is open and the person holds its browser.
    let alpha = rig.bot("alpha");
    let identities: std::collections::HashMap<&str, BotIdentity> = [
        ("alpha", Daemon::identity(&rig, "alpha")),
        ("beta", Daemon::identity(&rig, "beta")),
        ("gamma", Daemon::identity(&rig, "gamma")),
    ]
    .into_iter()
    .collect();
    let request = |daemon: &Daemon, bot: &str, command: Value| {
        let (broker, identity) = (daemon.broker.clone(), identities[bot].clone());
        runtime.spawn(async move {
            broker
                .request(identity, command, None)
                .await
                .map_err(|error| error.to_string())
        })
    };
    for id in ["alpha", "beta"] {
        let state = finish(
            &mut rig,
            &runtime,
            request(&first, id, json!({ "kind": "open" })),
        )
        .expect("open through the daemon");
        assert_eq!(state["botId"], id);
    }
    rig.load("alpha", "controls.html");
    let panel = rig.panel();
    let engine = rig.engine.clone();
    rig.ui.run({
        let alpha = alpha.clone();
        async move { engine.mount(&alpha, Some(panel)).await.expect("mount") }
    });
    let held = finish(
        &mut rig,
        &runtime,
        request(
            &first,
            "alpha",
            json!({ "kind": "control", "control": "human" }),
        ),
    )
    .expect("take control");
    assert_eq!(held["control"], "human");
    let generation = held["generation"].clone();

    // A phone holds gamma's browser, under the id the old daemon gave its connection; the bot's tools wait for it.
    finish(
        &mut rig,
        &runtime,
        request(&first, "gamma", json!({ "kind": "open" })),
    )
    .expect("open gamma");
    let phone = |action: Value| json!({ "kind": "remote", "controllerId": "rpc:old-phone", "action": action });
    let taken = finish(
        &mut rig,
        &runtime,
        request(
            &first,
            "gamma",
            phone(json!({ "kind": "control", "control": "human" })),
        ),
    )
    .expect("the phone takes control");
    assert_eq!(taken["control"], "human");
    let (engine, gamma) = (rig.engine.clone(), rig.bot("gamma"));
    let refused = rig
        .ui
        .run(async move { engine.resize(&gamma, 800, 600, None).await });
    assert_eq!(
        refused
            .expect_err("the bot waits for the person")
            .to_string(),
        "The person is controlling this browser. Wait until they return control."
    );

    // The daemon restarts: its server and broker go away.
    first.broker.close();
    let old_token = first.token.clone();
    runtime.block_on(first.server.close());
    let waiting = vault_locks.clone();
    rig.ui.wait_until(
        "the dead daemon to be reported",
        Duration::from_secs(15),
        move || waiting.load(Ordering::SeqCst) > 0,
    );
    old_bridge.close();
    assert!(
        vault_locks.load(Ordering::SeqCst) >= 1,
        "the app locks its vault when the daemon is lost"
    );

    // While it was down, beta's bot was deleted: its browser closes. Alpha's panel is untouched by all this.
    let engine = rig.engine.clone();
    rig.ui.run(async move {
        engine
            .on_bot_event(BotEvent::Deleted { id: "beta".into() })
            .await
    });
    assert!(
        rig.engine.snapshot("beta").is_none(),
        "a deleted bot's browser closes"
    );
    // A menu opens over the page while the daemon is down: the shield is the desktop's own and needs no daemon, so it
    // takes its picture and hides the page's window all the same, and lowering it brings the live page back.
    let engine = rig.engine.clone();
    let shield = rig
        .ui
        .run(async move { engine.raise_shield("alpha").await });
    assert!(
        desktop_host::browser::engine::png_size(&shield.png).is_some(),
        "a picture of the page was taken without a daemon"
    );
    assert!(
        rig.engine
            .snapshot("alpha")
            .is_some_and(|s| !s.container_shown),
        "and the page's window is behind the shield"
    );
    rig.engine.lower_shield("alpha");
    let still = rig.engine.snapshot("alpha").expect("alpha is still open");
    assert!(
        still.open && still.visible,
        "the panel's page survives the restart: {still:?}"
    );
    assert_eq!(
        rig.state("alpha").generation,
        generation.as_str().expect("generation"),
        "the same browser session"
    );

    // The old daemon's connections are gone with it, and so are their holds on browsers: no `disconnect` job will
    // ever come for the phone, so the engine hands its browser back itself.
    let engine = rig.engine.clone();
    rig.ui.run(async move { engine.daemon_restarted().await });
    assert_eq!(
        rig.state("gamma").control,
        protocol::browser::BrowserControl::Bot,
        "the phone's hold is gone"
    );

    // The new daemon has another port and another token; the bridge reconnects to it, and the same engine serves it.
    let second = runtime.block_on(Daemon::start('b'));
    assert_ne!(second.token, old_token);
    let (queue, jobs) = JobQueue::new();
    let spawner = rig.ui.spawner();
    rig.ui.spawn(serve(rig.engine.clone(), jobs, move |task| {
        use futures::task::LocalSpawnExt as _;
        let _ = spawner.spawn_local(task);
    }));
    let new_bridge = HostBridge::start(
        BridgeConfig {
            port: second.port,
            token: second.token.clone(),
        },
        Arc::new(queue),
        Arc::new(|| {}),
    )
    .expect("bridge");
    let state = finish(
        &mut rig,
        &runtime,
        request(&second, "alpha", json!({ "kind": "state" })),
    )
    .expect("state through the new daemon");
    assert_eq!(
        state["generation"], generation,
        "the new daemon sees the same live browser"
    );
    assert_eq!(state["control"], "human");
    // The old phone's late commands are refused; the person can take gamma's browser; the bot's tools work again.
    let late = finish(
        &mut rig,
        &runtime,
        request(&second, "gamma", phone(json!({ "kind": "state" }))),
    )
    .expect_err("a gone controller");
    assert_eq!(late, "This browser controller disconnected.");
    let person = finish(
        &mut rig,
        &runtime,
        request(
            &second,
            "gamma",
            json!({ "kind": "control", "control": "human" }),
        ),
    )
    .expect("the person takes control");
    assert_eq!(person["control"], "human");
    let back = finish(
        &mut rig,
        &runtime,
        request(
            &second,
            "gamma",
            json!({ "kind": "control", "control": "bot" }),
        ),
    )
    .expect("and hands it back");
    assert_eq!(back["control"], "bot");
    let (engine, gamma) = (rig.engine.clone(), rig.bot("gamma"));
    rig.ui
        .run(async move { engine.resize(&gamma, 800, 600, None).await })
        .expect("the bot's tools work without reopening");
    // The deleted bot's browser stays off, whoever asks.
    let refused = finish(
        &mut rig,
        &runtime,
        request(&second, "beta", json!({ "kind": "open" })),
    )
    .expect_err("a deleted bot");
    assert!(refused.contains("no longer exists"), "{refused}");

    new_bridge.close();
    runtime.block_on(second.server.close());
}

/// T3.3 through the whole chain: a bot's tool call goes from the daemon's broker through the bridge to the engine and
/// the real page, and the model's answer (text, an error flag) comes back the way the daemon's MCP proxy reads it.
#[test]
fn a_bots_tool_call_travels_through_the_broker_and_bridge_to_the_real_page() {
    let mut rig = Rig::new();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let daemon = runtime.block_on(Daemon::start('t'));
    let (queue, jobs) = JobQueue::new();
    let spawner = rig.ui.spawner();
    rig.ui.spawn(serve(rig.engine.clone(), jobs, move |task| {
        use futures::task::LocalSpawnExt as _;
        let _ = spawner.spawn_local(task);
    }));
    let bridge = HostBridge::start(
        BridgeConfig {
            port: daemon.port,
            token: daemon.token.clone(),
        },
        Arc::new(queue),
        Arc::new(|| {}),
    )
    .expect("bridge");
    let identity = Daemon::identity(&rig, "alpha");
    let request = |command: Value| {
        let (broker, identity) = (daemon.broker.clone(), identity.clone());
        runtime.spawn(async move {
            broker
                .request(identity, command, None)
                .await
                .map_err(|error| error.to_string())
        })
    };
    finish(&mut rig, &runtime, request(json!({ "kind": "open" }))).expect("open");
    rig.load("alpha", "tools.html");

    let clicked = finish(
        &mut rig,
        &runtime,
        request(json!({ "kind": "tool", "name": "browser_click", "args": { "target": "#save" } })),
    )
    .expect("a tool answer");
    let text = clicked["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("button \"Save\""), "{text}");
    assert!(
        clicked.get("isError").is_none(),
        "a tool that worked has no error flag: {clicked}"
    );
    assert_eq!(
        rig.eval("alpha", "document.getElementById('clicks').value"),
        "1"
    );

    // A tool that fails answers in words, with the flag, and is not a transport error.
    let failed = finish(
        &mut rig,
        &runtime,
        request(
            json!({ "kind": "tool", "name": "browser_click", "args": { "target": "#missing" } }),
        ),
    )
    .expect("a failed tool still answers");
    assert_eq!(failed["isError"], true, "{failed}");
    assert!(
        failed["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("exactly one"),
        "{failed}"
    );

    // Cancelling with nothing running is quiet.
    finish(
        &mut rig,
        &runtime,
        request(json!({ "kind": "cancel_tools" })),
    )
    .expect("cancel");

    bridge.close();
    runtime.block_on(daemon.server.close());
}
