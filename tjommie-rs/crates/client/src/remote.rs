//! Another PC, over Noise: the wires to it (`desktop/src/remote-wire.ts`, `client/src/frame-socket.ts`), the handshake that
//! proves who both sides are (`client/src/remote-peer.ts`), and the encrypted channel the [`crate::Client`] then runs on
//! (`encrypted-channel.ts`, `transport.ts`). Spec §3.3 has the framing: one Noise frame (1 to 65535 bytes) per LAN TCP
//! message behind a 4-byte length, or per relay WebSocket binary message.

use std::future::Future;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::remote::{
    RefusedReason, RelayUrl, RemoteConfirm, RemoteName, RemotePreface, RemoteReady, RemoteRefused,
    remote_prologue,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use transport::channel::{EncryptedMessages, FrameDecoder, length_prefixed};
use transport::{MAX_NOISE_FRAME, Session};

pub use transport::{Keypair, generate_keypair};

use crate::connection::{ConnectOptions, Incoming, Link};
use crate::error::ConnectError;
use crate::{Client, Endpoint};

/// At most this many open or opening wires of each kind (`MAX_WIRES`).
pub const MAX_WIRES: usize = 8;
/// At most this much written but not yet sent (`MAX_BACKLOG`).
pub const MAX_BACKLOG: usize = 32 * 1024 * 1024;
/// A dial may ask for 1 ms to 60 s.
const LONGEST_DIAL: Duration = Duration::from_secs(60);
/// The PC has this long to answer the handshake (`remote-peer.ts`).
const AUTH_TIMEOUT: Duration = Duration::from_secs(15);
/// Frames waiting for whoever reads the wire; beyond that the sender waits, and TCP pushes back.
const INBOX: usize = 64;

/// The wire can't be opened, or has closed. Says no more than the TypeScript's "The PC connection is unavailable.".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    Unavailable,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("The PC connection is unavailable.")
    }
}

impl std::error::Error for WireError {}

/// What a wire's two tasks and its owner share: the unsent byte count, and why it closed (`Some(code)` once closed, with
/// the relay's close code when there was one).
struct Shared {
    queued: AtomicUsize,
    closed: watch::Sender<Option<Option<u16>>>,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            queued: AtomicUsize::new(0),
            closed: watch::channel(None).0,
        })
    }

    /// Only the first close counts.
    fn close(&self, code: Option<u16>) {
        self.closed.send_if_modified(|state| {
            let first = state.is_none();
            if first {
                *state = Some(code);
            }
            first
        });
    }

    fn is_closed(&self) -> bool {
        self.closed.borrow().is_some()
    }
}

async fn until_closed(mut closed: watch::Receiver<Option<Option<u16>>>) -> Option<u16> {
    loop {
        let state = *closed.borrow_and_update();
        if let Some(code) = state {
            return code;
        }
        if closed.changed().await.is_err() {
            return None;
        }
    }
}

type Counts = Arc<Mutex<[usize; 2]>>;

/// One of the 8 places for a kind of wire; gives it back when dropped.
struct Slot {
    counts: Counts,
    kind: usize,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = counts.get_mut(self.kind) {
            *count = count.saturating_sub(1);
        }
    }
}

/// A connection to another PC that carries Noise frames: send one, wait for one. Dropping it closes it.
pub struct Wire {
    out: mpsc::UnboundedSender<Vec<u8>>,
    input: mpsc::Receiver<Vec<u8>>,
    shared: Arc<Shared>,
    _slot: Option<Slot>,
}

impl Wire {
    /// Queues one frame. A frame of no bytes or more than a Noise frame, more than 32 MiB waiting, or a closed wire closes
    /// the wire and fails.
    pub fn send(&self, frame: &[u8]) -> Result<(), WireError> {
        let cost = frame.len() + 4;
        let queued = self.shared.queued.load(Ordering::Relaxed);
        if self.shared.is_closed()
            || frame.is_empty()
            || frame.len() > MAX_NOISE_FRAME
            || queued + cost > MAX_BACKLOG
        {
            self.shared.close(None);
            return Err(WireError::Unavailable);
        }
        self.shared.queued.fetch_add(cost, Ordering::Relaxed);
        self.out.send(frame.to_vec()).map_err(|_| {
            self.shared.close(None);
            WireError::Unavailable
        })
    }

