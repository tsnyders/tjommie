//! The wires to another PC (`remote-wire.ts`, `frame-socket.ts`): one Noise frame per LAN TCP message (4-byte length
//! prefix) or per relay WebSocket binary message, with the limits of the TypeScript bridge: at most 8 open wires of each
//! kind, frames of 1 to 65535 bytes, and 32 MiB of unsent frames. Everything here is on 127.0.0.1.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::Duration;

use client::Endpoint;
use client::remote::{Wire, WireError, Wires, relay_connect_url, web_socket_frames};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const WAIT: Duration = Duration::from_secs(10);
const HOST_ID: &str = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11";

async fn listener() -> (TcpListener, Endpoint) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (
        listener,
        Endpoint::Lan {
            host: "127.0.0.1".into(),
            port,
        },
    )
}

async fn lan(wires: &Wires, endpoint: &Endpoint) -> Result<Wire, WireError> {
    wires.open(endpoint, WAIT).await
}

#[tokio::test]
async fn carries_one_length_prefixed_lan_frame_each_way() {
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(u32::from_be_bytes(header), 3);
        let mut body = [0u8; 3];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(body, [1, 2, 3]);
        // Two frames in one write: the decoder splits them.
        stream
            .write_all(&[0, 0, 0, 2, 4, 5, 0, 0, 0, 1, 6])
            .await
            .unwrap();
        stream
    });
    let mut wire = lan(&Wires::new(), &endpoint).await.unwrap();
    wire.send(&[1, 2, 3]).unwrap();
    assert_eq!(wire.next().await, Some(vec![4, 5]));
    assert_eq!(wire.next().await, Some(vec![6]));
    let _keep_open = server.await.unwrap();
}

#[tokio::test]
async fn closes_a_lan_wire_when_the_peer_sends_an_oversized_frame() {
    let (listener, endpoint) = listener().await;
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(&65_536u32.to_be_bytes()).await.unwrap();
        tokio::time::sleep(WAIT).await;
    });
    let mut wire = lan(&Wires::new(), &endpoint).await.unwrap();
    assert_eq!(tokio::time::timeout(WAIT, wire.next()).await.unwrap(), None);
    assert_eq!(wire.closed().await, None);
    assert!(wire.send(&[1]).is_err(), "a closed wire sends nothing");
}

#[tokio::test]
async fn rejects_a_ninth_simultaneous_wire_and_frees_a_slot_when_one_closes() {
    let (listener, endpoint) = listener().await;
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let wires = Wires::new();
    let mut open = Vec::new();
    for _ in 0..8 {
        open.push(lan(&wires, &endpoint).await.unwrap());
    }
    assert!(
        lan(&wires, &endpoint).await.is_err(),
        "the ninth is refused"
    );
    open.pop();
    // The slot comes back when the wire is dropped.
    assert!(lan(&wires, &endpoint).await.is_ok());
}

#[tokio::test]
async fn closes_a_lan_wire_before_its_queued_writes_exceed_32_mib() {
    let (listener, endpoint) = listener().await;
    tokio::spawn(async move {
        // Accepts and never reads, so what the client writes piles up.
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(WAIT * 3).await;
        drop(stream);
    });
    let wire = lan(&Wires::new(), &endpoint).await.unwrap();
    let frame = vec![7u8; 65_535];
    let mut accepted = 0usize;
    let failed = loop {
        match wire.send(&frame) {
            Ok(()) => accepted += frame.len() + 4,
            Err(error) => break error,
        }
        assert!(accepted < 256 * 1024 * 1024, "the backlog never filled");
    };
    // The operating system takes some of it; the rest waits here, and never more than 32 MiB of it.
    assert!(
        accepted <= 32 * 1024 * 1024 + 16 * 1024 * 1024,
        "{accepted}"
    );
    assert_eq!(failed, WireError::Unavailable);
    assert_eq!(wire.closed().await, None);
}

#[tokio::test]
async fn refuses_frames_that_are_empty_or_too_large_and_closes_the_wire() {
    for bad in [Vec::new(), vec![0u8; 65_536]] {
        let (server, endpoint) = listener().await;
        tokio::spawn(async move {
            let _held = server.accept().await;
            tokio::time::sleep(WAIT).await;
        });
        let wire = lan(&Wires::new(), &endpoint).await.unwrap();
        assert!(wire.send(&bad).is_err());
        assert!(
            wire.send(&[1]).is_err(),
            "the first violation closed the wire"
        );
    }
}

