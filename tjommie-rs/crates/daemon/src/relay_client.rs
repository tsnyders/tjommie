//! `relay-client.ts`: this PC's control connection to the relay. It answers the login challenge with the relay identity,
//! opens a channel leg for each device the relay announces, sends content-free pushes, pings every 30 s, and
//! reconnects with backoff, or at once when the wall clock jumps (the PC slept).
//!
//! One task per `start()` owns the connection; `stop()` ends it. Sockets come from a [`Connector`], so tests use fakes.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::future::BoxFuture;
use protocol::relay::{
    HostToRelay, LEGACY_PUSH_KINDS, PushResult, RELAY, RELAY_CLOSE, RelayToHost, login_message,
};
use protocol::remote::{PushKind, RelayState, RelayUrl, RemoteKey};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::peer_wire::{PeerWire, Socket};
use crate::relay_identity::RelayIdentity;

const RETRY_MS: [u64; 5] = [1_000, 2_000, 5_000, 10_000, 30_000];
const TICK: Duration = Duration::from_secs(5);
const MAX_DRIFT_MS: i64 = 15_000;

/// Opens a WebSocket to a relay URL.
pub type Connector = Arc<dyn Fn(String) -> BoxFuture<'static, io::Result<Socket>> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayError {
    Unavailable,
    Invalid,
}

impl fmt::Display for RelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            RelayError::Unavailable => "The relay connection is unavailable.",
            RelayError::Invalid => "Invalid relay push request.",
        })
    }
}

impl std::error::Error for RelayError {}

/// Opens real WebSockets (`wss:` with the Windows TLS stack).
pub fn default_connector() -> Connector {
    Arc::new(|url: String| Box::pin(async move { Socket::connect(&url).await }))
}

pub struct RelayClientOptions {
    /// The relay base URL from settings, read at every connect.
    pub url: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub host_id: String,
    pub identity: Arc<RelayIdentity>,
    /// A device's channel leg, ready for the remote host.
    pub on_channel: Arc<dyn Fn(PeerWire) + Send + Sync>,
    pub on_state: Arc<dyn Fn() + Send + Sync>,
    pub connect: Connector,
    /// Wall-clock milliseconds. Only drift is read from it; every timer runs on tokio time.
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl RelayClientOptions {
    pub fn new(
        url: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        host_id: String,
        identity: Arc<RelayIdentity>,
        on_channel: Arc<dyn Fn(PeerWire) + Send + Sync>,
        on_state: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            url,
            host_id,
            identity,
            on_channel,
            on_state,
            connect: default_connector(),
            clock: Arc::new(|| jiff::Timestamp::now().as_millisecond()),
        }
    }
}

/// Where a push's per-token results go.
type Reply = oneshot::Sender<Result<Vec<PushResult>, RelayError>>;

struct PushRequest {
    tokens: Vec<String>,
    kind: PushKind,
    bot_id: String,
    room_id: Option<String>,
    reply: Reply,
}

struct Running {
    stop: CancellationToken,
    pushes: mpsc::UnboundedSender<PushRequest>,
}

struct Shared {
    state: RelayState,
    push: bool,
    kinds: Vec<PushKind>,
}

struct Inner {
    options: RelayClientOptions,
    shared: Mutex<Shared>,
    states: watch::Sender<RelayState>,
    running: Mutex<Option<Running>>,
}

#[derive(Clone)]
pub struct RelayClient(Arc<Inner>);

/// How a control connection ended.
enum End {
    Stopped,
    /// The PC slept: log in again at once.
    Reconnect,
    /// No relay is configured.
    Idle,
    Failed(Option<u16>),
}

