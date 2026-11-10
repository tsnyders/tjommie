//! The relay lifecycle from `daemon.ts:334-404` and `settings.set`, ported from the relay cases of `daemon.test.ts`.
//! Relays are in-memory fakes: one that answers like the TypeScript test's `RelaySocket`, and one that never opens, like
//! `SilentRelaySocket`. Pushes come from `Remote::publish` of a notify event rather than a bot run (the runner is T3.5).

mod remote_support;

use std::future::pending;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daemon::devices_store::{add_device, set_push_token};
use daemon::peer_wire::{PeerWire, Socket};
use daemon::relay_client::Connector;
use daemon::remote::Remote;
use protocol::Event;
use protocol::remote::{RemotePreface, parse_pairing_code};
use protocol::wire::Notify;
use remote_support::device::{Pairing, authenticate};
use remote_support::harness::{Harness, Setup};
use remote_support::{MemoryVault, key_of, shared_db};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use transport::generate_keypair;

const RELAY: &str = "https://relay.example";

#[derive(Clone, Default)]
struct Relays {
    urls: Arc<Mutex<Vec<String>>>,
    sent: Arc<Mutex<Vec<Vec<String>>>>,
    closers: Arc<Mutex<Vec<CancellationToken>>>,
    closed: Arc<AtomicUsize>,
    /// Whether the host was listening when each socket was opened.
    listening: Arc<Mutex<Vec<bool>>>,
    remote: Arc<OnceLock<Arc<Remote>>>,
}

/// Counts a never-opened socket as closed when the client gives up on it.
struct Dropped(Arc<AtomicUsize>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Relays {
    fn record(&self, url: String) -> usize {
        let listening = self
            .remote
            .get()
            .is_some_and(|remote| remote.status().listening);
        self.listening.lock().unwrap().push(listening);
        let mut urls = self.urls.lock().unwrap();
        urls.push(url);
        self.sent.lock().unwrap().push(Vec::new());
        urls.len() - 1
    }

    /// Opens at once, challenges, accepts any login with push on, and answers every push.
    fn answering(&self) -> Connector {
        let relays = self.clone();
        Arc::new(move |url: String| {
            let index = relays.record(url);
            let (client, mut end) = Socket::pair();
            relays.closers.lock().unwrap().push(end.closer());
            let sent = relays.sent.clone();
            tokio::spawn(async move {
                let _ = end.send(Message::text(
                    json!({ "t": "challenge", "nonce": key_of(5) }).to_string(),
                ));
                while let Some(message) = end.recv().await {
                    let Message::Text(text) = message else {
                        continue;
                    };
                    sent.lock().unwrap()[index].push(text.to_string());
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let reply = match value["t"].as_str() {
                        Some("login") => {
                            json!({ "t": "ready", "push": true, "kinds": ["approval", "limit", "routine_failed", "done", "failed"] })
                        }
                        Some("push") => {
                            let results: Vec<Value> = value["tokens"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|_| json!({ "ok": true, "unregistered": false }))
                                .collect();
                            json!({ "t": "pushed", "id": value["id"], "results": results })
                        }
                        _ => continue,
                    };
                    let _ = end.send(Message::text(reply.to_string()));
                }
            });
            Box::pin(async move { Ok(client) })
        })
    }

    /// Never opens.
    fn silent(&self) -> Connector {
        let relays = self.clone();
        Arc::new(move |url: String| {
            relays.record(url);
            let dropped = Dropped(relays.closed.clone());
            Box::pin(async move {
                let _dropped = dropped;
                pending::<io::Result<Socket>>().await
            })
        })
    }

    fn created(&self) -> usize {
        self.urls.lock().unwrap().len()
    }

    fn sent(&self, index: usize) -> Vec<String> {
        self.sent.lock().unwrap()[index].clone()
    }
}

type Url = Arc<Mutex<Option<String>>>;

fn setting(url: &Url) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
    let url = url.clone();
    Arc::new(move || url.lock().unwrap().clone())
}

async fn start(
    relays: &Relays,
    connect: Connector,
    url: &Url,
    db: Arc<Mutex<rusqlite::Connection>>,
    vault: Arc<MemoryVault>,
) -> Harness {
    let harness = Harness::start(Setup {
        db,
        vault,
        relay_url: setting(url),
        relay_connect: connect,
        ..Setup::default()
    })
    .await;
    let _ = relays.remote.set(harness.remote.clone());
    harness
}