    /// The next frame, or `None` once the wire has closed and what arrived before has been read.
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        self.input.recv().await
    }

    pub fn close(&self) {
        self.shared.close(None);
    }

    /// Resolves when the wire has closed, with the relay's close code when it sent one (4404: the PC is offline).
    pub fn closed(&self) -> impl Future<Output = Option<u16>> + Send + 'static {
        until_closed(self.shared.closed.subscribe())
    }

    fn start(
        out: mpsc::UnboundedSender<Vec<u8>>,
        input: mpsc::Receiver<Vec<u8>>,
        shared: Arc<Shared>,
    ) -> Wire {
        Wire {
            out,
            input,
            shared,
            _slot: None,
        }
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.shared.close(None);
    }
}

/// The wires this device has open to other PCs, with the TypeScript bridge's limits (`RemoteWires`).
#[derive(Clone, Default)]
pub struct Wires {
    counts: Counts,
}

impl Wires {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dials a PC. `Endpoint::Local` isn't a PC and is refused. A dial that doesn't open within `timeout` (1 ms to 60 s)
    /// fails, and so does one that would be the ninth of its kind.
    pub async fn open(&self, endpoint: &Endpoint, timeout: Duration) -> Result<Wire, WireError> {
        if timeout.is_zero() || timeout > LONGEST_DIAL {
            return Err(WireError::Unavailable);
        }
        let (kind, lan, relay) = match endpoint {
            Endpoint::Lan { host, port } => {
                let address: IpAddr = host.parse().map_err(|_| WireError::Unavailable)?;
                if *port == 0 {
                    return Err(WireError::Unavailable);
                }
                (0, Some((address, *port)), None)
            }
            Endpoint::Relay { url, host_id } if is_uuid(host_id) => {
                (1, None, Some(relay_connect_url(url, host_id)?))
            }
            Endpoint::Relay { .. } => return Err(WireError::Unavailable),
            Endpoint::Local { .. } => return Err(WireError::Unavailable),
        };
        let slot = {
            let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
            let count = counts.get_mut(kind).ok_or(WireError::Unavailable)?;
            if *count >= MAX_WIRES {
                return Err(WireError::Unavailable);
            }
            *count += 1;
            Slot {
                counts: self.counts.clone(),
                kind,
            }
        };
        let mut wire = match (lan, relay) {
            (Some((address, port)), _) => tcp_frames(address, port, timeout).await?,
            (_, Some(url)) => web_socket_frames(&url, timeout).await?,
            _ => return Err(WireError::Unavailable),
        };
        wire._slot = Some(slot);
        Ok(wire)
    }
}

async fn tcp_frames(address: IpAddr, port: u16, timeout: Duration) -> Result<Wire, WireError> {
    let stream = tokio::time::timeout(timeout, TcpStream::connect((address, port)))
        .await
        .map_err(|_| WireError::Unavailable)?
        .map_err(|_| WireError::Unavailable)?;
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();
    let (out, mut queued) = mpsc::unbounded_channel::<Vec<u8>>();
    let (inbox, input) = mpsc::channel::<Vec<u8>>(INBOX);
    let shared = Shared::new();

    let read_shared = shared.clone();
    tokio::spawn(async move {
        let closed = read_shared.closed.subscribe();
        let mut decoder = FrameDecoder::default();
        let mut buffer = vec![0u8; 16 * 1024];
        let reading = async {
            loop {
                let Ok(count @ 1..) = reader.read(&mut buffer).await else {
                    return;
                };
                let Some(chunk) = buffer.get(..count) else {
                    return;
                };
                let Ok(frames) = decoder.feed(chunk) else {
                    return;
                };
                for frame in frames {
                    if inbox.send(frame).await.is_err() {
                        return;
                    }
                }
            }
        };
        tokio::select! {
            () = reading => {}
            _ = until_closed(closed) => {}
        }
        read_shared.close(None);
    });

    let write_shared = shared.clone();
    tokio::spawn(async move {
        let closed = write_shared.closed.subscribe();
        let writing = async {
            while let Some(frame) = queued.recv().await {
                let Ok(bytes) = length_prefixed(&frame) else {
                    return;
                };
                if writer.write_all(&bytes).await.is_err() {
                    return;
                }
                write_shared
                    .queued
                    .fetch_sub(frame.len() + 4, Ordering::Relaxed);
            }
        };
        tokio::select! {
            () = writing => {}
            _ = until_closed(closed) => {}
        }
        write_shared.close(None);
        let _ = writer.shutdown().await;
    });

    Ok(Wire::start(out, input, shared))
}

