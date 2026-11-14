//! The PCs this device is paired with: pairing, resuming, forgetting, and the race that reaches one over LAN and then
//! relay. Port of `packages/ui/lib/remote-hosts.ts` (the Android phone and the desktop app shared it). The wires and the
//! Noise handshake are `client::remote`; the device's key for each PC is a device key (`device_keys`); which PCs there are
//! lives in `desktop-ui.json` (`device_settings`) behind [`HostStore`]. Everything that touches the network or a disk is a
//! seam, so the TypeScript tests port without either.
//!
//! A PC's identity is sealed in two places while pairing: `identity-<id>.pending` from before the confirmation goes out,
//! and `identity-<id>` once the pairing succeeded. If the answer to the confirmation is lost the PC may have committed the
//! pairing, so the pending identity is kept and tried on the next connection; it replaces the saved one only after it
//! works.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use client::remote::{
    Auth, AuthError, Peer, PeerInfo, Wire, WireError, Wires, authenticate, connect_peer,
};
use client::{Client, ConnectOptions, Endpoint};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use protocol::remote::{
    RefusedReason, RelayUrl, RemoteKey, RemoteName, RemotePreface, parse_pairing_code,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::device_keys::{CredentialStore, DeviceKeyError, DeviceKeys};
pub use crate::device_settings::{MAX_HOSTS, RemoteHost};

/// How long LAN dials get before the relay joins the race (`LINK.lanWindowMs`).
pub const LAN_WINDOW: Duration = Duration::from_secs(1);
/// The whole race, LAN and relay (`authenticate` in `remote-hosts.ts`).
pub const RACE_DEADLINE: Duration = Duration::from_secs(30);
/// What a resumed connection calls itself in the handshake. The PC only uses the name when pairing.
const RESUME_NAME: &str = "Tjommie desktop";

/// Where the PC lists live. Both lists hold at most [`MAX_HOSTS`] and are written as given.
pub trait HostStore: Send + Sync {
    fn hosts(&self) -> Vec<RemoteHost>;
    fn pending(&self) -> Vec<RemoteHost>;
    fn set_hosts(&self, hosts: Vec<RemoteHost>);
    fn set_pending(&self, hosts: Vec<RemoteHost>);
}

/// Why a PC couldn't be reached. The messages are the TypeScript's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialError {
    /// The PC answered, over the authenticated channel, that this device isn't paired with it.
    Refused(RefusedReason),
    /// The PC's relay said the PC isn't connected to it (close 4404), and nothing reached it on the LAN either.
    HostOffline,
    /// Nothing reached the PC. `relay`: whether it has a relay (which changes what to check).
    Unreachable {
        relay: bool,
    },
    Failed(String),
}

impl std::fmt::Display for DialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Refused(_) => "This device is not paired with this PC.",
            Self::HostOffline => "Your PC is offline. Check that it's awake with Tjommie running.",
            Self::Unreachable { relay: true } => {
                "Can't reach your PC. Check that it's awake with Tjommie open."
            }
            Self::Unreachable { relay: false } => {
                "Can't reach your PC. Keep both devices on the same Wi-Fi, and keep Tjommie open on the PC."
            }
            Self::Failed(message) => message,
        })
    }
}

impl std::error::Error for DialError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    Dial(DialError),
    /// Both lists are full.
    TooMany,
    /// The pairing code is bad or has expired (one message for every cause, so the input is never echoed).
    BadCode(String),
    BadName,
    /// A PC with this id is already paired under another key.
    KeyChanged,
    /// The pending identity isn't there.
    PendingMissing,
    /// The sealed identity isn't the one for this PC.
    IdentityMismatch,
    NotPaired,
    Storage(DeviceKeyError),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dial(error) => error.fmt(formatter),
            Self::TooMany => formatter.write_str("Forget a PC before pairing another one."),
            Self::BadCode(message) => formatter.write_str(message),
            Self::BadName => formatter.write_str("Enter a device name without control characters."),
            Self::KeyChanged => formatter.write_str(
                "This PC's connection key changed. Check the PC before forgetting it and pairing again.",
            ),
            Self::PendingMissing => {
                formatter.write_str("The pending connection key is missing. Pair again.")
            }
            Self::IdentityMismatch => formatter.write_str(
                "The saved PC identity does not match its protected connection key. Forget this PC and pair again.",
            ),
            Self::NotPaired => formatter.write_str("Pair this device with your PC first."),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HostError {}

