//! The race `NetworkDialer` runs to reach a PC (`authenticate` in `remote-hosts.ts`), against the real Rust daemon's LAN host
//! on 127.0.0.1. Ports of three tests in `packages/ui/lib/connection.heartbeat.test.ts`: the first LAN endpoint to finish the
//! handshake wins and the slower one is closed; the relay joins after the LAN window when every LAN endpoint fails; and a
//! LAN endpoint that connects but stalls its handshake doesn't hold the relay back past that window.
//!
//! The PC is real, so the handshake is the real Noise one. What differs per endpoint is a seam, [`Routes`] (an `OpenWire`):
//! it maps each endpoint of the saved PC onto 127.0.0.1 (the PC's port, a listener that never answers, or nowhere at all), and
//! writes down every open: which endpoint, the dial timeout it was given, when, and whether its wire has closed since. The
//! window is short (the dialer takes it as a parameter) and the time is real. The PC's vault and database are in memory, and
//! so are this side's credentials and host lists: nothing here touches Credential Manager, a data root, a real relay, or any
//! address but loopback (the LAN and relay addresses below are labels that `Routes` never dials).

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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use client::remote::{Wire, WireError, Wires};
use client::{Client, Endpoint};
use daemon::relay_client::default_connector;
use daemon::remote::{Remote, RemoteOptions};
use daemon::remote_host::{Dispatch, RemoteHostOptions};
use daemon::remote_identity::Vault;
use daemon::server::{Handlers, RequestContext, Server, ServerOptions};
use desktop_host::device_keys::{CredentialStore, DeviceKeys};
use desktop_host::remote_hosts::{
    HostStore, Hosts, NetworkDialer, OpenWire, RACE_DEADLINE, RemoteHost, new_private_key,
};
use protocol::remote::{LanEndpoint, RelayUrl, parse_pairing_code};
use protocol::wire::{Empty, method};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

/// The LAN window these tests give the dialer, instead of the 1 s one: it is a parameter.
const WINDOW: Duration = Duration::from_millis(500);
/// How late a timer may fire on a busy machine before a test calls it late. The failures these tests look for are 15 s late.
const SLACK: Duration = Duration::from_secs(3);
/// The slow endpoint's handshake: far past anything a loopback handshake takes.
const SLOW: Duration = Duration::from_secs(3);
const RELAY: &str = "https://relay.example.workers.dev";

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
        broadcast: Arc::new(|_: protocol::wire::Event| {}),
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

/// How an endpoint of the saved PC behaves.
#[derive(Clone)]
enum Route {
    /// Dials 127.0.0.1:`port`, and hands the wire over `delay` after it connected: the handshake can't start before that.
    Dial { port: u16, delay: Duration },
    /// Nothing answers: the dial runs out its timeout, as an address on another network does.
    Unreachable,
}

impl Route {
    fn to(port: u16) -> Self {
        Self::Dial {
            port,
            delay: Duration::ZERO,
        }
    }
}

/// One open `Routes` was asked for.
#[derive(Clone)]
struct Opened {
    endpoint: Endpoint,
    /// The dial timeout the dialer gave it.
    timeout: Duration,
    at: Instant,
    /// Set once the wire this open brought has closed (stays unset when no wire was ever opened).
    closed: Arc<AtomicBool>,
}

/// `OpenWire` for the tests: only the endpoints in `routes` go anywhere, and only to 127.0.0.1.
struct Routes {
    wires: Wires,
    routes: Vec<(Endpoint, Route)>,
    log: Arc<Mutex<Vec<Opened>>>,
}

impl Routes {
    fn new(routes: Vec<(Endpoint, Route)>) -> Self {
        Self {
            wires: Wires::new(),
            routes,
            log: Arc::default(),
        }
    }
}