/// The `wss:` URL for a relay path: the base's host, nothing else from it.
fn socket_url(base: &url::Url, path: &str) -> String {
    let mut url = base.clone();
    let _ = url.set_scheme("wss");
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// `new URL(base).host`: the host, and the port when it isn't the default.
fn login_host(base: &url::Url) -> String {
    let host = base.host_str().unwrap_or_default();
    match base.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

impl RelayClient {
    pub fn new(options: RelayClientOptions) -> RelayClient {
        RelayClient(Arc::new(Inner {
            options,
            shared: Mutex::new(Shared {
                state: RelayState::Off,
                push: false,
                kinds: LEGACY_PUSH_KINDS.to_vec(),
            }),
            states: watch::Sender::new(RelayState::Off),
            running: Mutex::default(),
        }))
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.0.shared.lock().unwrap()
    }

    pub fn state(&self) -> RelayState {
        self.shared().state
    }

    pub fn push_enabled(&self) -> bool {
        let shared = self.shared();
        shared.state == RelayState::Online && shared.push
    }

    pub fn kinds(&self) -> Vec<PushKind> {
        self.shared().kinds.clone()
    }

    fn active(&self) -> bool {
        self.0.running.lock().unwrap().is_some()
    }

    pub async fn wait_until_online(&self) -> Result<(), RelayError> {
        let mut states = self.0.states.subscribe();
        if *states.borrow_and_update() == RelayState::Online {
            return Ok(());
        }
        if !self.active() || (self.0.options.url)().is_none() {
            return Err(RelayError::Unavailable);
        }
        let online = async {
            loop {
                if states.changed().await.is_err() {
                    return Err(RelayError::Unavailable);
                }
                match *states.borrow_and_update() {
                    RelayState::Online => return Ok(()),
                    RelayState::Off => return Err(RelayError::Unavailable),
                    _ => {}
                }
            }
        };
        timeout(Duration::from_millis(RELAY.accept_timeout_ms), online)
            .await
            .unwrap_or(Err(RelayError::Unavailable))
    }

    pub fn start(&self) {
        let mut running = self.0.running.lock().unwrap();
        if running.is_some() {
            return;
        }
        let stop = CancellationToken::new();
        let (pushes, requests) = mpsc::unbounded_channel();
        *running = Some(Running {
            stop: stop.clone(),
            pushes,
        });
        tokio::spawn(self.clone().run(stop, requests));
    }

    pub fn stop(&self) {
        let Some(running) = self.0.running.lock().unwrap().take() else {
            self.set(&CancellationToken::new(), RelayState::Off);
            return;
        };
        running.stop.cancel();
        // Synchronous, like the TypeScript client: the task may still be unwinding, but nothing it does counts now.
        self.disconnected(&CancellationToken::new());
        self.set(&CancellationToken::new(), RelayState::Off);
    }

    /// Sends one push and waits for the relay's per-token results.
    pub async fn push(
        &self,
        tokens: Vec<String>,
        kind: PushKind,
        bot_id: &str,
        room_id: Option<&str>,
    ) -> Result<Vec<PushResult>, RelayError> {
        let pushes = match &*self.0.running.lock().unwrap() {
            Some(running) if self.push_enabled() => running.pushes.clone(),
            _ => return Err(RelayError::Unavailable),
        };
        let mut message =
            json!({ "t": "push", "id": 0, "tokens": tokens, "kind": kind, "botId": bot_id });
        if let Some(room_id) = room_id {
            message["roomId"] = room_id.into();
        }
        protocol::parse::<HostToRelay>(message).map_err(|_| RelayError::Invalid)?;
        let (reply, answer) = oneshot::channel();
        let request = PushRequest {
            tokens,
            kind,
            bot_id: bot_id.into(),
            room_id: room_id.map(Into::into),
            reply,
        };
        pushes.send(request).map_err(|_| RelayError::Unavailable)?;
        match timeout(Duration::from_millis(RELAY.accept_timeout_ms), answer).await {
            Ok(Ok(result)) => result,
            _ => Err(RelayError::Unavailable),
        }
    }

    /// Changes the state unless this task was stopped. A panicking callback can't take the relay down with it.
    fn set(&self, stop: &CancellationToken, state: RelayState) {
        {
            let mut shared = self.shared();
            if stop.is_cancelled() || shared.state == state {
                return;
            }
            shared.state = state;
        }
        self.0.states.send_replace(state);
        let _ = catch_unwind(AssertUnwindSafe(|| (self.0.options.on_state)()));
    }

    fn disconnected(&self, stop: &CancellationToken) {
        let mut shared = self.shared();
        if !stop.is_cancelled() {
            shared.push = false;
            shared.kinds = LEGACY_PUSH_KINDS.to_vec();
        }
    }

    /// `#tick`'s first check: true when the wall clock jumped since the last tick.
    fn drifted(&self, last_tick: &mut i64) -> bool {
        let now = (self.0.options.clock)();
        let drift = now - *last_tick;
        *last_tick = now;
        !(0..=MAX_DRIFT_MS).contains(&drift)
    }

    async fn run(self, stop: CancellationToken, mut pushes: mpsc::UnboundedReceiver<PushRequest>) {
        let mut ticks = interval_at(Instant::now() + TICK, TICK);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_tick = (self.0.options.clock)();
        let mut attempt = 0usize;
        let mut next_id = 0i64;
        loop {
            let end = self
                .session(
                    &stop,
                    &mut pushes,
                    &mut ticks,
                    &mut last_tick,
                    &mut attempt,
                    &mut next_id,
                )
                .await;
            self.disconnected(&stop);
            let wait_until = match end {
                End::Stopped => return,
                End::Reconnect => continue,
                End::Idle => None,
                End::Failed(_) if (self.0.options.url)().is_none() => {
                    self.set(&stop, RelayState::Off);
                    None
                }
                End::Failed(code) => {
                    self.set(&stop, RelayState::Error);
                    if code == Some(RELAY_CLOSE.bad_login) {
                        attempt = RETRY_MS.len() - 1;
                    }
                    let delay = RETRY_MS[attempt.min(RETRY_MS.len() - 1)];
                    attempt += 1;
                    Some(Instant::now() + Duration::from_millis(delay))
                }
            };
            // Wait for the retry (or, with no relay, for a stop), keeping the drift check and refusing pushes.
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return,
                    () = sleep_until(wait_until.unwrap_or_else(Instant::now)), if wait_until.is_some() => break,
                    request = pushes.recv() => {
                        if let Some(request) = request {
                            let _ = request.reply.send(Err(RelayError::Unavailable));
                        }
                    }
                    _ = ticks.tick() => if self.drifted(&mut last_tick) { break },
                }
            }
        }
    }

    async fn session(
        &self,
        stop: &CancellationToken,
        pushes: &mut mpsc::UnboundedReceiver<PushRequest>,
        ticks: &mut tokio::time::Interval,
        last_tick: &mut i64,
        attempt: &mut usize,
        next_id: &mut i64,
    ) -> End {
        let options = &self.0.options;
        let Some(base) = (options.url)() else {
            self.set(stop, RelayState::Off);
            return End::Idle;
        };
        self.set(stop, RelayState::Connecting);
        let Ok(relay) = RelayUrl::new(&base) else {
            return End::Failed(None);
        };
        let Ok(base) = url::Url::parse(relay.as_str()) else {
            return End::Failed(None);
        };
        let host_id = &options.host_id;
        // Ends with this session: closes every channel leg it opened.
        let session = stop.child_token();
        let _closing = session.clone().drop_guard();
        let login_deadline = Instant::now() + Duration::from_millis(RELAY.accept_timeout_ms);
        let connecting =
            (options.connect)(socket_url(&base, &format!("/v1/hosts/{host_id}/control")));
        tokio::pin!(connecting);
        let mut socket = loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => return End::Stopped,
                () = sleep_until(login_deadline) => return End::Failed(None),
                connected = &mut connecting => match connected {
                    Ok(socket) => break socket,
                    Err(_) => return End::Failed(None),
                },
                request = pushes.recv() => {
                    if let Some(request) = request {
                        let _ = request.reply.send(Err(RelayError::Unavailable));
                    }
                }
                _ = ticks.tick() => if self.drifted(last_tick) { return End::Reconnect },
            }
        };
        let end = self
            .control(
                stop,
                &session,
                &base,
                &mut socket,
                login_deadline,
                pushes,
                ticks,
                last_tick,
                attempt,
                next_id,
            )
            .await;
        socket.close();
        end
    }

    #[allow(clippy::too_many_arguments)]
    async fn control(
        &self,
        stop: &CancellationToken,
        session: &CancellationToken,
        base: &url::Url,
        socket: &mut Socket,
        login_deadline: Instant,
        pushes: &mut mpsc::UnboundedReceiver<PushRequest>,
        ticks: &mut tokio::time::Interval,
        last_tick: &mut i64,
        attempt: &mut usize,
        next_id: &mut i64,
    ) -> End {
        let (mut challenged, mut authenticated) = (false, false);
        let mut pending: HashMap<i64, (usize, Reply)> = HashMap::new();
        let mut channels: Vec<CancellationToken> = Vec::new();
        let (mut last_ping, mut last_pong) = (Instant::now(), Instant::now());
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => return End::Stopped,
                () = sleep_until(login_deadline), if !authenticated => return End::Failed(None),
                message = socket.recv() => {
                    let text = match message {
                        None => return End::Failed(None),
                        Some(Message::Close(frame)) => return End::Failed(frame.map(|frame| u16::from(frame.code))),
                        Some(Message::Text(text)) => text.to_string(),
                        Some(Message::Ping(_) | Message::Pong(_)) => continue,
                        Some(_) => return End::Failed(None),
                    };
                    if text == "pong" {
                        last_pong = Instant::now();
                        continue;
                    }
                    let parsed = serde_json::from_str::<Value>(&text).ok().and_then(|value| protocol::parse::<RelayToHost>(value).ok());
                    let Some(message) = parsed else { return End::Failed(None) };
                    match message {
                        RelayToHost::Challenge { nonce } => {
                            if challenged || authenticated {
                                return End::Failed(None);
                            }
                            challenged = true;
                            let identity = &self.0.options.identity;
                            let signed = identity.sign(&login_message(&self.0.options.host_id, nonce.as_str(), &login_host(base)));
                            let Ok(public_key) = RemoteKey::new(URL_SAFE_NO_PAD.encode(identity.public_key)) else {
                                return End::Failed(None);
                            };
                            let login = HostToRelay::Login { public_key, signature: URL_SAFE_NO_PAD.encode(signed) };
                            let text = serde_json::to_string(&login).expect("a login serializes");
                            if socket.send(Message::text(text)).is_err() {
                                return End::Failed(None);
                            }
                        }
                        RelayToHost::Ready { push, kinds } => {
                            if !challenged || authenticated {
                                return End::Failed(None);
                            }
                            authenticated = true;
                            *attempt = 0;
                            (last_ping, last_pong) = (Instant::now(), Instant::now());
                            {
                                let mut shared = self.shared();
                                if !stop.is_cancelled() {
                                    shared.push = push;
                                    shared.kinds = kinds.map_or_else(
                                        || LEGACY_PUSH_KINDS.to_vec(),
                                        |kinds| kinds.into_iter().filter_map(|kind| serde_json::from_value(Value::String(kind)).ok()).collect(),
                                    );
                                }
                            }
                            self.set(stop, RelayState::Online);
                        }
                        RelayToHost::Open { channel } => {
                            if !authenticated {
                                return End::Failed(None);
                            }
                            self.open_channel(session, base, channel.as_str(), &mut channels);
                        }
                        RelayToHost::Pushed { id, results } => {
                            if !authenticated {
                                return End::Failed(None);
                            }
                            if let Some((count, reply)) = pending.remove(&id) {
                                let _ = reply.send(if results.len() == count { Ok(results) } else { Err(RelayError::Unavailable) });
                            }
                        }
                    }
                }
                request = pushes.recv() => {
                    let Some(request) = request else { continue };
                    if !authenticated {
                        let _ = request.reply.send(Err(RelayError::Unavailable));
                        continue;
                    }
                    *next_id += 1;
                    let count = request.tokens.len();
                    let message = HostToRelay::Push {
                        id: *next_id,
                        tokens: request.tokens,
                        kind: request.kind,
                        bot_id: request.bot_id,
                        room_id: request.room_id,
                    };
                    let text = serde_json::to_string(&message).expect("a push serializes");
                    if socket.send(Message::text(text)).is_err() {
                        let _ = request.reply.send(Err(RelayError::Unavailable));
                        return End::Failed(None);
                    }
                    pending.insert(*next_id, (count, request.reply));
                }
                _ = ticks.tick() => {
                    if self.drifted(last_tick) {
                        return End::Reconnect;
                    }
                    if !authenticated {
                        continue;
                    }
                    let now = Instant::now();
                    if now.duration_since(last_pong) > Duration::from_millis(RELAY.host_dead_ms) {
                        return End::Failed(None);
                    }
                    // Callers that gave up waiting no longer need their entry.
                    pending.retain(|_, (_, reply)| !reply.is_closed());
                    if now.duration_since(last_ping) < Duration::from_millis(RELAY.host_ping_ms) {
                        continue;
                    }
                    if socket.send(Message::text("ping")).is_err() {
                        return End::Failed(None);
                    }
                    last_ping = now;
                }
            }
        }
    }

    /// Opens the device side of a channel. The relay expires a channel nobody accepts, so failures stay silent.
    fn open_channel(
        &self,
        session: &CancellationToken,
        base: &url::Url,
        channel: &str,
        channels: &mut Vec<CancellationToken>,
    ) {
        channels.retain(|slot| !slot.is_cancelled());
        if channels.len() >= RELAY.max_channels as usize {
            return;
        }
        let slot = session.child_token();
        channels.push(slot.clone());
        let host_id = &self.0.options.host_id;
        let connecting = (self.0.options.connect)(socket_url(
            base,
            &format!("/v1/hosts/{host_id}/channels/{channel}"),
        ));
        let on_channel = self.0.options.on_channel.clone();
        tokio::spawn(async move {
            let opened = tokio::select! {
                () = slot.cancelled() => return,
                opened = timeout(Duration::from_millis(RELAY.accept_timeout_ms), connecting) => opened,
            };
            let Ok(Ok(socket)) = opened else {
                slot.cancel();
                return;
            };
            let closer = socket.closer();
            if slot.is_cancelled() {
                closer.cancel();
                return;
            }
            let _ = catch_unwind(AssertUnwindSafe(|| on_channel(PeerWire::websocket(socket))));
            tokio::select! {
                () = slot.cancelled() => closer.cancel(),
                () = closer.cancelled() => slot.cancel(),
            }
        });
    }
}