/// `settings.set({ relayUrl })`.
async fn set_relay(h: &Harness, url: &Url, value: Option<&str>) {
    *url.lock().unwrap() = value.map(Into::into);
    h.remote.relay_url_changed().await;
}

async fn wait_for(mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting");
}

fn relay_status(h: &Harness) -> Value {
    serde_json::to_value(h.remote.status().relay).unwrap()
}

fn notify_done(bot_id: &str) -> Event {
    Event::Notify(Notify::Done {
        bot_id: bot_id.into(),
        title: "Private room reply".into(),
        body: "Private room body".into(),
        room_id: Some("team".into()),
    })
}

#[tokio::test]
async fn starts_the_relay_for_an_invite_after_lan_listening_restarts_on_settings_and_sends_content_free_pushes()
 {
    let (relays, url, vault) = (
        Relays::default(),
        Url::default(),
        Arc::new(MemoryVault::default()),
    );
    let h = start(
        &relays,
        relays.answering(),
        &url,
        shared_db(),
        vault.clone(),
    )
    .await;
    set_relay(&h, &url, Some("https://relay.example/old-base")).await;
    assert_eq!(relays.created(), 0, "nothing needs the relay yet");
    assert_eq!(
        relay_status(&h),
        json!({ "url": "https://relay.example/old-base", "state": "off", "push": false })
    );
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    assert_eq!(
        *relays.listening.lock().unwrap(),
        [true],
        "the relay starts after LAN listening"
    );
    let code = parse_pairing_code(
        invite["code"].as_str().unwrap(),
        jiff::Timestamp::now().as_millisecond(),
    )
    .unwrap();
    assert_eq!(
        code.relay.as_ref().map(|relay| relay.as_str()),
        Some("https://relay.example/old-base")
    );
    assert!(relays.urls.lock().unwrap()[0].ends_with("/control"));
    let status = h.call("remote.status", json!({})).await.unwrap();
    assert_eq!(status["listening"], true);
    assert_eq!(
        status["relay"],
        json!({ "url": "https://relay.example/old-base", "state": "online", "push": true })
    );
    let relay_keys = || {
        vault
            .values
            .lock()
            .unwrap()
            .keys()
            .filter(|target| target.ends_with("/relay-login"))
            .count()
    };
    assert_eq!(relay_keys(), 1);

    set_relay(&h, &url, Some("https://new-relay.example")).await;
    wait_for(|| relays.created() == 2).await;
    assert!(relays.closers.lock().unwrap()[0].is_cancelled());
    assert!(relays.urls.lock().unwrap()[1].contains("new-relay.example"));
    assert_eq!(
        *vault.writes.lock().unwrap() as usize,
        2,
        "one remote-host and one relay-login write, never a new key"
    );
    assert_eq!(
        h.call("remote.status", json!({})).await.unwrap()["port"],
        status["port"]
    );
    wait_for(|| h.remote.status().relay.push).await;

    let device = add_device(
        &h.db.lock().unwrap(),
        &URL_SAFE_NO_PAD.encode([9; 32]),
        "Synthetic phone",
    )
    .unwrap();
    set_push_token(
        &h.db.lock().unwrap(),
        &device.id,
        Some("synthetic-device-token"),
        None,
    )
    .unwrap();
    h.remote.publish(&Event::Notify(Notify::Approval {
        bot_id: "relay-bot".into(),
        title: "Private summary".into(),
        body: "Private body".into(),
        approval_id: Some("relay-approval".into()),
    }));
    wait_for(|| {
        relays
            .sent(1)
            .iter()
            .any(|text| text.contains(r#""t":"push""#))
    })
    .await;
    let push = relays
        .sent(1)
        .into_iter()
        .find(|text| text.contains(r#""t":"push""#))
        .unwrap();
    assert!(push.contains(r#""kind":"approval""#) && push.contains(r#""botId":"relay-bot""#));
    assert!(!push.contains("Private") && !push.contains("relay-approval"));
    h.remote.publish(&notify_done("relay-bot"));
    wait_for(|| {
        relays
            .sent(1)
            .iter()
            .any(|text| text.contains(r#""roomId":"team""#))
    })
    .await;
    let room: Value = serde_json::from_str(
        &relays
            .sent(1)
            .into_iter()
            .find(|text| text.contains(r#""roomId":"team""#))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        room,
        json!({ "t": "push", "id": room["id"], "tokens": ["synthetic-device-token"], "kind": "done", "botId": "relay-bot", "roomId": "team" })
    );

    h.call("pairing.cancel", json!({ "id": invite["id"] }))
        .await
        .unwrap();
    assert_eq!(relay_status(&h)["state"], "online");
    h.call("devices.revoke", json!({ "id": device.id }))
        .await
        .unwrap();
    assert_eq!(
        relay_status(&h)["state"],
        "online",
        "a revoked device still keeps the relay"
    );
    assert!(h.changes() > 0);
    h.remote.close().await;
}

#[tokio::test]
async fn keeps_the_relay_available_for_the_last_revoked_device_after_restarting_the_daemon() {
    let (relays, url, vault, db) = (
        Relays::default(),
        Url::default(),
        Arc::new(MemoryVault::default()),
        shared_db(),
    );
    let first = start(&relays, relays.silent(), &url, db.clone(), vault.clone()).await;
    let invite = first.call("pairing.create", json!({})).await.unwrap();
    first
        .call("pairing.cancel", json!({ "id": invite["id"] }))
        .await
        .unwrap();
    let device = add_device(&db.lock().unwrap(), &key_of(7), "Only phone").unwrap();
    set_relay(&first, &url, Some(RELAY)).await;
    assert_eq!(relay_status(&first)["state"], "connecting");
    wait_for(|| relays.created() == 1).await;
    first
        .call("devices.revoke", json!({ "id": device.id }))
        .await
        .unwrap();
    first.remote.close().await;

    let restarted = Relays::default();
    let second = start(&restarted, restarted.silent(), &url, db, vault).await;
    second.remote.start().await;
    wait_for(|| restarted.created() == 1).await;
    assert!(second.call("devices.list", json!({})).await.unwrap()[0]["revokedAt"].is_string());
    assert_eq!(relay_status(&second)["state"], "connecting");
    second.remote.close().await;
}

/// Pairs a Rust device over LAN, then hands back its key and the code.
async fn paired_over_lan(h: &Harness) -> (transport::Keypair, protocol::remote::PairingCode) {
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let code = parse_pairing_code(
        invite["code"].as_str().unwrap(),
        jiff::Timestamp::now().as_millisecond(),
    )
    .unwrap();
    let keys = generate_keypair().unwrap();
    let paired = authenticate(
        PeerWire::tcp(
            TcpStream::connect(("127.0.0.1", code.lan[0].port))
                .await
                .unwrap(),
        ),
        Pairing {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key: &keys.private_key[..],
            host_key: &URL_SAFE_NO_PAD.decode(code.host_key.as_str()).unwrap(),
            psk: Some(&URL_SAFE_NO_PAD.decode(code.secret.as_str()).unwrap()),
            name: "Synthetic device",
        },
    )
    .await
    .map_err(|(error, _)| error)
    .unwrap();
    paired.close();
    (keys, code)
}

async fn resume_over_lan(
    keys: &transport::Keypair,
    code: &protocol::remote::PairingCode,
) -> Option<String> {
    let resumed = authenticate(
        PeerWire::tcp(
            TcpStream::connect(("127.0.0.1", code.lan[0].port))
                .await
                .unwrap(),
        ),
        Pairing {
            preface: RemotePreface::resume(&code.host_id),
            private_key: &keys.private_key[..],
            host_key: &URL_SAFE_NO_PAD.decode(code.host_key.as_str()).unwrap(),
            psk: None,
            name: "Synthetic device",
        },
    )
    .await
    .map_err(|(error, _)| error)
    .unwrap();
    let relay = resumed.relay.clone();
    let phone = resumed.connect().await;
    assert_eq!(phone.call("remote.ping", json!({})).await, Ok(json!({})));
    phone.close();
    relay
}

#[tokio::test]
async fn announces_the_configured_relay_in_ready_when_a_device_resumes_over_lan_during_relay_login()
{
    let (relays, url) = (Relays::default(), Url::default());
    let h = start(&relays, relays.silent(), &url, shared_db(), Arc::default()).await;
    let (keys, code) = paired_over_lan(&h).await;
    set_relay(&h, &url, Some(RELAY)).await;
    wait_for(|| relays.created() == 1).await;
    assert_eq!(relay_status(&h)["state"], "connecting");
    assert_eq!(resume_over_lan(&keys, &code).await.as_deref(), Some(RELAY));
    h.remote.close().await;
}

#[tokio::test]
async fn keeps_ready_relay_metadata_while_a_separate_pairing_code_is_being_built() {
    let (relays, url) = (Relays::default(), Url::default());
    let h = start(&relays, relays.silent(), &url, shared_db(), Arc::default()).await;
    let (keys, code) = paired_over_lan(&h).await;
    set_relay(&h, &url, Some(RELAY)).await;
    // This pairing waits for a relay that never answers.
    let remote = h.server.clone();
    let waiting =
        tokio::spawn(async move { remote.dispatch("pairing.create", json!({}), None).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiting.is_finished());
    assert_eq!(resume_over_lan(&keys, &code).await.as_deref(), Some(RELAY));
    waiting.abort();
    h.remote.close().await;
}

#[tokio::test(start_paused = true)]
async fn returns_a_lan_only_pairing_code_when_the_relay_socket_never_answers() {
    let (relays, url) = (Relays::default(), Url::default());
    let h = start(&relays, relays.silent(), &url, shared_db(), Arc::default()).await;
    set_relay(&h, &url, Some(RELAY)).await;
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    assert_eq!(relays.created(), 1);
    let code = parse_pairing_code(
        invite["code"].as_str().unwrap(),
        jiff::Timestamp::now().as_millisecond(),
    )
    .unwrap();
    assert_eq!(code.relay, None);
    assert_eq!(relay_status(&h)["state"], "error");
    h.remote.close().await;
}

#[tokio::test]
async fn keeps_the_relay_running_while_another_pairing_request_still_waits() {
    let (relays, url) = (Relays::default(), Url::default());
    let h = start(
        &relays,
        relays.answering(),
        &url,
        shared_db(),
        Arc::default(),
    )
    .await;
    set_relay(&h, &url, Some(RELAY)).await;
    let (one, two) = tokio::join!(
        h.call("pairing.create", json!({})),
        h.call("pairing.create", json!({}))
    );
    for invite in [one.unwrap(), two.unwrap()] {
        let code = parse_pairing_code(
            invite["code"].as_str().unwrap(),
            jiff::Timestamp::now().as_millisecond(),
        );
        // The first code was replaced by the second; only the newest is still valid, and it names the relay.
        if let Ok(code) = code {
            assert_eq!(
                code.relay.map(|relay| relay.as_str().to_owned()).as_deref(),
                Some(RELAY)
            );
        }
    }
    assert_eq!(relays.created(), 1, "both requests share one relay login");
    assert!(!relays.closers.lock().unwrap()[0].is_cancelled());
    assert_eq!(relays.closed.load(Ordering::Relaxed), 0);
    assert_eq!(relay_status(&h)["state"], "online");
    h.remote.close().await;
}

#[tokio::test]
async fn reports_a_startup_lan_listen_failure_in_relay_status() {
    let (relays, url, vault, db) = (
        Relays::default(),
        Url::default(),
        Arc::new(MemoryVault::default()),
        shared_db(),
    );
    let first = start(&relays, relays.silent(), &url, db.clone(), vault.clone()).await;
    first.call("pairing.create", json!({})).await.unwrap();
    first.remote.close().await;
    add_device(&db.lock().unwrap(), &key_of(4), "Synthetic phone").unwrap();
    let blocker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let h = Harness::start(Setup {
        db,
        vault,
        relay_url: setting(&url),
        relay_connect: relays.silent(),
        port: blocker.local_addr().unwrap().port(),
        ..Setup::default()
    })
    .await;
    h.remote.start().await;
    assert_eq!(
        h.call("remote.status", json!({})).await.unwrap()["listening"],
        false
    );
    set_relay(&h, &url, Some(RELAY)).await;
    wait_for(|| h.remote.status().relay.state == protocol::remote::RelayState::Error).await;
    h.remote.close().await;
}

#[tokio::test]
async fn stops_the_relay_when_the_setting_is_cleared_and_reports_off() {
    let (relays, url) = (Relays::default(), Url::default());
    let h = start(
        &relays,
        relays.answering(),
        &url,
        shared_db(),
        Arc::default(),
    )
    .await;
    set_relay(&h, &url, Some(RELAY)).await;
    h.call("pairing.create", json!({})).await.unwrap();
    assert_eq!(relay_status(&h)["state"], "online");
    set_relay(&h, &url, None).await;
    wait_for(|| relays.closers.lock().unwrap()[0].is_cancelled()).await;
    assert_eq!(
        relay_status(&h),
        json!({ "url": null, "state": "off", "push": false })
    );
    h.remote.close().await;
}
