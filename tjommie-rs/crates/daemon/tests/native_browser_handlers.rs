//! Port of `packages/daemon/src/native-browser-handlers.test.ts`: `browser.*` for an authenticated connection, sent to
//! the desktop's native browser as remote commands under the caller's own controller id.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::native_browser_handlers::{self, NativeRequest};
use daemon::server::{Handlers, Principal, RequestContext, RpcError, Server, ServerOptions};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn state() -> Value {
    json!({
        "botId": "one", "generation": "g", "control": "bot", "activePageId": "p",
        "tabs": [{ "id": "p", "title": "Page", "url": "https://example.com/" }], "dialog": null
    })
}

fn context(signal: CancellationToken) -> RequestContext {
    RequestContext {
        connection_id: "trusted-connection".into(),
        principal: Principal::Device {
            device_id: "device-one".into(),
        },
        signal,
    }
}

type Calls = Arc<Mutex<Vec<(String, Value, bool)>>>;

fn recording(
    answer: impl Fn(&Value) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync + 'static,
) -> (NativeRequest, Calls) {
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let request: NativeRequest = Arc::new(move |bot: String, command: Value, authorized| {
        seen.lock()
            .unwrap()
            .push((bot, command.clone(), authorized.is_some()));
        answer(&command)
    });
    (request, calls)
}

