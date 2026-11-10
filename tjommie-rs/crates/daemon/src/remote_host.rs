//! `remote-host.ts`: the PC side of phone and paired-PC connections, over LAN TCP or relay channels. A device sends a
//! preface, completes a Noise IK handshake (IKpsk2 with the pairing secret when pairing), proves itself with an
//! encrypted confirm, then says hello and makes requests like a local client. Its requests run with a `Device`
//! principal; `remote.ping` and `device.registerPush` are answered here.
//!
//! Each connection is one task that owns its Noise session. Other tasks reach it through its outbox, which carries
//! JSON text to encrypt and send in order.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::future::BoxFuture;
use protocol::remote::{
    Device, LanEndpoint, PairingCode, PairingStart, PrefaceMode, PushKind, RefusedReason,
    RelayState, RelayStatus, RelayUrl, RemoteConfirm, RemoteKey, RemoteName, RemotePreface,
    RemoteReady, RemoteRefused, RemoteStatus, encode_pairing_code, remote_prologue,
};
use protocol::{
    ClientMessage, DecodeError, ErrorCode, Event, PROTOCOL_VERSION, Request, Response,
    ServerMessage,
};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;
use transport::channel::EncryptedMessages;
use transport::{Session, generate_keypair};
use zeroize::Zeroizing;

use crate::devices_store::{
    self, PushTarget, add_device, device_for_key, get_device, iso, list_devices, new_id, now_iso,
    revoke_device, set_push_token, touch_device,
};
use crate::peer_wire::{PeerWire, WireKind, WireWriter};
use crate::remote_identity::{
    CredentialManager, HostIdentity, IdentityError, Vault, host_name, key_text,
    load_remote_identity,
};
use crate::server::{Principal, RequestContext, RpcError};

/// The LAN port phones connect to (`remote-host.ts:24`). The daemon takes it from `Launch::lan_port`.
pub const REMOTE_PORT: u16 = 47101;
const MAX_ACTIVE: usize = 32;
const MAX_PENDING: usize = 8;
const MAX_PENDING_PER_ADDRESS: usize = 4;
const MAX_ATTEMPTS_PER_MINUTE: u32 = 20;
const MAX_TRACKED_ADDRESSES: usize = 256;
const MAX_PREFACE: usize = 2048;
const MAX_IN_FLIGHT: usize = 32;
const MAX_BUFFERED: usize = 32 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(150);
const SWEEP_EVERY: Duration = Duration::from_secs(30);
const ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
/// How long a pairing code stays valid.
pub const INVITE_LIFETIME: Duration = Duration::from_secs(600);
/// Gives an encrypted refusal time to leave before the wire is destroyed.
const REFUSAL_GRACE: Duration = Duration::from_secs(1);
const UNAVAILABLE: &str = "This request could not be completed on the PC.";

pub type Db = Arc<Mutex<Connection>>;
/// Runs a device's request through the daemon's handlers, like `Server::dispatch`.
pub type Dispatch = Arc<
    dyn Fn(String, Value, RequestContext) -> BoxFuture<'static, Result<Value, RpcError>>
        + Send
        + Sync,
>;

pub struct RemoteHostOptions {
    pub db: Db,
    /// The data root; it picks the Credential Manager namespace of the host key.
    pub root: PathBuf,
    pub dispatch: Dispatch,
    /// Devices connected, disconnected or changed.
    pub on_changed: Arc<dyn Fn() + Send + Sync>,
    pub vault: Arc<dyn Vault>,
    pub port: u16,
    /// Production listens on every interface, `0.0.0.0:47101`, so phones on the network reach the PC; that is the
    /// default here. Tests and manual runs on a shared PC set `127.0.0.1`, which keeps Windows Firewall from prompting.
    pub bind: IpAddr,
    /// The addresses written into pairing codes. `None` asks Windows for this PC's IPv4 addresses.
    pub addresses: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// The relay base URL, for a pairing code (`true`) or for status and ready (`false`).
    pub relay_url: Arc<dyn Fn(bool) -> Option<String> + Send + Sync>,
}