/// A WebSocket to a relay (`webSocketFrames`): every message is one binary Noise frame; anything else closes the wire.
pub async fn web_socket_frames(url: &str, timeout: Duration) -> Result<Wire, WireError> {
    // A Noise frame is at most MAX_NOISE_FRAME bytes, so a relay has no reason to send more in one message: stop reading at
    // that, where tungstenite's defaults would buffer up to 64 MiB of a hostile relay's message first.
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_NOISE_FRAME))
        .max_frame_size(Some(MAX_NOISE_FRAME));
    let (socket, _) = tokio::time::timeout(
        timeout,
        tokio_tungstenite::connect_async_with_config(url, Some(config), false),
    )
    .await
    .map_err(|_| WireError::Unavailable)?
    .map_err(|_| WireError::Unavailable)?;
    let (mut sink, mut stream) = socket.split();
    let (out, mut queued) = mpsc::unbounded_channel::<Vec<u8>>();
    let (inbox, input) = mpsc::channel::<Vec<u8>>(INBOX);
    let shared = Shared::new();

    let read_shared = shared.clone();
    tokio::spawn(async move {
        let closed = read_shared.closed.subscribe();
        let reading = async {
            loop {
                match stream.next().await {
                    Some(Ok(Message::Binary(frame))) => {
                        if frame.is_empty() || frame.len() > MAX_NOISE_FRAME {
                            return None;
                        }
                        if inbox.send(frame.to_vec()).await.is_err() {
                            return None;
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        return frame.map(|frame| u16::from(frame.code));
                    }
                    _ => return None,
                }
            }
        };
        let code = tokio::select! {
            code = reading => code,
            _ = until_closed(closed) => None,
        };
        read_shared.close(code);
    });

    let write_shared = shared.clone();
    tokio::spawn(async move {
        let closed = write_shared.closed.subscribe();
        let writing = async {
            while let Some(frame) = queued.recv().await {
                let cost = frame.len() + 4;
                if sink.send(Message::binary(frame)).await.is_err() {
                    return;
                }
                write_shared.queued.fetch_sub(cost, Ordering::Relaxed);
            }
        };
        tokio::select! {
            () = writing => {}
            _ = until_closed(closed) => {}
        }
        write_shared.close(None);
        let _ = sink.close().await;
    });

    Ok(Wire::start(out, input, shared))
}

/// `relayConnectUrl`: the relay's wss URL for connecting to one PC.
pub fn relay_connect_url(base: &str, host_id: &str) -> Result<String, WireError> {
    let valid = RelayUrl::new(base).map_err(|_| WireError::Unavailable)?;
    let mut url = url::Url::parse(valid.as_str()).map_err(|_| WireError::Unavailable)?;
    url.set_scheme("wss").map_err(|()| WireError::Unavailable)?;
    url.path_segments_mut()
        .map_err(|()| WireError::Unavailable)?
        .clear()
        .extend(["v1", "hosts", host_id, "connect"]);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

/// A host id is a UUID.
pub(crate) fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(at, byte)| match at {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// Why [`authenticate`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// The PC answered, over the authenticated channel, that this device isn't paired with it.
    Refused(RefusedReason),
    /// The PC could not be authenticated, or the connection closed. Check the pairing code and that the PC is reachable.
    Failed,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Refused(_) => "This device is not paired with this PC.",
            Self::Failed => {
                "The PC could not be authenticated. Check the pairing code and that the PC is reachable."
            }
        })
    }
}

impl std::error::Error for AuthError {}

/// What a device brings to the PC: the first message, the keys, and the name it asks to be known by.
pub struct Auth<'a> {
    /// `pair` (with the invitation's id) or `resume`.
    pub preface: RemotePreface,
    /// This device's private key for this PC (32 bytes).
    pub private_key: &'a [u8],
    /// The PC's public key, known from the pairing code.
    pub host_key: &'a [u8],
    /// The pairing secret, when pairing.
    pub psk: Option<&'a [u8]>,
    pub name: &'a str,
}

/// An authenticated, confirmed channel to a PC, before the hello. Connect the client to it with [`connect_peer`].
pub struct Peer {
    link: PeerLink,
    pub device_id: String,
    pub host_id: String,
    pub host_name: String,
    /// The relay the PC says to use next time.
    pub relay: Option<String>,
}

/// `authenticateRemote`: sends the preface and the Noise handshake, then the first encrypted message, which proves
/// possession of the pairing secret. Neither side treats the handshake alone as permission to make requests. On failure the
/// wire is closed. `on_confirm_sent` runs once the confirmation has been sent: from then on the PC may have committed the
/// pairing, whether or not its answer arrives.
pub async fn authenticate(
    wire: Wire,
    auth: Auth<'_>,
    on_confirm_sent: impl FnOnce(),
) -> Result<Peer, AuthError> {
    tokio::time::timeout(AUTH_TIMEOUT, handshake(wire, auth, on_confirm_sent))
        .await
        .unwrap_or(Err(AuthError::Failed))
}

