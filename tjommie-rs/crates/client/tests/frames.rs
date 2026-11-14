//! The client against a scripted server: what the real daemon never sends (answers that break the contract, a wrong
//! protocol, a dropped connection) and what it sends in an order the daemon doesn't produce (replies out of order).

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::Duration;

use client::{Client, ConnectError, ConnectOptions, Endpoint, RpcError};
use futures_util::{SinkExt, StreamExt};
use protocol::wire::{Empty, method};
use protocol::{ErrorCode, Event};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

type Ws = WebSocketStream<TcpStream>;

/// Serves one connection with `script`; returns the port.
async fn serve<F, Fut>(script: F) -> u16
where
    F: FnOnce(Ws) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        script(tokio_tungstenite::accept_async(stream).await.unwrap()).await;
    });
    port
}

async fn next_text(ws: &mut Ws) -> Value {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::text(value.to_string())).await.unwrap();
}

async fn close_with(ws: &mut Ws, code: u16, reason: &str) {
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: reason.to_owned().into(),
    };
    let _ = ws.close(Some(frame)).await;
}

/// Reads the hello, checks what it carries and welcomes the client.
async fn welcome(ws: &mut Ws) {
    let hello = next_text(ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["protocol"], protocol::PROTOCOL_VERSION);
    assert_eq!(hello["token"], "tok");
    send(
        ws,
        json!({ "type": "welcome", "protocol": protocol::PROTOCOL_VERSION }),
    )
    .await;
}

async fn connect(port: u16) -> Result<Client, ConnectError> {
    Client::connect(Endpoint::Local { port }, "tok").await
}

fn settings(chain_limit: u32) -> Value {
    json!({ "chainLimit": chain_limit, "relayUrl": null })
}

#[tokio::test]
async fn hello_carries_the_protocol_and_token_and_a_result_comes_back_typed() {
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        let request = next_text(&mut ws).await;
        assert_eq!(request["type"], "req");
        assert_eq!(request["method"], "bots.list");
        assert_eq!(request["params"], json!({}));
        send(
            &mut ws,
            json!({ "type": "res", "id": request["id"], "ok": true, "result": [] }),
        )
        .await;
    })
    .await;
    let client = connect(port).await.unwrap();
    assert!(
        client
            .call::<method::BotsList>(Empty {})
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_daemon_error_becomes_an_rpc_error() {
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        let request = next_text(&mut ws).await;
        send(
            &mut ws,
            json!({ "type": "res", "id": request["id"], "ok": false, "error": { "code": "not_found", "message": "No such bot" } }),
        )
        .await;
        // The connection stays up after a refused call.
        let again = next_text(&mut ws).await;
        send(&mut ws, json!({ "type": "res", "id": again["id"], "ok": true, "result": settings(3) })).await;
    })
    .await;
    let client = connect(port).await.unwrap();
    let refused = client.call::<method::BotsList>(Empty {}).await.unwrap_err();
    match refused {
        RpcError::Daemon { code, message } => {
            assert_eq!(code, ErrorCode::NotFound);
            assert_eq!(message, "No such bot");
        }
        other => panic!("expected a daemon error, got {other}"),
    }
    assert_eq!(
        client
            .call::<method::SettingsGet>(Empty {})
            .await
            .unwrap()
            .chain_limit,
        3
    );
}

#[tokio::test]
async fn a_result_that_breaks_the_contract_closes_the_connection_with_4400() {
    let (seen, closed) = tokio::sync::oneshot::channel();
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        let request = next_text(&mut ws).await;
        send(
            &mut ws,
            json!({ "type": "res", "id": request["id"], "ok": true, "result": { "nope": true } }),
        )
        .await;
        let _ = seen.send(ws.next().await);
    })
    .await;
    let client = connect(port).await.unwrap();
    let failed = client.call::<method::BotsList>(Empty {}).await.unwrap_err();
    match failed {
        RpcError::Closed(reason) => {
            assert_eq!(reason.code, 4400);
            assert!(
                reason.reason.starts_with("Invalid server frame"),
                "{reason:?}"
            );
        }
        other => panic!("expected the connection to close, got {other}"),
    }
    match closed.await.unwrap() {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 4400),
        other => panic!("the server should see a 4400 close, got {other:?}"),
    }
}

#[tokio::test]
async fn an_event_it_does_not_know_closes_the_connection_with_4400() {
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        send(
            &mut ws,
            json!({ "type": "event", "event": "made.up", "data": {} }),
        )
        .await;
        let _ = ws.next().await;
    })
    .await;
    let client = connect(port).await.unwrap();
    let reason = tokio::time::timeout(Duration::from_secs(10), client.closed())
        .await
        .unwrap();
    assert_eq!(reason.code, 4400);
}