impl From<DeviceKeyError> for HostError {
    fn from(error: DeviceKeyError) -> Self {
        Self::Storage(error)
    }
}

/// What a connection attempt brings: who this device is to the PC, and how to greet it.
pub struct Request<'a> {
    pub preface: RemotePreface,
    pub private_key: &'a [u8],
    /// The pairing secret, when pairing.
    pub psk: Option<&'a [u8]>,
    pub name: &'a str,
    /// Called once the confirmation is sent: from then on the PC may have committed the pairing.
    pub on_confirm_sent: &'a (dyn Fn() + Sync),
}

/// The network behind pairing and resuming. [`NetworkDialer`] is the real one; tests give a fake.
pub trait Dialer: Send + Sync {
    type Peer: Send;
    type Client: Send;
    /// Reaches the PC and proves who both sides are.
    fn authenticate(
        &self,
        host: &RemoteHost,
        request: &Request<'_>,
    ) -> impl Future<Output = Result<(Self::Peer, PeerInfo), DialError>> + Send;
    /// Runs the client on an authenticated channel.
    fn open(
        &self,
        peer: Self::Peer,
    ) -> impl Future<Output = Result<Self::Client, DialError>> + Send;
}

fn identity_name(id: &str, pending: bool) -> String {
    format!("identity-{id}{}", if pending { ".pending" } else { "" })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Identity {
    v: u8,
    host_id: String,
    host_key: String,
    private_key: String,
}

/// `encodeMobileIdentity`: the device's private key for a PC, bound to that PC's id and key, so editing the host list
/// can't make a saved identity work for another PC.
pub fn encode_identity(
    host_id: &str,
    host_key: &RemoteKey,
    private_key: &[u8],
) -> Result<Zeroizing<Vec<u8>>, HostError> {
    if private_key.len() != 32 {
        return Err(HostError::IdentityMismatch);
    }
    let identity = Identity {
        v: 1,
        host_id: host_id.to_owned(),
        host_key: host_key.as_str().to_owned(),
        private_key: URL_SAFE_NO_PAD.encode(private_key),
    };
    serde_json::to_vec(&identity)
        .map(Zeroizing::new)
        .map_err(|_| HostError::IdentityMismatch)
}

/// `decodeMobileIdentity`: the private key, if the envelope is for this PC.
pub fn decode_identity(
    bytes: &[u8],
    host_id: &str,
    host_key: &RemoteKey,
) -> Result<Zeroizing<Vec<u8>>, HostError> {
    let saved: Identity = serde_json::from_slice(bytes).map_err(|_| HostError::IdentityMismatch)?;
    let key = RemoteKey::new(&saved.private_key).map_err(|_| HostError::IdentityMismatch)?;
    if saved.v != 1 || saved.host_id != host_id || saved.host_key != host_key.as_str() {
        return Err(HostError::IdentityMismatch);
    }
    URL_SAFE_NO_PAD
        .decode(key.as_str())
        .map(Zeroizing::new)
        .map_err(|_| HostError::IdentityMismatch)
}

/// A new private key for pairing.
pub fn new_private_key() -> Result<Zeroizing<Vec<u8>>, HostError> {
    client::remote::generate_keypair()
        .map(|keys| Zeroizing::new(keys.private_key.to_vec()))
        .map_err(|_| HostError::Dial(DialError::Failed("Couldn't make a connection key.".into())))
}

/// This device's PCs. Borrowed from wherever the app keeps its store, keys and network.
pub struct Hosts<'a, S: HostStore, C: CredentialStore, D: Dialer> {
    pub store: &'a S,
    pub keys: &'a DeviceKeys<C>,
    pub dialer: &'a D,
}

