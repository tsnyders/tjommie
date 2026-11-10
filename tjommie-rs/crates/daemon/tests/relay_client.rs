//! Port of `packages/daemon/src/relay-client.test.ts`. Relay sockets are in-memory fakes handed out by the injected
//! connector; time is paused tokio time, advanced in small steps so every 5-second tick sees its own moment, as
//! vitest's fake timers do. The wall clock used for drift is fake too.

mod remote_support;

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daemon::peer_wire::{PeerWire, Socket, WireKind};
use daemon::relay_client::{Connector, RelayClient, RelayClientOptions, RelayError};
use daemon::relay_identity::{RelayIdentity, load_relay_identity};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use protocol::relay::{LEGACY_PUSH_KINDS, PushResult, login_message};
use protocol::remote::{PushKind, RelayState};
use remote_support::MemoryVault;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const HOST_ID: &str = "13653af3-8ca6-4e95-bc95-0949edc28715";

fn key(byte: u8) -> String {
    URL_SAFE_NO_PAD.encode([byte; 32])
}

struct Fake {
    url: String,
    opener: Option<oneshot::Sender<io::Result<Socket>>>,
    end: Option<Socket>,
    sent: Vec<String>,
}

/// Every socket the client asked for, in order, like the TypeScript test's `sockets`.
#[derive(Clone, Default)]
struct Fakes(Arc<Mutex<Vec<Fake>>>);

impl Fakes {
    fn connector(&self) -> Connector {
        let fakes = self.0.clone();
        Arc::new(move |url: String| {
            let (opener, opened) = oneshot::channel();
            fakes.lock().unwrap().push(Fake {
                url,
                opener: Some(opener),
                end: None,
                sent: Vec::new(),
            });
            Box::pin(async move {
                opened
                    .await
                    .unwrap_or_else(|_| Err(io::ErrorKind::ConnectionRefused.into()))
            })
        })
    }

    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    fn url(&self, index: usize) -> String {
        self.0.lock().unwrap()[index].url.clone()
    }

    fn last(&self) -> usize {
        self.len() - 1
    }

    fn open(&self, index: usize) {
        let (client, end) = Socket::pair();
        let mut fakes = self.0.lock().unwrap();
        let fake = &mut fakes[index];
        fake.opener.take().unwrap().send(Ok(client)).ok().unwrap();
        fake.end = Some(end);
    }

    fn receive(&self, index: usize, value: Value) {
        let text = if let Value::String(text) = value {
            text
        } else {
            value.to_string()
        };
        self.message(index, Message::text(text));
    }

    fn message(&self, index: usize, message: Message) {
        self.0.lock().unwrap()[index]
            .end
            .as_ref()
            .unwrap()
            .send(message)
            .unwrap();
    }

    /// The far end closes: before it opened, the connect fails; after, a close frame with `code`.
    fn close(&self, index: usize, code: u16) {
        let mut fakes = self.0.lock().unwrap();
        let fake = &mut fakes[index];
        fake.opener = None;
        if let Some(end) = &fake.end {
            let _ = end.send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: "".into(),
            })));
            end.close();
        }
    }

    /// Everything the client sent on this socket so far.
    fn sent(&self, index: usize) -> Vec<String> {
        let mut fakes = self.0.lock().unwrap();
        let fake = &mut fakes[index];
        if let Some(end) = fake.end.as_mut() {
            while let Some(message) = end.try_recv() {
                if let Message::Text(text) = message {
                    fake.sent.push(text.to_string());
                }
            }
        }
        fake.sent.clone()
    }

    /// `readyState === 3`.
    fn is_closed(&self, index: usize) -> bool {
        let fakes = self.0.lock().unwrap();
        fakes[index].end.as_ref().is_some_and(Socket::is_closed)
    }
}

struct Setup {
    client: RelayClient,
    fakes: Fakes,
    wires: Arc<Mutex<Vec<PeerWire>>>,
    identity: Arc<RelayIdentity>,
    url: Arc<Mutex<Option<String>>>,
    /// Added to the fake wall clock, like `vi.setSystemTime`.
    jump: Arc<AtomicI64>,
}

