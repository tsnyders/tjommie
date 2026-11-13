//! `browser.stream` and `browser.streamAck` for an authenticated connection (M8): the stream is turned on under the caller's
//! own controller id, the daemon watches for that connection only, and acknowledgements free a slot without asking the desktop.

use std::sync::{Arc, Mutex};

use daemon::browser_stream::{BrowserStreams, EventTo};
use daemon::native_browser_handlers::{self, NativeRequest};
use daemon::server::{Handlers, Principal, RequestContext, RpcError, Server, ServerOptions};
use futures_util::future::BoxFuture;
use protocol::browser::BrowserFrame;
use protocol::{ErrorCode, Event};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn state(private: bool) -> Value {
    json!({
        "botId": "one", "generation": "g", "control": "bot", "activePageId": "p", "tabs": [], "dialog": null, "privateActive": private
    })
}

fn context(connection: &str) -> RequestContext {
    RequestContext {
        connection_id: connection.into(),
        principal: Principal::Device {
            device_id: "device-one".into(),
        },
        signal: CancellationToken::new(),
    }
}

fn frame(sequence: u64) -> BrowserFrame {
    serde_json::from_value(json!({
        "data": "image", "mime": "image/jpeg", "width": 1280, "height": 800, "sequence": sequence, "streamId": "stream"
    }))
    .unwrap()
}

type Calls = Arc<Mutex<Vec<Value>>>;
type Sent = Arc<Mutex<Vec<(String, Event)>>>;

struct Fixture {
    server: Server,
    streams: Arc<BrowserStreams>,
    calls: Calls,
    sent: Sent,
}