impl OpenWire for Routes {
    async fn open(&self, endpoint: &Endpoint, timeout: Duration) -> Result<Wire, WireError> {
        let closed = Arc::new(AtomicBool::new(false));
        self.log.lock().unwrap().push(Opened {
            endpoint: endpoint.clone(),
            timeout,
            at: Instant::now(),
            closed: closed.clone(),
        });
        let route = self
            .routes
            .iter()
            .find(|(known, _)| known == endpoint)
            .map(|(_, route)| route.clone())
            .ok_or(WireError::Unavailable)?;
        let (port, delay) = match route {
            Route::Unreachable => {
                tokio::time::sleep(timeout).await;
                return Err(WireError::Unavailable);
            }
            Route::Dial { port, delay } => (port, delay),
        };
        let wire = self
            .wires
            .open(
                &Endpoint::Lan {
                    host: "127.0.0.1".into(),
                    port,
                },
                timeout,
            )
            .await?;
        let gone = wire.closed();
        tokio::spawn(async move {
            gone.await;
            closed.store(true, Ordering::SeqCst);
        });
        // Held, not dropped: a leg that loses the race while it waits here closes this wire.
        tokio::time::sleep(delay).await;
        Ok(wire)
    }
}

/// Accepts one connection on 127.0.0.1 and never says a word (something answers on that address, but not the PC). The flag
/// is set once the dialer has let go of the connection.
async fn stalled_lan() -> (u16, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let let_go = Arc::new(AtomicBool::new(false));
    let flag = let_go.clone();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 4096];
        // The preface and the handshake arrive and go unanswered; the read ends when the dialer closes its side.
        while let Ok(1..) = stream.read(&mut buffer).await {}
        flag.store(true, Ordering::SeqCst);
    });
    (port, let_go)
}

