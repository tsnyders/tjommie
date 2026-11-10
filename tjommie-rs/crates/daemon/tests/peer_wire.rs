//! Port of `packages/daemon/src/peer-wire.test.ts`. Everything binds 127.0.0.1 on an ephemeral port.

use std::time::Duration;

use daemon::peer_wire::{PeerWire, Socket, WireKind};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use transport::channel::{FrameDecoder, length_prefixed};

const WAIT: Duration = Duration::from_secs(10);

async fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Reads until the peer closes the connection.
async fn until_closed(client: &mut TcpStream) {
    let mut sink = [0; 64];
    while timeout(WAIT, client.read(&mut sink))
        .await
        .unwrap()
        .unwrap_or(0)
        > 0
    {}
}

#[tokio::test]
async fn turns_split_tcp_packets_into_frames_and_prefixes_each_written_frame() {
    let (listener, port) = listener().await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut wire = PeerWire::tcp(stream);
        assert_eq!(wire.kind, WireKind::Lan);
        assert_eq!(wire.address, "127.0.0.1");
        let mut received = Vec::new();
        while received.len() < 2 {
            let frame = wire.next_frame().await.unwrap();
            wire.writer().write(frame.clone()).unwrap();
            received.push(frame);
        }
        (received, wire)
    });
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut packet = length_prefixed(&[1, 2]).unwrap();
    packet.extend(length_prefixed(&[3]).unwrap());
    client.write_all(&packet[..2]).await.unwrap();
    client.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    client.write_all(&packet[2..]).await.unwrap();
    let mut decoder = FrameDecoder::default();
    let mut echoed = Vec::new();
    let mut buffer = [0; 64];
    while echoed.len() < 2 {
        let read = timeout(WAIT, client.read(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert!(read > 0);
        echoed.extend(decoder.feed(&buffer[..read]).unwrap());
    }
    let (received, _wire) = server.await.unwrap();
    assert_eq!(received, [vec![1, 2], vec![3]]);
    assert_eq!(echoed, received);
}

#[tokio::test]
async fn propagates_close_when_a_malformed_frame_destroys_the_socket() {
    let (listener, port) = listener().await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut wire = PeerWire::tcp(stream);
        let writer = wire.writer();
        assert_eq!(wire.next_frame().await, None);
        timeout(WAIT, writer.closed()).await.unwrap();
        writer.is_closed()
    });
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(&[0, 0, 0, 0]).await.unwrap();
    until_closed(&mut client).await;
    assert!(server.await.unwrap());
}

#[tokio::test]
async fn does_not_deliver_later_frames_from_a_chunk_after_destruction() {
    let (listener, port) = listener().await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut wire = PeerWire::tcp(stream);
        let mut received = Vec::new();
        // Let both frames arrive before the first is handled.
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Some(frame) = wire.next_frame().await {
            received.push(frame[0]);
            wire.writer().destroy();
        }
        received
    });
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut packet = length_prefixed(&[1]).unwrap();
    packet.extend(length_prefixed(&[2]).unwrap());
    client.write_all(&packet).await.unwrap();
    until_closed(&mut client).await;
    assert_eq!(server.await.unwrap(), [1]);
}

#[tokio::test]
async fn refuses_writes_after_the_wire_closed() {
    let (listener, port) = listener().await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut wire = PeerWire::tcp(stream);
        assert_eq!(wire.next_frame().await, None);
        let error = wire.writer().write(vec![1]).unwrap_err();
        assert_eq!(error.to_string(), "Connection closed.");
    });
    drop(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    server.await.unwrap();
}

/// A WebSocket server on 127.0.0.1 and a [`Socket`] connected to it, as `new WebSocket` in the TypeScript test.
async fn websocket() -> (Socket, tokio_tungstenite::WebSocketStream<TcpStream>) {
    let (listener, port) = listener().await;
    let accepted = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio_tungstenite::accept_async(stream).await.unwrap()
    });
    let socket = Socket::connect(&format!("ws://127.0.0.1:{port}"))
        .await
        .unwrap();
    (socket, accepted.await.unwrap())
}

#[tokio::test]
async fn maps_one_binary_websocket_message_to_one_frame_and_rejects_text() {
    let (socket, mut peer) = websocket().await;
    let mut wire = PeerWire::websocket(socket);
    assert_eq!(wire.kind, WireKind::Relay);
    assert_eq!(wire.address, "relay");
    wire.writer().write(vec![1, 2, 3]).unwrap();
    let sent = timeout(WAIT, peer.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(sent, Message::binary(vec![1, 2, 3]));
    peer.send(Message::binary(vec![4, 5])).await.unwrap();
    assert_eq!(
        timeout(WAIT, wire.next_frame()).await.unwrap(),
        Some(vec![4, 5])
    );
    let writer = wire.writer();
    peer.send(Message::text("text frame")).await.unwrap();
    assert_eq!(timeout(WAIT, wire.next_frame()).await.unwrap(), None);
    timeout(WAIT, writer.closed()).await.unwrap();
}

#[tokio::test]
async fn closes_a_websocket_wire_on_an_empty_or_oversized_binary_frame() {
    for size in [0, 65_536] {
        let (socket, mut peer) = websocket().await;
        let mut wire = PeerWire::websocket(socket);
        let writer = wire.writer();
        peer.send(Message::binary(vec![0; size])).await.unwrap();
        assert_eq!(
            timeout(WAIT, wire.next_frame()).await.unwrap(),
            None,
            "{size}"
        );
        timeout(WAIT, writer.closed()).await.unwrap();
    }
}

#[tokio::test]
async fn rejects_an_oversized_or_closing_websocket_wire_write() {
    for case in ["oversized", "closing"] {
        let (socket, _peer) = websocket().await;
        let wire = PeerWire::websocket(socket);
        let writer = wire.writer();
        if case == "closing" {
            writer.destroy();
        }
        let size = if case == "oversized" { 65_536 } else { 1 };
        let error = writer.write(vec![0; size]).unwrap_err();
        assert_eq!(error.to_string(), "Connection closed.", "{case}");
        timeout(WAIT, writer.closed()).await.unwrap();
    }
}

#[tokio::test]
async fn an_in_memory_wire_carries_frames_both_ways_and_closes_from_either_end() {
    let (mut wire, mut end) = PeerWire::memory(WireKind::Lan, "192.0.2.1");
    assert_eq!(wire.address, "192.0.2.1");
    end.send(vec![1]).unwrap();
    assert_eq!(wire.next_frame().await, Some(vec![1]));
    wire.writer().write(vec![2]).unwrap();
    assert_eq!(end.recv().await, Some(vec![2]));
    end.close();
    assert_eq!(wire.next_frame().await, None);
    assert!(wire.writer().is_closed());
    assert_eq!(end.recv().await, None);
}