async fn fixture(
    answer: impl Fn(&Value) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync + 'static,
) -> Fixture {
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let request: NativeRequest = Arc::new(move |_bot: String, command: Value, _authorized| {
        seen.lock().unwrap().push(command.clone());
        answer(&command)
    });
    let sent: Sent = Arc::default();
    let record = sent.clone();
    let streams = BrowserStreams::new();
    let to: EventTo = Arc::new(move |connection, event| {
        record
            .lock()
            .unwrap()
            .push((connection.to_owned(), event.clone()));
        true
    });
    streams.connect(to);
    let handlers =
        native_browser_handlers::register_streaming(Handlers::default(), request, streams.clone());
    let server = Server::start(ServerOptions {
        port: 0,
        token: "synthetic-token".into(),
        handlers,
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap();
    Fixture {
        server,
        streams,
        calls,
        sent,
    }
}

fn answering(private: bool) -> impl Fn(&Value) -> BoxFuture<'static, Result<Value, RpcError>> {
    move |_| Box::pin(async move { Ok(state(private)) })
}

impl Fixture {
    async fn stream(&self, connection: &str, params: Value) -> Result<Value, RpcError> {
        self.server
            .dispatch("browser.stream", params, Some(context(connection)))
            .await
    }

    fn last_command(&self) -> Value {
        self.calls.lock().unwrap().last().unwrap().clone()
    }

    fn sequences(&self, connection: &str) -> Vec<u64> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(to, _)| to == connection)
            .filter_map(|(_, event)| match event {
                Event::BrowserStreamFrame(sent) => sent.frame.as_ref().map(|frame| frame.sequence),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn turns_the_stream_on_under_the_callers_controller_and_answers_the_browser_state() {
    let f = fixture(answering(false)).await;
    let result = f
        .stream("conn", json!({ "botId": "one", "on": true, "maxFps": 30 }))
        .await
        .unwrap();
    protocol::assert_valid("browser.stream", &result);
    assert_eq!(
        f.last_command(),
        json!({ "kind": "remote", "controllerId": "rpc:conn", "action": { "kind": "stream", "on": true, "maxFps": 30 } })
    );
    assert_eq!(f.streams.watching("one"), 1);
    // Without a cap the desktop is asked for every frame it makes.
    f.stream("other", json!({ "botId": "one", "on": true }))
        .await
        .unwrap();
    assert_eq!(f.last_command()["action"]["maxFps"], 60);
    assert_eq!(f.streams.watching("one"), 2);
    f.server.close().await;
}

#[tokio::test]
async fn does_not_watch_when_the_desktop_refuses_or_a_private_interaction_is_on() {
    let refusing = fixture(|_| {
        Box::pin(async {
            Err(RpcError::new(
                ErrorCode::BadState,
                "This bot's browser is off.",
            ))
        })
    })
    .await;
    let error = refusing
        .stream("conn", json!({ "botId": "one", "on": true }))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadState);
    assert_eq!(refusing.streams.watching("one"), 0);
    refusing.server.close().await;

    let private = fixture(answering(true)).await;
    let result = private
        .stream("conn", json!({ "botId": "one", "on": true }))
        .await
        .unwrap();
    assert_eq!(
        result["privateActive"], true,
        "the state says why nothing streams"
    );
    assert_eq!(private.streams.watching("one"), 0);
    private.server.close().await;
}

#[tokio::test]
async fn turning_the_stream_off_stops_watching_and_tells_the_desktop() {
    let f = fixture(answering(false)).await;
    f.stream("conn", json!({ "botId": "one", "on": true }))
        .await
        .unwrap();
    f.stream("conn", json!({ "botId": "one", "on": false }))
        .await
        .unwrap();
    assert_eq!(
        f.last_command(),
        json!({ "kind": "remote", "controllerId": "rpc:conn", "action": { "kind": "stream", "on": false } })
    );
    assert_eq!(f.streams.watching("one"), 0);
    f.server.close().await;
}

#[tokio::test]
async fn an_acknowledgement_frees_a_slot_without_asking_the_desktop() {
    let f = fixture(answering(false)).await;
    f.stream("conn", json!({ "botId": "one", "on": true }))
        .await
        .unwrap();
    let asked = f.calls.lock().unwrap().len();
    for sequence in 1..=3 {
        f.streams.frame("one", frame(sequence));
    }
    assert_eq!(f.sequences("conn"), [1, 2]);
    let acknowledged = f
        .server
        .dispatch(
            "browser.streamAck",
            json!({ "botId": "one", "sequence": 1 }),
            Some(context("conn")),
        )
        .await
        .unwrap();
    assert_eq!(acknowledged, json!({}));
    assert_eq!(f.sequences("conn"), [1, 2, 3]);
    assert_eq!(f.calls.lock().unwrap().len(), asked);
    f.server.close().await;
}

#[tokio::test]
async fn needs_an_active_connection_for_both_methods() {
    let f = fixture(answering(false)).await;
    for (method, params) in [
        ("browser.stream", json!({ "botId": "one", "on": true })),
        (
            "browser.streamAck",
            json!({ "botId": "one", "sequence": 1 }),
        ),
    ] {
        let error = f.server.dispatch(method, params, None).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::BadState, "{method}");
    }
    assert_eq!(f.streams.watching("one"), 0);
    assert!(f.calls.lock().unwrap().is_empty());
    f.server.close().await;
}

#[tokio::test]
async fn is_not_a_method_without_the_streaming_install() {
    let request: NativeRequest =
        Arc::new(|_bot: String, _command: Value, _authorized| Box::pin(async { Ok(state(false)) }));
    let server = Server::start(ServerOptions {
        port: 0,
        token: "synthetic-token".into(),
        handlers: native_browser_handlers::register(Handlers::default(), request),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap();
    for (method, params) in [
        ("browser.stream", json!({ "botId": "one", "on": true })),
        (
            "browser.streamAck",
            json!({ "botId": "one", "sequence": 1 }),
        ),
    ] {
        let error = server
            .dispatch(method, params, Some(context("conn")))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnknownMethod, "{method}");
    }
    server.close().await;
}

#[tokio::test]
async fn a_connection_that_ends_stops_being_watched_without_a_word() {
    let f = fixture(answering(false)).await;
    let ending = context("ending");
    f.server
        .dispatch(
            "browser.stream",
            json!({ "botId": "one", "on": true }),
            Some(ending.clone()),
        )
        .await
        .unwrap();
    assert_eq!(f.streams.watching("one"), 1);
    ending.signal.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while f.streams.watching("one") != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the watcher is removed when its connection ends");
    f.streams.frame("one", frame(1));
    assert!(f.sequences("ending").is_empty());
    f.server.close().await;
}