#[tokio::test]
async fn refuses_malformed_endpoints_and_timeouts() {
    let wires = Wires::new();
    let bad = [
        Endpoint::Lan {
            host: "not an ip".into(),
            port: 47101,
        },
        Endpoint::Lan {
            host: "example.com".into(),
            port: 47101,
        },
        Endpoint::Lan {
            host: "127.0.0.1".into(),
            port: 0,
        },
        Endpoint::Relay {
            url: "http://relay.example.workers.dev".into(),
            host_id: HOST_ID.into(),
        },
        Endpoint::Relay {
            url: "https://relay.example.workers.dev".into(),
            host_id: "not-a-uuid".into(),
        },
        Endpoint::Local { port: 47100 },
    ];
    for endpoint in &bad {
        assert!(wires.open(endpoint, WAIT).await.is_err(), "{endpoint:?}");
    }
    let (_listener, endpoint) = listener().await;
    for timeout in [Duration::ZERO, Duration::from_secs(61)] {
        assert!(wires.open(&endpoint, timeout).await.is_err(), "{timeout:?}");
    }
    // Nothing is listening there, so the dial fails rather than hanging.
    let (closed, endpoint) = listener().await;
    drop(closed);
    assert!(lan(&wires, &endpoint).await.is_err());
}

#[test]
fn the_relay_url_is_wss_with_the_hosts_connect_path() {
    assert_eq!(
        relay_connect_url("https://relay.example.workers.dev/ignored?x=1#y", HOST_ID).unwrap(),
        format!("wss://relay.example.workers.dev/v1/hosts/{HOST_ID}/connect")
    );
    assert_eq!(
        relay_connect_url("https://relay.example.workers.dev:8443", "a b/c").unwrap(),
        "wss://relay.example.workers.dev:8443/v1/hosts/a%20b%2Fc/connect"
    );
    assert!(relay_connect_url("http://relay.example.workers.dev", HOST_ID).is_err());
}

/// A relay that sends `script`, then closes with `code`.
async fn relay(script: Vec<Message>, code: u16) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "ws://127.0.0.1:{}/v1/hosts/{HOST_ID}/connect",
        listener.local_addr().unwrap().port()
    );
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        for message in script {
            socket.send(message).await.unwrap();
        }
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: "".into(),
        };
        let _ = socket.close(Some(frame)).await;
        while socket.next().await.is_some() {}
    });
    url
}

#[tokio::test]
async fn forwards_relay_frames_and_reports_the_relays_close_code() {
    let url = relay(vec![Message::binary(vec![9, 9])], 4404).await;
    let mut wire = web_socket_frames(&url, WAIT).await.unwrap();
    assert_eq!(wire.next().await, Some(vec![9, 9]));
    assert_eq!(wire.next().await, None);
    // 4404: the PC is offline. The close code is kept, even though nobody was waiting for it.
    assert_eq!(wire.closed().await, Some(4404));
}

#[tokio::test]
async fn a_relay_that_sends_text_or_an_empty_frame_is_closed() {
    for bad in [Message::text("hello"), Message::binary(Vec::<u8>::new())] {
        let url = relay(vec![bad], 1000).await;
        let mut wire = web_socket_frames(&url, WAIT).await.unwrap();
        assert_eq!(wire.next().await, None);
        assert!(wire.send(&[1]).is_err());
    }
}

#[tokio::test]
async fn sends_each_frame_to_the_relay_as_one_binary_message() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket.next().await.unwrap().unwrap()
    });
    let wire = web_socket_frames(&url, WAIT).await.unwrap();
    wire.send(&[5, 6]).unwrap();
    assert_eq!(server.await.unwrap(), Message::binary(vec![5, 6]));
    // A Noise frame is 1 to 65535 bytes; anything else closes the wire.
    assert!(wire.send(&vec![0u8; 65_536]).is_err());
    assert!(wire.send(&[1]).is_err());
}

#[tokio::test]
async fn a_relay_dial_that_times_out_is_an_error() {
    // Accepts the TCP connection and never answers the upgrade.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        let held: TcpStream = listener.accept().await.unwrap().0;
        tokio::time::sleep(WAIT).await;
        drop(held);
    });
    assert!(
        web_socket_frames(&url, Duration::from_millis(200))
            .await
            .is_err()
    );
}