impl RemoteHostOptions {
    pub fn new(db: Db, root: PathBuf, dispatch: Dispatch) -> Self {
        Self {
            db,
            root,
            dispatch,
            on_changed: Arc::new(|| {}),
            vault: Arc::new(CredentialManager),
            port: REMOTE_PORT,
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            addresses: None,
            relay_url: Arc::new(|_| None),
        }
    }
}

#[derive(Debug)]
pub enum StartError {
    Closed,
    Identity(IdentityError),
    Listen(io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartError::Closed => formatter.write_str("The phone connection is closed."),
            StartError::Identity(error) => error.fmt(formatter),
            StartError::Listen(_) => {
                formatter.write_str("The phone connection could not start. Retry from Devices.")
            }
        }
    }
}

impl std::error::Error for StartError {}

impl From<StartError> for RpcError {
    fn from(error: StartError) -> Self {
        let code = match error {
            StartError::Closed => ErrorCode::BadState,
            _ => ErrorCode::Internal,
        };
        RpcError::new(code, error.to_string())
    }
}

struct Invite {
    secret: Zeroizing<[u8; 32]>,
    expires: Instant,
}

struct Attempt {
    at: Instant,
    count: u32,
}

struct Peer {
    kind: WireKind,
    address: String,
    writer: WireWriter,
    device_id: Mutex<Option<String>>,
    welcomed: AtomicBool,
    last_frame: Mutex<Instant>,
    outbox: mpsc::UnboundedSender<String>,
}

impl Peer {
    fn close(&self) {
        self.writer.destroy();
    }