fn setup() -> Setup {
    setup_with(Arc::new(|| {}))
}

fn setup_with(on_state: Arc<dyn Fn() + Send + Sync>) -> Setup {
    let identity = Arc::new(
        load_relay_identity(Path::new("synthetic-relay-root"), &MemoryVault::default()).unwrap(),
    );
    let fakes = Fakes::default();
    let wires: Arc<Mutex<Vec<PeerWire>>> = Arc::default();
    let url = Arc::new(Mutex::new(Some(
        "https://relay.example/old-base?unused=1".to_owned(),
    )));
    let jump = Arc::new(AtomicI64::new(0));
    let origin = tokio::time::Instant::now();
    let (current, seen, shift) = (url.clone(), wires.clone(), jump.clone());
    let client = RelayClient::new(RelayClientOptions {
        url: Arc::new(move || current.lock().unwrap().clone()),
        host_id: HOST_ID.into(),
        identity: identity.clone(),
        on_channel: Arc::new(move |wire| seen.lock().unwrap().push(wire)),
        on_state,
        connect: fakes.connector(),
        clock: Arc::new(move || {
            origin.elapsed().as_millis() as i64 + shift.load(Ordering::Relaxed)
        }),
    });
    Setup {
        client,
        fakes,
        wires,
        identity,
        url,
        jump,
    }
}

/// Lets the client's task catch up.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

/// `vi.advanceTimersByTimeAsync`, in 500 ms steps.
async fn advance(ms: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(500);
        tokio::time::advance(Duration::from_millis(step)).await;
        settle().await;
        left -= step;
    }
}

fn verifies(identity: &RelayIdentity, nonce: &str, signature: &str) -> bool {
    let signature: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signature)
        .unwrap()
        .try_into()
        .unwrap();
    VerifyingKey::from_bytes(&identity.public_key)
        .unwrap()
        .verify(
            &login_message(HOST_ID, nonce, "relay.example"),
            &Signature::from_bytes(&signature),
        )
        .is_ok()
}

