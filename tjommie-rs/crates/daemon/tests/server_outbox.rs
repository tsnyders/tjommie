//! The server's per-connection event queue (M8, T3.1 review item 5): `ws` queued without limit; this one holds at most 64 MiB
//! for a client and disconnects one that stops reading, and an event can go to one connection by its id.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::connect;
use daemon::server::{Handlers, Server, ServerOptions};
use protocol::Event;
use protocol::wire::{Id, MessageDelta, method};
use serde_json::json;

fn options(handlers: Handlers) -> ServerOptions {
    ServerOptions {
        port: 0,
        token: "t".into(),
        handlers,
        allowed_origins: vec![],
        defer_requests: false,
    }
}

/// Handlers whose `bots.list` records the connection id of each caller, in call order.
fn recording(ids: Arc<Mutex<Vec<String>>>) -> Handlers {
    Handlers::default().on::<method::BotsList, _>(move |_, context| {
        ids.lock()
            .unwrap()
            .push(context.expect("a connection").connection_id);
        async { Ok(Vec::new()) }
    })
}

#[tokio::test]
async fn sends_an_event_to_the_named_connection_only() {
    let ids: Arc<Mutex<Vec<String>>> = Arc::default();
    let server = Server::start(options(recording(ids.clone())))
        .await
        .unwrap();
    let (mut first, mut second) = (connect(server.port()).await, connect(server.port()).await);
    for client in [&mut first, &mut second] {
        client.hello("t").await;
        client.call(1, "bots.list", json!({})).await;
    }
    let (one, two) = {
        let ids = ids.lock().unwrap();
        (ids[0].clone(), ids[1].clone())
    };
    assert_ne!(one, two);
    let events = server.broadcaster();
    let event = Event::BotDeleted(Id { id: "b1".into() });
    assert!(events.send_to(&one, &event));
    assert_eq!(
        first.next().await,
        json!({ "type": "event", "event": "bot.deleted", "data": { "id": "b1" } })
    );
    assert!(
        second.try_next(Duration::from_millis(300)).await.is_none(),
        "the other connection hears nothing"
    );
    assert!(!events.send_to("no-such-connection", &event));
    server.close().await;
}

#[tokio::test]
async fn disconnects_a_client_that_stops_reading_instead_of_queueing_for_it_without_limit() {
    let server = Server::start(options(Handlers::default())).await.unwrap();
    // `idle` says hello and then never reads again; `keeping_up` reads everything.
    let mut idle = connect(server.port()).await;
    idle.hello("t").await;
    let mut keeping_up = connect(server.port()).await;
    keeping_up.hello("t").await;
    let reader = tokio::spawn(async move {
        let mut received = 0u32;
        while keeping_up.try_next(Duration::from_secs(3)).await.is_some() {
            received += 1;
        }
        received
    });
    let delta = Event::MessageDelta(MessageDelta {
        id: "m".into(),
        conversation_id: "c".into(),
        delta: "x".repeat(1024 * 1024),
    });
    // 200 MiB offered. A queue without a limit would keep all of it for the idle client; this one drops the client once
    // 64 MiB (plus what its socket buffers took) is waiting.
    let events = server.broadcaster();
    for _ in 0..200 {
        events.send(delta.clone());
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        idle.closed().await,
        None,
        "the idle client is dropped, not told anything it could not read"
    );
    assert_eq!(
        reader.await.unwrap(),
        200,
        "a client that keeps reading still gets every event"
    );
    server.close().await;
}
