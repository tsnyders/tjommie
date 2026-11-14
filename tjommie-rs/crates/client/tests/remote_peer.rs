//! The handshake's last guard (`remote-peer.ts:60`): a PC that answers the Noise handshake correctly but whose `ready`
//! names another host is not trusted, and its wire is closed. A scripted PC speaks the real protocol (the preface, the Noise
//! handshake, the encrypted confirmation, then `ready`), so the only thing that differs between the two tests is the host id.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::VecDeque;
use std::time::Duration;

use client::Endpoint;
use client::remote::{Auth, AuthError, Wires, authenticate, generate_keypair};
use protocol::remote::{PrefaceMode, RemotePreface, remote_prologue};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use transport::Session;
use transport::channel::{EncryptedMessages, FrameDecoder, length_prefixed};

const WAIT: Duration = Duration::from_secs(10);
const HOST_ID: &str = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11";
const OTHER_HOST_ID: &str = "8d1f4c1e-2b7a-4e0b-b1d3-0a6c5e9f7a22";
const DEVICE_ID: &str = "3c9a7e52-1d4b-4f80-9b6e-7a2f5d8c1e04";

async fn next_frame(
    stream: &mut TcpStream,
    decoder: &mut FrameDecoder,
    queue: &mut VecDeque<Vec<u8>>,
) -> Vec<u8> {
    loop {
        if let Some(frame) = queue.pop_front() {
            return frame;
        }
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(
            read > 0,
            "the client closed before sending what the PC waits for"
        );
        queue.extend(decoder.feed(&chunk[..read]).unwrap());
    }
}

/// A PC on 127.0.0.1 that completes the resume handshake with `host_key` and answers the confirmation with a `ready`
/// naming `ready_host`. Returns its address and a task that ends when the client has closed the wire.
async fn scripted_pc(
    host_private: [u8; 32],
    ready_host: &'static str,
) -> (Endpoint, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (mut decoder, mut queue) = (FrameDecoder::default(), VecDeque::new());
        let preface: RemotePreface =
            serde_json::from_slice(&next_frame(&mut stream, &mut decoder, &mut queue).await)
                .unwrap();
        let mut noise =
            Session::responder(&host_private, None, &remote_prologue(&preface).unwrap()).unwrap();
        let handshake = next_frame(&mut stream, &mut decoder, &mut queue).await;
        assert!(noise.read_handshake(&handshake).unwrap().is_empty());
        let reply = length_prefixed(&noise.write_handshake(b"").unwrap()).unwrap();
        stream.write_all(&reply).await.unwrap();
        let mut messages = EncryptedMessages::new(noise);
        // The confirmation: the client's proof it holds its key, sent only after the handshake finished.
        let confirmation = loop {
            let frame = next_frame(&mut stream, &mut decoder, &mut queue).await;
            if let Some(text) = messages.receive(&frame).unwrap() {
                break text;
            }
        };
        assert!(confirmation.contains("\"name\""), "{confirmation}");
        let ready = json!({ "type": "ready", "deviceId": DEVICE_ID, "hostId": ready_host, "hostName": "Test PC", "relay": null });
        for frame in messages.encode(&ready.to_string()).unwrap() {
            stream
                .write_all(&length_prefixed(&frame).unwrap())
                .await
                .unwrap();
        }
        // Whatever the client does with the answer, it ends by closing the wire: nothing more is read, and EOF arrives.
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(WAIT, stream.read_to_end(&mut rest)).await;
    });
    (
        Endpoint::Lan {
            host: "127.0.0.1".into(),
            port,
        },
        task,
    )
}

async fn run(ready_host: &'static str) -> (Result<String, AuthError>, bool) {
    let (host, device) = (generate_keypair().unwrap(), generate_keypair().unwrap());
    let (endpoint, pc) = scripted_pc(*host.private_key, ready_host).await;
    let wire = Wires::new().open(&endpoint, WAIT).await.unwrap();
    let preface = RemotePreface {
        v: 1,
        host_id: HOST_ID.to_owned(),
        mode: PrefaceMode::Resume,
        pairing_id: None,
    };
    let auth = Auth {
        preface,
        private_key: &*device.private_key,
        host_key: &host.public_key,
        psk: None,
        name: "My laptop",
    };
    let outcome = authenticate(wire, auth, || {})
        .await
        .map(|peer| peer.host_id.clone());
    // The scripted PC's task ends when the client's side of the wire closes (or after WAIT, which fails the next assertion).
    let closed = tokio::time::timeout(WAIT, pc).await.is_ok();
    (outcome, closed)
}

#[tokio::test]
async fn a_ready_naming_the_host_that_was_asked_for_is_accepted() {
    // The control: the scripted PC is a valid peer, so the next test can only fail on the host id.
    let (outcome, _) = run(HOST_ID).await;
    assert_eq!(outcome.unwrap(), HOST_ID);
}

#[tokio::test]
async fn a_ready_that_names_another_host_is_not_trusted_and_the_wire_is_closed() {
    let (outcome, closed) = run(OTHER_HOST_ID).await;
    assert!(matches!(outcome, Err(AuthError::Failed)), "{outcome:?}");
    assert!(closed, "the client closed the wire instead of using it");
}