impl<S: HostStore, C: CredentialStore, D: Dialer> Hosts<'_, S, C, D> {
    /// Every PC: the paired ones, then the pending ones that aren't also paired.
    pub fn all(&self) -> Vec<RemoteHost> {
        let mut all = self.store.hosts();
        for host in self.store.pending() {
            if !all.iter().any(|entry| entry.id == host.id) {
                all.push(host);
            }
        }
        all
    }

    fn save_host(&self, host: RemoteHost, pending: bool) -> Result<(), HostError> {
        let mut current = if pending {
            self.store.pending()
        } else {
            self.store.hosts()
        };
        current.retain(|entry| entry.id != host.id);
        if current.len() >= MAX_HOSTS {
            return Err(HostError::TooMany);
        }
        current.insert(0, host);
        if pending {
            self.store.set_pending(current);
        } else {
            self.store.set_hosts(current);
        }
        Ok(())
    }

    fn clear_pending(&self, id: &str) -> Result<(), HostError> {
        self.keys.delete(&identity_name(id, true))?;
        let mut pending = self.store.pending();
        pending.retain(|host| host.id != id);
        self.store.set_pending(pending);
        Ok(())
    }

    /// The pending identity becomes the identity, and the PC is saved as paired.
    fn promote(&self, host: RemoteHost) -> Result<(), HostError> {
        let sealed = self
            .keys
            .read(&identity_name(&host.id, true))?
            .ok_or(HostError::PendingMissing)?;
        decode_identity(&sealed, &host.id, &host.host_key)?;
        self.keys.seal(&identity_name(&host.id, false), &sealed)?;
        let id = host.id.clone();
        self.save_host(host, false)?;
        self.clear_pending(&id)
    }

    /// `pairRemote`: pairs with the PC an invitation names, as `name`, with `private_key` as this device's key for it
    /// ([`new_private_key`]). `now_ms` is the clock, for the invitation's expiry. A pairing that fails before the
    /// confirmation was sent leaves nothing behind; one that fails after it keeps the pending identity.
    pub async fn pair(
        &self,
        input: &str,
        name: &str,
        now_ms: i64,
        private_key: &[u8],
    ) -> Result<RemoteHost, HostError> {
        let code = parse_pairing_code(input, now_ms)
            .map_err(|error| HostError::BadCode(error.to_string()))?;
        RemoteName::new(name).map_err(|_| HostError::BadName)?;
        if self
            .store
            .hosts()
            .iter()
            .any(|host| host.id == code.host_id && host.host_key != code.host_key)
        {
            return Err(HostError::KeyChanged);
        }
        let psk = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(code.secret.as_str())
                .map_err(|_| HostError::BadName)?,
        );
        let mut host = RemoteHost {
            id: code.host_id.clone(),
            name: code.host_name.clone(),
            device_id: None,
            host_key: code.host_key.clone(),
            lan: code
                .lan
                .iter()
                .map(|lan| protocol::remote::LanEndpoint {
                    host: lan.host.clone(),
                    port: lan.port,
                })
                .collect(),
            relay: code.relay.clone(),
        };
        // Sealed before the confirmation goes out: if the acknowledgement is lost after the PC commits, a reconnect can
        // recover with this key without reusing the consumed invitation.
        let envelope = encode_identity(&host.id, &host.host_key, private_key)?;
        self.keys.seal(&identity_name(&host.id, true), &envelope)?;
        let confirmation_sent = AtomicBool::new(false);
        let request = Request {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key,
            psk: Some(&psk),
            name,
            on_confirm_sent: &|| confirmation_sent.store(true, Ordering::Relaxed),
        };
        let paired = match self.dialer.authenticate(&host, &request).await {
            Ok((peer, info)) => {
                drop(peer); // pairing opens no client
                host.device_id = Some(info.device_id);
                host.relay = info.relay.and_then(|relay| RelayUrl::new(relay).ok());
                self.promote(host.clone())
            }
            Err(error) => Err(HostError::Dial(error)),
        };
        match paired {
            Ok(()) => Ok(host),
            Err(error) => {
                if confirmation_sent.load(Ordering::Relaxed) {
                    // Recovery may resume a committed pairing whose acknowledgement was lost.
                    self.save_host(host, true)?;
                } else {
                    self.clear_pending(&host.id)?;
                }
                Err(error)
            }
        }
    }

    /// `openRemoteClient`: connects to a PC with the saved identity and then, if there is one, the pending identity. The
    /// PC refusing one identity outweighs the other one not getting through. What the PC says about itself (its device id
    /// for us, its name, the relay it now uses) is saved, and a pending identity that worked becomes the identity.
    pub async fn open(&self, id: &str) -> Result<D::Client, HostError> {
        let paired = self.store.hosts().into_iter().find(|host| host.id == id);
        let pending = self.store.pending().into_iter().find(|host| host.id == id);
        let mut failure = HostError::NotPaired;
        for (index, host) in [paired, pending].into_iter().enumerate() {
            let Some(host) = host else { continue };
            let Some(sealed) = self.keys.read(&identity_name(id, index == 1))? else {
                continue;
            };
            match self.connect_with(host, &sealed, index == 1).await {
                Ok(client) => return Ok(client),
                Err(error) => {
                    if !matches!(failure, HostError::Dial(DialError::Refused(_))) {
                        failure = error;
                    }
                }
            }
        }
        Err(failure)
    }

    async fn connect_with(
        &self,
        mut host: RemoteHost,
        sealed: &[u8],
        pending: bool,
    ) -> Result<D::Client, HostError> {
        let key = decode_identity(sealed, &host.id, &host.host_key)?;
        let request = Request {
            preface: RemotePreface::resume(&host.id),
            private_key: &key,
            psk: None,
            name: RESUME_NAME,
            on_confirm_sent: &|| {},
        };
        let (peer, info) = self
            .dialer
            .authenticate(&host, &request)
            .await
            .map_err(HostError::Dial)?;
        host.device_id = Some(info.device_id);
        if let Ok(name) = RemoteName::new(&info.host_name) {
            host.name = name;
        }
        host.relay = info.relay.and_then(|relay| RelayUrl::new(relay).ok());
        if pending {
            self.promote(host)?;
        } else {
            self.save_host(host, false)?;
        }
        self.dialer.open(peer).await.map_err(HostError::Dial)
    }

    /// `forgetRemoteHost`
    pub fn forget(&self, id: &str) -> Result<(), HostError> {
        self.keys.delete(&identity_name(id, false))?;
        self.clear_pending(id)?;
        let mut hosts = self.store.hosts();
        hosts.retain(|host| host.id != id);
        self.store.set_hosts(hosts);
        Ok(())
    }
}

