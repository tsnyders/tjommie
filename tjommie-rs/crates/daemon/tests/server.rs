//! `server.test.ts`, plus the two `client.test.ts` cases that run against the real server.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    INVALID_UTF8_FRAME, OVERSIZED_FRAME_HEADER, UNMASKED_FRAME, connect, http_get, refused,
    send_raw_frame, try_connect,
};
use daemon::server::{Handlers, Principal, RequestContext, RpcError, Server, ServerOptions};
use protocol::model::Bot;
use protocol::wire::{Empty, Id, method};
use protocol::{ErrorCode, Event, PROTOCOL_VERSION};
use serde_json::json;

fn options(token: &str, handlers: Handlers) -> ServerOptions {
    ServerOptions {
        port: 0,
        token: token.into(),
        handlers,
        allowed_origins: vec![],
        defer_requests: false,
    }
}

fn no_bots() -> Handlers {
    Handlers::default().on::<method::BotsList, _>(|_, _| async { Ok(Vec::<Bot>::new()) })
}

#[tokio::test]
async fn stops_admission_on_existing_sockets_and_internal_dispatch_before_closing_transports() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let handlers = Handlers::default().on::<method::RemotePing, _>(move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { Ok(Empty {}) }
    });
    let server = Server::start(options("fixture", handlers)).await.unwrap();
    let mut client = connect(server.port()).await;
    client.hello("fixture").await;
    assert_eq!(client.call(1, "remote.ping", json!({})).await["ok"], true);
    server.stop_requests();
    assert_eq!(
        client.call(2, "remote.ping", json!({})).await["error"]["code"],
        "bad_state"
    );
    assert_eq!(
        server
            .dispatch("remote.ping", json!({}), None)
            .await
            .unwrap_err()
            .code,
        ErrorCode::BadState
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        server
            .activate("fixture", Handlers::default())
            .unwrap_err()
            .message
            .contains("stopping")
    );
    server.close().await;
}

#[tokio::test]
async fn binds_trusted_context_to_each_authenticated_connection_and_aborts_it_on_close() {
    let observed: Arc<Mutex<Vec<Option<RequestContext>>>> = Arc::default();
    let seen = observed.clone();
    let handlers = Handlers::default().on::<method::BotsList, _>(move |_, context| {
        seen.lock().unwrap().push(context);
        async { Ok(Vec::<Bot>::new()) }
    });
    let server = Server::start(options("synthetic-context-token", handlers))
        .await
        .unwrap();
    let (mut first, mut second) = (connect(server.port()).await, connect(server.port()).await);
    first.hello("synthetic-context-token").await;
    second.hello("synthetic-context-token").await;
    let spoofed = json!({ "context": { "connectionId": "spoofed", "principal": { "kind": "device", "deviceId": "spoofed" } } });
    first.call(1, "bots.list", spoofed).await;
    second.call(2, "bots.list", json!({})).await;
    let contexts: Vec<RequestContext> = observed
        .lock()
        .unwrap()
        .iter()
        .map(|context| context.clone().unwrap())
        .collect();
    assert_eq!(contexts[0].principal, Principal::Local);
    assert_ne!(contexts[0].connection_id, "spoofed");
    assert_ne!(contexts[1].connection_id, contexts[0].connection_id);
    first.close().await;
    tokio::time::timeout(Duration::from_secs(10), contexts[0].signal.cancelled())
        .await
        .expect("the closed connection's context is aborted");
    assert!(!contexts[1].signal.is_cancelled());
    server.dispatch("bots.list", json!({}), None).await.unwrap();
    assert!(observed.lock().unwrap()[2].is_none());
    server.close().await;
    assert!(contexts[1].signal.is_cancelled());
}

