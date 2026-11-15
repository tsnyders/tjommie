//! The whole path a desktop pairing takes, against the real Rust daemon's LAN host on 127.0.0.1: `Hosts::pair` (invitation,
//! pending identity, Noise handshake, promotion), `Hosts::open` (resume, the client over the encrypted channel) and
//! `forget`, with the real `NetworkDialer` racing a dead LAN address against the live one. The PC's credential vault and
//! database are in memory, and so are this side's credentials and host lists: nothing here touches Credential Manager,
//! a data root or a network interface other than loopback.

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
use std::time::{SystemTime, UNIX_EPOCH};

use daemon::relay_client::default_connector;
use daemon::remote::{Remote, RemoteOptions};
use daemon::remote_host::{Dispatch, RemoteHostOptions};
use daemon::remote_identity::Vault;
use daemon::server::{Handlers, RequestContext, Server, ServerOptions};
use desktop_host::device_keys::{CredentialStore, DeviceKeys};
use desktop_host::remote_hosts::{HostStore, Hosts, NetworkDialer, RemoteHost, new_private_key};
use protocol::remote::{LanEndpoint, encode_pairing_code, parse_pairing_code};
use protocol::wire::{Empty, Event, method};
use serde_json::{Value, json};

#[derive(Default)]
struct PcVault(Mutex<HashMap<String, String>>);

impl Vault for PcVault {
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

#[derive(Default)]
struct Creds(Mutex<HashMap<String, String>>);

impl CredentialStore for Creds {
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
    fn delete(&self, target: &str) -> io::Result<()> {
        self.0.lock().unwrap().remove(target);
        Ok(())
    }
}

#[derive(Default)]
struct Store {
    hosts: Mutex<Vec<RemoteHost>>,
    pending: Mutex<Vec<RemoteHost>>,
}

impl HostStore for Store {
    fn hosts(&self) -> Vec<RemoteHost> {
        self.hosts.lock().unwrap().clone()
    }
    fn pending(&self) -> Vec<RemoteHost> {
        self.pending.lock().unwrap().clone()
    }
    fn set_hosts(&self, hosts: Vec<RemoteHost>) {
        *self.hosts.lock().unwrap() = hosts;
    }
    fn set_pending(&self, hosts: Vec<RemoteHost>) {
        *self.pending.lock().unwrap() = hosts;
    }
}

async fn start_pc() -> (Arc<Server>, Arc<Remote>) {
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
    // Never the production 0.0.0.0:47101 bind, and never the real Credential Manager.
    host.port = 0;
    host.bind = [127, 0, 0, 1].into();
    host.addresses = Some(Arc::new(|| vec!["127.0.0.1".into()]));
    host.vault = Arc::new(PcVault::default());
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
    (server, remote)
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[tokio::test]
async fn pairs_opens_a_client_and_forgets_through_the_real_network_dialer() {
    let (server, _remote) = start_pc().await;
    let started = server
        .dispatch("pairing.create", json!({}), None)
        .await
        .unwrap();
    let mut code = parse_pairing_code(started["code"].as_str().unwrap(), now_ms()).unwrap();
    // A LAN address nothing listens on comes first: the race must not wait for it.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = dead.local_addr().unwrap().port();
    drop(dead);
    code.lan.insert(
        0,
        LanEndpoint {
            host: "127.0.0.1".into(),
            port: dead_port,
        },
    );
    let invitation = encode_pairing_code(&code).unwrap();

    let store = Store::default();
    let keys = DeviceKeys::with_store(Path::new("C:/isolated/tjommie-hosts-lan"), Creds::default())
        .unwrap();
    let dialer = NetworkDialer::new();
    let hosts = Hosts {
        store: &store,
        keys: &keys,
        dialer: &dialer,
    };

    let paired = hosts
        .pair(
            &invitation,
            "Synthetic desktop",
            now_ms(),
            &new_private_key().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(paired.id, code.host_id);
    assert!(
        paired.device_id.is_some(),
        "the PC told us who we are to it"
    );
    assert_eq!(store.hosts(), vec![paired.clone()]);
    assert!(
        store.pending().is_empty(),
        "the pending pairing was promoted"
    );

    let client = hosts.open(&paired.id).await.unwrap();
    client.call::<method::RemotePing>(Empty {}).await.unwrap();
    let devices = client.call::<method::DevicesList>(Empty {}).await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(Some(devices[0].id.as_str()), paired.device_id.as_deref());
    assert_eq!(devices[0].name.as_str(), "Synthetic desktop");
    client.close().await;

    // The PC revokes this device: the next connection is refused, and says so.
    server
        .dispatch("devices.revoke", json!({ "id": paired.device_id }), None)
        .await
        .unwrap();
    let refused = hosts.open(&paired.id).await.err().unwrap();
    assert_eq!(
        refused.to_string(),
        "This device is not paired with this PC."
    );

    hosts.forget(&paired.id).unwrap();
    assert!(hosts.all().is_empty());
    assert!(keys.store().0.lock().unwrap().is_empty());
}