    fn device(&self) -> Option<String> {
        self.device_id.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct State {
    identity: Option<Arc<HostIdentity>>,
    port: Option<u16>,
    listener: Option<JoinHandle<()>>,
    sweeping: bool,
    invites: HashMap<String, Invite>,
    peers: Vec<Arc<Peer>>,
    attempts: HashMap<String, Attempt>,
}

struct Inner {
    options: RemoteHostOptions,
    state: Mutex<State>,
    opening: tokio::sync::Mutex<()>,
    closed: CancellationToken,
}

#[derive(Clone)]
pub struct RemoteHost(Arc<Inner>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preface,
    Handshake,
    Confirm,
    Refused,
    Hello,
    Rpc,
}

/// One connection's protocol state, owned by its task.
struct Link {
    phase: Phase,
    preface: Option<RemotePreface>,
    noise: Option<Session>,
    messages: Option<EncryptedMessages<Session>>,
    context: Option<RequestContext>,
    in_flight: Arc<Mutex<HashSet<i64>>>,
}

enum Step {
    Continue,
    Refused,
}

/// Any protocol violation: the connection closes without saying why.
struct Violation;

impl<E: std::error::Error> From<E> for Violation {
    fn from(_: E) -> Self {
        Violation
    }
}

fn json_text(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("protocol frames serialize")
}

fn is_revoked(db: &Connection, device_id: &str) -> bool {
    get_device(db, device_id).map_or(true, |device| device.revoked_at.is_some())
}

impl RemoteHost {
    pub fn new(options: RemoteHostOptions) -> RemoteHost {
        // A pairing secret exists only in the process that created it, so a restart cancels outstanding codes.
        let _ = options
            .db
            .lock()
            .unwrap()
            .execute("DELETE FROM pairing_codes WHERE used_at IS NULL", []);
        RemoteHost(Arc::new(Inner {
            options,
            state: Mutex::default(),
            opening: tokio::sync::Mutex::new(()),
            closed: CancellationToken::new(),
        }))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap()
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.0.options.db.lock().unwrap()
    }

    fn relay_url(&self, for_pairing: bool) -> Option<RelayUrl> {
        (self.0.options.relay_url)(for_pairing).and_then(|url| RelayUrl::new(url).ok())
    }

    pub fn status(&self) -> RemoteStatus {
        let state = self.state();
        let name = state
            .identity
            .as_ref()
            .map_or_else(host_name, |identity| identity.name.clone());
        let mut connected: Vec<String> = Vec::new();
        for id in state.peers.iter().filter_map(|peer| peer.device()) {
            if !connected.contains(&id) {
                connected.push(id);
            }
        }
        RemoteStatus {
            host_id: state.identity.as_ref().map(|identity| identity.id.clone()),
            host_name: RemoteName::new(&name)
                .unwrap_or_else(|_| RemoteName::new("This PC").expect("a valid name")),
            listening: state.port.is_some(),
            port: state.port,
            connected_device_ids: connected,
            relay: RelayStatus {
                url: self.relay_url(false),
                state: RelayState::Off,
                push: false,
            },
        }
    }

    /// `start`: loads the host key, starts the sweep, then listens. A failed listen leaves the key and sweep in place,
    /// so relay devices still connect.
    pub async fn ensure_listening(&self) -> Result<(), StartError> {
        if self.0.closed.is_cancelled() {
            return Err(StartError::Closed);
        }
        let _opening = self.0.opening.lock().await;
        if self.0.closed.is_cancelled() {
            return Err(StartError::Closed);
        }
        if self.state().port.is_some() {
            return Ok(());
        }
        if self.state().identity.is_none() {
            let options = &self.0.options;
            let identity = load_remote_identity(
                &self.db(),
                &options.root,
                &host_name(),
                options.vault.as_ref(),
                generate_keypair,
            )
            .map_err(StartError::Identity)?;
            self.state().identity = Some(Arc::new(identity));
        }
        self.start_sweep();
        let options = &self.0.options;
        let listener = TcpListener::bind((options.bind, options.port))
            .await
            .map_err(StartError::Listen)?;
        let port = listener.local_addr().map_err(StartError::Listen)?.port();
        if self.0.closed.is_cancelled() {
            return Err(StartError::Closed);
        }
        let host = self.clone();
        let accepting = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    () = host.0.closed.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                match accepted {
                    Ok((stream, _)) => host.accept_wire(PeerWire::tcp(stream)),
                    // A failed accept is reported by the next explicit pairing attempt; don't spin on it.
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
        });
        let mut state = self.state();
        // `close()` cancels before it takes this lock, so a close is either seen here or clears the state after us.
        if self.0.closed.is_cancelled() {
            drop(state);
            accepting.abort();
            return Err(StartError::Closed);
        }
        state.port = Some(port);
        state.listener = Some(accepting);
        Ok(())
    }

    fn start_sweep(&self) {
        let mut state = self.state();
        if state.sweeping {
            return;
        }
        state.sweeping = true;
        let host = self.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval_at(Instant::now() + SWEEP_EVERY, SWEEP_EVERY);
            loop {
                tokio::select! {
                    () = host.0.closed.cancelled() => break,
                    _ = ticks.tick() => host.expire(),
                }
            }
        });
    }

    pub async fn create_pairing(&self) -> Result<PairingStart, RpcError> {
        self.ensure_listening().await?;
        let unavailable =
            || RpcError::new(ErrorCode::BadState, "The phone connection is unavailable.");
        let (identity, port) = {
            let state = self.state();
            match (&state.identity, state.port) {
                (Some(identity), Some(port)) if !self.0.closed.is_cancelled() => {
                    (identity.clone(), port)
                }
                _ => return Err(unavailable()),
            }
        };
        let addresses = match &self.0.options.addresses {
            Some(addresses) => addresses(),
            None => lan_addresses(),
        };
        if addresses.is_empty() {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "Connect this PC to Wi-Fi or Ethernet before pairing a phone.",
            ));
        }
        let previous: Vec<String> = self.state().invites.keys().cloned().collect();
        for id in previous {
            self.cancel_pairing(&id);
        }
        let id = new_id();
        let mut secret = Zeroizing::new([0u8; 32]);
        getrandom::fill(secret.as_mut()).map_err(|_| unavailable())?;
        let expires = iso(jiff::Timestamp::now() + INVITE_LIFETIME);
        self.db()
            .execute(
                "INSERT INTO pairing_codes(id,secret_hash,expires_at,created_at) VALUES(?,?,?,?)",
                [
                    &id,
                    &format!("{:x}", Sha256::digest(secret.as_ref())),
                    &expires,
                    &now_iso(),
                ],
            )
            .map_err(devices_store::database)?;
        let secret_text = Zeroizing::new(key_text(secret.as_ref()));
        self.state().invites.insert(
            id.clone(),
            Invite {
                secret,
                expires: Instant::now() + INVITE_LIFETIME,
            },
        );
        let mut hosts: Vec<String> = Vec::new();
        for host in addresses {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        let invalid = |_| {
            RpcError::new(
                ErrorCode::Internal,
                "The pairing code could not be created.",
            )
        };
        let code = encode_pairing_code(&PairingCode {
            v: 2,
            relay: self.relay_url(true),
            pairing_id: id.clone(),
            host_id: identity.id.clone(),
            host_name: RemoteName::new(&identity.name).map_err(invalid)?,
            host_key: RemoteKey::new(key_text(&identity.public_key)).map_err(invalid)?,
            secret: RemoteKey::new(secret_text.as_str()).map_err(invalid)?,
            lan: hosts
                .into_iter()
                .take(16)
                .map(|host| LanEndpoint { host, port })
                .collect(),
            expires: expires.clone(),
        })
        .map_err(invalid)?;
        Ok(PairingStart {
            id,
            uri: format!("tjommie://pair#{code}"),
            code,
            expires,
        })
    }

    pub fn cancel_pairing(&self, id: &str) {
        // Dropping the invite wipes its secret.
        self.state().invites.remove(id);
        let _ = self.db().execute(
            "DELETE FROM pairing_codes WHERE id = ? AND used_at IS NULL",
            [id],
        );
    }

    fn expire(&self) {
        let now = Instant::now();
        let expired: Vec<String> = self
            .state()
            .invites
            .iter()
            .filter(|(_, invite)| invite.expires <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.cancel_pairing(&id);
        }
        let mut state = self.state();
        state
            .attempts
            .retain(|_, attempt| now.duration_since(attempt.at) <= ATTEMPT_WINDOW);
        for peer in &state.peers {
            if now.duration_since(*peer.last_frame.lock().unwrap()) >= IDLE_TIMEOUT {
                peer.close();
            }
        }
    }

    pub fn push_targets(&self) -> Vec<PushTarget> {
        devices_store::push_targets(&self.db()).unwrap_or_default()
    }

    pub fn clear_push_token(&self, device_id: &str, token: &str) -> Result<(), RpcError> {
        devices_store::clear_push_token(&self.db(), device_id, token)
    }

    pub fn revoke(&self, id: &str) -> Result<Device, RpcError> {
        let device = revoke_device(&self.db(), id)?;
        for peer in &self.state().peers {
            if peer.device().as_deref() == Some(id) {
                peer.close();
            }
        }
        (self.0.options.on_changed)();
        Ok(device)
    }

    /// Sends an event to every welcomed device. A device revoked meanwhile is disconnected instead.
    pub fn broadcast(&self, event: &Event) {
        let text = json_text(&ServerMessage::Event(event.clone()));
        let peers = self.state().peers.clone();
        for peer in peers {
            let Some(device_id) = peer.device() else {
                continue;
            };
            if !peer.welcomed.load(Ordering::Acquire) {
                continue;
            }
            if is_revoked(&self.db(), &device_id) || peer.outbox.send(text.clone()).is_err() {
                peer.close();
            }
        }
    }

    pub fn has_devices(&self) -> bool {
        list_devices(&self.db()).is_ok_and(|devices| !devices.is_empty())
    }

    /// Takes a new connection from the LAN listener or the relay.
    pub fn accept_wire(&self, wire: PeerWire) {
        self.expire();
        let now = Instant::now();
        let mut state = self.state();
        let (kind, address) = (wire.kind, wire.address.clone());
        let mut attempts = 0;
        if kind == WireKind::Lan {
            let tracked = state.attempts.len();
            let previous = state.attempts.get(&address);
            let at = previous.map_or(now, |attempt| attempt.at);
            attempts = previous.map_or(0, |attempt| attempt.count) + 1;
            if tracked < MAX_TRACKED_ADDRESSES || previous.is_some() {
                state.attempts.insert(
                    address.clone(),
                    Attempt {
                        at,
                        count: attempts,
                    },
                );
            }
        }
        let pending: Vec<Arc<Peer>> = state
            .peers
            .iter()
            .filter(|peer| peer.device().is_none() && peer.kind == kind)
            .cloned()
            .collect();
        let active = state
            .peers
            .iter()
            .filter(|peer| peer.device().is_some())
            .count();
        let crowded = kind == WireKind::Lan
            && (pending
                .iter()
                .filter(|peer| peer.address == address)
                .count()
                >= MAX_PENDING_PER_ADDRESS
                || attempts > MAX_ATTEMPTS_PER_MINUTE);
        let Some(identity) = state.identity.clone() else {
            wire.writer().destroy();
            return;
        };
        if self.0.closed.is_cancelled() || active >= MAX_ACTIVE || crowded {
            wire.writer().destroy();
            return;
        }
        // Rotate only within this wire kind, so relay attempts cannot evict LAN handshakes.
        if pending.len() >= MAX_PENDING {
            pending[0].close();
            state.peers.retain(|peer| !Arc::ptr_eq(peer, &pending[0]));
        }
        let (outbox, queued) = mpsc::unbounded_channel();
        let peer = Arc::new(Peer {
            kind,
            address,
            writer: wire.writer(),
            device_id: Mutex::default(),
            welcomed: AtomicBool::new(false),
            last_frame: Mutex::new(now),
            outbox,
        });
        state.peers.push(peer.clone());
        drop(state);
        tokio::spawn(self.clone().run(peer, wire, queued, identity));
    }

    async fn run(
        self,
        peer: Arc<Peer>,
        mut wire: PeerWire,
        mut queued: mpsc::UnboundedReceiver<String>,
        identity: Arc<HostIdentity>,
    ) {
        let mut link = Link {
            phase: Phase::Preface,
            preface: None,
            noise: None,
            messages: None,
            context: None,
            in_flight: Arc::default(),
        };
        let handshake_deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut refusal_deadline: Option<Instant> = None;
        loop {
            let welcomed = peer.welcomed.load(Ordering::Acquire);
            tokio::select! {
                biased;
                () = peer.writer.closed() => break,
                () = sleep_until(handshake_deadline), if !welcomed => break,
                () = sleep_until(refusal_deadline.unwrap_or(handshake_deadline)), if refusal_deadline.is_some() => break,
                text = queued.recv() => {
                    let Some(text) = text else { break };
                    if self.send(&peer, &mut link, &text).is_err() {
                        break;
                    }
                }
                frame = wire.next_frame() => {
                    let Some(frame) = frame else { break };
                    *peer.last_frame.lock().unwrap() = Instant::now();
                    if link.phase == Phase::Refused {
                        continue;
                    }
                    match self.receive(&peer, &mut link, &identity, &frame) {
                        Ok(Step::Continue) => {}
                        Ok(Step::Refused) => {
                            link.phase = Phase::Refused;
                            refusal_deadline = Some(Instant::now() + REFUSAL_GRACE);
                        }
                        Err(Violation) => break,
                    }
                }
            }
        }
        peer.close();
        if let Some(messages) = link.messages.as_mut() {
            messages.close();
        }
        self.state()
            .peers
            .retain(|entry| !Arc::ptr_eq(entry, &peer));
        if peer.welcomed.load(Ordering::Acquire) && !self.0.closed.is_cancelled() {
            if let Some(device_id) = peer.device() {
                // Revoked devices stay revoked.
                let _ = touch_device(&self.db(), &device_id, None);
            }
            (self.0.options.on_changed)();
        }
    }

    /// Writes one raw frame, refusing a peer that has stopped reading.
    fn write(peer: &Peer, frame: Vec<u8>) -> Result<(), Violation> {
        if peer.writer.buffered() + frame.len() > MAX_BUFFERED {
            return Err(Violation);
        }
        peer.writer.write(frame).map_err(|_| Violation)
    }

    fn send(&self, peer: &Peer, link: &mut Link, text: &str) -> Result<(), Violation> {
        let messages = link.messages.as_mut().ok_or(Violation)?;
        for frame in messages.encode(text)? {
            Self::write(peer, frame)?;
        }
        Ok(())
    }

    fn receive(
        &self,
        peer: &Arc<Peer>,
        link: &mut Link,
        identity: &HostIdentity,
        frame: &[u8],
    ) -> Result<Step, Violation> {
        match link.phase {
            Phase::Preface => {
                if frame.len() > MAX_PREFACE {
                    return Err(Violation);
                }
                let preface: RemotePreface = protocol::parse(serde_json::from_slice(frame)?)?;
                if preface.host_id != identity.id {
                    return Err(Violation);
                }
                let psk = match (&preface.mode, &preface.pairing_id) {
                    (PrefaceMode::Pair, Some(pairing_id)) => {
                        let state = self.state();
                        let invite = state.invites.get(pairing_id).ok_or(Violation)?;
                        if invite.expires <= Instant::now() {
                            return Err(Violation);
                        }
                        Some(invite.secret.clone())
                    }
                    (PrefaceMode::Pair, None) => return Err(Violation),
                    (PrefaceMode::Resume, _) => None,
                };
                let prologue = remote_prologue(&preface)?;
                link.noise = Some(Session::responder(
                    &identity.private_key[..],
                    psk.as_ref().map(|secret| &secret[..]),
                    &prologue,
                )?);
                link.preface = Some(preface);
                link.phase = Phase::Handshake;
                Ok(Step::Continue)
            }
            Phase::Handshake => {
                let mut noise = link.noise.take().ok_or(Violation)?;
                if !noise.read_handshake(frame)?.is_empty() {
                    return Err(Violation);
                }
                Self::write(peer, noise.write_handshake(b"")?)?;
                if !noise.handshake_finished() {
                    return Err(Violation);
                }
                link.messages = Some(EncryptedMessages::new(noise));
                link.phase = Phase::Confirm;
                Ok(Step::Continue)
            }
            _ => {
                let messages = link.messages.as_mut().ok_or(Violation)?;
                let Some(text) = messages.receive(frame)? else {
                    return Ok(Step::Continue);
                };
                if link.phase == Phase::Confirm {
                    return self.confirm(peer, link, identity, &text);
                }
                let device_id = peer.device().ok_or(Violation)?;
                if is_revoked(&self.db(), &device_id) {
                    return Err(Violation);
                }
                let request: ClientMessage = protocol::parse(serde_json::from_str(&text)?)?;
                match (link.phase, request) {
                    (Phase::Hello, ClientMessage::Hello { protocol, token })
                        if protocol == i64::from(PROTOCOL_VERSION) && token == "device" =>
                    {
                        self.send(
                            peer,
                            link,
                            &json_text(&ServerMessage::Welcome {
                                protocol: PROTOCOL_VERSION,
                            }),
                        )?;
                        link.phase = Phase::Rpc;
                        peer.welcomed.store(true, Ordering::Release);
                        (self.0.options.on_changed)();
                        Ok(Step::Continue)
                    }
                    (Phase::Rpc, ClientMessage::Req { id, method, params }) => {
                        {
                            let mut in_flight = link.in_flight.lock().unwrap();
                            if in_flight.len() >= MAX_IN_FLIGHT || !in_flight.insert(id) {
                                return Err(Violation);
                            }
                        }
                        let context = link.context.clone().ok_or(Violation)?;
                        tokio::spawn(self.clone().request(
                            peer.clone(),
                            device_id,
                            context,
                            link.in_flight.clone(),
                            id,
                            method,
                            params,
                        ));
                        Ok(Step::Continue)
                    }
                    _ => Err(Violation),
                }
            }
        }
    }

    /// The first encrypted message: proof of the pairing secret, or a known device key.
    fn confirm(
        &self,
        peer: &Arc<Peer>,
        link: &mut Link,
        identity: &HostIdentity,
        text: &str,
    ) -> Result<Step, Violation> {
        let confirm: RemoteConfirm = protocol::parse(serde_json::from_str(text)?)?;
        let preface = link.preface.as_ref().ok_or(Violation)?;
        let remote = link
            .messages
            .as_ref()
            .and_then(|messages| messages.cipher().remote_public_key())
            .ok_or(Violation)?;
        let device = match (&preface.mode, &preface.pairing_id) {
            (PrefaceMode::Pair, Some(pairing_id)) => {
                if self
                    .state()
                    .invites
                    .get(pairing_id)
                    .is_none_or(|invite| invite.expires <= Instant::now())
                {
                    return Err(Violation);
                }
                let device = {
                    let db = self.db();
                    let transaction = db.unchecked_transaction()?;
                    let now = now_iso();
                    let used = transaction.execute(
                        "UPDATE pairing_codes SET used_at = ? WHERE id = ? AND used_at IS NULL AND expires_at > ?",
                        [&now, pairing_id, &now],
                    )?;
                    if used != 1 {
                        return Err(Violation);
                    }
                    let device =
                        add_device(&transaction, &key_text(&remote), confirm.name.as_str())?;
                    transaction.commit()?;
                    device
                };
                self.state().invites.remove(pairing_id);
                device
            }
            (PrefaceMode::Pair, None) => return Err(Violation),
            (PrefaceMode::Resume, _) => {
                let known = device_for_key(&self.db(), &key_text(&remote))?;
                match known {
                    Some(device) if device.revoked_at.is_none() => device,
                    known => {
                        let reason = if known.is_some() {
                            RefusedReason::Revoked
                        } else {
                            RefusedReason::Unknown
                        };
                        self.send(peer, link, &json_text(&RemoteRefused { reason }))?;
                        return Ok(Step::Refused);
                    }
                }
            }
        };
        let active = self
            .state()
            .peers
            .iter()
            .filter(|entry| entry.device().is_some())
            .count();
        if active >= MAX_ACTIVE {
            return Err(Violation);
        }
        *peer.device_id.lock().unwrap() = Some(device.id.clone());
        touch_device(&self.db(), &device.id, None)?;
        link.context = Some(RequestContext {
            connection_id: new_id(),
            principal: Principal::Device {
                device_id: device.id.clone(),
            },
            signal: peer.writer.signal(),
        });
        let ready = RemoteReady {
            device_id: device.id,
            host_id: identity.id.clone(),
            host_name: RemoteName::new(&identity.name)?,
            relay: self.relay_url(false),
        };
        self.send(peer, link, &json_text(&ready))?;
        link.phase = Phase::Hello;
        Ok(Step::Continue)
    }

    #[allow(clippy::too_many_arguments)]
    async fn request(
        self,
        peer: Arc<Peer>,
        device_id: String,
        context: RequestContext,
        in_flight: Arc<Mutex<HashSet<i64>>>,
        id: i64,
        method: String,
        params: Value,
    ) {
        let usable = || !peer.writer.is_closed() && !is_revoked(&self.db(), &device_id);
        if usable() {
            // `params.parse` as zod runs it. A device learns only that its request failed its schema, never why;
            // an unknown method falls through so the daemon answers `unknown_method` as usual.
            let decoded = match Request::decode(&method, params.clone()) {
                Err(DecodeError::BadParams(_)) => {
                    Err(RpcError::new(ErrorCode::BadParams, UNAVAILABLE))
                }
                Err(DecodeError::UnknownMethod) => Ok(None),
                Ok(request) => Ok(Some(request)),
            };
            let result = match decoded {
                Err(error) => Err(error),
                Ok(Some(Request::RemotePing(_))) => Ok(json!({})),
                Ok(Some(Request::DeviceRegisterPush(registration))) => {
                    // Unknown kinds are dropped here; the device keeps asking for them on newer hosts.
                    let kinds: Option<Vec<PushKind>> = registration.kinds.map(|kinds| {
                        kinds
                            .into_iter()
                            .filter_map(|kind| serde_json::from_value(Value::String(kind)).ok())
                            .collect()
                    });
                    set_push_token(
                        &self.db(),
                        &device_id,
                        registration.token.as_deref(),
                        kinds.as_deref(),
                    )
                    .map(|()| json!({}))
                }
                Ok(_) => (self.0.options.dispatch)(method, params, context).await,
            };
            let response = match result {
                Ok(result) => usable().then(|| Response::ok(id, result)),
                // Internal failures never carry their detail to a device.
                Err(error) if error.code == ErrorCode::Internal => {
                    (!peer.writer.is_closed()).then(|| Response::error(id, error.code, UNAVAILABLE))
                }
                Err(error) => (!peer.writer.is_closed())
                    .then(|| Response::error(id, error.code, error.message)),
            };
            if let Some(response) = response {
                let _ = peer.outbox.send(json_text(&ServerMessage::Res(response)));
            }
        }
        in_flight.lock().unwrap().remove(&id);
    }

    pub async fn close(&self) {
        if self.0.closed.is_cancelled() {
            return;
        }
        self.0.closed.cancel();
        let listener = {
            let mut state = self.state();
            state.invites.clear();
            for peer in &state.peers {
                peer.close();
            }
            state.port = None;
            state.listener.take()
        };
        if let Some(listener) = listener {
            let _ = listener.await;
        }
        let _opening = self.0.opening.lock().await;
        self.state().identity = None;
    }
}

