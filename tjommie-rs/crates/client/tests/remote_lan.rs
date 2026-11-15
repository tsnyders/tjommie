//! Interop: this client, as a device, pairs with the Rust daemon's LAN host over TCP on 127.0.0.1, talks to it through
//! the Noise channel, resumes with its key, and is refused when the PC doesn't know it or has revoked it. The host is the
//! real one (`daemon::remote`), on a loopback port, with an in-memory credential vault and an in-memory database: nothing
//! here touches Credential Manager, a data root or a network interface. The relay leg needs wrangler's local relay, which
//! only the daemon's ignored `real_relay_client` test runs.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use client::remote::{Auth, AuthError, Wires, authenticate, connect_peer, generate_keypair};
use client::{ConnectOptions, Endpoint};
use daemon::relay_client::default_connector;
use daemon::remote::{Remote, RemoteOptions};
use daemon::remote_host::{Dispatch, RemoteHostOptions};
use daemon::remote_identity::Vault;
use daemon::server::{Handlers, RequestContext, Server, ServerOptions};
use protocol::remote::{PairingCode, RefusedReason, RemotePreface, parse_pairing_code};
use protocol::wire::{Empty, Event, method};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(15);

/// The synthetic credential vault of the daemon's own tests: a map.
#[derive(Default)]
struct Memory(Mutex<HashMap<String, String>>);

impl Vault for Memory {
    fn read(&self, target: &str) -> io::Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(target).cloned())
    }
    fn write(&self, target: &str, value: &str) -> io::Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(target.to_owned(), value.to_owned());
        Ok(())
    }
}

struct Pc {
    server: Arc<Server>,
    remote: Arc<Remote>,
}

async fn start() -> Pc {
    let db = Arc::new(Mutex::new(daemon::db::open(Path::new(":memory:")).unwrap()));
    let slot: Arc<OnceLock<Arc<Server>>> = Arc::default();
    let to_server = slot.clone();
    let dispatch: Dispatch = Arc::new(
        move |method: String, params: Value, context: RequestContext| {
            let slot = to_server.clone();
            Box::pin(async move {
                slot.get()
                    .unwrap()
                    .dispatch(&method, params, Some(context))
                    .await
            })
        },
    );
    let mut host = RemoteHostOptions::new(db, PathBuf::from("synthetic-handlers-root"), dispatch);
    // Never the production 0.0.0.0:47101 bind: loopback on an ephemeral port, and never the real Credential Manager.
    host.port = 0;
    host.bind = [127, 0, 0, 1].into();
    host.addresses = Some(Arc::new(|| vec!["127.0.0.1".into()]));
    host.vault = Arc::new(Memory::default());
    let remote = Remote::new(RemoteOptions {
        host,
        relay_url: Arc::new(|| None),
        relay_connect: default_connector(),
        broadcast: Arc::new(|_: Event| {}),
    });
    let server = Arc::new(
        Server::start(ServerOptions {
            port: 0,
            token: "synthetic-token".into(),
            handlers: remote.handlers(Handlers::default()),
            allowed_origins: Vec::new(),
            defer_requests: false,
        })
        .await
        .unwrap(),
    );
    assert!(slot.set(server.clone()).is_ok());
    Pc { server, remote }
}

impl Pc {
    async fn call(&self, method: &str, params: Value) -> Value {
        self.server.dispatch(method, params, None).await.unwrap()
    }

    /// A fresh invitation, as the user would copy it from the PC.
    async fn invite(&self) -> PairingCode {
        let started = self.call("pairing.create", json!({})).await;
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        parse_pairing_code(started["code"].as_str().unwrap(), now).unwrap()
    }
}

fn bytes(key: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(key).unwrap()
}

async fn wire(code: &PairingCode) -> client::remote::Wire {
    let lan = code.lan.first().expect("the code names a LAN address");
    let endpoint = Endpoint::Lan {
        host: lan.host.clone(),
        port: lan.port,
    };
    Wires::new().open(&endpoint, WAIT).await.unwrap()
}

