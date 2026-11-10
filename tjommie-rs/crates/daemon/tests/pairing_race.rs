//! The reviewer's pairing race (M7 LAN review): on the multi-thread runtime production uses, several devices that
//! finished their handshakes confirm on one pairing code at once, and exactly one may pair, round after round.

mod remote_support;

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daemon::devices_store::list_devices;
use daemon::peer_wire::PeerWire;
use protocol::remote::{PairingCode, RemotePreface, parse_pairing_code, remote_prologue};
use remote_support::harness::{Harness, Setup};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use transport::Session;
use transport::channel::EncryptedMessages;

const WAIT: Duration = Duration::from_secs(10);

async fn start() -> (Harness, PairingCode) {
    let h = Harness::start(Setup::default()).await;
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let code = parse_pairing_code(
        invite["code"].as_str().unwrap(),
        jiff::Timestamp::now().as_millisecond(),
    )
    .unwrap();
    (h, code)
}

fn b64(key: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(key).unwrap()
}

async fn raw(code: &PairingCode) -> TcpStream {
    TcpStream::connect(("127.0.0.1", code.lan[0].port))
        .await
        .unwrap()
}

struct Device {
    wire: PeerWire,
    messages: EncryptedMessages<Session>,
}

async fn handshake(
    code: &PairingCode,
    preface: RemotePreface,
    psk: Option<&[u8]>,
    private_key: &[u8],
) -> Device {
    let mut wire = PeerWire::tcp(raw(code).await);
    let prologue = remote_prologue(&preface).unwrap();
    let mut noise =
        Session::initiator(private_key, &b64(code.host_key.as_str()), psk, &prologue).unwrap();
    let writer = wire.writer();
    writer.write(serde_json::to_vec(&preface).unwrap()).unwrap();
    writer.write(noise.write_handshake(b"").unwrap()).unwrap();
    let reply = timeout(WAIT, wire.next_frame())
        .await
        .unwrap()
        .expect("handshake reply");
    assert!(noise.read_handshake(&reply).unwrap().is_empty());
    assert!(noise.handshake_finished());
    Device {
        wire,
        messages: EncryptedMessages::new(noise),
    }
}

impl Device {
    fn send(&mut self, text: &str) -> Vec<Vec<u8>> {
        let frames = self.messages.encode(text).unwrap();
        for frame in &frames {
            self.wire.writer().write(frame.clone()).unwrap();
        }
        frames
    }
    async fn recv(&mut self) -> Option<Value> {
        loop {
            let frame = timeout(WAIT, self.wire.next_frame()).await.ok()??;
            if let Some(text) = self.messages.receive(&frame).ok()? {
                return Some(serde_json::from_str(&text).unwrap());
            }
        }
    }
}

/// Production runs the multi-thread runtime, where two confirm handlers can interleave between the in-memory invite
/// check and the database update. Repeated races must still pair exactly one device per code.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn repeated_multithread_pairing_races_pair_exactly_one_device_each() {
    let (h, _) = start().await;
    for round in 0..5usize {
        let invite = h.call("pairing.create", json!({})).await.unwrap();
        let code = std::sync::Arc::new(
            parse_pairing_code(
                invite["code"].as_str().unwrap(),
                jiff::Timestamp::now().as_millisecond(),
            )
            .unwrap(),
        );
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(4));
        let mut tasks = Vec::new();
        for index in 0..4 {
            let (code, barrier) = (code.clone(), barrier.clone());
            tasks.push(tokio::spawn(async move {
                let keys = transport::generate_keypair().unwrap();
                let mut device = handshake(
                    &code,
                    RemotePreface::pair(&code.host_id, &code.pairing_id),
                    Some(&b64(code.secret.as_str())),
                    &keys.private_key[..],
                )
                .await;
                barrier.wait().await;
                device.send(
                    &json!({"type":"confirm","protocol":1,"name":format!("Racer {index}")})
                        .to_string(),
                );
                let won = matches!(device.recv().await, Some(value) if value["type"] == "ready");
                device.wire.writer().destroy();
                won
            }));
        }
        let mut winners = 0;
        for task in tasks {
            if task.await.unwrap() {
                winners += 1;
            }
        }
        let devices = list_devices(&h.db.lock().unwrap()).unwrap().len();
        assert_eq!((winners, devices), (1, round + 1), "round {round}");
        // Let the losing peers finish tearing down before the next code.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.remote.close().await;
}