/// Polls until the flag is set, for up to 5 s.
async fn eventually(flag: &AtomicBool) -> bool {
    for _ in 0..500 {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// A phone paired with a live PC: the PC's LAN host is on `pc_port`, and the pairing is saved with the identity it sealed.
struct Device {
    pc_port: u16,
    store: Store,
    keys: DeviceKeys<Creds>,
    paired: RemoteHost,
    _pc: (Arc<Server>, Arc<Remote>),
}

async fn paired_device() -> Device {
    let pc = start_pc().await;
    let started =
        pc.0.dispatch("pairing.create", json!({}), None)
            .await
            .unwrap();
    let invitation = started["code"].as_str().unwrap().to_owned();
    let code = parse_pairing_code(&invitation, now_ms()).unwrap();
    let pc_port = code.lan.first().unwrap().port;
    let store = Store::default();
    let keys =
        DeviceKeys::with_store(Path::new("C:/isolated/tjommie-race"), Creds::default()).unwrap();
    let paired = {
        let dialer = NetworkDialer::new();
        let hosts = Hosts {
            store: &store,
            keys: &keys,
            dialer: &dialer,
        };
        hosts
            .pair(
                &invitation,
                "Synthetic desktop",
                now_ms(),
                &new_private_key().unwrap(),
            )
            .await
            .unwrap()
    };
    Device {
        pc_port,
        store,
        keys,
        paired,
        _pc: pc,
    }
}

/// What a connection attempt left behind.
struct Raced {
    client: Client,
    opens: Vec<Opened>,
    /// From just before the attempt began.
    began: Instant,
    took: Duration,
}

impl Raced {
    fn endpoints(&self) -> Vec<Endpoint> {
        self.opens
            .iter()
            .map(|opened| opened.endpoint.clone())
            .collect()
    }

    /// When the n-th open began, counted from the start of the attempt.
    fn open_at(&self, n: usize) -> Duration {
        self.opens[n].at.duration_since(self.began)
    }
}

/// Connects to the PC as saved with `lan` and `relay`, through the real `NetworkDialer` with `WINDOW` and the 30 s deadline.
async fn race(
    device: &Device,
    lan: &[LanEndpoint],
    relay: Option<&str>,
    routes: Vec<(Endpoint, Route)>,
) -> Raced {
    device.store.set_hosts(vec![RemoteHost {
        lan: lan.to_vec(),
        relay: relay.map(|url| RelayUrl::new(url).unwrap()),
        ..device.paired.clone()
    }]);
    let opener = Routes::new(routes);
    let log = opener.log.clone();
    let dialer = NetworkDialer::with(opener, WINDOW, RACE_DEADLINE);
    let hosts = Hosts {
        store: &device.store,
        keys: &device.keys,
        dialer: &dialer,
    };
    let began = Instant::now();
    let client = hosts.open(&device.paired.id).await.unwrap();
    let took = began.elapsed();
    let opens = log.lock().unwrap().clone();
    // The connection is the PC's, authenticated: it answers.
    client.call::<method::RemotePing>(Empty {}).await.unwrap();
    Raced {
        client,
        opens,
        began,
        took,
    }
}

fn first() -> LanEndpoint {
    LanEndpoint {
        host: "192.168.1.2".into(),
        port: 47101,
    }
}

fn second() -> LanEndpoint {
    LanEndpoint {
        host: "10.0.0.7".into(),
        port: 47101,
    }
}

fn lan(endpoint: &LanEndpoint) -> Endpoint {
    Endpoint::Lan {
        host: endpoint.host.clone(),
        port: endpoint.port,
    }
}

fn relay(device: &Device) -> Endpoint {
    Endpoint::Relay {
        url: RELAY.into(),
        host_id: device.paired.id.clone(),
    }
}

#[tokio::test]
async fn races_the_lan_endpoints_the_first_to_finish_the_handshake_wins_and_the_slower_one_is_closed()
 {
    let device = paired_device().await;
    let raced = race(
        &device,
        &[first(), second()],
        None,
        vec![
            // The first endpoint is the slow one: its handshake can't begin for SLOW.
            (
                lan(&first()),
                Route::Dial {
                    port: device.pc_port,
                    delay: SLOW,
                },
            ),
            (lan(&second()), Route::to(device.pc_port)),
        ],
    )
    .await;
    assert_eq!(raced.endpoints(), vec![lan(&first()), lan(&second())]);
    assert!(
        raced.took < SLOW,
        "the race waited for the slow endpoint: {:?}",
        raced.took
    );
    assert!(
        eventually(&raced.opens[0].closed).await,
        "the slower wire was left open"
    );
    // Give a wire that was wrongly closed along with it time to show.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !raced.opens[1].closed.load(Ordering::SeqCst),
        "the winner's wire was closed"
    );
    raced.client.close().await;
}

#[tokio::test]
async fn tries_the_relay_after_the_lan_window_when_every_lan_endpoint_fails() {
    let device = paired_device().await;
    let raced = race(
        &device,
        &[first(), second()],
        Some(RELAY),
        vec![
            (lan(&first()), Route::Unreachable),
            (lan(&second()), Route::Unreachable),
            (relay(&device), Route::to(device.pc_port)),
        ],
    )
    .await;
    assert_eq!(
        raced.endpoints(),
        vec![lan(&first()), lan(&second()), relay(&device)]
    );
    // Both LAN addresses were dialed at once, each with the window as its dial timeout, and the relay waited for it.
    assert!(raced.open_at(0) < WINDOW && raced.open_at(1) < WINDOW);
    assert_eq!(raced.opens[0].timeout, WINDOW);
    assert_eq!(raced.opens[1].timeout, WINDOW);
    assert!(
        raced.open_at(2) >= WINDOW,
        "the relay joined before the LAN window passed: {:?}",
        raced.open_at(2)
    );
    assert!(
        raced.open_at(2) < WINDOW + SLACK,
        "the relay joined late: {:?}",
        raced.open_at(2)
    );
    // It dials with what is left of the whole race.
    assert!(raced.opens[2].timeout <= RACE_DEADLINE - WINDOW);
    raced.client.close().await;
}

#[tokio::test]
async fn does_not_let_a_lan_endpoint_that_connects_but_stalls_its_handshake_hold_the_relay_back_past_the_lan_window()
 {
    let device = paired_device().await;
    let (stalled_port, let_go) = stalled_lan().await;
    let raced = race(
        &device,
        &[first()],
        Some(RELAY),
        vec![
            (lan(&first()), Route::to(stalled_port)),
            (relay(&device), Route::to(device.pc_port)),
        ],
    )
    .await;
    assert_eq!(raced.endpoints(), vec![lan(&first()), relay(&device)]);
    // The stalled handshake would have run for 15 s: the relay joined when the window passed, not when that ended.
    assert!(
        raced.open_at(1) >= WINDOW,
        "the relay joined before the LAN window passed: {:?}",
        raced.open_at(1)
    );
    assert!(
        raced.took < WINDOW + SLACK,
        "the stalled endpoint held the relay back: {:?}",
        raced.took
    );
    assert!(eventually(&let_go).await, "the stalled wire was left open");
    raced.client.close().await;
}