/// Opens wires. [`Wires`] is the real one; a test can point the relay at a local server.
pub trait OpenWire: Send + Sync {
    fn open(
        &self,
        endpoint: &Endpoint,
        timeout: Duration,
    ) -> impl Future<Output = Result<Wire, WireError>> + Send;
}

impl OpenWire for Wires {
    fn open(
        &self,
        endpoint: &Endpoint,
        timeout: Duration,
    ) -> impl Future<Output = Result<Wire, WireError>> + Send {
        Wires::open(self, endpoint, timeout)
    }
}

/// The real network: every LAN address is dialed at once; the first to finish the handshake wins and the others are
/// dropped. The relay joins once the LAN window passes, or sooner when every LAN address has failed. A refusal is the PC's
/// own authenticated answer, so it ends the race. All within [`RACE_DEADLINE`].
pub struct NetworkDialer<O: OpenWire = Wires> {
    opener: O,
    lan_window: Duration,
    deadline: Duration,
}

impl NetworkDialer {
    pub fn new() -> Self {
        Self::with(Wires::new(), LAN_WINDOW, RACE_DEADLINE)
    }
}

impl Default for NetworkDialer {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: OpenWire> NetworkDialer<O> {
    pub fn with(opener: O, lan_window: Duration, deadline: Duration) -> Self {
        Self {
            opener,
            lan_window,
            deadline,
        }
    }
}

enum Leg {
    Refused(RefusedReason),
    /// The relay closed with 4404 before or while we waited on it: the PC isn't connected to it.
    Offline,
    Failed,
}

type Authenticated = (Peer, PeerInfo);

impl<O: OpenWire> NetworkDialer<O> {
    async fn leg(
        &self,
        endpoint: Endpoint,
        timeout: Duration,
        host_key: &[u8],
        request: &Request<'_>,
    ) -> Result<Authenticated, Leg> {
        let wire = self
            .opener
            .open(&endpoint, timeout)
            .await
            .map_err(|_| Leg::Failed)?;
        let closed = wire.closed();
        let auth = Auth {
            preface: request.preface.clone(),
            private_key: request.private_key,
            host_key,
            psk: request.psk,
            name: request.name,
        };
        match authenticate(wire, auth, || (request.on_confirm_sent)()).await {
            Ok(peer) => {
                let info = PeerInfo {
                    device_id: peer.device_id.clone(),
                    host_id: peer.host_id.clone(),
                    host_name: peer.host_name.clone(),
                    relay: peer.relay.clone(),
                };
                Ok((peer, info))
            }
            Err(AuthError::Refused(reason)) => Err(Leg::Refused(reason)),
            Err(AuthError::Failed) => {
                // The close code the relay sent, even when it sent it before the handshake got going.
                let code = tokio::time::timeout(Duration::from_millis(50), closed).await;
                Err(if matches!(code, Ok(Some(4404))) {
                    Leg::Offline
                } else {
                    Leg::Failed
                })
            }
        }
    }
}

impl<O: OpenWire> Dialer for NetworkDialer<O> {
    type Peer = Peer;
    type Client = Client;

