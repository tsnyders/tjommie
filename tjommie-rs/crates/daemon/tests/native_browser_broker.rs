//! Port of `packages/daemon/src/native-browser-broker.test.ts`: the desktop main process long-polls `/browser-host`
//! with its private host token for browser jobs and posts each result back.

mod host_support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, HttpBody, to_bytes};
use axum::response::Response;
use daemon::host_poll;
use daemon::native_browser_broker::{BotIdentity, NativeBrowserBroker};
use daemon::server::{Handlers, Routes, Server, ServerOptions};
use host_support::{get, post};
use protocol::native_browser::{NativeBrowserStreamMessage, StreamEnd};
use serde_json::{Value, json};

fn secret() -> String {
    "h".repeat(43)
}

fn auth() -> String {
    format!("Bearer {}", secret())
}

fn bot() -> BotIdentity {
    BotIdentity {
        id: "bot-one".into(),
        name: "One".into(),
        home_folder: "C:/synthetic/bot-one".into(),
        project_folder: None,
    }
}

struct Fixture {
    broker: Arc<NativeBrowserBroker>,
    server: Server,
    port: u16,
}

async fn fixture(timeout: Duration) -> Fixture {
    let broker = NativeBrowserBroker::new(secret(), timeout);
    let server = Server::start(ServerOptions {
        port: 0,
        token: "renderer-token".into(),
        handlers: Handlers::default(),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap();
    server
        .activate_with(
            "renderer-token",
            Handlers::default(),
            Routes::default().route("/browser-host", broker.route()),
        )
        .unwrap();
    let port = server.port();
    Fixture {
        broker,
        server,
        port,
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

    async fn close(self) {
        self.broker.close();
        self.server.close().await;
    }
}

#[tokio::test]
async fn does_not_accept_the_renderers_daemon_token_or_a_browser_origin() {
    let f = fixture(Duration::from_secs(1)).await;
    assert_eq!(
        get(
            f.port,
            "/browser-host",
            &[("Authorization", "Bearer renderer-token")]
        )
        .await
        .status,
        401
    );
    assert_eq!(
        get(
            f.port,
            "/browser-host",
            &[("Authorization", &auth()), ("Origin", "app://tjommie")]
        )
        .await
        .status,
        403
    );
    f.close().await;
}

#[tokio::test]
async fn delivers_only_to_the_private_host_and_settles_each_command_once() {
    let f = fixture(Duration::from_secs(1)).await;
    let result = f.request(bot(), json!({ "kind": "state" }));
    let job = get(f.port, "/browser-host", &[("Authorization", &auth())])
        .await
        .json();
    assert_eq!(
        job["bot"],
        json!({ "id": "bot-one", "name": "One", "homeFolder": "C:/synthetic/bot-one" })
    );
    assert_eq!(job["command"], json!({ "kind": "state" }));
    let reply =
        json!({ "id": job["id"], "ok": true, "result": { "title": "Synthetic page" } }).to_string();
    assert_eq!(
        post(
            f.port,
            "/browser-host",
            &[("Authorization", &auth())],
            reply.as_bytes()
        )
        .await
        .status,
        204
    );
    assert_eq!(
        result.await.unwrap(),
        Ok(json!({ "title": "Synthetic page" }))
    );
    assert_eq!(
        post(
            f.port,
            "/browser-host",
            &[("Authorization", &auth())],
            reply.as_bytes()
        )
        .await
        .status,
        404
    );
    f.close().await;
}

#[tokio::test]
async fn does_not_accept_unsolicited_or_malformed_replies() {
    let f = fixture(Duration::from_secs(1)).await;
    assert_eq!(
        post(
            f.port,
            "/browser-host",
            &[("Authorization", &auth())],
            b"{}"
        )
        .await
        .status,
        400
    );
    let unknown = json!({ "id": "unknown", "ok": true }).to_string();
    assert_eq!(
        post(
            f.port,
            "/browser-host",
            &[("Authorization", &auth())],
            unknown.as_bytes()
        )
        .await
        .status,
        404
    );
    let oversized = vec![b'x'; 8 * 1024 * 1024 + 1];
    assert_eq!(
        post(
            f.port,
            "/browser-host",
            &[("Authorization", &auth())],
            &oversized
        )
        .await
        .status,
        413
    );
    f.close().await;
}

#[tokio::test]
async fn bounds_unavailable_host_waits_and_cancels_only_the_closed_bots_jobs() {
    let f = fixture(Duration::from_millis(25)).await;
    let late = f
        .broker
        .request(bot(), json!({ "kind": "state" }), None)
        .await
        .unwrap_err();
    assert!(late.to_string().contains("not started"), "{late}");
    let f = fixture(Duration::from_secs(5)).await;
    let other = BotIdentity {
        id: "bot-two".into(),
        ..bot()
    };
    let closing = f.request(bot(), json!({ "kind": "state" }));
    let kept = f.request(other, json!({ "kind": "state" }));
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.broker.close_bot("bot-one");
    assert!(closing.await.unwrap().unwrap_err().contains("closed"));
    // The other bot's job is still waiting for the host.
    let job = get(f.port, "/browser-host", &[("Authorization", &auth())])
        .await
        .json();
    assert_eq!(job["bot"]["id"], "bot-two");
    let reply =
        json!({ "id": job["id"], "ok": false, "error": "Synthetic page error" }).to_string();
    post(
        f.port,
        "/browser-host",
        &[("Authorization", &auth())],
        reply.as_bytes(),
    )
    .await;
    assert_eq!(kept.await.unwrap(), Err("Synthetic page error".to_owned()));
    f.close().await;
}

#[tokio::test]
async fn sends_urgent_control_on_its_own_lane_and_keeps_one_job_per_bot_in_flight() {
    let f = fixture(Duration::from_secs(5)).await;
    let first = f.request(
        bot(),
        json!({ "kind": "tool", "name": "browser_snapshot", "args": {} }),
    );
    let job = get(f.port, "/browser-host", &[("Authorization", &auth())])
        .await
        .json();
    assert_eq!(job["command"]["kind"], "tool");
    // The bot's next normal job waits for the first to answer; a disconnect goes out on the control lane at once.
    let second = f.request(
        bot(),
        json!({ "kind": "tool", "name": "browser_snapshot", "args": {} }),
    );
    let disconnect = f.request(
        bot(),
        json!({ "kind": "remote", "controllerId": "rpc:one", "action": { "kind": "disconnect" } }),
    );
    let control = get(
        f.port,
        "/browser-host?control=1",
        &[("Authorization", &auth())],
    )
    .await
    .json();
    assert_eq!(control["command"]["action"]["kind"], "disconnect");
    let port = f.port;
    let waiting =
        tokio::spawn(
            async move { get(port, "/browser-host", &[("Authorization", &auth())]).await },
        );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !waiting.is_finished(),
        "nothing else for this bot while its tool runs"
    );
    for id in [&job["id"], &control["id"]] {
        let reply = json!({ "id": id, "ok": true, "result": null }).to_string();
        post(
            port,
            "/browser-host",
            &[("Authorization", &auth())],
            reply.as_bytes(),
        )
        .await;
    }
    assert_eq!(first.await.unwrap(), Ok(Value::Null));
    assert_eq!(disconnect.await.unwrap(), Ok(Value::Null));
    let next = waiting.await.unwrap().json();
    assert_eq!(next["command"]["kind"], "tool");
    f.broker.cancel_tools("bot-one");
    assert!(
        second
            .await
            .unwrap()
            .unwrap_err()
            .contains("tool session closed")
    );
    f.close().await;
}

#[tokio::test]
async fn takes_a_live_frame_or_the_end_of_a_stream_from_the_desktop_without_a_job() {
    let f = fixture(Duration::from_secs(1)).await;
    let post_message = |body: Value| {
        let port = f.port;
        async move {
            post(
                port,
                "/browser-host",
                &[("Authorization", &auth())],
                body.to_string().as_bytes(),
            )
            .await
            .status
        }
    };
    let frame = json!({ "data": "image", "mime": "image/jpeg", "width": 1280, "height": 800, "sequence": 4, "streamId": "stream" });
    let live = json!({ "kind": "stream_frame", "botId": "bot-one", "frame": frame });
    // Nobody listens until the daemon installs a sink: the desktop is told to stop.
    assert_eq!(post_message(live.clone()).await, 404);
    let taken: Arc<Mutex<Vec<NativeBrowserStreamMessage>>> = Arc::default();
    let watched = Arc::new(AtomicBool::new(true));
    let (record, watching) = (taken.clone(), watched.clone());
    f.broker.on_stream(Arc::new(move |message| {
        record.lock().unwrap().push(message);
        watching.load(Ordering::SeqCst)
    }));
    assert_eq!(post_message(live.clone()).await, 204);
    watched.store(false, Ordering::SeqCst);
    assert_eq!(
        post_message(live).await,
        404,
        "the sink says nobody watches that bot"
    );
    watched.store(true, Ordering::SeqCst);
    let end = json!({ "kind": "stream_end", "botId": "bot-one", "reason": "private" });
    assert_eq!(post_message(end).await, 204);
    {
        let taken = taken.lock().unwrap();
        assert_eq!(taken.len(), 3);
        assert!(matches!(
            &taken[0],
            NativeBrowserStreamMessage::StreamFrame { bot_id, frame } if bot_id == "bot-one" && frame.sequence == 4
        ));
        assert!(matches!(
            &taken[2],
            NativeBrowserStreamMessage::StreamEnd {
                reason: StreamEnd::Private,
                ..
            }
        ));
    }
    // Malformed posts never reach the sink.
    let unnamed = json!({ "data": "image", "mime": "image/jpeg", "width": 1280, "height": 800, "sequence": 4 });
    for bad in [
        json!({ "kind": "stream_frame", "botId": "bot-one" }),
        json!({ "kind": "stream_frame", "botId": "bot-one", "frame": unnamed }),
        json!({ "kind": "stream_end", "botId": "bot-one", "reason": "bored" }),
        json!({ "kind": "frame", "botId": "bot-one" }),
    ] {
        assert_eq!(post_message(bad.clone()).await, 400, "{bad}");
    }
    assert_eq!(taken.lock().unwrap().len(), 3);
    // The host token is still required.
    let status = post(
        f.port,
        "/browser-host",
        &[("Authorization", "Bearer renderer-token")],
        json!({ "kind": "stream_end", "botId": "bot-one", "reason": "closed" })
            .to_string()
            .as_bytes(),
    )
    .await
    .status;
    assert_eq!(status, 401);
    f.close().await;
}

/// A long poll made straight on the broker's route, so the response can be held and dropped as hyper would.
fn poll(broker: &Arc<NativeBrowserBroker>) -> tokio::task::JoinHandle<Response> {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/browser-host")
        .header("authorization", auth())
        .header("host", "127.0.0.1")
        .body(Body::empty())
        .unwrap();
    tokio::spawn(broker.route()(request))
}

#[tokio::test]
async fn gives_a_job_back_to_the_next_poll_when_the_connection_drops_its_response_unwritten() {
    let broker = NativeBrowserBroker::new(secret(), Duration::from_secs(5));
    let asked = {
        let broker = broker.clone();
        tokio::spawn(async move {
            broker
                .request(bot(), json!({ "kind": "state" }), None)
                .await
        })
    };
    let handed = tokio::time::timeout(Duration::from_secs(5), poll(&broker))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(handed.status(), 200);
    // The connection died before the server took the body to write it: the response is dropped unread.
    drop(handed);
    let next = tokio::time::timeout(Duration::from_secs(5), poll(&broker))
        .await
        .expect("the next poll is handed the job")
        .unwrap();
    assert_eq!(next.status(), 200);
    let job: Value =
        serde_json::from_slice(&to_bytes(next.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(job["command"], json!({ "kind": "state" }));
    // That body was taken to be written, so the job is the desktop's now: a third poll is not handed it again.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), poll(&broker))
            .await
            .is_err(),
        "a job that was written is not handed out twice"
    );
    broker.close();
    assert!(asked.await.unwrap().is_err());
}

#[tokio::test]
async fn a_job_response_reports_its_length_and_calls_back_only_if_dropped_unread() {
    let called = Arc::new(Mutex::new(0u32));
    let record = |called: &Arc<Mutex<u32>>| {
        let called = called.clone();
        Box::new(move || *called.lock().unwrap() += 1) as Box<dyn FnOnce() + Send + Sync>
    };
    let value = json!({ "id": "job", "command": { "kind": "state" } });
    let length = serde_json::to_vec(&value).unwrap().len() as u64;
    let unread = host_poll::job_or(&value, record(&called));
    assert_eq!(unread.headers()["content-type"], "application/json");
    assert_eq!(unread.headers()["cache-control"], "no-store");
    assert_eq!(unread.body().size_hint().exact(), Some(length));
    drop(unread);
    assert_eq!(*called.lock().unwrap(), 1, "dropped before it was written");
    let read = host_poll::job_or(&value, record(&called));
    let bytes = to_bytes(read.into_body(), usize::MAX).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), value);
    assert_eq!(
        *called.lock().unwrap(),
        1,
        "a body that was taken is not called back"
    );
}