/// This PC's IPv4 addresses on adapters that are up, without loopback: what `os.networkInterfaces()` lists as
/// non-internal IPv4.
pub fn lan_addresses() -> Vec<String> {
    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    let mut size = 16 * 1024u32;
    for _ in 0..4 {
        // u64 elements keep the adapter records 8-byte aligned.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        // SAFETY: the buffer holds `size` bytes, as the call is told; it is read only while `buffer` lives.
        let result = unsafe {
            GetAdaptersAddresses(u32::from(AF_INET.0), flags, None, Some(first), &mut size)
        };
        if result == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if result != NO_ERROR.0 {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut adapter = first.cast_const();
        while !adapter.is_null() {
            // SAFETY: the list and its address lists are linked records inside `buffer`.
            let current = unsafe { &*adapter };
            if current.OperStatus == IfOperStatusUp {
                let mut unicast = current.FirstUnicastAddress.cast_const();
                while !unicast.is_null() {
                    // SAFETY: as above.
                    let address = unsafe { &*unicast };
                    let socket = address.Address.lpSockaddr;
                    // SAFETY: an AF_INET socket address is a SOCKADDR_IN.
                    if !socket.is_null() && unsafe { (*socket).sa_family } == AF_INET {
                        let ipv4 = unsafe { &*socket.cast::<SOCKADDR_IN>() };
                        // S_addr is in network order, so its bytes in memory are the address bytes.
                        let ip = Ipv4Addr::from(unsafe { ipv4.sin_addr.S_un.S_addr }.to_ne_bytes());
                        if !ip.is_loopback() {
                            found.push(ip.to_string());
                        }
                    }
                    unicast = address.Next.cast_const();
                }
            }
            adapter = current.Next.cast_const();
        }
        return found;
    }
    Vec::new()
}