#[tokio::test]
async fn an_event_whose_data_breaks_the_contract_closes_the_connection_with_4400() {
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        // 99 reads as a u32, so only the manifest (chainLimit is 1 to 20) can refuse it.
        send(
            &mut ws,
            json!({ "type": "event", "event": "settings.changed", "data": settings(99) }),
        )
        .await;
        let _ = ws.next().await;
    })
    .await;
    let client = connect(port).await.unwrap();
    let mut events = client.events();
    let reason = tokio::time::timeout(Duration::from_secs(10), client.closed())
        .await
        .unwrap();
    assert_eq!(reason.code, 4400);
    assert!(events.try_recv().is_err(), "the event was never delivered");
}

#[tokio::test]
async fn events_arrive_in_order() {
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        for limit in [3, 4] {
            send(
                &mut ws,
                json!({ "type": "event", "event": "settings.changed", "data": settings(limit) }),
            )
            .await;
        }
        send(
            &mut ws,
            json!({ "type": "event", "event": "devices.changed", "data": {} }),
        )
        .await;
        let _ = ws.next().await;
    })
    .await;
    let client = connect(port).await.unwrap();
    let mut events = client.events();
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(
            tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(
        matches!(&seen[0], Event::SettingsChanged(settings) if settings.chain_limit == 3),
        "{seen:?}"
    );
    assert!(
        matches!(&seen[1], Event::SettingsChanged(settings) if settings.chain_limit == 4),
        "{seen:?}"
    );
    assert!(matches!(seen[2], Event::DevicesChanged(_)), "{seen:?}");
}

#[tokio::test]
async fn replies_out_of_order_still_reach_their_own_calls() {
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        let mut requests = Vec::new();
        for _ in 0..3 {
            requests.push(next_text(&mut ws).await);
        }
        // Answer last request first. Each method has its own result type, so a reply sent to the wrong call can't pass.
        for request in requests.iter().rev() {
            let result = match request["method"].as_str() {
                Some("settings.get") => settings(9),
                _ => json!([]),
            };
            send(
                &mut ws,
                json!({ "type": "res", "id": request["id"], "ok": true, "result": result }),
            )
            .await;
        }
        let _ = ws.next().await;
    })
    .await;
    let client = connect(port).await.unwrap();
    let (bots, rooms, settings) = tokio::join!(
        client.call::<method::BotsList>(Empty {}),
        client.call::<method::RoomsList>(Empty {}),
        client.call::<method::SettingsGet>(Empty {}),
    );
    assert!(bots.unwrap().is_empty());
    assert!(rooms.unwrap().is_empty());
    assert_eq!(settings.unwrap().chain_limit, 9);
}

#[tokio::test]
async fn a_protocol_the_daemon_refuses_is_typed() {
    let port = serve(|mut ws| async move {
        let _hello = next_text(&mut ws).await;
        close_with(&mut ws, 4003, "protocol 2 required").await;
    })
    .await;
    assert!(matches!(
        connect(port).await,
        Err(ConnectError::ProtocolMismatch)
    ));
}

#[tokio::test]
async fn a_welcome_for_another_protocol_is_a_bad_frame() {
    let port = serve(|mut ws| async move {
        let _hello = next_text(&mut ws).await;
        send(&mut ws, json!({ "type": "welcome", "protocol": 1 })).await;
        let _ = ws.next().await;
    })
    .await;
    assert!(matches!(
        connect(port).await,
        Err(ConnectError::BadFrame(_))
    ));
}

#[tokio::test]
async fn no_welcome_in_time_is_a_timeout() {
    let port = serve(|mut ws| async move {
        let _hello = next_text(&mut ws).await;
        let _ = ws.next().await; // never answers
    })
    .await;
    let options = ConnectOptions {
        welcome_timeout: Duration::from_millis(200),
    };
    let result = Client::connect_with(Endpoint::Local { port }, "tok", options).await;
    assert!(matches!(result, Err(ConnectError::TimedOut)));
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    assert!(matches!(
        connect(port).await,
        Err(ConnectError::Unreachable(_))
    ));
}

#[tokio::test]
async fn a_connection_that_ends_mid_call_fails_the_call_and_reports_the_close() {
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        let _request = next_text(&mut ws).await;
        close_with(&mut ws, 4500, "going away").await;
    })
    .await;
    let client = connect(port).await.unwrap();
    let closed = client.closed();
    let failed = client.call::<method::BotsList>(Empty {}).await.unwrap_err();
    let reason = match failed {
        RpcError::Closed(reason) => reason,
        other => panic!("expected the connection to close, got {other}"),
    };
    assert_eq!(
        (reason.code, reason.reason.as_str(), reason.opened),
        (4500, "going away", true)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), closed)
            .await
            .unwrap(),
        reason
    );
}

#[tokio::test]
async fn closing_the_client_says_goodbye_with_1000() {
    let (seen, closed) = tokio::sync::oneshot::channel();
    let port = serve(|mut ws| async move {
        welcome(&mut ws).await;
        let _ = seen.send(ws.next().await);
    })
    .await;
    let client = connect(port).await.unwrap();
    client.close().await;
    match closed.await.unwrap() {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 1000),
        other => panic!("the server should see a 1000 close, got {other:?}"),
    }
    let after = client.call::<method::BotsList>(Empty {}).await.unwrap_err();
    assert!(matches!(after, RpcError::Closed(_)), "{after}");
}