#[tokio::test]
async fn pairs_talks_resumes_and_is_refused_over_lan() {
    let pc = start().await;
    let code = pc.invite().await;
    let keys = generate_keypair().unwrap();
    let host_key = bytes(code.host_key.as_str());

    // Pair: the invitation's secret is the pre-shared key, and the first encrypted message proves we hold it.
    let mut confirmed = false;
    let peer = authenticate(
        wire(&code).await,
        Auth {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key: &keys.private_key[..],
            host_key: &host_key,
            psk: Some(&bytes(code.secret.as_str())),
            name: "Synthetic Rust device",
        },
        || confirmed = true,
    )
    .await
    .unwrap();
    assert!(
        confirmed,
        "the confirmation was sent before the answer was read"
    );
    assert_eq!(
        (peer.host_id.as_str(), peer.host_name.as_str()),
        (code.host_id.as_str(), code.host_name.as_str())
    );
    let device_id = peer.device_id.clone();

    // Requests the host answers itself, requests it dispatches to the daemon, and an event.
    let (client, info) = connect_peer(peer, ConnectOptions::default()).await.unwrap();
    assert_eq!(info.device_id, device_id);
    client.call::<method::RemotePing>(Empty {}).await.unwrap();
    let devices = client.call::<method::DevicesList>(Empty {}).await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(
        (devices[0].id.as_str(), devices[0].name.as_str()),
        (device_id.as_str(), "Synthetic Rust device")
    );
    let mut events = client.events();
    pc.remote.publish(&Event::DevicesChanged(Empty {}));
    let event = tokio::time::timeout(WAIT, events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(event, Event::DevicesChanged(_)), "{event:?}");
    client.close().await;

    // Resume: same key, no secret, same device.
    let resumed = authenticate(
        wire(&code).await,
        Auth {
            preface: RemotePreface::resume(&code.host_id),
            private_key: &keys.private_key[..],
            host_key: &host_key,
            psk: None,
            name: "Synthetic Rust device",
        },
        || {},
    )
    .await
    .unwrap();
    assert_eq!(resumed.device_id, device_id);
    drop(resumed);

    // A key the PC has never paired is refused over the authenticated channel, not just disconnected.
    let stranger = generate_keypair().unwrap();
    let refused = authenticate(
        wire(&code).await,
        Auth {
            preface: RemotePreface::resume(&code.host_id),
            private_key: &stranger.private_key[..],
            host_key: &host_key,
            psk: None,
            name: "Stranger",
        },
        || {},
    )
    .await;
    assert_eq!(
        refused.err(),
        Some(AuthError::Refused(RefusedReason::Unknown))
    );

    // Revoked on the PC: refused as revoked.
    pc.call("devices.revoke", json!({ "id": device_id })).await;
    let revoked = authenticate(
        wire(&code).await,
        Auth {
            preface: RemotePreface::resume(&code.host_id),
            private_key: &keys.private_key[..],
            host_key: &host_key,
            psk: None,
            name: "Synthetic Rust device",
        },
        || {},
    )
    .await;
    assert_eq!(
        revoked.err(),
        Some(AuthError::Refused(RefusedReason::Revoked))
    );
}

#[tokio::test]
async fn a_wrong_pairing_secret_or_pc_key_does_not_authenticate() {
    let pc = start().await;
    let code = pc.invite().await;
    let keys = generate_keypair().unwrap();
    let wrong = generate_keypair().unwrap();
    let host_key = bytes(code.host_key.as_str());

    // The pairing secret is wrong: the PC can't read our first message and closes.
    let bad_secret = authenticate(
        wire(&code).await,
        Auth {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key: &keys.private_key[..],
            host_key: &host_key,
            psk: Some(&[9u8; 32]),
            name: "Synthetic Rust device",
        },
        || {},
    )
    .await;
    assert_eq!(bad_secret.err(), Some(AuthError::Failed));

    // We expect a different PC key than the one that answers: Noise IK fails, and nothing is sent that needs trust.
    let code = pc.invite().await;
    let mut sent_confirm = false;
    let bad_host = authenticate(
        wire(&code).await,
        Auth {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key: &keys.private_key[..],
            host_key: &wrong.public_key,
            psk: Some(&bytes(code.secret.as_str())),
            name: "Synthetic Rust device",
        },
        || sent_confirm = true,
    )
    .await;
    assert_eq!(bad_host.err(), Some(AuthError::Failed));
    assert!(
        !sent_confirm,
        "the confirmation is only sent after the PC proved its key"
    );
}
