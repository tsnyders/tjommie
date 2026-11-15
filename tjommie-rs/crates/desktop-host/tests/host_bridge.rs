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

/// An executor that only counts how often it is told the daemon is lost.
struct CountsLosses(Arc<AtomicUsize>);

impl Executor for CountsLosses {
    fn execute(&self, _job: Job) -> BoxFuture<'static, Result<Reply, String>> {
        Box::pin(async { Ok(Reply::empty()) })
    }

    fn daemon_lost(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
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
async fn the_executor_is_told_the_daemon_is_lost_once_per_outage_not_once_per_error() {
    let fixture = fixture('h', Duration::from_secs(5)).await;
    let (errors, losses) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let counted = errors.clone();
    let bridge = HostBridge::start(
        BridgeConfig {
            port: fixture.port,
            // A refused token: every poll fails, again and again.
            token: secret('x'),
        },
        Arc::new(CountsLosses(losses.clone())),
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("bridge");
    let deadline = Instant::now() + Duration::from_secs(20);
    while errors.load(Ordering::SeqCst) < 3 {
        assert!(Instant::now() < deadline, "the errors keep coming");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        losses.load(Ordering::SeqCst),
        1,
        "{} errors, one outage",
        errors.load(Ordering::SeqCst)
    );
    bridge.close();
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

/// A daemon that hands out one job, then answers every POST of an answer with 413 once, and 204 after that. It records
/// the answers it was sent.
async fn refusing_daemon() -> (u16, Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let (seen, handed) = (bodies.clone(), Arc::new(AtomicUsize::new(0)));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let (seen, handed) = (seen.clone(), handed.clone());
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head_end, length) = loop {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    raw.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while raw.len() < head_end + length {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    raw.extend_from_slice(&chunk[..read]);
                }
                let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
                let reply = if head.starts_with("GET") && handed.fetch_add(1, Ordering::SeqCst) == 0
                {
                    let job = json!({ "id": uuid::Uuid::new_v4().to_string(), "bot": { "id": "one", "name": "One", "homeFolder": "C:/synthetic/one" }, "command": { "kind": "state" } }).to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{job}",
                        job.len()
                    )
                } else if head.starts_with("POST") {
                    let mut posted = seen.lock().unwrap();
                    posted.push(String::from_utf8_lossy(&raw[head_end..]).into_owned());
                    let status = if posted.len() == 1 {
                        "413 Payload Too Large"
                    } else {
                        "204 No Content"
                    };
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                } else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                };
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    (port, bodies)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_the_daemon_refuses_as_too_large_is_not_counted_delivered_and_is_replaced_by_a_short_error()
 {
    let (port, bodies) = refusing_daemon().await;
    let bridge = HostBridge::start(
        BridgeConfig {
            port,
            token: secret('h'),
        },
        Arc::new(By(|job: Job| {
            let state = sample_state(&job.bot.id);
            Box::pin(async move { Ok(Reply::state(state)) })
        })),
        Arc::new(|| {}),
    )
    .expect("bridge");
    let deadline = Instant::now() + Duration::from_secs(10);
    while bodies.lock().unwrap().len() < 2 {
        assert!(
            Instant::now() < deadline,
            "the daemon must be sent a short error after refusing the answer"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let posted = bodies.lock().unwrap().clone();
    assert!(
        posted[0].contains("\"ok\":true"),
        "the first answer is the real one: {}",
        posted[0]
    );
    assert!(
        posted[1].contains("\"ok\":false") && posted[1].contains("too large"),
        "then a short error: {}",
        posted[1]
    );
    assert_eq!(
        bridge.undelivered(),
        1,
        "the refused answer is counted, not treated as delivered"
    );
    bridge.close();
}

/// A daemon that hands out `jobs` jobs (one per GET, `unreadable` ones the host cannot parse) and never answers a POST:
/// it reads the request and holds the connection open. Returns the port and how many POSTs arrived.
async fn deaf_daemon(jobs: usize, unreadable: bool) -> (u16, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (posts, handed) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let counted = posts.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let (posts, handed) = (counted.clone(), handed.clone());
            tokio::spawn(async move {
                let mut chunk = [0u8; 8192];
                let Ok(read) = stream.read(&mut chunk).await else {
                    return;
                };
                let head = String::from_utf8_lossy(&chunk[..read]).into_owned();
                if head.starts_with("POST") {
                    posts.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(3600)).await; // never answers
                    return;
                }
                // Jobs go to the normal lane only; the urgent lane (`?control=1`) just waits.
                let reply = if !head.contains("control=1")
                    && handed.fetch_add(1, Ordering::SeqCst) < jobs
                {
                    let command = if unreadable {
                        json!({ "kind": "teleport" })
                    } else {
                        json!({ "kind": "state" })
                    };
                    let job = json!({ "id": uuid::Uuid::new_v4().to_string(), "bot": { "id": "one", "name": "One", "homeFolder": "C:/synthetic/one" }, "command": command }).to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{job}",
                        job.len()
                    )
                } else {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                };
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    (port, posts)
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_bridge_is_prompt_even_while_an_answer_to_the_daemon_hangs() {
    let (port, posts) = deaf_daemon(1, true).await;
    let bridge = HostBridge::start(
        BridgeConfig {
            port,
            token: secret('h'),
        },
        Arc::new(By(|_job: Job| Box::pin(async { Ok(Reply::empty()) }))),
        Arc::new(|| {}),
    )
    .expect("bridge");
    while posts.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The UI thread closes the bridge at shutdown: a daemon that never answers must not hold it there.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        bridge.close();
        let _ = done.send(());
    });
    assert!(
        finished.recv_timeout(Duration::from_secs(3)).is_ok(),
        "close() was held by an unanswered POST"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_the_daemon_never_takes_does_not_pin_a_lane_slot_for_good() {
    let (port, posts) = deaf_daemon(5, false).await;
    let ran = Arc::new(AtomicUsize::new(0));
    let counted = ran.clone();
    let bridge = HostBridge::start(
        BridgeConfig {
            port,
            token: secret('h'),
        },
        Arc::new(By(move |_job: Job| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Reply::empty()) })
        })),
        Arc::new(|| {}),
    )
    .expect("bridge");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        ran.load(Ordering::SeqCst),
        4,
        "four unanswered posts hold the four normal slots for now"
    );
    // After the answer timeout the slots free and the fifth job runs.
    let deadline = Instant::now()
        + desktop_host::browser::host_bridge::ANSWER_TIMEOUT
        + Duration::from_secs(5);
    while ran.load(Ordering::SeqCst) < 5 {
        assert!(
            Instant::now() < deadline,
            "the slots were never freed ({} posts)",
            posts.load(Ordering::SeqCst)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        bridge.undelivered() >= 1,
        "an answer that timed out is counted as undelivered"
    );
    bridge.close();
}