    async fn authenticate(
        &self,
        host: &RemoteHost,
        request: &Request<'_>,
    ) -> Result<Authenticated, DialError> {
        let host_key = URL_SAFE_NO_PAD
            .decode(host.host_key.as_str())
            .map_err(|_| DialError::Failed("The PC's key is invalid.".into()))?;
        let relay_endpoint = host.relay.as_ref().map(|relay| Endpoint::Relay {
            url: relay.as_str().to_owned(),
            host_id: host.id.clone(),
        });
        let started = tokio::time::Instant::now();
        let deadline = tokio::time::sleep(self.deadline);
        let window = tokio::time::sleep(self.lan_window);
        tokio::pin!(deadline, window);
        let mut legs = FuturesUnordered::new();
        for lan in &host.lan {
            let endpoint = Endpoint::Lan {
                host: lan.host.clone(),
                port: lan.port,
            };
            legs.push(self.leg(endpoint, self.lan_window, &host_key, request));
        }
        let mut relay_pending = relay_endpoint;
        let mut offline = false;
        let unreachable = |offline: bool| {
            if offline {
                DialError::HostOffline
            } else {
                DialError::Unreachable {
                    relay: host.relay.is_some(),
                }
            }
        };
        loop {
            tokio::select! {
                result = legs.next(), if !legs.is_empty() => match result {
                    Some(Ok(found)) => return Ok(found),
                    Some(Err(Leg::Refused(reason))) => return Err(DialError::Refused(reason)),
                    Some(Err(Leg::Offline)) => offline = true,
                    Some(Err(Leg::Failed)) | None => {}
                },
                // The LAN window passed: the relay joins, with whatever is left of the deadline.
                () = &mut window, if relay_pending.is_some() => {}
                () = &mut deadline => return Err(DialError::Unreachable { relay: host.relay.is_some() }),
            }
            // The window passed, or every LAN address has failed (don't wait out the window then).
            if window.is_elapsed() || legs.is_empty() {
                match relay_pending.take() {
                    Some(endpoint) => {
                        let left = self
                            .deadline
                            .saturating_sub(started.elapsed())
                            .max(Duration::from_millis(1));
                        legs.push(self.leg(endpoint, left, &host_key, request));
                    }
                    None if legs.is_empty() => return Err(unreachable(offline)),
                    None => {}
                }
            }
        }
    }