fn failed<E>(_: E) -> AuthError {
    AuthError::Failed
}

async fn handshake(
    mut wire: Wire,
    auth: Auth<'_>,
    on_confirm_sent: impl FnOnce(),
) -> Result<Peer, AuthError> {
    let prologue = remote_prologue(&auth.preface).map_err(failed)?;
    let mut noise =
        Session::initiator(auth.private_key, auth.host_key, auth.psk, &prologue).map_err(failed)?;
    wire.send(&serde_json::to_vec(&auth.preface).map_err(failed)?)
        .map_err(failed)?;
    wire.send(&noise.write_handshake(b"").map_err(failed)?)
        .map_err(failed)?;
    let frame = wire.next().await.ok_or(AuthError::Failed)?;
    if !noise.read_handshake(&frame).map_err(failed)?.is_empty() || !noise.handshake_finished() {
        return Err(AuthError::Failed);
    }
    let mut messages = EncryptedMessages::new(noise);
    let confirm = RemoteConfirm {
        protocol: 1,
        name: RemoteName::new(auth.name).map_err(failed)?,
    };
    for frame in messages
        .encode(&serde_json::to_string(&confirm).map_err(failed)?)
        .map_err(failed)?
    {
        wire.send(&frame).map_err(failed)?;
    }
    on_confirm_sent();
    loop {
        let frame = wire.next().await.ok_or(AuthError::Failed)?;
        let Some(text) = messages.receive(&frame).map_err(failed)? else {
            continue;
        };
        let response: Value = serde_json::from_str(&text).map_err(failed)?;
        if response.get("type").and_then(Value::as_str) == Some("refused") {
            let refused: RemoteRefused = protocol::parse(response).map_err(failed)?;
            return Err(AuthError::Refused(refused.reason));
        }
        let ready: RemoteReady = protocol::parse(response).map_err(failed)?;
        if ready.host_id != auth.preface.host_id {
            return Err(AuthError::Failed);
        }
        return Ok(Peer {
            link: PeerLink { wire, messages },
            device_id: ready.device_id,
            host_id: ready.host_id,
            host_name: ready.host_name.as_str().to_owned(),
            relay: ready.relay.map(|relay| relay.as_str().to_owned()),
        });
    }
}

/// The encrypted channel to a PC as the client's connection: text messages over Noise records.
pub(crate) struct PeerLink {
    wire: Wire,
    messages: EncryptedMessages<Session>,
}

impl Link for PeerLink {
    async fn send(&mut self, text: String) -> Result<(), String> {
        let sent = self
            .messages
            .encode(&text)
            .map_err(|error| error.to_string())
            .and_then(|frames| {
                frames
                    .iter()
                    .try_for_each(|frame| self.wire.send(frame).map_err(|error| error.to_string()))
            });
        if sent.is_err() {
            self.messages.close();
            self.wire.close();
        }
        sent.map_err(|_| "The PC connection is closed.".to_owned())
    }

    async fn next(&mut self) -> Incoming {
        loop {
            let Some(frame) = self.wire.next().await else {
                return Incoming::Close(Some((
                    1006,
                    "The PC connection closed. Check Wi-Fi and whether this device is still paired."
                        .to_owned(),
                )));
            };
            match self.messages.receive(&frame) {
                Ok(Some(text)) => return Incoming::Text(text),
                Ok(None) => {}
                Err(error) => {
                    self.wire.close();
                    return Incoming::Failed(error.to_string());
                }
            }
        }
    }

    async fn hang_up(&mut self, _code: u16, _reason: &str) {
        self.messages.close();
        self.wire.close();
    }

    async fn finish(&mut self) {}
}

/// The PC an authenticated channel reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub device_id: String,
    pub host_id: String,
    pub host_name: String,
    pub relay: Option<String>,
}

/// `connectTransport`: runs the client on an authenticated channel. The hello carries the fixed label `device`, which holds
/// no credential; the encrypted channel is what authenticates.
pub async fn connect_peer(
    peer: Peer,
    options: ConnectOptions,
) -> Result<(Client, PeerInfo), ConnectError> {
    let info = PeerInfo {
        device_id: peer.device_id,
        host_id: peer.host_id,
        host_name: peer.host_name,
        relay: peer.relay,
    };
    Client::over(peer.link, "device", options)
        .await
        .map(|client| (client, info))
}