#[tokio::test]
async fn holds_http_and_websocket_requests_until_recovery_activates_the_server() {
    let server = Server::start(ServerOptions {
        defer_requests: true,
        ..options("", Handlers::default())
    })
    .await
    .unwrap();
    assert_eq!(http_get(server.port(), "/health").await.0, 503);
    assert_eq!(refused(try_connect(server.port(), &[]).await), 503);
    server.activate("ready", no_bots()).unwrap();
    assert_eq!(
        http_get(server.port(), "/health").await,
        (200, r#"{"ok":true,"protocol":2}"#.to_owned())
    );
    let mut client = connect(server.port()).await;
    assert_eq!(
        client.hello("ready").await,
        json!({ "type": "welcome", "protocol": PROTOCOL_VERSION })
    );
    assert_eq!(
        client.call(1, "bots.list", json!({})).await["result"],
        json!([])
    );
    server.close().await;
}

#[tokio::test]
async fn answers_requests_after_a_valid_hello() {
    let server = Server::start(options("secret", no_bots())).await.unwrap();
    let mut client = connect(server.port()).await;
    assert_eq!(
        client.hello("secret").await,
        json!({ "type": "welcome", "protocol": PROTOCOL_VERSION })
    );
    assert_eq!(
        client.call(7, "bots.list", json!({})).await,
        json!({ "type": "res", "id": 7, "ok": true, "result": [] })
    );
    server.close().await;
}

#[tokio::test]
async fn closes_with_4401_on_a_wrong_or_missing_token_and_4003_on_a_wrong_protocol() {
    let server = Server::start(options("secret", Handlers::default()))
        .await
        .unwrap();
    let mut wrong_token = connect(server.port()).await;
    wrong_token
        .send(json!({ "type": "hello", "protocol": PROTOCOL_VERSION, "token": "nope" }))
        .await;
    // client.test.ts: the reason tells the client why.
    assert_eq!(
        wrong_token.closed().await,
        Some((4401, "unauthorized".into()))
    );
    let mut missing_token = connect(server.port()).await;
    missing_token
        .send(json!({ "type": "hello", "protocol": PROTOCOL_VERSION }))
        .await;
    assert_eq!(
        missing_token.closed().await.map(|(code, _)| code),
        Some(4401)
    );
    let mut wrong_protocol = connect(server.port()).await;
    wrong_protocol
        .send(json!({ "type": "hello", "protocol": 99, "token": "secret" }))
        .await;
    assert_eq!(
        wrong_protocol.closed().await,
        Some((4003, "protocol 2 required".into()))
    );
    server.close().await;
}

#[tokio::test]
async fn closes_with_4400_on_malformed_json() {
    let server = Server::start(options("secret", Handlers::default()))
        .await
        .unwrap();
    let mut client = connect(server.port()).await;
    client.send_text("{not json").await;
    assert_eq!(client.closed().await, Some((4400, "bad message".into())));
    server.close().await;
}

#[tokio::test]
async fn survives_a_bad_websocket_frame() {
    let server = Server::start(options("secret", Handlers::default()))
        .await
        .unwrap();
    assert_eq!(
        send_raw_frame(server.port(), UNMASKED_FRAME).await,
        Some(1002)
    );
    let mut fresh = connect(server.port()).await;
    assert_eq!(
        fresh.hello("secret").await,
        json!({ "type": "welcome", "protocol": PROTOCOL_VERSION })
    );
    server.close().await;
}

#[tokio::test]
async fn reports_bad_params_and_handler_errors_without_closing_the_connection() {
    let handlers = Handlers::default()
        .on::<method::BotsDelete, _>(|_, _| async {
            Err::<Empty, _>(RpcError::new(ErrorCode::NotFound, "No such bot"))
        })
        .on::<method::SettingsGet, _>(|_, _| async {
            Ok(protocol::model::Settings {
                chain_limit: 3,
                relay_url: None,
            })
        });
    let server = Server::start(options("t", handlers)).await.unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    let bad = client.call(1, "bots.delete", json!({})).await;
    assert_eq!(
        (bad["ok"].clone(), bad["error"]["code"].clone()),
        (json!(false), json!("bad_params"))
    );
    assert_eq!(
        client.call(2, "bots.delete", json!({ "id": "x" })).await,
        json!({ "type": "res", "id": 2, "ok": false, "error": { "code": "not_found", "message": "No such bot" } })
    );
    assert_eq!(
        client.call(3, "bots.nope", json!({})).await["error"]["code"],
        "unknown_method"
    );
    // client.test.ts: the connection stays usable after an error.
    assert_eq!(
        client.call(4, "settings.get", json!({})).await["result"],
        json!({ "chainLimit": 3, "relayUrl": null })
    );
    server.close().await;
}

#[tokio::test]
async fn refuses_a_foreign_origin_and_a_foreign_host_and_accepts_the_app_origin() {
    let server = Server::start(ServerOptions {
        allowed_origins: vec!["app://tjommie".into()],
        ..options("t", Handlers::default())
    })
    .await
    .unwrap();
    assert_eq!(
        refused(try_connect(server.port(), &[("Origin", "https://evil.example")]).await),
        403
    );
    let host = format!("evil.example:{}", server.port());
    assert_eq!(
        refused(try_connect(server.port(), &[("Host", &host)]).await),
        403
    );
    try_connect(server.port(), &[("Origin", "app://tjommie")])
        .await
        .expect("the app origin connects");
    server.close().await;
}

#[tokio::test]
async fn broadcasts_events_to_authenticated_clients() {
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    server.broadcast(Event::BotDeleted(Id { id: "b1".into() }));
    assert_eq!(
        client.next().await,
        json!({ "type": "event", "event": "bot.deleted", "data": { "id": "b1" } })
    );
    server.close().await;
}

// Not in server.test.ts: behaviour of server.ts the port keeps.

#[tokio::test]
async fn answers_health_and_nothing_else_over_plain_http() {
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    assert_eq!(
        http_get(server.port(), "/health").await,
        (200, r#"{"ok":true,"protocol":2}"#.to_owned())
    );
    assert_eq!(http_get(server.port(), "/health?x=1").await.0, 404);
    assert_eq!(http_get(server.port(), "/other").await.0, 404);
    server.close().await;
}

#[tokio::test]
async fn names_a_method_without_a_handler_unknown_before_checking_its_params() {
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    let response = client.call(1, "bots.delete", json!({})).await;
    assert_eq!(
        response["error"],
        json!({ "code": "unknown_method", "message": "Unknown method bots.delete" })
    );
    server.close().await;
}

#[tokio::test]
async fn closes_a_socket_that_sends_no_hello_within_five_seconds() {
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    let client = connect(server.port()).await;
    assert_eq!(client.closed().await, Some((4401, "hello required".into())));
    server.close().await;
}

#[tokio::test]
async fn answers_internal_when_a_handler_panics_and_keeps_the_connection() {
    let handlers = no_bots().on::<method::BotsDelete, _>(|_, _| async {
        panic!("synthetic handler failure") as Result<Empty, RpcError>
    });
    let server = Server::start(options("t", handlers)).await.unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    assert_eq!(
        client.call(1, "bots.delete", json!({ "id": "b" })).await["error"]["code"],
        "internal"
    );
    assert_eq!(client.call(2, "bots.list", json!({})).await["ok"], true);
    server.close().await;
}

// T3.1 review follow-up: behaviour of the Node `ws` server the port keeps, and two cases server.test.ts lacks.

#[tokio::test]
async fn echoes_the_clients_close_frame() {
    // RFC 6455 §5.5.1, as ws does: the reply carries the client's code and reason, or no code when the client sent none.
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    assert_eq!(
        client.close_with(Some((1000, "bye"))).await,
        Some(Some((1000, "bye".into())))
    );
    let mut quiet = connect(server.port()).await;
    quiet.hello("t").await;
    assert_eq!(quiet.close_with(None).await, Some(None));
    server.close().await;
}

#[tokio::test]
async fn closes_with_the_code_that_names_a_bad_frame() {
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    assert_eq!(
        send_raw_frame(server.port(), INVALID_UTF8_FRAME).await,
        Some(1007)
    );
    assert_eq!(
        send_raw_frame(server.port(), OVERSIZED_FRAME_HEADER).await,
        Some(1009)
    );
    server.close().await;
}

#[tokio::test]
async fn refuses_a_request_with_two_origin_headers() {
    let server = Server::start(ServerOptions {
        allowed_origins: vec!["app://tjommie".into()],
        ..options("t", Handlers::default())
    })
    .await
    .unwrap();
    let twice = [("Origin", "app://tjommie"), ("Origin", "app://tjommie")];
    assert_eq!(refused(try_connect(server.port(), &twice).await), 403);
    server.close().await;
}

#[tokio::test]
async fn keeps_an_idle_client_connected_after_its_welcome() {
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        client.call(1, "remote.ping", json!({})).await["error"]["code"],
        "unknown_method"
    );
    server.close().await;
}

#[tokio::test]
async fn sends_no_events_to_a_socket_that_has_not_said_hello() {
    let server = Server::start(options("t", Handlers::default()))
        .await
        .unwrap();
    let mut stranger = connect(server.port()).await;
    let mut member = connect(server.port()).await;
    member.hello("t").await;
    server.broadcast(Event::BotDeleted(Id { id: "b1".into() }));
    assert_eq!(member.next().await["event"], "bot.deleted");
    assert_eq!(stranger.try_next(Duration::from_secs(1)).await, None);
    server.close().await;
}

#[tokio::test]
async fn a_slow_handler_does_not_hold_up_a_fast_one() {
    // A handler may wait on SQLite, files or retries. That work runs on the blocking pool, so even on this one-thread
    // runtime the fast request is answered first.
    let handlers = Handlers::default()
        .on::<method::BotsList, _>(|_, _| {
            std::thread::sleep(Duration::from_secs(2));
            async { Ok(Vec::<Bot>::new()) }
        })
        .on::<method::RemotePing, _>(|_, _| async { Ok(Empty {}) });
    let server = Server::start(options("t", handlers)).await.unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    client
        .send(json!({ "type": "req", "id": 1, "method": "bots.list", "params": {} }))
        .await;
    client
        .send(json!({ "type": "req", "id": 2, "method": "remote.ping", "params": {} }))
        .await;
    assert_eq!(client.next().await["id"], 2);
    assert_eq!(client.next().await["id"], 1);
    server.close().await;
}