    async fn open(&self, peer: Peer) -> Result<Client, DialError> {
        connect_peer(peer, ConnectOptions::default())
            .await
            .map(|(client, _)| client)
            .map_err(|error| DialError::Failed(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::path::Path;
    use std::sync::Mutex;

    use futures_util::SinkExt;
    use protocol::remote::{LanEndpoint, PairingCode, PrefaceMode, encode_pairing_code};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    use super::*;

    const ID: &str = "ee87d56c-f1ba-453f-8210-d0f3536e5f53";
    const DEVICE_ID: &str = "0d6ff97e-a44e-42f4-86a0-27ace431918d";
    const RELAY: &str = "https://relay.example.workers.dev";
    const NOW: i64 = 1_790_000_000_000;

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

    /// The PC, as the TypeScript test's mocked `authenticateRemote` plays it, keyed by the first byte of the device key.
    #[derive(Default)]
    struct Fake {
        accept: Mutex<HashSet<u8>>,
        refuse: Mutex<HashSet<u8>>,
        after_confirm: AtomicBool,
        relay: Mutex<Option<String>>,
    }

    impl Dialer for Fake {
        type Peer = ();
        type Client = ();

        async fn authenticate(
            &self,
            _: &RemoteHost,
            request: &Request<'_>,
        ) -> Result<((), PeerInfo), DialError> {
            let key = request.private_key.first().copied().unwrap_or(0);
            if request.preface.mode == PrefaceMode::Pair {
                if self.after_confirm.load(Ordering::Relaxed) {
                    (request.on_confirm_sent)();
                    self.accept.lock().unwrap().insert(key);
                }
                return Err(DialError::Failed("Synthetic lost connection".into()));
            }
            if self.refuse.lock().unwrap().contains(&key) {
                return Err(DialError::Refused(RefusedReason::Revoked));
            }
            if !self.accept.lock().unwrap().contains(&key) {
                return Err(DialError::Failed("Synthetic unknown key".into()));
            }
            Ok((
                (),
                PeerInfo {
                    device_id: DEVICE_ID.into(),
                    host_id: ID.into(),
                    host_name: "Synthetic PC".into(),
                    relay: self.relay.lock().unwrap().clone(),
                },
            ))
        }

        async fn open(&self, _: ()) -> Result<(), DialError> {
            Ok(())
        }
    }

    fn host_key() -> RemoteKey {
        RemoteKey::new(format!("AQ{}", "A".repeat(41))).unwrap()
    }

    fn host() -> RemoteHost {
        RemoteHost {
            id: ID.into(),
            name: RemoteName::new("Synthetic PC").unwrap(),
            device_id: Some("322bccbc-6ba0-409e-8d7f-b736bb7ce3e3".into()),
            host_key: host_key(),
            lan: vec![LanEndpoint {
                host: "192.168.1.2".into(),
                port: 47101,
            }],
            relay: None,
        }
    }

    fn invite() -> String {
        encode_pairing_code(&PairingCode {
            v: 2,
            relay: None,
            pairing_id: "8074dc6f-2eb2-4e53-84d5-6263dfe4c0a2".into(),
            host_id: ID.into(),
            host_name: RemoteName::new("Synthetic PC").unwrap(),
            host_key: host_key(),
            lan: vec![LanEndpoint {
                host: "192.168.1.2".into(),
                port: 47101,
            }],
            secret: RemoteKey::new(format!("Ag{}", "A".repeat(41))).unwrap(),
            expires: jiff::Timestamp::from_millisecond(NOW + 600_000)
                .unwrap()
                .to_string(),
        })
        .unwrap()
    }

    struct World {
        store: Store,
        keys: DeviceKeys<Creds>,
        dialer: Fake,
    }

    fn world() -> World {
        World {
            store: Store::default(),
            keys: DeviceKeys::with_store(Path::new("C:/isolated/tjommie-hosts"), Creds::default())
                .unwrap(),
            dialer: Fake::default(),
        }
    }

    impl World {
        fn hosts(&self) -> Hosts<'_, Store, Creds, Fake> {
            Hosts {
                store: &self.store,
                keys: &self.keys,
                dialer: &self.dialer,
            }
        }
        fn seal(&self, pending: bool, byte: u8) -> Vec<u8> {
            let envelope = encode_identity(ID, &host_key(), &[byte; 32]).unwrap();
            self.keys
                .seal(&identity_name(ID, pending), &envelope)
                .unwrap();
            envelope.to_vec()
        }
        fn sealed(&self, pending: bool) -> Option<Vec<u8>> {
            self.keys
                .read(&identity_name(ID, pending))
                .unwrap()
                .map(|bytes| bytes.to_vec())
        }
        fn stored_entries(&self) -> usize {
            self.keys.store().0.lock().unwrap().len()
        }
    }

    #[tokio::test]
    async fn preserves_a_working_paired_identity_when_a_new_pairing_fails_before_confirmation() {
        let w = world();
        let original = w.seal(false, 9);
        w.store.set_hosts(vec![host()]);
        assert!(
            w.hosts()
                .pair(&invite(), "Phone", NOW, &[20; 32])
                .await
                .is_err()
        );
        assert_eq!(w.sealed(false), Some(original));
        assert_eq!(w.sealed(true), None);
        assert_eq!(w.hosts().all(), vec![host()]);
    }

    #[tokio::test]
    async fn leaves_no_unusable_saved_pc_when_a_first_pairing_fails_before_confirmation() {
        let w = world();
        assert!(
            w.hosts()
                .pair(&invite(), "Phone", NOW, &[20; 32])
                .await
                .is_err()
        );
        assert_eq!(w.hosts().all(), vec![]);
        assert_eq!(w.stored_entries(), 0);
    }

    #[tokio::test]
    async fn recovers_a_lost_acknowledgement_with_the_protected_pending_identity_and_promotes_only_after_success()
     {
        let w = world();
        w.dialer.after_confirm.store(true, Ordering::Relaxed);
        w.seal(false, 9);
        w.store.set_hosts(vec![host()]);
        assert!(
            w.hosts()
                .pair(&invite(), "Phone", NOW, &[20; 32])
                .await
                .is_err()
        );
        assert_eq!(w.hosts().all(), vec![host()]);
        assert!(w.sealed(true).is_some(), "the pending identity is kept");
        w.hosts().open(ID).await.unwrap();
        assert_eq!(w.sealed(true), None);
        let saved = w.sealed(false).unwrap();
        assert_eq!(
            decode_identity(&saved, ID, &host_key()).unwrap().first(),
            Some(&20)
        );
        assert_eq!(
            w.hosts()
                .all()
                .first()
                .and_then(|host| host.device_id.as_deref()),
            Some(DEVICE_ID)
        );
    }

    #[tokio::test]
    async fn keeps_the_pcs_refusal_of_the_saved_identity_when_the_pending_one_cant_connect_and_still_recovers_with_one_that_works()
     {
        let w = world();
        w.dialer.refuse.lock().unwrap().insert(9); // the saved identity was revoked
        w.seal(false, 9);
        w.seal(true, 20);
        w.store.set_hosts(vec![host()]);
        w.store.set_pending(vec![host()]);
        // The pending identity failed in the ordinary way; the PC's refusal of the other outweighs it.
        assert_eq!(
            w.hosts().open(ID).await,
            Err(HostError::Dial(DialError::Refused(RefusedReason::Revoked)))
        );

        w.dialer.accept.lock().unwrap().insert(20); // the pending pairing did land on the PC
        w.hosts().open(ID).await.unwrap();
        assert_eq!(w.sealed(true), None);
        let saved = w.sealed(false).unwrap();
        assert_eq!(
            decode_identity(&saved, ID, &host_key()).unwrap().first(),
            Some(&20)
        );
    }

    #[test]
    fn reads_a_pc_saved_before_the_relay_existed_as_having_no_relay() {
        let before = serde_json::json!({
            "id": ID, "name": "Synthetic PC", "hostKey": host_key().as_str(),
            "lan": [{ "host": "192.168.1.2", "port": 47101 }], "deviceId": "322bccbc-6ba0-409e-8d7f-b736bb7ce3e3"
        });
        let read: RemoteHost = serde_json::from_value(before).unwrap();
        assert_eq!(read.relay, None);
    }

    #[tokio::test]
    async fn keeps_the_relay_the_pc_announces_in_its_ready_message_so_the_next_connection_can_use_it()
     {
        let w = world();
        *w.dialer.relay.lock().unwrap() = Some(RELAY.into());
        w.dialer.accept.lock().unwrap().insert(9);
        w.seal(false, 9);
        w.store.set_hosts(vec![host()]);
        w.hosts().open(ID).await.unwrap();
        assert_eq!(
            w.hosts()
                .all()
                .first()
                .and_then(|host| host.relay.as_ref().map(RelayUrl::as_str)),
            Some(RELAY)
        );
        *w.dialer.relay.lock().unwrap() = None; // the PC turned its relay off
        w.hosts().open(ID).await.unwrap();
        assert_eq!(
            w.hosts().all().first().map(|host| host.relay.clone()),
            Some(None)
        );
    }

    #[tokio::test]
    async fn refuses_a_pairing_under_another_key_before_sealing_anything() {
        let w = world();
        let mut other = host();
        other.host_key = RemoteKey::new(format!("AgAA{}", "A".repeat(39))).unwrap();
        w.store.set_hosts(vec![other]);
        assert_eq!(
            w.hosts().pair(&invite(), "Phone", NOW, &[20; 32]).await,
            Err(HostError::KeyChanged)
        );
        assert_eq!(w.stored_entries(), 0);
        // A bad name, and an expired invitation, are refused before anything is touched too.
        let w = world();
        assert_eq!(
            w.hosts().pair(&invite(), "bad\nname", NOW, &[20; 32]).await,
            Err(HostError::BadName)
        );
        assert!(matches!(
            w.hosts()
                .pair(&invite(), "Phone", NOW + 700_000, &[20; 32])
                .await,
            Err(HostError::BadCode(_))
        ));
        assert_eq!(w.stored_entries(), 0);
    }

    #[tokio::test]
    async fn keeps_at_most_sixteen_pcs_and_replaces_one_it_already_has() {
        let w = world();
        let many: Vec<RemoteHost> = (0..16)
            .map(|n| RemoteHost {
                id: format!("5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c{n:02x}"),
                ..host()
            })
            .collect();
        w.store.set_hosts(many.clone());
        let extra = RemoteHost {
            id: ID.into(),
            ..host()
        };
        assert_eq!(w.hosts().save_host(extra, false), Err(HostError::TooMany));
        let again = RemoteHost {
            name: RemoteName::new("Renamed").unwrap(),
            ..many[3].clone()
        };
        w.hosts().save_host(again.clone(), false).unwrap();
        assert_eq!(w.store.hosts().len(), 16);
        assert_eq!(w.store.hosts().first(), Some(&again));
    }

    #[test]
    fn an_identity_belongs_to_its_pc_and_key() {
        let envelope = encode_identity(ID, &host_key(), &[7; 32]).unwrap();
        assert_eq!(
            decode_identity(&envelope, ID, &host_key())
                .unwrap()
                .as_slice(),
            &[7u8; 32]
        );
        let other_key = RemoteKey::new(format!("AgAA{}", "A".repeat(39))).unwrap();
        assert_eq!(
            decode_identity(&envelope, ID, &other_key),
            Err(HostError::IdentityMismatch)
        );
        assert_eq!(
            decode_identity(
                &envelope,
                "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11",
                &host_key()
            ),
            Err(HostError::IdentityMismatch)
        );
        assert_eq!(
            decode_identity(b"{\"v\":1}", ID, &host_key()),
            Err(HostError::IdentityMismatch)
        );
        assert_eq!(
            encode_identity(ID, &host_key(), &[1; 31]),
            Err(HostError::IdentityMismatch)
        );
    }

    #[tokio::test]
    async fn forgetting_a_pc_removes_both_identities_and_the_pc() {
        let w = world();
        w.seal(false, 9);
        w.seal(true, 20);
        w.store.set_hosts(vec![host()]);
        w.store.set_pending(vec![host()]);
        w.hosts().forget(ID).unwrap();
        assert_eq!(w.hosts().all(), vec![]);
        assert_eq!(w.stored_entries(), 0);
    }

    /// Dials the relay at a local server instead of `wss://`; every LAN address is dead.
    struct LocalRelay(String);

    impl OpenWire for LocalRelay {
        async fn open(&self, endpoint: &Endpoint, timeout: Duration) -> Result<Wire, WireError> {
            match endpoint {
                Endpoint::Relay { .. } => client::remote::web_socket_frames(&self.0, timeout).await,
                _ => Err(WireError::Unavailable),
            }
        }
    }

    #[tokio::test]
    async fn says_the_pc_is_offline_when_the_relay_reports_4404_before_the_handshake_gets_going() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let frame = CloseFrame {
                code: CloseCode::from(4404),
                reason: "".into(),
            };
            let _ = socket.close(Some(frame)).await;
            let _ = socket.flush().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let dialer = NetworkDialer::with(
            LocalRelay(url),
            Duration::from_millis(50),
            Duration::from_secs(10),
        );
        let host = RemoteHost {
            relay: Some(RelayUrl::new(RELAY).unwrap()),
            ..host()
        };
        let request = Request {
            preface: RemotePreface::resume(ID),
            private_key: &[9; 32],
            psk: None,
            name: RESUME_NAME,
            on_confirm_sent: &|| {},
        };
        let error = dialer.authenticate(&host, &request).await.err();
        assert_eq!(error, Some(DialError::HostOffline));
        // With no relay to blame, the same failure says to check the Wi-Fi.
        let lan_only = dialer
            .authenticate(
                &RemoteHost {
                    relay: None,
                    ..host.clone()
                },
                &request,
            )
            .await
            .err();
        assert_eq!(lan_only, Some(DialError::Unreachable { relay: false }));
    }
}