/// Opens socket `index` and logs in with a `ready` that advertises `ready`'s extra fields.
async fn online(s: &Setup, index: usize, ready: Value) {
    s.fakes.open(index);
    settle().await;
    s.fakes
        .receive(index, json!({ "t": "challenge", "nonce": key(7) }));
    settle().await;
    s.fakes.receive(index, ready);
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn signs_the_domain_separated_challenge_and_handles_a_channel_leg() {
    let s = setup();
    s.client.start();
    settle().await;
    assert_eq!(
        s.fakes.url(0),
        format!("wss://relay.example/v1/hosts/{HOST_ID}/control")
    );
    s.fakes.open(0);
    settle().await;
    s.fakes
        .receive(0, json!({ "t": "challenge", "nonce": key(7) }));
    settle().await;
    let login: Value = serde_json::from_str(&s.fakes.sent(0)[0]).unwrap();
    assert_eq!(login["t"], "login");
    assert_eq!(
        login["publicKey"],
        URL_SAFE_NO_PAD.encode(s.identity.public_key)
    );
    assert!(verifies(
        &s.identity,
        &key(7),
        login["signature"].as_str().unwrap()
    ));
    s.fakes.receive(0, json!({ "t": "ready", "push": true }));
    settle().await;
    assert_eq!(s.client.state(), RelayState::Online);
    s.fakes
        .receive(0, json!({ "t": "open", "channel": key(8) }));
    settle().await;
    assert_eq!(
        s.fakes.url(1),
        format!("wss://relay.example/v1/hosts/{HOST_ID}/channels/{}", key(8))
    );
    s.fakes.open(1);
    settle().await;
    assert_eq!(s.wires.lock().unwrap().len(), 1);
    assert_eq!(s.wires.lock().unwrap()[0].kind, WireKind::Relay);
    s.client.stop();
    settle().await;
    assert!(s.fakes.is_closed(1), "stopping closes channel legs");
}

#[tokio::test(start_paused = true)]
async fn drift_triggers_relogin() {
    let s = setup();
    s.client.start();
    settle().await;
    online(&s, 0, json!({ "t": "ready", "push": true })).await;
    s.jump.store(60_000, Ordering::Relaxed);
    advance(5_000).await;
    assert_eq!(s.fakes.len(), 2);
    assert!(s.fakes.is_closed(0));
    s.fakes.open(1);
    settle().await;
    s.fakes
        .receive(1, json!({ "t": "challenge", "nonce": key(6) }));
    settle().await;
    let login: Value = serde_json::from_str(&s.fakes.sent(1)[0]).unwrap();
    assert!(verifies(
        &s.identity,
        &key(6),
        login["signature"].as_str().unwrap()
    ));
    s.fakes.receive(1, json!({ "t": "ready", "push": true }));
    settle().await;
    assert_eq!(s.client.state(), RelayState::Online);
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn rereads_a_changed_relay_url_on_restart() {
    let s = setup();
    s.client.start();
    settle().await;
    *s.url.lock().unwrap() = Some("https://new-relay.example/subpath".into());
    s.client.stop();
    s.client.start();
    settle().await;
    assert_eq!(
        s.fakes.url(1),
        format!("wss://new-relay.example/v1/hosts/{HOST_ID}/control")
    );
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn uses_1_2_5_10_and_30_second_retry_delays() {
    let s = setup();
    s.client.start();
    settle().await;
    for delay in [1_000, 2_000, 5_000, 10_000, 30_000, 30_000] {
        s.fakes.close(s.fakes.last(), 1000);
        settle().await;
        advance(delay - 1).await;
        let count = s.fakes.len();
        advance(1).await;
        assert_eq!(s.fakes.len(), count + 1, "{delay}");
    }
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn matches_push_replies_to_requests() {
    let s = setup();
    s.client.start();
    settle().await;
    online(&s, 0, json!({ "t": "ready", "push": true })).await;
    let client = s.client.clone();
    let pending = tokio::spawn(async move {
        client
            .push(
                vec!["synthetic-device-token".into()],
                PushKind::Done,
                "bot-1",
                Some("team"),
            )
            .await
    });
    settle().await;
    let sent: Value = serde_json::from_str(&s.fakes.sent(0)[1]).unwrap();
    assert_eq!(
        sent,
        json!({ "t": "push", "id": sent["id"], "tokens": ["synthetic-device-token"], "kind": "done", "botId": "bot-1", "roomId": "team" })
    );
    s.fakes.receive(0, json!({ "t": "pushed", "id": sent["id"], "results": [{ "ok": true, "unregistered": false }] }));
    settle().await;
    assert_eq!(
        pending.await.unwrap(),
        Ok(vec![PushResult {
            ok: true,
            unregistered: false
        }])
    );
    s.fakes
        .receive(0, json!({ "t": "open", "channel": key(8) }));
    settle().await;
    s.fakes.close(1, 1000);
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn assumes_legacy_kinds_until_a_ready_advertises_more_then_resets_on_disconnect() {
    let s = setup();
    assert_eq!(s.client.kinds(), LEGACY_PUSH_KINDS);
    s.client.start();
    settle().await;
    online(
        &s,
        0,
        json!({ "t": "ready", "push": true, "kinds": ["approval", "done"] }),
    )
    .await;
    assert_eq!(s.client.kinds(), [PushKind::Approval, PushKind::Done]);
    s.client.stop();
    assert_eq!(s.client.kinds(), LEGACY_PUSH_KINDS);
}

#[tokio::test(start_paused = true)]
async fn keeps_a_newer_relay_online_while_advertising_only_known_push_kinds() {
    let s = setup();
    s.client.start();
    settle().await;
    online(
        &s,
        0,
        json!({ "t": "ready", "push": true, "kinds": ["approval", "limit", "routine_failed", "done", "failed", "mention"] }),
    )
    .await;
    assert_eq!(s.client.state(), RelayState::Online);
    assert_eq!(s.client.kinds(), PushKind::ALL);
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn keeps_secrets_out_of_errors_on_failure_paths() {
    let s = setup();
    s.client.start();
    settle().await;
    online(&s, 0, json!({ "t": "ready", "push": true })).await;
    let client = s.client.clone();
    let stopped = tokio::spawn(async move {
        client
            .push(
                vec!["synthetic-device-token".into()],
                PushKind::Approval,
                "bot-1",
                None,
            )
            .await
    });
    settle().await;
    s.client.stop();
    let error = stopped.await.unwrap().unwrap_err();
    assert_eq!(error, RelayError::Unavailable);
    assert!(!format!("{error} {error:?}").contains("synthetic-device-token"));
    s.client.start();
    settle().await;
    online(&s, 1, json!({ "t": "ready", "push": true })).await;
    let client = s.client.clone();
    let timed_out = tokio::spawn(async move {
        client
            .push(
                vec!["synthetic-device-token".into()],
                PushKind::Approval,
                "bot-1",
                None,
            )
            .await
    });
    advance(10_000).await;
    assert_eq!(timed_out.await.unwrap(), Err(RelayError::Unavailable));
    s.client.stop();
    s.client.start();
    settle().await;
    s.fakes.open(2);
    settle().await;
    s.fakes
        .receive(2, json!({ "t": "open", "channel": key(8) }));
    settle().await;
    assert_eq!(
        s.client.state(),
        RelayState::Error,
        "a channel before login is refused"
    );
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn pings_every_30_seconds_and_reconnects_when_pong_stops() {
    let s = setup();
    s.client.start();
    settle().await;
    online(&s, 0, json!({ "t": "ready", "push": true })).await;
    advance(29_999).await;
    assert_eq!(s.fakes.sent(0).len(), 1);
    advance(1).await;
    assert_eq!(s.fakes.sent(0)[1], "ping");
    s.fakes.receive(0, json!("pong"));
    settle().await;
    advance(30_000).await;
    assert_eq!(s.fakes.sent(0)[2], "ping");
    advance(50_000).await;
    assert_eq!(s.client.state(), RelayState::Error);
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn times_out_login_and_rejects_invalid_control_messages() {
    let s = setup();
    s.client.start();
    settle().await;
    s.fakes.open(0);
    advance(10_000).await;
    assert_eq!(s.client.state(), RelayState::Error);
    s.client.stop();
    for invalid in [
        json!({ "t": "ready", "push": true }),
        json!({ "t": "open", "channel": key(8) }),
        json!({ "t": "pushed", "id": 1, "results": [] }),
        json!("{bad-json"),
        json!({ "t": "challenge", "nonce": "bad" }),
    ] {
        s.client.start();
        settle().await;
        let index = s.fakes.last();
        s.fakes.open(index);
        settle().await;
        s.fakes.receive(index, invalid.clone());
        settle().await;
        assert!(s.fakes.is_closed(index), "{invalid}");
        s.client.stop();
    }
    s.client.start();
    settle().await;
    let index = s.fakes.last();
    s.fakes.open(index);
    settle().await;
    s.fakes.message(index, Message::binary(vec![1]));
    settle().await;
    assert!(s.fakes.is_closed(index));
    s.client.stop();
    s.client.start();
    settle().await;
    let index = s.fakes.last();
    s.fakes.open(index);
    settle().await;
    s.fakes
        .receive(index, json!({ "t": "challenge", "nonce": key(7) }));
    s.fakes
        .receive(index, json!({ "t": "challenge", "nonce": key(7) }));
    settle().await;
    assert!(s.fakes.is_closed(index));
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn resets_backoff_after_ready_and_uses_a_30_second_cap_after_close_4401() {
    let s = setup();
    s.client.start();
    settle().await;
    s.fakes.close(0, 1000);
    settle().await;
    advance(1_000).await;
    online(&s, 1, json!({ "t": "ready", "push": true })).await;
    s.fakes.close(1, 1000);
    settle().await;
    advance(1_000).await;
    assert_eq!(s.fakes.len(), 3);
    s.fakes.open(2);
    settle().await;
    s.fakes.close(2, 4401);
    settle().await;
    assert_eq!(s.client.state(), RelayState::Error);
    advance(29_999).await;
    assert_eq!(s.fakes.len(), 3);
    advance(1).await;
    assert_eq!(s.fakes.len(), 4);
    s.fakes.open(3);
    settle().await;
    s.fakes.close(3, 4404);
    settle().await;
    advance(30_000).await;
    assert_eq!(s.fakes.len(), 5);
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn rejects_pending_pushes_on_stop_or_bad_result_counts_and_caps_channels() {
    let s = setup();
    s.client.start();
    settle().await;
    online(&s, 0, json!({ "t": "ready", "push": true })).await;
    let client = s.client.clone();
    let mismatched = tokio::spawn(async move {
        client
            .push(
                vec!["synthetic-token".into()],
                PushKind::Limit,
                "bot-1",
                None,
            )
            .await
    });
    settle().await;
    let id = serde_json::from_str::<Value>(&s.fakes.sent(0)[1]).unwrap()["id"].clone();
    s.fakes
        .receive(0, json!({ "t": "pushed", "id": id, "results": [] }));
    settle().await;
    let error = mismatched.await.unwrap().unwrap_err();
    assert_eq!(error.to_string(), "The relay connection is unavailable.");
    for _ in 0..33 {
        s.fakes
            .receive(0, json!({ "t": "open", "channel": key(8) }));
    }
    settle().await;
    assert_eq!(s.fakes.len(), 33);
    let client = s.client.clone();
    let pending = tokio::spawn(async move {
        client
            .push(
                vec!["synthetic-token".into()],
                PushKind::Limit,
                "bot-1",
                None,
            )
            .await
    });
    settle().await;
    s.client.stop();
    assert_eq!(pending.await.unwrap(), Err(RelayError::Unavailable));
}

#[tokio::test(start_paused = true)]
async fn delivers_relay_wires_and_contains_a_panicking_state_callback() {
    let s = setup_with(Arc::new(|| panic!("synthetic UI failure")));
    s.client.start();
    settle().await;
    online(&s, 0, json!({ "t": "ready", "push": true })).await;
    assert_eq!(s.client.state(), RelayState::Online);
    s.fakes
        .receive(0, json!({ "t": "open", "channel": key(8) }));
    settle().await;
    s.fakes.open(1);
    settle().await;
    assert_eq!(s.wires.lock().unwrap()[0].kind, WireKind::Relay);
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn waits_until_online_or_says_why_not() {
    let s = setup();
    assert_eq!(
        s.client.wait_until_online().await,
        Err(RelayError::Unavailable),
        "not started"
    );
    s.client.start();
    settle().await;
    let client = s.client.clone();
    let waiting = tokio::spawn(async move { client.wait_until_online().await });
    online(&s, 0, json!({ "t": "ready", "push": false })).await;
    assert_eq!(waiting.await.unwrap(), Ok(()));
    assert!(!s.client.push_enabled());
    assert_eq!(
        s.client
            .push(vec!["t".into()], PushKind::Done, "bot", None)
            .await,
        Err(RelayError::Unavailable),
        "the relay said push is off"
    );
    s.client.stop();
    s.client.start();
    settle().await;
    let client = s.client.clone();
    let silent = tokio::spawn(async move { client.wait_until_online().await });
    advance(10_000).await;
    assert_eq!(silent.await.unwrap(), Err(RelayError::Unavailable));
    s.client.stop();
    *s.url.lock().unwrap() = None;
    s.client.start();
    settle().await;
    assert_eq!(s.client.state(), RelayState::Off);
    assert_eq!(
        s.client.wait_until_online().await,
        Err(RelayError::Unavailable)
    );
    s.client.stop();
}

#[tokio::test(start_paused = true)]
async fn refuses_an_invalid_push_request() {
    let s = setup();
    s.client.start();
    settle().await;
    online(&s, 0, json!({ "t": "ready", "push": true })).await;
    assert_eq!(
        s.client.push(vec![], PushKind::Done, "bot", None).await,
        Err(RelayError::Invalid)
    );
    assert_eq!(
        s.client
            .push(vec!["t".into(); 17], PushKind::Done, "bot", None)
            .await,
        Err(RelayError::Invalid)
    );
    assert_eq!(s.fakes.sent(0).len(), 1, "nothing but the login was sent");
    s.client.stop();
}
