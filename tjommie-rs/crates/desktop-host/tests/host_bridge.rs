//! Port of `native-browser-host.test.ts`, against the daemon's real broker and server (in-process, on loopback): the
//! bridge long-polls `/browser-host` with the private host token, runs each job and posts its answer. Plus what a
//! daemon restart asks of the bridge (review focus 2, browser half).

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use daemon::native_browser_broker::{BotIdentity, NativeBrowserBroker};
use daemon::server::{Handlers, Routes, Server, ServerOptions};
use desktop_host::browser::engine::Reply;
use desktop_host::browser::host_bridge::{BridgeConfig, Executor, HostBridge, Job};
use futures::future::BoxFuture;
use protocol::browser::BrowserState;
use protocol::native_browser::{NativeBrowserCommand, NativeBrowserStreamMessage, StreamEnd};
use serde_json::{Value, json};
use tokio::sync::Notify;

fn secret(letter: char) -> String {
    letter.to_string().repeat(43)
}

fn bot(id: &str) -> BotIdentity {
    BotIdentity {
        id: id.into(),
        name: id.into(),
        home_folder: format!("C:/synthetic/{id}"),
        project_folder: None,
    }
}

fn sample_state(bot_id: &str) -> BrowserState {
    serde_json::from_value(json!({
        "botId": bot_id, "generation": "g1", "control": "bot", "controlEpoch": 0, "privateEpoch": 0,
        "privateActive": false, "canControl": false, "revision": 3, "viewportRevision": 1, "busy": false,
        "activePageId": "p1", "tabs": [{ "id": "p1", "url": "about:blank", "title": "Synthetic native page" }],
        "dialog": null,
    }))
    .expect("a state the protocol accepts")
}

struct Fixture {
    broker: Arc<NativeBrowserBroker>,
    server: Server,
    port: u16,
    token: String,
}

async fn fixture(token: char, timeout: Duration) -> Fixture {
    let broker = NativeBrowserBroker::new(secret(token), timeout);
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
    Fixture {
        broker,
        server,
        port,
        token: secret(token),
    }
}