async fn server(request: NativeRequest) -> Arc<Server> {
    Arc::new(
        Server::start(ServerOptions {
            port: 0,
            token: "synthetic-token".into(),
            handlers: native_browser_handlers::register(Handlers::default(), request),
            allowed_origins: Vec::new(),
            defer_requests: false,
        })
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn binds_frame_input_and_navigation_to_the_transport_connection_and_forwards_the_current_epochs()
 {
    let (request, calls) = recording(|command| {
        let frame = command["action"]["kind"] == "frame";
        Box::pin(async move {
            Ok(if frame {
                json!({ "state": state(), "frame": null })
            } else {
                state()
            })
        })
    });
    let server = server(request).await;
    let ctx = Some(context(CancellationToken::new()));
    let frame = server
        .dispatch("browser.frame", json!({ "botId": "one" }), ctx.clone())
        .await
        .unwrap();
    protocol::assert_valid("browser.frame", &frame);
    assert_eq!(frame, json!({ "state": state(), "frame": null }));
    let navigated = server
        .dispatch("browser.navigate", json!({ "botId": "one", "generation": "g", "pageId": "p", "controlEpoch": 4, "url": "https://example.com/next" }), ctx.clone())
        .await
        .unwrap();
    protocol::assert_valid("browser.navigate", &navigated);
    assert_eq!(
        calls.lock().unwrap().last().unwrap(),
        &(
            "one".to_owned(),
            json!({ "kind": "remote", "controllerId": "rpc:trusted-connection", "action": { "kind": "navigate", "generation": "g", "pageId": "p", "controlEpoch": 4, "url": "https://example.com/next" } }),
            true
        )
    );
    let input = json!({ "botId": "one", "generation": "g", "pageId": "p", "controlEpoch": 4, "viewportRevision": 2, "input": { "kind": "text", "text": "synthetic PIN" } });
    assert_eq!(
        server.dispatch("browser.input", input, ctx).await.unwrap(),
        json!({})
    );
    let (_, sent, _) = calls.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        (&sent["controllerId"], &sent["action"]["kind"]),
        (&json!("rpc:trusted-connection"), &json!("input"))
    );
    assert_eq!(
        (
            &sent["action"]["controlEpoch"],
            &sent["action"]["viewportRevision"]
        ),
        (&json!(4), &json!(2))
    );
}

#[tokio::test]
async fn rejects_malformed_native_state_instead_of_passing_it_to_old_clients() {
    let (request, _) = recording(|_| Box::pin(async { Ok(json!({ "bad": "state" })) }));
    let server = server(request).await;
    assert!(
        server
            .dispatch(
                "browser.open",
                json!({ "botId": "one" }),
                Some(context(CancellationToken::new()))
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejects_absent_or_internal_principals_and_missing_input_epochs_before_host_dispatch() {
    let (request, calls) = recording(|_| Box::pin(async { Ok(state()) }));
    let server = server(request).await;
    let refused = server
        .dispatch("browser.open", json!({ "botId": "one" }), None)
        .await
        .unwrap_err();
    assert!(
        refused.message.contains("authenticated"),
        "{}",
        refused.message
    );
    let ctx = Some(context(CancellationToken::new()));
    let refused = server
        .dispatch("browser.navigate", json!({ "botId": "one", "generation": "g", "pageId": "p", "url": "https://example.com/" }), ctx.clone())
        .await
        .unwrap_err();
    assert!(refused.message.contains("epoch"), "{}", refused.message);
    let refused = server
        .dispatch("browser.input", json!({ "botId": "one", "generation": "g", "pageId": "p", "controlEpoch": 1, "input": { "kind": "text", "text": "synthetic" } }), ctx)
        .await
        .unwrap_err();
    assert!(refused.message.contains("viewport"), "{}", refused.message);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn releases_urgently_on_revoke_or_disconnect_and_discards_a_late_frame() {
    let finish: Arc<Mutex<Option<oneshot::Sender<Value>>>> = Arc::default();
    let parked = finish.clone();
    let (request, calls) = recording(move |command| {
        if command["action"]["kind"] == "frame" {
            let (send, receive) = oneshot::channel();
            *parked.lock().unwrap() = Some(send);
            Box::pin(async move {
                receive
                    .await
                    .map_err(|_| RpcError::new(protocol::ErrorCode::Internal, "gone"))
            })
        } else {
            Box::pin(async { Ok(json!({})) })
        }
    });
    let server = server(request).await;
    let lifetime = CancellationToken::new();
    let caller = server.clone();
    let signal = lifetime.clone();
    let pending = tokio::spawn(async move {
        caller
            .dispatch(
                "browser.frame",
                json!({ "botId": "one" }),
                Some(context(signal)),
            )
            .await
    });
    while finish.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    lifetime.cancel();
    let disconnect = (
        "one".to_owned(),
        json!({ "kind": "remote", "controllerId": "rpc:trusted-connection", "action": { "kind": "disconnect" } }),
        false,
    );
    for _ in 0..200 {
        if calls.lock().unwrap().contains(&disconnect) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(calls.lock().unwrap().contains(&disconnect));
    let _ = finish
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(json!({ "state": state(), "frame": null }));
    let refused = pending.await.unwrap().unwrap_err();
    assert!(refused.message.contains("discarded"), "{}", refused.message);
}

#[tokio::test]
async fn suppresses_unexpected_private_frames_and_stale_ones() {
    let private = |command: &Value| {
        let mut current = state();
        current["privateActive"] = json!(true);
        let frame = json!({ "data": "AAAA", "mime": "image/jpeg", "width": 640, "height": 480, "sequence": 1, "pageId": "p" });
        let _ = command;
        Box::pin(async move { Ok(json!({ "state": current, "frame": frame })) })
            as BoxFuture<'static, Result<Value, RpcError>>
    };
    let (request, _) = recording(private);
    let server = server(request).await;
    let refused = server
        .dispatch(
            "browser.frame",
            json!({ "botId": "one" }),
            Some(context(CancellationToken::new())),
        )
        .await
        .unwrap_err();
    assert!(refused.message.contains("Private"), "{}", refused.message);
    let (stale, _) = recording(|_| {
        let frame = json!({ "data": "AAAA", "mime": "image/jpeg", "width": 640, "height": 480, "sequence": 1, "pageId": "other" });
        Box::pin(async move { Ok(json!({ "state": state(), "frame": frame })) })
    });
    let server = server_for(stale).await;
    let refused = server
        .dispatch(
            "browser.frame",
            json!({ "botId": "one" }),
            Some(context(CancellationToken::new())),
        )
        .await
        .unwrap_err();
    assert!(refused.message.contains("stale"), "{}", refused.message);
}

async fn server_for(request: NativeRequest) -> Arc<Server> {
    server(request).await
}

#[tokio::test]
async fn shares_one_host_frame_request_among_identical_concurrent_calls() {
    let release: Arc<Mutex<Vec<oneshot::Sender<Value>>>> = Arc::default();
    let parked = release.clone();
    let (request, calls) = recording(move |_| {
        let (send, receive) = oneshot::channel();
        parked.lock().unwrap().push(send);
        Box::pin(async move {
            receive
                .await
                .map_err(|_| RpcError::new(protocol::ErrorCode::Internal, "gone"))
        })
    });
    let server = server(request).await;
    let ctx = context(CancellationToken::new());
    let calls_made = |server: Arc<Server>, ctx: RequestContext| {
        tokio::spawn(async move {
            server
                .dispatch("browser.frame", json!({ "botId": "one" }), Some(ctx))
                .await
        })
    };
    let (one, two) = (
        calls_made(server.clone(), ctx.clone()),
        calls_made(server.clone(), ctx),
    );
    while calls.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Ample time for the second call to reach the handler; it must join the first rather than ask the host again.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "the second identical frame request joined the first"
    );
    let _ = release
        .lock()
        .unwrap()
        .remove(0)
        .send(json!({ "state": state(), "frame": null }));
    assert_eq!(one.await.unwrap().unwrap(), two.await.unwrap().unwrap());
}