impl Fixture {
    fn request(
        &self,
        bot: BotIdentity,
        command: Value,
    ) -> tokio::task::JoinHandle<Result<Value, String>> {
        let broker = self.broker.clone();
        tokio::spawn(async move {
            broker
                .request(bot, command, None)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn bridge(&self, executor: impl Executor) -> HostBridge {
        HostBridge::start(
            BridgeConfig {
                port: self.port,
                token: self.token.clone(),
            },
            Arc::new(executor),
            Arc::new(|| {}),
        )
        .expect("bridge")
    }

    async fn close(self) {
        self.broker.close();
        self.server.close().await;
    }
}

/// An executor from a closure that returns a future.
struct By<F>(F);

impl<F, Fut> Executor for By<F>
where
    F: Fn(Job) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Reply, String>> + Send + 'static,
{
    fn execute(&self, job: Job) -> BoxFuture<'static, Result<Reply, String>> {
        Box::pin((self.0)(job))
    }
}

fn kind_of(job: &Job) -> &'static str {
    match &job.command {
        NativeBrowserCommand::Ui(ui) => match ui {
            protocol::native_browser::NativeBrowserUiCommand::Open => "open",
            protocol::native_browser::NativeBrowserUiCommand::State => "state",
            _ => "ui",
        },
        NativeBrowserCommand::Host(host) => match host {
            protocol::native_browser::NativeBrowserHostCommand::CancelTools => "cancel_tools",
            protocol::native_browser::NativeBrowserHostCommand::Remote(_) => "remote",
            _ => "host",
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bridges_a_private_command_and_result_without_sending_jobs_through_renderer_events() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let bridge = fixture.bridge(By(move |job: Job| {
        log.lock()
            .unwrap()
            .push((job.bot.id.clone(), job.bot.home_folder.clone()));
        let state = sample_state(&job.bot.id);
        Box::pin(async move { Ok(Reply::state(state)) })
    }));
    let result = fixture
        .request(bot("one"), json!({ "kind": "open" }))
        .await
        .unwrap()
        .expect("result");
    assert_eq!(result, serde_json::to_value(sample_state("one")).unwrap());
    assert_eq!(result["tabs"][0]["title"], "Synthetic native page");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [("one".to_string(), "C:/synthetic/one".to_string())]
    );
    bridge.close();
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn returns_a_bounded_tool_error_and_can_execute_the_next_job() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let first = Arc::new(AtomicUsize::new(0));
    let bridge = fixture.bridge(By(move |_job: Job| {
        let nth = first.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if nth == 0 {
                Err(format!("Synthetic failure{}", "!".repeat(5000)))
            } else {
                Ok(Reply::empty())
            }
        })
    }));
    let failed = fixture
        .request(bot("one"), json!({ "kind": "state" }))
        .await
        .unwrap()
        .expect_err("a failure");
    assert!(failed.starts_with("Synthetic failure"));
    assert!(
        failed.chars().count() <= 2000,
        "the error is bounded: {} chars",
        failed.chars().count()
    );
    let next = fixture
        .request(bot("one"), json!({ "kind": "state" }))
        .await
        .unwrap()
        .expect("the next job");
    assert_eq!(next, json!({}));
    bridge.close();
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_bots_pending_operation_does_not_stall_a_different_bot() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let gate = Arc::new(Notify::new());
    let waiting = gate.clone();
    let bridge = fixture.bridge(By(move |job: Job| {
        let waiting = waiting.clone();
        Box::pin(async move {
            if job.bot.id == "one" {
                waiting.notified().await;
            }
            Ok(Reply::state(sample_state(&job.bot.id)))
        })
    }));
    let one = fixture.request(bot("one"), json!({ "kind": "state" }));
    let two = fixture
        .request(bot("two"), json!({ "kind": "state" }))
        .await
        .unwrap()
        .expect("bot two");
    assert_eq!(two["botId"], "two");
    gate.notify_one();
    assert_eq!(one.await.unwrap().expect("bot one")["botId"], "one");
    bridge.close();
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keeps_commands_for_the_same_bot_in_dispatch_order() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let gate = Arc::new(Notify::new());
    let order = Arc::new(Mutex::new(Vec::new()));
    let (waiting, log) = (gate.clone(), order.clone());
    let bridge = fixture.bridge(By(move |job: Job| {
        let (waiting, log) = (waiting.clone(), log.clone());
        Box::pin(async move {
            let kind = kind_of(&job);
            log.lock().unwrap().push(kind);
            if kind == "open" {
                waiting.notified().await;
            }
            Ok(Reply::empty())
        })
    }));
    let first = fixture.request(bot("one"), json!({ "kind": "open" }));
    // The second command is issued once the first is running: spawned tasks alone give no arrival order.
    while order.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let second = fixture.request(bot("one"), json!({ "kind": "state" }));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        order.lock().unwrap().as_slice(),
        ["open"],
        "the second waits for the first"
    );
    gate.notify_one();
    first.await.unwrap().expect("first");
    second.await.unwrap().expect("second");
    assert_eq!(order.lock().unwrap().as_slice(), ["open", "state"]);
    bridge.close();
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keeps_cancellation_responsive_when_all_four_normal_workers_are_occupied() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let gate = Arc::new(Notify::new());
    let waiting = gate.clone();
    let bridge = fixture.bridge(By(move |job: Job| {
        let waiting = waiting.clone();
        Box::pin(async move {
            if kind_of(&job) == "open" {
                waiting.notified().await;
            }
            Ok(Reply::empty())
        })
    }));
    let jobs: Vec<_> = ["one", "two", "three", "four"]
        .into_iter()
        .map(|id| fixture.request(bot(id), json!({ "kind": "open" })))
        .collect();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // All four normal workers are busy; the urgent lane still answers.
    let cancelled = tokio::time::timeout(
        Duration::from_secs(3),
        fixture.request(bot("one"), json!({ "kind": "cancel_tools" })),
    )
    .await
    .expect("cancellation must not wait behind the busy workers")
    .unwrap()
    .expect("cancel_tools");
    assert_eq!(cancelled, json!({}));
    for _ in 0..4 {
        gate.notify_one();
    }
    for job in jobs {
        job.await.unwrap().expect("job");
    }
    bridge.close();
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn releases_a_remote_controller_through_the_urgent_lane_while_its_frame_is_pending() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let gate = Arc::new(Notify::new());
    let started = Arc::new(AtomicUsize::new(0));
    let (waiting, begun) = (gate.clone(), started.clone());
    let bridge = fixture.bridge(By(move |job: Job| {
        let (waiting, begun) = (waiting.clone(), begun.clone());
        Box::pin(async move {
            if let NativeBrowserCommand::Host(
                protocol::native_browser::NativeBrowserHostCommand::Remote(remote),
            ) = &job.command
                && matches!(
                    remote.action,
                    protocol::native_browser::NativeBrowserRemoteAction::Frame { .. }
                )
            {
                begun.fetch_add(1, Ordering::SeqCst);
                waiting.notified().await;
            }
            Ok(Reply::empty())
        })
    }));
    let pending = fixture.request(
        bot("one"),
        json!({ "kind": "remote", "controllerId": "connection-one", "action": { "kind": "frame" } }),
    );
    while started.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let released = tokio::time::timeout(
        Duration::from_secs(3),
        fixture.request(
            bot("one"),
            json!({ "kind": "remote", "controllerId": "connection-one", "action": { "kind": "disconnect" } }),
        ),
    )
    .await
    .expect("the disconnect must not wait behind the pending frame")
    .unwrap()
    .expect("disconnect");
    assert_eq!(released, json!({}));
    gate.notify_one();
    pending.await.unwrap().expect("frame");
    bridge.close();
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_the_host_cannot_read_is_answered_so_the_daemon_does_not_wait() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let bridge = fixture.bridge(By(|_job: Job| Box::pin(async { Ok(Reply::empty()) })));
    let refused = fixture
        .request(bot("one"), json!({ "kind": "teleport" }))
        .await
        .unwrap()
        .expect_err("a mismatch");
    assert_eq!(
        refused,
        "The desktop browser command did not match the installed host protocol."
    );
    bridge.close();
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bridge_with_the_wrong_token_reports_errors_and_still_closes_at_once() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let errors = Arc::new(AtomicUsize::new(0));
    let counted = errors.clone();
    let bridge = HostBridge::start(
        BridgeConfig {
            port: fixture.port,
            token: secret('x'),
        },
        Arc::new(By(|_job: Job| Box::pin(async { Ok(Reply::empty()) }))),
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("bridge");
    let deadline = Instant::now() + Duration::from_secs(5);
    while errors.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "a refused token must be reported"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let closing = Instant::now();
    bridge.close();
    assert!(
        closing.elapsed() < Duration::from_secs(2),
        "closing took {:?}",
        closing.elapsed()
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_is_served_by_a_new_bridge_with_its_new_port_and_token() {
    let errors = Arc::new(AtomicUsize::new(0));
    let counted = errors.clone();
    let first = fixture('a', Duration::from_secs(5)).await;
    let old = HostBridge::start(
        BridgeConfig {
            port: first.port,
            token: first.token.clone(),
        },
        Arc::new(By(|job: Job| {
            let state = sample_state(&job.bot.id);
            Box::pin(async move { Ok(Reply::state(state)) })
        })),
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("bridge");
    assert!(
        first
            .request(bot("one"), json!({ "kind": "state" }))
            .await
            .unwrap()
            .is_ok()
    );
    // The daemon goes away while the bridge is waiting on its long poll.
    let old_token = first.token.clone();
    first.close().await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while errors.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "the dead daemon must be reported"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let closing = Instant::now();
    old.close();
    assert!(
        closing.elapsed() < Duration::from_secs(2),
        "closing the old bridge took {:?}",
        closing.elapsed()
    );
    // The new daemon has another port and another token; a new bridge serves it.
    let second = fixture('b', Duration::from_secs(5)).await;
    assert_ne!(
        second.token, old_token,
        "a restarted daemon has a new host token"
    );
    let fresh = second.bridge(By(|job: Job| {
        let state = sample_state(&job.bot.id);
        Box::pin(async move { Ok(Reply::state(state)) })
    }));
    let answered = second
        .request(bot("two"), json!({ "kind": "state" }))
        .await
        .unwrap()
        .expect("served by the new bridge");
    assert_eq!(answered["botId"], "two");
    fresh.close();
    second.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn posts_a_stream_message_and_learns_whether_anybody_took_it() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let taken = Arc::new(AtomicUsize::new(1));
    let (log, accept) = (seen.clone(), taken.clone());
    fixture.broker.on_stream(Arc::new(move |message| {
        log.lock().unwrap().push(message);
        accept.load(Ordering::SeqCst) == 1
    }));
    let bridge = fixture.bridge(By(|_job: Job| Box::pin(async { Ok(Reply::empty()) })));
    let end = NativeBrowserStreamMessage::StreamEnd {
        bot_id: "one".into(),
        reason: StreamEnd::Private,
    };
    assert!(bridge.send(&end).await, "a taken message is accepted");
    taken.store(0, Ordering::SeqCst);
    assert!(
        !bridge.send(&end).await,
        "with nobody watching the desktop is told to stop"
    );
    assert_eq!(seen.lock().unwrap().len(), 2);
    bridge.close();
    fixture.close().await;
}
