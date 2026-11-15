//! Port of `packages/ui/lib/connection.ts` (T1.3, the part the first M1 pass deferred): the app's one connection to a daemon,
//! the desktop's own or a saved PC's. It owns the app state (`AppStore`), connects, keeps the link alive and replaces it
//! when it dies, and hands screens a [`Daemon`] whose calls are limited to 16 in flight (the PC's host for phones closes a
//! connection at 32).
//!
//! The TypeScript is module state plus promises and timers. Here it is [`AppConnection`], and what it does not own is behind
//! traits: [`Environment`] (everything the app provides: the saved PCs, opening them, the file host, push, keep-alive, the
//! clock, and where detached work runs) and [`DaemonLink`] (one connection: calls, events, closing).
//!
//! Time is sans-IO, like `browser_preview.rs`: nothing in here sleeps. Every timer (the heartbeat, the retry, the pong
//! deadline, the waits inside `forget_remote` and `reacquire`) is registered here, and the app asks [`next_timer`], waits
//! until then, and calls [`on_timer`] (it learns that the next timer changed from [`timers_changed`]). The clock is
//! [`Environment::now`].
//!
//! Detached work (the TypeScript's `void promise` and un-awaited `.then`) goes to [`Environment::spawn`]: the attempt that
//! connects, loads the lists and then watches the link until it ends, the heartbeat's ping, the catch-up fetches. Whatever
//! holds a flag across an await has a drop guard, so a task that is dropped leaves nothing stuck.
//!
//! Not ported, and why: `desktop()` and its `onConnection`/`onDaemonFailed` subscriptions (the app calls [`on_connection`]
//! and [`on_daemon_failed`]; the browser-outside-Electron `outside` status for a missing bridge doesn't exist here);
//! `document` (the visibility is [`on_visibility`]; the page identity `captureAttachmentSave` compared is a WebView reload,
//! which has no counterpart); `onNetworkChange` (the app calls [`network_changed`]); `invalidateThumbnailWork` (the app's
//! [`Environment::invalidate_files`] also retires its thumbnail queue, `ThumbnailQueue::invalidate`).
//!
//! The rules the app must keep: methods of [`Environment`] and the store's listeners must not call back into the connection
//! (they run while it holds its lock); and the tasks handed to `spawn` must keep running until the app shuts down.
//!
//! [`next_timer`]: AppConnection::next_timer
//! [`on_timer`]: AppConnection::on_timer
//! [`timers_changed`]: AppConnection::timers_changed
//! [`on_connection`]: AppConnection::on_connection
//! [`on_daemon_failed`]: AppConnection::on_daemon_failed
//! [`on_visibility`]: AppConnection::on_visibility
//! [`network_changed`]: AppConnection::network_changed

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use client::{CloseReason, ConnectError, Rpc, RpcError};
use futures::FutureExt as _;
use futures::future::{BoxFuture, Shared};
use protocol::relay::PushData;
use protocol::remote::DeviceRegisterPush;
use protocol::wire::{ChatHistory, Empty, Id, RoomHistory, RoutinesList, method};
use protocol::{ClientMethod, Event};
use tokio::sync::{Notify, Semaphore, broadcast, oneshot, watch};

use super::document_ranges::Signal;
use super::labels::error_text;
use super::setup::failed_setup;
use super::state::{
    AppState, Connection, ConversationStatus, LINK, MessageEvent, OfflineReason, Page, Snapshot,
    add_older, apply_event, catch_up, connected, finish_loading, forget_conversation,
    start_loading,
};
use super::store::{AppStore, SubscriptionId};

/// Messages per history page.
pub const PAGE: u32 = 50;
/// Failed tries before "Starting Tjommie" says why it can't connect.
pub const STARTING_TRIES: u32 = 3;
/// Calls in flight at once; the PC's host for phones closes a connection with 32.
pub const CALLS: usize = 16;
/// How long Forget waits on the PC, then on the phone's push service, to let go of push.
pub const FORGET_WAIT: Duration = Duration::from_secs(3);
/// The desktop daemon's retry after its connection failed or dropped.
const LOCAL_RETRY: Duration = Duration::from_secs(2);
/// How long a download waits, while the app is in front, for the file host to answer again.
const REACQUIRE_WAIT: Duration = Duration::from_secs(30);
const REACQUIRE_TICK: Duration = Duration::from_millis(100);

const NOT_CONNECTED: &str = "Tjommie isn't connected right now.";
const FILE_HOST_OFFLINE: &str = "The file host is offline. Reconnect to download it.";
const FILE_HOST_GONE: &str = "The file host did not reconnect. Try downloading again.";
const BACKGROUND_SERVICE: &str = "Tjommie couldn't reach its background service.";
const FELL_BEHIND: &str = "The connection fell behind the daemon's events.";

/// Where the desktop's own daemon listens (`DesktopConnection`). The token is a secret, so `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq)]
pub struct DesktopConnection {
    pub port: u16,
    pub token: String,
}

impl fmt::Debug for DesktopConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DesktopConnection")
            .field("port", &self.port)
            .field("token", &"<hidden>")
            .finish()
    }
}

/// A PC this device has saved (the part of `RemoteHost` the connection reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPc {
    pub id: String,
    pub name: String,
}

/// What a file cleanup covers (`invalidateFiles`'s scope): the transfers in flight, or everything the selected PC had
/// authorized (a switch of PC, or the PC refusing this device).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesScope {
    All,
    Transfers,
}

/// Why a link couldn't be opened or was lost. What the TypeScript told apart by `error.code`, by `instanceof`, and by text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Close code 4003: the daemon speaks another protocol version, so this app needs updating.
    Update,
    /// The PC answered, over the authenticated channel, that this device isn't paired with it (`RemoteRefusedError`).
    Refused,
    /// The PC's relay says the PC isn't connected (`HostOfflineError`), with the sentence to show.
    HostOffline(String),
    /// The connection couldn't be reached, or ended (`ClientConnectionError`), with its text.
    Unreachable(String),
    /// Anything else, with its text (a daemon's reason, a PC that can't be reached).
    Other(String),
}

impl Failure {
    /// The sentence for a failed screen (`errorText(error)`).
    fn sentence(&self) -> String {
        match self {
            Self::Update => "Tjommie needs an update to talk to this PC.".to_owned(),
            Self::Refused => "This device is not paired with this PC.".to_owned(),
            Self::HostOffline(text) | Self::Unreachable(text) | Self::Other(text) => {
                error_text(text)
            }
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.sentence())
    }
}

/// The close code of a daemon that speaks another protocol version.
const PROTOCOL_MISMATCH: u16 = 4003;

impl From<&CloseReason> for Failure {
    fn from(reason: &CloseReason) -> Self {
        if reason.code == PROTOCOL_MISMATCH {
            Self::Update
        } else {
            Self::Unreachable(reason.to_string())
        }
    }
}

impl From<&ConnectError> for Failure {
    fn from(error: &ConnectError) -> Self {
        match error {
            ConnectError::ProtocolMismatch => Self::Update,
            ConnectError::Closed(reason) => Self::from(reason),
            other => Self::Unreachable(other.to_string()),
        }
    }
}

impl From<&RpcError> for Failure {
    fn from(error: &RpcError) -> Self {
        match error {
            RpcError::Closed(reason) => Self::from(reason),
            other => Self::Other(other.to_string()),
        }
    }
}

/// Why a call through the connection failed.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    /// "Tjommie isn't connected right now."
    NotConnected,
    Rpc(RpcError),
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => formatter.write_str(NOT_CONNECTED),
            Self::Rpc(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RequestError {}

impl From<RpcError> for RequestError {
    fn from(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}

/// One connection to a daemon, as the app uses it. `client::Client` is the real one (below); a test gives a fake.
pub trait DaemonLink: Rpc + Clone + 'static {
    /// The events the daemon pushes from now on, in order. A receiver that lags has missed state: the connection treats
    /// that as a dead link and reconnects.
    fn events(&self) -> broadcast::Receiver<Event>;
    /// Resolves, once, when the connection has ended, with why.
    fn closed(&self) -> impl Future<Output = CloseReason> + Send + 'static;
    /// Says goodbye, and waits for the connection to end. Calls still waiting fail.
    fn close(&self) -> impl Future<Output = ()> + Send;
}

impl DaemonLink for client::Client {
    fn events(&self) -> broadcast::Receiver<Event> {
        client::Client::events(self)
    }

    fn closed(&self) -> impl Future<Output = CloseReason> + Send + 'static {
        client::Client::closed(self)
    }

    fn close(&self) -> impl Future<Output = ()> + Send {
        let link = self.clone();
        async move { client::Client::close(&link).await }
    }
}

/// Everything the connection needs from the app. The methods are called while the connection holds its lock, so they must
/// not call back into it; the futures they return run without it.
pub trait Environment: Send + Sync + 'static {
    type Link: DaemonLink;

    // The device.

    /// `isAndroidApp()`: a phone connects to a saved PC and never to a daemon of its own.
    fn is_android(&self) -> bool;
    /// The time now.
    fn now(&self) -> Instant;
    /// Runs detached work. It must run to its end (the connection's attempt task lives as long as its link does).
    fn spawn(&self, task: BoxFuture<'static, ()>);

    // The desktop's own daemon (`desktop()`).

    /// `desktop().connection()`: where the daemon is, or why the app couldn't start it.
    fn local_daemon(&self) -> impl Future<Output = Result<DesktopConnection, String>> + Send;
    /// `desktop().restartDaemon()`: starts the daemon again after it failed. The answer comes as `on_connection` or
    /// `on_daemon_failed`.
    fn restart_daemon(&self);
    /// `connect(port, token)`: `Failure::Update` for a daemon that speaks another protocol, `Unreachable` for one that
    /// can't be reached.
    fn connect_local(
        &self,
        connection: &DesktopConnection,
    ) -> impl Future<Output = Result<Self::Link, Failure>> + Send;

    // Saved PCs (`remote-hosts.ts`).

    /// `remoteHosts()`.
    fn saved_hosts(&self) -> Vec<SavedPc>;
    /// `openRemoteClient(id)`. `DialError::Refused` is `Failure::Refused`, `DialError::HostOffline` is
    /// `Failure::HostOffline`, protocol 4003 is `Failure::Update`, anything else is `Failure::Other` with its sentence.
    fn open_remote(&self, id: &str) -> impl Future<Output = Result<Self::Link, Failure>> + Send;
    /// `forgetRemoteHost(id)`: deletes the PC and its keys, or says why it couldn't.
    fn forget_host(&self, id: &str) -> impl Future<Output = Result<(), String>> + Send;

    // The file host (`attachmentDevice().invalidate`).

    /// Revokes what the file host had authorized for `scope`, at once (before this returns); the future finishes when the
    /// native side has cleaned up. The app also retires its thumbnail queue here (`invalidateThumbnailWork`), with this
    /// future as the cleanup it waits for. Without a native file host, the future is ready.
    fn invalidate_files(
        &self,
        scope: FilesScope,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static;

    // Keep-alive and push (phone only; the desktop answers `false`, ignores the sync and has nothing to register).

    /// `keepAliveRunning()`.
    fn keep_alive_running(&self) -> bool;
    /// `syncKeepAlive(status, name)`.
    fn sync_keep_alive(&self, status: &Connection, host_name: Option<&str>);
    /// `registerPushIfAllowed(link)`: the link is the raw one, which sends at once, without waiting in the call queue.
    fn register_push(&self, link: &Self::Link) -> impl Future<Output = Result<(), String>> + Send;
    /// `stopSendingPush()`.
    fn stop_sending_push(&self);
    /// `unregisterPush()`.
    fn unregister_push(&self) -> impl Future<Output = ()> + Send;
}

/// A link with at most `CALLS` calls in flight; the rest wait their turn, in order. A call that ends hands its slot straight
/// to the next one waiting (the semaphore queues fairly, and a call that is dropped gives its slot, or its place, back).
struct Limited<L> {
    link: L,
    slots: Semaphore,
}

/// The live daemon, as screens call it (`daemon()`): every call goes through the limit, so no burst, such as each room's
/// or bot's history being read again at once, can trip the phone host's limit and drop the connection. Two handles are
/// equal when they are the same connection.
pub struct Daemon<L> {
    limited: Arc<Limited<L>>,
    id: u64,
}

impl<L> Clone for Daemon<L> {
    fn clone(&self) -> Self {
        Self {
            limited: Arc::clone(&self.limited),
            id: self.id,
        }
    }
}

impl<L> PartialEq for Daemon<L> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<L> fmt::Debug for Daemon<L> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Daemon").field(&self.id).finish()
    }
}

impl<L> Daemon<L> {
    fn new(link: L, id: u64) -> Self {
        Self {
            limited: Arc::new(Limited {
                link,
                slots: Semaphore::new(CALLS),
            }),
            id,
        }
    }
}

impl<L: DaemonLink> Rpc for Daemon<L> {
    fn call<M: ClientMethod>(
        &self,
        params: M::Params,
    ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
        let limited = Arc::clone(&self.limited);
        async move {
            let Ok(_slot) = limited.slots.acquire().await else {
                return Err(RpcError::Closed(CloseReason {
                    code: 1006,
                    reason: NOT_CONNECTED.to_owned(),
                    opened: true,
                }));
            };
            limited.link.call::<M>(params).await
        }
    }
}

/// A chat's pages (`fetchPage`): the page before the given message, or the newest page for `None`.
pub type FetchPage =
    Arc<dyn Fn(Option<String>) -> BoxFuture<'static, Result<Page, RequestError>> + Send + Sync>;

/// A connection target (`ConnectionTarget`).
#[derive(Clone)]
enum Target {
    Local(DesktopConnection),
    Remote(String),
}

/// What the window is connecting to now. Each connect gets its own serial, so an attempt (or a timer) that belongs to an
/// older one sees that it was replaced, as the TypeScript's `latest !== connection` did.
struct Latest {
    serial: u64,
    target: Target,
}

/// The ready connection: what screens call, and the same link without the limit, for a PC's pings.
struct Ready<L> {
    id: u64,
    daemon: Daemon<L>,
    raw: L,
}

struct Retry {
    due: Instant,
    serial: u64,
}

struct Heartbeat {
    due: Instant,
    link: u64,
}

struct Sleeper {
    due: Instant,
    wake: oneshot::Sender<()>,
}

/// A download that may be cancelled by a change of PC (`downloads`).
struct Intent {
    abort: Signal,
    transferring: bool,
}

/// The message events that arrived while a conversation's newest page is fetched again.
struct Arrival {
    id: u64,
    events: Vec<MessageEvent>,
}

struct Inner<L> {
    store: AppStore,
    /// The last id handed out: serials, links, sleeps, intents and arrivals share it.
    ids: u64,
    client: Option<Ready<L>>,
    latest: Option<Latest>,
    retry: Option<Retry>,
    heartbeat: Option<Heartbeat>,
    ping_generation: u64,
    /// The latest connection's failed tries in a row.
    failures: u32,
    started: bool,
    visible: bool,
    /// Every file cleanup so far, finished (`filesClosed`).
    files_closed: Shared<BoxFuture<'static, ()>>,
    selection_epoch: u64,
    downloads: HashMap<u64, Intent>,
    /// Each loaded conversation's pages, to catch up after a reconnect.
    fetchers: HashMap<String, FetchPage>,
    arriving: HashMap<String, Arrival>,
    sleeps: BTreeMap<u64, Sleeper>,
    /// The visibility listeners of the downloads that wait for the file host.
    reacquiring: HashMap<u64, Arc<Notify>>,
}

impl<L> Inner<L> {
    fn new() -> Self {
        Self {
            store: AppStore::new(),
            ids: 0,
            client: None,
            latest: None,
            retry: None,
            heartbeat: None,
            ping_generation: 0,
            failures: 0,
            started: false,
            visible: true,
            files_closed: futures::future::ready(()).boxed().shared(),
            selection_epoch: 0,
            downloads: HashMap::new(),
            fetchers: HashMap::new(),
            arriving: HashMap::new(),
            sleeps: BTreeMap::new(),
            reacquiring: HashMap::new(),
        }
    }

    fn next_id(&mut self) -> u64 {
        self.ids += 1;
        self.ids
    }

    fn is_latest(&self, serial: u64) -> bool {
        self.latest
            .as_ref()
            .is_some_and(|latest| latest.serial == serial)
    }

    fn is_remote(&self) -> bool {
        self.remote_id().is_some()
    }

    /// The saved PC the window is connecting to (`latest.remoteHostId`).
    fn remote_id(&self) -> Option<&str> {
        match self.latest.as_ref().map(|latest| &latest.target) {
            Some(Target::Remote(id)) => Some(id),
            _ => None,
        }
    }

    fn ready_id(&self) -> Option<u64> {
        self.client.as_ref().map(|ready| ready.id)
    }

    fn status(&self) -> &Connection {
        &self.store.state().connection
    }
}

/// Sets `slot` and says whether it changed.
fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

/// The connection's status becomes `status`.
fn status_is(status: Connection) -> impl FnOnce(&mut AppState) -> bool {
    move |state| set(&mut state.connection, status)
}

/// A live link that is replaced, or lost, says so while the new one opens.
fn reconnecting_if_ready(state: &mut AppState) -> bool {
    if state.connection == Connection::Ready {
        state.connection = Connection::Reconnecting;
        true
    } else {
        false
    }
}

/// The conversation a message event belongs to, with the event (`event.data.conversationId`).
fn message_event(event: &Event) -> Option<(String, MessageEvent)> {
    match event {
        Event::MessageAdded(message) => Some((
            message.conversation_id.clone(),
            MessageEvent::Added(message.clone()),
        )),
        Event::MessageUpdated(message) => Some((
            message.conversation_id.clone(),
            MessageEvent::Updated(message.clone()),
        )),
        Event::MessageDelta(delta) => Some((
            delta.conversation_id.clone(),
            MessageEvent::Delta(delta.clone()),
        )),
        _ => None,
    }
}

/// `z.uuid()` from zod: version 1 to 8 and the RFC variant, or the nil and max UUIDs.
fn is_uuid(text: &str) -> bool {
    const SIZES: [usize; 5] = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = text.split('-').collect();
    if parts.len() != SIZES.len()
        || !parts
            .iter()
            .zip(SIZES)
            .all(|(part, size)| part.len() == size && part.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return false;
    }
    let digits = text.replace('-', "");
    if digits.chars().all(|c| c == '0') || digits.chars().all(|c| c.eq_ignore_ascii_case(&'f')) {
        return true;
    }
    let version = parts.get(2).and_then(|part| part.chars().next());
    let variant = parts.get(3).and_then(|part| part.chars().next());
    version.is_some_and(|c| ('1'..='8').contains(&c))
        && variant.is_some_and(|c| matches!(c, '8' | '9' | 'a' | 'b' | 'A' | 'B'))
}

/// JavaScript's `encodeURIComponent`.
fn encode_uri_component(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `z.string().min(1).max(64)`: JavaScript counts UTF-16 code units.
fn is_room_id(text: &str) -> bool {
    (1..=64).contains(&text.encode_utf16().count())
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app's connection to a daemon. Share it as an `Arc`: the methods that start detached work take `&Arc<Self>`.
pub struct AppConnection<E: Environment> {
    env: E,
    inner: Mutex<Inner<E::Link>>,
    timers: watch::Sender<u64>,
}

/// Takes a sleep back out of the connection when its future is dropped before it woke.
struct SleepGuard<'a, E: Environment> {
    connection: &'a AppConnection<E>,
    id: u64,
}

impl<E: Environment> Drop for SleepGuard<'_, E> {
    fn drop(&mut self) {
        self.connection.lock().sleeps.remove(&self.id);
    }
}

/// Takes a conversation's arrivals back out when its catch-up fetch ends, or is dropped.
struct ArrivalGuard<E: Environment> {
    connection: Arc<AppConnection<E>>,
    conversation: String,
    id: u64,
}

impl<E: Environment> Drop for ArrivalGuard<E> {
    fn drop(&mut self) {
        let mut s = self.connection.lock();
        if s.arriving
            .get(&self.conversation)
            .is_some_and(|arrival| arrival.id == self.id)
        {
            s.arriving.remove(&self.conversation);
        }
    }
}

/// Takes a waiting download's visibility listener back out when it ends, or is dropped.
struct ListenerGuard<'a, E: Environment> {
    connection: &'a AppConnection<E>,
    id: u64,
}

impl<E: Environment> Drop for ListenerGuard<'_, E> {
    fn drop(&mut self) {
        self.connection.lock().reacquiring.remove(&self.id);
    }
}

/// Why a download that waited for the file host stopped waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReacquireError {
    /// The download was aborted, or what it had authorized is gone (the TypeScript's `AbortError`).
    Cancelled,
    /// 30 visible seconds went by without the file host answering.
    Gone,
}

impl fmt::Display for ReacquireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "Cancelled",
            Self::Gone => FILE_HOST_GONE,
        })
    }
}

impl std::error::Error for ReacquireError {}

/// A download can't start: the file host isn't connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHostOffline;

impl fmt::Display for FileHostOffline {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(FILE_HOST_OFFLINE)
    }
}

impl std::error::Error for FileHostOffline {}

/// A transfer was refused because what authorized it is gone (the TypeScript's `AbortError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Cancelled")
    }
}

impl std::error::Error for Cancelled {}

impl<E: Environment> AppConnection<E> {
    pub fn new(env: E) -> Arc<Self> {
        let (timers, _) = watch::channel(0);
        Arc::new(Self {
            env,
            inner: Mutex::new(Inner::new()),
            timers,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner<E::Link>> {
        locked(&self.inner)
    }

    /// Tells the app's timer loop that a timer was set.
    fn touch(&self) {
        self.timers.send_modify(|version| *version += 1);
    }

    // The state (`store.ts`, which the TypeScript keeps in module state).

    /// `getState()`: a copy of the state as it is now.
    pub fn state(&self) -> AppState {
        self.lock().store.snapshot()
    }

    /// Reads the state without copying it. `read` must not call back into the connection.
    pub fn read<T>(&self, read: impl FnOnce(&AppState) -> T) -> T {
        read(self.lock().store.state())
    }

    /// `update(change)`: `change` edits the state and returns whether its content changed.
    pub fn update(&self, change: impl FnOnce(&mut AppState) -> bool) {
        self.lock().store.update(change);
    }

    /// `subscribe(listener)`: told the state after each change. It must not call back into the connection.
    pub fn subscribe(&self, listener: impl FnMut(&AppState) + Send + 'static) -> SubscriptionId {
        self.lock().store.subscribe(listener)
    }

    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        self.lock().store.unsubscribe(id)
    }

    // Calls.

    /// The live daemon client. Screens call it only while the connection is ready.
    pub fn daemon(&self) -> Result<Daemon<E::Link>, RequestError> {
        self.lock()
            .client
            .as_ref()
            .map(|ready| ready.daemon.clone())
            .ok_or(RequestError::NotConnected)
    }

    /// Calls the daemon, or fails when Tjommie isn't connected.
    pub async fn request<M: ClientMethod>(
        &self,
        params: M::Params,
    ) -> Result<M::Result, RequestError> {
        let daemon = self.daemon()?;
        daemon.call::<M>(params).await.map_err(RequestError::Rpc)
    }

    // The conversations (`botHistory`, `roomHistory`, `exchangePage`, `loadConversation`, ...).

    /// A bot chat's pages, newest first.
    pub fn bot_history(self: &Arc<Self>, bot_id: &str) -> FetchPage {
        let connection = Arc::clone(self);
        let bot_id = bot_id.to_owned();
        Arc::new(move |before| {
            let connection = Arc::clone(&connection);
            let bot_id = bot_id.clone();
            async move {
                let page = connection
                    .request::<method::ChatHistory>(ChatHistory {
                        bot_id,
                        before,
                        limit: PAGE,
                    })
                    .await?;
                Ok(Page {
                    messages: page.messages,
                    has_more: page.has_more,
                })
            }
            .boxed()
        })
    }

    /// A room's pages, newest first.
    pub fn room_history(self: &Arc<Self>, room_id: &str) -> FetchPage {
        let connection = Arc::clone(self);
        let room_id = room_id.to_owned();
        Arc::new(move |before| {
            let connection = Arc::clone(&connection);
            let room_id = room_id.clone();
            async move {
                let page = connection
                    .request::<method::RoomHistory>(RoomHistory {
                        before,
                        limit: PAGE,
                        room_id,
                    })
                    .await?;
                Ok(Page {
                    messages: page.messages,
                    has_more: page.has_more,
                })
            }
            .boxed()
        })
    }

    /// An exchange's messages: the newest 200, with no older pages to load.
    pub fn exchange_page(self: &Arc<Self>, exchange_id: &str) -> FetchPage {
        let connection = Arc::clone(self);
        let exchange_id = exchange_id.to_owned();
        Arc::new(move |_| {
            let connection = Arc::clone(&connection);
            let id = exchange_id.clone();
            async move {
                let view = connection
                    .request::<method::ExchangesGet>(Id { id })
                    .await?;
                Ok(Page {
                    messages: view.messages,
                    has_more: false,
                })
            }
            .boxed()
        })
    }

    /// Runs `work` on its own task and waits for it. A caller that stops waiting (its future is dropped) doesn't cancel it:
    /// the work leaves state behind that must be finished (a chat that is loading, a PC that is half forgotten), as the
    /// TypeScript's promises, which nothing cancels, do. `None`: the app shut the task down.
    async fn detached<T: Send + 'static>(
        &self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> Option<T> {
        let (done, finished) = oneshot::channel();
        self.env.spawn(Box::pin(async move {
            let _ = done.send(work.await);
        }));
        finished.await.ok()
    }

    /// Loads a conversation's newest page once; events keep it current from then on, and a reconnect catches it up. The
    /// load finishes even if the caller stops waiting.
    pub async fn load_conversation(self: &Arc<Self>, conversation_id: &str, fetch: FetchPage) {
        {
            let mut s = self.lock();
            if s.store.state().conversations.contains_key(conversation_id) {
                return;
            }
            s.fetchers
                .insert(conversation_id.to_owned(), Arc::clone(&fetch));
            s.store
                .update(|state| start_loading(state, conversation_id));
        }
        let this = Arc::clone(self);
        let conversation_id = conversation_id.to_owned();
        self.detached(async move {
            let outcome = fetch(None).await;
            let mut s = this.lock();
            match outcome {
                Ok(page) => {
                    s.store
                        .update(|state| finish_loading(state, &conversation_id, page));
                }
                Err(_) => s.store.update(|state| {
                    let Some(conversation) = state.conversations.get_mut(&conversation_id) else {
                        return false;
                    };
                    if conversation.status != ConversationStatus::Loading {
                        return false;
                    }
                    conversation.status = ConversationStatus::Failed;
                    conversation.pending.clear();
                    true
                }),
            }
        })
        .await;
    }

    /// Retry after a failed load: `load_conversation` keeps a failed chat until it's forgotten.
    pub async fn retry_conversation(self: &Arc<Self>, conversation_id: &str, fetch: FetchPage) {
        self.update(|state| forget_conversation(state, conversation_id));
        self.load_conversation(conversation_id, fetch).await;
    }

    /// A closed view's conversation (an exchange) is forgotten, so it stops following events. It loads again if reopened.
    pub fn close_conversation(&self, conversation_id: &str) {
        let mut s = self.lock();
        s.fetchers.remove(conversation_id);
        s.store
            .update(|state| forget_conversation(state, conversation_id));
    }

    /// Loads the page before the oldest loaded message.
    pub async fn load_older(
        &self,
        conversation_id: &str,
        fetch: FetchPage,
    ) -> Result<(), RequestError> {
        let before = {
            let s = self.lock();
            let Some(conversation) = s.store.state().conversations.get(conversation_id) else {
                return Ok(());
            };
            if conversation.status != ConversationStatus::Ready
                || !conversation.has_more
                || conversation.messages.is_empty()
            {
                return Ok(());
            }
            conversation
                .messages
                .first()
                .map(|message| message.id.clone())
        };
        let page = fetch(before).await?;
        self.update(|state| add_older(state, conversation_id, page));
        Ok(())
    }

    // Connecting.

    /// Connects once for the life of the window, and again whenever the app restarts the daemon.
    pub fn start_connection(self: &Arc<Self>) {
        let mut s = self.lock();
        if s.started {
            return;
        }
        s.started = true;
        if self.env.is_android() {
            self.connect_remote_in(&mut s, None);
            return;
        }
        self.open_local();
    }

    /// The desktop app pushed a new connection to its daemon (`api.onConnection`). A window on another PC ignores it.
    pub fn on_connection(self: &Arc<Self>, next: DesktopConnection) {
        let mut s = self.lock();
        if !s.is_remote() {
            self.open(&mut s, next);
        }
    }

    /// The desktop app couldn't start the daemon (`api.onDaemonFailed`). A window on another PC ignores it.
    pub fn on_daemon_failed(self: &Arc<Self>, reason: String) {
        let mut s = self.lock();
        if !s.is_remote() {
            self.failed(&mut s, reason);
        }
    }

    /// Asks the desktop app to start the daemon again after it failed. On another PC, it tries that PC again.
    pub fn restart_daemon(self: &Arc<Self>) {
        let mut s = self.lock();
        if self.env.is_android() || s.is_remote() {
            let id = s.remote_id().map(str::to_owned);
            self.connect_remote_in(&mut s, id.as_deref());
            return;
        }
        s.store.update(status_is(Connection::Connecting));
        self.env.restart_daemon();
    }

    /// Connects to a saved PC now: on start, from the connect screen, the host switcher or a push. Another PC's cached
    /// screens go first; the same PC's stay up while it reconnects. With no PC (Switch PC), this device goes to the connect
    /// screen. `None` is the first saved PC.
    pub fn connect_remote(self: &Arc<Self>, id: Option<&str>) {
        let mut s = self.lock();
        self.connect_remote_in(&mut s, id);
    }

    fn connect_remote_in(self: &Arc<Self>, s: &mut Inner<E::Link>, id: Option<&str>) {
        let id = id.map(str::to_owned).or_else(|| {
            self.env
                .saved_hosts()
                .into_iter()
                .next()
                .map(|host| host.id)
        });
        if s.remote_id() != id.as_deref() || id.is_none() {
            self.leave(s);
        }
        let Some(id) = id else {
            if self.env.is_android() && self.env.saved_hosts().is_empty() {
                let this = Arc::clone(self);
                self.env.spawn(Box::pin(async move {
                    this.env.unregister_push().await;
                }));
            }
            s.store.update(status_is(Connection::Outside));
            return;
        };
        // Connect was pressed.
        s.store.update(|state| {
            if matches!(state.connection, Connection::Failed { .. }) {
                state.connection = Connection::Connecting;
                true
            } else {
                false
            }
        });
        self.begin(s, id);
    }

    /// The desktop app's own PC again, after another PC.
    pub fn connect_local(self: &Arc<Self>) {
        let mut s = self.lock();
        if !s.is_remote() {
            return;
        }
        self.leave(&mut s);
        self.open_local();
    }

    /// The PC's keys go first: if deleting them fails, nothing has moved (the removed screen stays and says why). Only then
    /// does the window leave that PC, for the next saved one on a phone and this PC's own Tjommie on the desktop. Letting go
    /// of push is tidying up, so a PC or push service that fails or doesn't answer in time never holds Forget up. Forget
    /// finishes even if the caller stops waiting: it is never left half done.
    pub async fn forget_remote(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let this = Arc::clone(self);
        let id = id.to_owned();
        self.detached(async move { this.forget_remote_now(&id).await })
            .await
            .unwrap_or_else(|| Err("Forget didn't finish: Tjommie is closing.".to_owned()))
    }

    async fn forget_remote_now(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let android = self.env.is_android();
        let (on_this_pc, current) = {
            let s = self.lock();
            (
                android && s.remote_id() == Some(id),
                s.client.as_ref().map(|ready| (ready.id, ready.raw.clone())),
            )
        };
        if on_this_pc {
            self.env.stop_sending_push(); // before its token is cleared there, so nothing reaches it after
            if let Some((_, link)) = &current {
                let cleared = link.call::<method::DeviceRegisterPush>(DeviceRegisterPush {
                    token: None,
                    kinds: None,
                });
                self.at_most(FORGET_WAIT, cleared).await;
            }
        }
        if let Err(error) = self.env.forget_host(id).await {
            let again = {
                let s = self.lock();
                match &current {
                    Some((ready, link))
                        if android && s.remote_id() == Some(id) && s.ready_id() == Some(*ready) =>
                    {
                        Some(link.clone())
                    }
                    _ => None,
                }
            };
            if let Some(link) = again {
                let this = Arc::clone(self);
                self.env.spawn(Box::pin(async move {
                    let _ = this.env.register_push(&link).await;
                }));
            }
            return Err(error);
        }
        if android && self.env.saved_hosts().is_empty() {
            self.at_most(FORGET_WAIT, self.env.unregister_push()).await;
        }
        let mut s = self.lock();
        if s.remote_id() != Some(id) {
            return Ok(());
        }
        self.leave(&mut s);
        if android {
            self.connect_remote_in(&mut s, None);
        } else {
            self.open_local();
        }
        Ok(())
    }

    /// A tapped push: its PC becomes the current one if it isn't (and is saved here), and the room it came from, or else
    /// the bot's chat, is where to go.
    pub fn route_push(self: &Arc<Self>, data: &PushData) -> Option<String> {
        let mut s = self.lock();
        let saved = self
            .env
            .saved_hosts()
            .iter()
            .any(|host| host.id == data.host_id);
        if !saved || !is_uuid(&data.bot_id) {
            return None;
        }
        if s.remote_id() != Some(data.host_id.as_str()) {
            self.connect_remote_in(&mut s, Some(&data.host_id));
        }
        Some(
            match data.room_id.as_deref().filter(|room| is_room_id(room)) {
                Some(room) => format!("/room/?id={}", encode_uri_component(room)),
                None => format!("/chat/?bot={}", encode_uri_component(&data.bot_id)),
            },
        )
    }

    /// Asks the desktop app for its daemon, and connects to it unless the window went to another PC meanwhile.
    fn open_local(self: &Arc<Self>) {
        let this = Arc::clone(self);
        self.env.spawn(Box::pin(async move {
            let result = this.env.local_daemon().await;
            let mut s = this.lock();
            if s.is_remote() {
                return;
            }
            match result {
                Ok(connection) => this.open(&mut s, connection),
                Err(reason) => this.failed(&mut s, reason),
            }
        }));
    }

    /// The desktop app couldn't start the daemon. Stop retrying the old connection; "Try again" asks for a new one.
    fn failed(&self, s: &mut Inner<E::Link>, reason: String) {
        self.invalidate_files(s, FilesScope::Transfers);
        s.client = None;
        s.retry = None;
        self.env.stop_sending_push();
        s.latest = None;
        s.store.update(status_is(Connection::Failed { reason }));
    }

    fn open(self: &Arc<Self>, s: &mut Inner<E::Link>, connection: DesktopConnection) {
        if let Some(Latest {
            target: Target::Local(current),
            ..
        }) = &s.latest
            && current.token == connection.token
        {
            return; // already connecting with this one
        }
        let serial = s.next_id();
        s.latest = Some(Latest {
            serial,
            target: Target::Local(connection),
        });
        s.failures = 0;
        self.spawn_attempt(serial);
    }

    /// Connects to this PC now, from the first backoff step. A ready link is closed (its calls in flight fail and are never
    /// sent again), and the cached screens stay up while it reconnects. A failed first connect keeps its screen, so pairing
    /// there isn't interrupted.
    fn begin(self: &Arc<Self>, s: &mut Inner<E::Link>, id: String) {
        s.retry = None;
        s.failures = 0;
        let serial = s.next_id();
        s.latest = Some(Latest {
            serial,
            target: Target::Remote(id.clone()),
        });
        s.store.update(|state| {
            let mut changed = false;
            if state.host.as_deref() != Some(id.as_str()) {
                state.host = Some(id);
                changed = true;
            }
            if !matches!(
                state.connection,
                Connection::Failed { .. } | Connection::Connecting | Connection::Reconnecting
            ) {
                let next = if state.settings.is_some() {
                    Connection::Reconnecting
                } else {
                    Connection::Connecting
                };
                changed |= set(&mut state.connection, next);
            }
            changed
        });
        self.spawn_attempt(serial);
    }

    /// Drops the current connection and everything cached from its PC.
    fn leave(&self, s: &mut Inner<E::Link>) {
        self.invalidate_files(s, FilesScope::All);
        s.retry = None;
        s.heartbeat = None;
        s.latest = None;
        let previous = s.client.take();
        if let Some(previous) = previous {
            self.close_in_background(previous.raw);
        }
        self.env.stop_sending_push();
        s.fetchers.clear();
        s.store.update(|state| {
            let initial = AppState::default();
            if *state == initial {
                false
            } else {
                *state = initial;
                true
            }
        });
        self.env.sync_keep_alive(&Connection::Outside, None);
    }

    /// Revokes the file host's authorization for `scope` now. `files_closed` is then the wait for every cleanup so far.
    fn invalidate_files(&self, s: &mut Inner<E::Link>, scope: FilesScope) {
        if scope == FilesScope::All {
            s.selection_epoch += 1;
        }
        for intent in s.downloads.values() {
            if scope == FilesScope::All || intent.transferring {
                intent.abort.abort();
            }
        }
        // The cleanup runs on its own (the TypeScript's native promise); a failed one, or one that never finishes because the
        // app shut down, holds nothing up.
        let closing = self.env.invalidate_files(scope);
        let (done, finished) = oneshot::channel::<()>();
        self.env.spawn(Box::pin(async move {
            let _ = closing.await;
            let _ = done.send(());
        }));
        let earlier = s.files_closed.clone();
        s.files_closed = async move {
            earlier.await;
            let _ = finished.await;
        }
        .boxed()
        .shared();
    }

    /// Closes a link on its own, without waiting (`void link.close().catch(() => {})`).
    fn close_in_background(&self, link: E::Link) {
        self.env.spawn(Box::pin(async move {
            link.close().await;
        }));
    }

    fn spawn_attempt(self: &Arc<Self>, serial: u64) {
        self.env.spawn(Box::pin(Arc::clone(self).attempt(serial)));
    }

    /// A hidden app without keep-alive is frozen soon after, so its link stops pinging and retrying until it is back in
    /// front.
    fn awake(&self, s: &Inner<E::Link>) -> bool {
        s.visible || self.env.keep_alive_running()
    }

    // The remote link's health.

    /// The remote link may be dead: the network changed, a ping went unanswered, the app is back after a sleep, or the link
    /// closed. It is replaced at once; a hidden app without keep-alive only lets it go, and its next return starts again.
    fn reconnect_now(self: &Arc<Self>, s: &mut Inner<E::Link>) {
        let Some(id) = s.remote_id().map(str::to_owned) else {
            return;
        };
        if *s.status() == Connection::Removed {
            return;
        }
        if self.awake(s) {
            self.begin(s, id);
            return;
        }
        s.heartbeat = None;
        let previous = s.client.take();
        self.env.stop_sending_push();
        self.invalidate_files(s, FilesScope::Transfers);
        if let Some(previous) = previous {
            self.close_in_background(previous.raw);
        }
        s.store.update(reconnecting_if_ready);
    }

    /// The network changed (`onNetworkChange`): a phone's link may be dead.
    pub fn network_changed(self: &Arc<Self>) {
        let mut s = self.lock();
        self.reconnect_now(&mut s);
    }

    /// The app came to the front or went away (`visibilitychange`).
    pub fn on_visibility(self: &Arc<Self>, visible: bool) {
        let mut s = self.lock();
        s.visible = visible;
        self.visibility_changed(&mut s);
        for listener in s.reacquiring.values() {
            listener.notify_one();
        }
    }

    fn visibility_changed(self: &Arc<Self>, s: &mut Inner<E::Link>) {
        if !s.is_remote() {
            return;
        }
        if !s.visible {
            s.ping_generation += 1; // Background suspension can delay a foreground ping's reply past its deadline.
            let wait = self.default_wait(s);
            self.beat(s, wait); // slower with keep-alive, or none
            return;
        }
        let status = s.status().clone();
        if status == Connection::Removed {
            return;
        }
        self.env.sync_keep_alive(&status, None);
        // Check before replacing a healthy link: returning from a native Save picker must retain its staged file.
        if s.client.is_some() {
            s.ping_generation += 1;
            self.beat(s, Some(Duration::ZERO));
        } else {
            self.reconnect_now(s);
        }
    }

    /// The wait to the next ping: every 20 s in front, every 60 s hidden while keep-alive runs, otherwise none.
    fn default_wait(&self, s: &Inner<E::Link>) -> Option<Duration> {
        if s.visible {
            Some(Duration::from_millis(LINK.ping_foreground_ms))
        } else if self.env.keep_alive_running() {
            Some(Duration::from_millis(LINK.ping_background_ms))
        } else {
            None
        }
    }

    /// The next ping on the remote link, in `wait` (`None`: none).
    fn beat(&self, s: &mut Inner<E::Link>, wait: Option<Duration>) {
        s.heartbeat = None;
        let (Some(link), Some(wait)) = (s.ready_id(), wait) else {
            return;
        };
        s.heartbeat = Some(Heartbeat {
            due: self.env.now() + wait,
            link,
        });
        self.touch();
    }

    async fn ping(self: Arc<Self>, link: u64, generation: u64) {
        let raw = {
            let s = self.lock();
            match &s.client {
                Some(ready) if ready.id == link => ready.raw.clone(),
                _ => return,
            }
        };
        let answered = self.probe_link(&raw, None).await;
        let mut s = self.lock();
        if s.ready_id() != Some(link) || generation != s.ping_generation {
            return; // replaced or checked again meanwhile
        }
        if !answered {
            self.reconnect_now(&mut s);
            return;
        }
        let wait = self.default_wait(&s);
        self.beat(&mut s, wait);
    }

    /// Whether the link answers a ping within the pong deadline. Any answer counts, an error too (a PC from before
    /// `remote.ping` says unknown_method); only silence or a closed link means it's gone.
    async fn probe_link(&self, link: &E::Link, signal: Option<&Signal>) -> bool {
        if signal.is_some_and(Signal::aborted) {
            return false;
        }
        let answer = async {
            !matches!(
                link.call::<method::RemotePing>(Empty {}).await,
                Err(RpcError::Closed(_))
            )
        };
        let aborted = async {
            match signal {
                Some(signal) => signal.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            biased;
            answered = answer => answered,
            () = aborted => false,
            () = self.sleep(Duration::from_millis(LINK.pong_timeout_ms)) => false,
        }
    }

    // Time.

    /// When the earliest timer is due: the heartbeat, the retry, or a wait inside a call. The app calls `on_timer` then.
    pub fn next_timer(&self) -> Option<Instant> {
        let s = self.lock();
        s.retry
            .iter()
            .map(|retry| retry.due)
            .chain(s.heartbeat.iter().map(|beat| beat.due))
            .chain(s.sleeps.values().map(|sleeper| sleeper.due))
            .min()
    }

    /// Fires every timer that is due at `now`, in order. Call it at `next_timer()`. Calling it for a timer that isn't due,
    /// or twice for one, does nothing the second time: a stale timer starts nothing.
    pub fn on_timer(self: &Arc<Self>, now: Instant) {
        let mut s = self.lock();
        loop {
            let retry = s
                .retry
                .as_ref()
                .map(|retry| retry.due)
                .filter(|due| *due <= now);
            let beat = s
                .heartbeat
                .as_ref()
                .map(|beat| beat.due)
                .filter(|due| *due <= now);
            let sleep = s
                .sleeps
                .iter()
                .map(|(id, sleeper)| (sleeper.due, *id))
                .filter(|(due, _)| *due <= now)
                .min();
            // The earliest first; at the same instant the retry, then the heartbeat, then the waits.
            let earliest = [retry, beat, sleep.map(|(due, _)| due)]
                .into_iter()
                .flatten()
                .min();
            let Some(earliest) = earliest else {
                return;
            };
            if retry == Some(earliest) {
                if let Some(Retry { serial, .. }) = s.retry.take()
                    && s.is_latest(serial)
                {
                    self.spawn_attempt(serial);
                }
            } else if beat == Some(earliest) {
                if let Some(Heartbeat { link, .. }) = s.heartbeat.take() {
                    s.ping_generation += 1;
                    let generation = s.ping_generation;
                    if s.ready_id() == Some(link) {
                        self.env
                            .spawn(Box::pin(Arc::clone(self).ping(link, generation)));
                    }
                }
            } else if let Some((_, id)) = sleep
                && let Some(sleeper) = s.sleeps.remove(&id)
            {
                let _ = sleeper.wake.send(());
            }
        }
    }

    /// A watch that changes whenever a timer is set, so the app's timer loop looks at `next_timer()` again.
    pub fn timers_changed(&self) -> watch::Receiver<u64> {
        self.timers.subscribe()
    }

    /// Waits for `wait` on the connection's own timers. Dropping it takes the wait back.
    async fn sleep(&self, wait: Duration) {
        let (wake, woken) = oneshot::channel();
        let id = {
            let mut s = self.lock();
            let id = s.next_id();
            s.sleeps.insert(
                id,
                Sleeper {
                    due: self.env.now() + wait,
                    wake,
                },
            );
            id
        };
        self.touch();
        let _guard = SleepGuard {
            connection: self,
            id,
        };
        let _ = woken.await;
    }

    /// Waits for the work for at most `wait`, and never fails: whether it finished in time (`atMost`).
    async fn at_most<T>(&self, wait: Duration, work: impl Future<Output = T>) -> bool {
        tokio::select! {
            biased;
            _ = work => true,
            () = self.sleep(wait) => false,
        }
    }

    // Opening a link.

    /// Opens the link, loads the lists, then watches the link until it ends. It is the one task per connection the app's
    /// executor runs (`Environment::spawn`).
    async fn attempt(self: Arc<Self>, serial: u64) {
        let (target, previous) = {
            let mut s = self.lock();
            let Some(target) = s
                .latest
                .as_ref()
                .filter(|latest| latest.serial == serial)
                .map(|latest| latest.target.clone())
            else {
                return; // replaced before it began
            };
            s.retry = None;
            s.heartbeat = None;
            let previous = s.client.take();
            self.env.stop_sending_push();
            // A new connection is replacing one that still looked up (the daemon restarted, or the remote link went quiet):
            // until it opens, calls fail as they do while reconnecting, so the pill says so.
            if previous.is_some() {
                s.store.update(reconnecting_if_ready);
            }
            (target, previous)
        };
        if let Some(previous) = previous {
            previous.raw.close().await;
        }
        let closing = {
            let mut s = self.lock();
            self.invalidate_files(&mut s, FilesScope::Transfers);
            s.files_closed.clone()
        };
        closing.await;
        if !self.lock().is_latest(serial) {
            return; // replaced while the old one closed
        }
        let opened = match &target {
            Target::Remote(id) => self.env.open_remote(id).await,
            Target::Local(connection) => self.env.connect_local(connection).await,
        };
        let raw = match opened {
            Ok(raw) => raw,
            Err(failure) => {
                let mut s = self.lock();
                self.lost(&mut s, serial, failure);
                return;
            }
        };
        let id = {
            let mut s = self.lock();
            if !s.is_latest(serial) {
                self.close_in_background(raw);
                return;
            }
            s.next_id()
        };
        let daemon = Daemon::new(raw.clone(), id);
        // Events that arrive while the lists load are applied on top of them, in order. The daemon sends events and
        // replies in the order things happen, so the last word on each bot is always the newest.
        let mut events = raw.events();
        let mut early: Vec<Event> = Vec::new();
        let mut events_open = true;
        let loaded = {
            let lists = Self::load_lists(&daemon);
            tokio::pin!(lists);
            loop {
                tokio::select! {
                    biased;
                    received = events.recv(), if events_open => match received {
                        Ok(event) => early.push(event),
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            break Err(Failure::Unreachable(FELL_BEHIND.to_owned()));
                        }
                        Err(broadcast::error::RecvError::Closed) => events_open = false,
                    },
                    result = &mut lists => break result.map_err(|error| Failure::from(&error)),
                }
            }
        };
        let snapshot = match loaded {
            Ok(snapshot) => snapshot,
            Err(failure) => {
                self.close_in_background(raw);
                let mut s = self.lock();
                self.lost(&mut s, serial, failure);
                return;
            }
        };
        {
            let mut s = self.lock();
            if !s.is_latest(serial) {
                self.close_in_background(raw);
                return;
            }
            s.client = Some(Ready {
                id,
                daemon: daemon.clone(),
                raw: raw.clone(),
            });
            s.failures = 0;
            s.store.update(|state| {
                let mut changed = connected(state, snapshot);
                for event in &early {
                    changed |= apply_event(state, event);
                }
                changed
            });
            self.spawn_setup_status(daemon.clone(), id);
            self.catch_up_all(&mut s, id);
            if let Target::Remote(host) = &target {
                let wait = self.default_wait(&s);
                self.beat(&mut s, wait);
                let this = Arc::clone(&self);
                let link = raw.clone();
                self.env.spawn(Box::pin(async move {
                    let _ = this.env.register_push(&link).await; // the raw link sends at once, without waiting in the call queue
                }));
                let name = self
                    .env
                    .saved_hosts()
                    .into_iter()
                    .find(|saved| saved.id == *host)
                    .map(|saved| saved.name);
                self.env
                    .sync_keep_alive(&Connection::Ready, name.as_deref());
            }
        }
        // Now the link is watched: its events go into the state, and its end starts a reconnect.
        let closed = raw.closed();
        tokio::pin!(closed);
        let reason = loop {
            tokio::select! {
                biased;
                received = events.recv(), if events_open => match received {
                    Ok(event) => self.on_event(&event),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Events were lost, so the state can't be trusted: let the link end, and reconnect.
                        self.close_in_background(raw.clone());
                    }
                    Err(broadcast::error::RecvError::Closed) => events_open = false,
                },
                reason = &mut closed => break reason,
            }
        };
        let mut s = self.lock();
        if s.ready_id() != Some(id) {
            return;
        }
        if matches!(target, Target::Remote(_)) {
            self.reconnect_now(&mut s);
        } else {
            self.lost(&mut s, serial, Failure::from(&reason));
        }
    }

    /// The six lists a new connection starts with. An older daemon has no accounts: the list stays unknown and the screens
    /// that need it say so.
    async fn load_lists(daemon: &Daemon<E::Link>) -> Result<Snapshot, RpcError> {
        let (bots, settings, routines, rooms, accounts, notifications) = tokio::try_join!(
            daemon.call::<method::BotsList>(Empty {}),
            daemon.call::<method::SettingsGet>(Empty {}),
            daemon.call::<method::RoutinesList>(RoutinesList { bot_id: None }),
            daemon.call::<method::RoomsList>(Empty {}),
            async { Ok::<_, RpcError>(daemon.call::<method::AccountsList>(Empty {}).await.ok()) },
            daemon.call::<method::NotificationsPending>(Empty {}),
        )?;
        Ok(Snapshot {
            bots,
            settings,
            routines,
            rooms,
            accounts,
            notifications: Some(notifications),
        })
    }

    /// One event from the daemon: into the state, and into the catch-up of its conversation if that is being fetched.
    fn on_event(&self, event: &Event) {
        let mut s = self.lock();
        s.store.apply(event);
        if let Some((conversation, message)) = message_event(event)
            && let Some(arrival) = s.arriving.get_mut(&conversation)
        {
            arrival.events.push(message);
        }
    }

    /// `setup.status` runs both CLIs' checks, so it's asked once per connection and never polled. If the check fails, both
    /// setup cards say why and offer Check again.
    fn spawn_setup_status(self: &Arc<Self>, daemon: Daemon<E::Link>, id: u64) {
        let this = Arc::clone(self);
        self.env.spawn(Box::pin(async move {
            let outcome = daemon.call::<method::SetupStatus>(Empty {}).await;
            let mut s = this.lock();
            if s.ready_id() != Some(id) {
                return;
            }
            let setup = match outcome {
                Ok(setup) => setup,
                Err(error) => failed_setup(&error_text(&error)),
            };
            s.store.update(|state| set(&mut state.setup, Some(setup)));
        }));
    }

    /// After a reconnect, each loaded chat fetches its newest page again and merges it, so what happened meanwhile shows up
    /// without blanking.
    fn catch_up_all(self: &Arc<Self>, s: &mut Inner<E::Link>, link: u64) {
        let loaded: Vec<(String, FetchPage)> = s
            .store
            .state()
            .conversations
            .iter()
            .filter(|(_, conversation)| conversation.status == ConversationStatus::Ready)
            .filter_map(|(id, _)| {
                s.fetchers
                    .get(id)
                    .map(|fetch| (id.clone(), Arc::clone(fetch)))
            })
            .collect();
        for (conversation, fetch) in loaded {
            let arrival = s.next_id();
            s.arriving.insert(
                conversation.clone(),
                Arrival {
                    id: arrival,
                    events: Vec::new(),
                },
            );
            let this = Arc::clone(self);
            self.env.spawn(Box::pin(async move {
                let _arrival = ArrivalGuard {
                    connection: Arc::clone(&this),
                    conversation: conversation.clone(),
                    id: arrival,
                };
                // A fetch that fails means the link went again; the next connection catches up.
                let Ok(page) = fetch(None).await else {
                    return;
                };
                let mut s = this.lock();
                if s.ready_id() != Some(link) {
                    return;
                }
                let events = s
                    .arriving
                    .get(&conversation)
                    .filter(|arrival_now| arrival_now.id == arrival)
                    .map(|arrival_now| arrival_now.events.clone())
                    .unwrap_or_default();
                s.store
                    .update(|state| catch_up(state, &conversation, page, &events));
            }));
        }
    }

    /// A connection attempt failed, or the desktop's daemon connection dropped.
    fn lost(self: &Arc<Self>, s: &mut Inner<E::Link>, serial: u64, failure: Failure) {
        if !s.is_latest(serial) {
            return;
        }
        self.invalidate_files(s, FilesScope::Transfers);
        s.client = None;
        self.env.stop_sending_push();
        if failure == Failure::Update {
            s.store.update(status_is(Connection::Update));
            return;
        }
        if s.is_remote() {
            self.remote_failed(s, serial, failure);
            return;
        }
        s.failures += 1;
        let failures = s.failures;
        s.store.update(|state| match &state.connection {
            Connection::Ready => {
                state.connection = Connection::Reconnecting;
                true
            }
            Connection::Reconnecting => false,
            _ if failures < STARTING_TRIES => false,
            _ => {
                // Still starting (or after Try again): say why instead of spinning. The tries go on, so a daemon that
                // recovers connects on its own.
                let reason = match &failure {
                    Failure::Unreachable(_) => BACKGROUND_SERVICE.to_owned(),
                    other => other.sentence(),
                };
                set(&mut state.connection, Connection::Failed { reason })
            }
        });
        // If the daemon restarts, the desktop app sends a new connection. Until then, keep trying this one in case only the
        // socket dropped.
        s.retry = Some(Retry {
            due: self.env.now() + LOCAL_RETRY,
            serial,
        });
        self.touch();
    }

    /// A saved PC couldn't be reached. With its screens cached they stay, marked offline (its relay says it's off, or it
    /// can't be reached), and the tries go on with backoff while the app can run. Only a first connect with nothing cached
    /// says why in full. The PC refusing this device is final.
    fn remote_failed(self: &Arc<Self>, s: &mut Inner<E::Link>, serial: u64, failure: Failure) {
        if failure == Failure::Refused {
            self.invalidate_files(s, FilesScope::All);
            s.retry = None;
            s.store.update(status_is(Connection::Removed));
            self.env.sync_keep_alive(&Connection::Removed, None);
            return;
        }
        s.failures += 1;
        let failures = s.failures;
        s.store.update(|state| {
            if state.settings.is_some() {
                let reason = if matches!(failure, Failure::HostOffline(_)) {
                    OfflineReason::HostOffline
                } else {
                    OfflineReason::Unreachable
                };
                return set(&mut state.connection, Connection::Offline { reason });
            }
            if failures < STARTING_TRIES {
                return false;
            }
            set(
                &mut state.connection,
                Connection::Failed {
                    reason: failure.sentence(),
                },
            )
        });
        self.env
            .sync_keep_alive(&s.store.state().connection.clone(), None);
        if !self.awake(s) {
            return;
        }
        let at = usize::try_from(failures)
            .unwrap_or(usize::MAX)
            .min(LINK.backoff_ms.len() - 1);
        let wait = LINK.backoff_ms.get(at).copied().unwrap_or(10_000);
        s.retry = Some(Retry {
            due: self.env.now() + Duration::from_millis(wait),
            serial,
        });
        self.touch();
    }

    // Files (`captureAttachmentSave` and `captureAttachmentDownload`).

    /// The selected PC's epoch survives an ordinary reconnect, but never a switch away and back.
    pub fn capture_attachment_save(self: &Arc<Self>) -> Option<SaveGuard<E>> {
        let s = self.lock();
        if s.client.is_none() || *s.status() != Connection::Ready {
            return None;
        }
        Some(SaveGuard {
            connection: Arc::clone(self),
            epoch: s.selection_epoch,
        })
    }

    /// Captures the live connection for a download: the file host must be connected. `abort` is what a change of PC
    /// aborts. Dropping the result (or `close`) ends the capture.
    pub fn capture_attachment_download(
        self: &Arc<Self>,
        abort: Signal,
    ) -> Result<AttachmentDownload<E>, FileHostOffline> {
        let guard = self.capture_attachment_save().ok_or(FileHostOffline)?;
        let mut s = self.lock();
        let client = s
            .client
            .as_ref()
            .map(|ready| ready.daemon.clone())
            .ok_or(FileHostOffline)?;
        let intent = s.next_id();
        s.downloads.insert(
            intent,
            Intent {
                abort: abort.clone(),
                transferring: false,
            },
        );
        Ok(AttachmentDownload {
            guard,
            intent,
            abort,
            client,
        })
    }

    /// Whether the selection `epoch` is still the one selected, and the PC hasn't removed this device.
    fn selection_current(&self, s: &Inner<E::Link>, epoch: u64) -> bool {
        epoch == s.selection_epoch && *s.status() != Connection::Removed
    }
}

/// What a native Save captured when it started (`AttachmentSaveGuard`).
pub struct SaveGuard<E: Environment> {
    connection: Arc<AppConnection<E>>,
    epoch: u64,
}

impl<E: Environment> Clone for SaveGuard<E> {
    fn clone(&self) -> Self {
        Self {
            connection: Arc::clone(&self.connection),
            epoch: self.epoch,
        }
    }
}

impl<E: Environment> SaveGuard<E> {
    /// Whether the PC selected when the Save began is still the selected one.
    pub fn current(&self) -> bool {
        let s = self.connection.lock();
        self.connection.selection_current(&s, self.epoch)
    }

    /// Called when the picker answers: the connection it must still be on to write, or `None` if it already changed.
    pub fn activate(&self) -> Option<SaveActivation<E>> {
        let s = self.connection.lock();
        if !self.connection.selection_current(&s, self.epoch) || *s.status() != Connection::Ready {
            return None;
        }
        let ready = s.ready_id()?;
        Some(SaveActivation {
            guard: self.clone(),
            ready,
        })
    }
}

/// A Save that has been activated: still current while the connection it was activated on is.
pub struct SaveActivation<E: Environment> {
    guard: SaveGuard<E>,
    ready: u64,
}

impl<E: Environment> SaveActivation<E> {
    pub fn current(&self) -> bool {
        let s = self.guard.connection.lock();
        self.guard
            .connection
            .selection_current(&s, self.guard.epoch)
            && s.ready_id() == Some(self.ready)
            && *s.status() == Connection::Ready
    }
}

/// A download's hold on the connection (`captureAttachmentDownload`'s result).
pub struct AttachmentDownload<E: Environment> {
    guard: SaveGuard<E>,
    intent: u64,
    abort: Signal,
    client: Daemon<E::Link>,
}

impl<E: Environment> AttachmentDownload<E> {
    /// The daemon the download started on.
    pub fn client(&self) -> &Daemon<E::Link> {
        &self.client
    }

    /// Ends the capture; a change of PC no longer aborts the download. Dropping does the same.
    pub fn close(&self) {
        self.guard.connection.lock().downloads.remove(&self.intent);
    }

    /// Called as the transfer opens on `ready`: a transfer on a connection that was replaced, or whose authorization is
    /// gone, is refused (and aborted). From then on the PC's link ending aborts the download.
    pub fn transferring(&self, ready: &Daemon<E::Link>) -> Result<(), Cancelled> {
        let connection = &self.guard.connection;
        let mut s = connection.lock();
        let live = !self.abort.aborted()
            && connection.selection_current(&s, self.guard.epoch)
            && s.ready_id() == Some(ready.id)
            && *s.status() == Connection::Ready;
        let Some(raw) = s.client.as_ref().map(|current| current.raw.clone()) else {
            self.abort.abort();
            return Err(Cancelled);
        };
        if !live {
            self.abort.abort();
            return Err(Cancelled);
        }
        if let Some(intent) = s.downloads.get_mut(&self.intent) {
            intent.transferring = true;
        }
        let this = Arc::clone(connection);
        let intent = self.intent;
        let abort = self.abort.clone();
        connection.env.spawn(Box::pin(async move {
            raw.closed().await;
            if this.lock().downloads.contains_key(&intent) {
                abort.abort();
            }
        }));
        Ok(())
    }

    /// Once the picker has answered: the live connection, when the PC's file host answers (a PC's link is pinged first; a
    /// reconnect is started if it doesn't). Waits at most 30 seconds the app is in front. Cancelled when `signal` is
    /// aborted or what authorized the download is gone.
    pub async fn reacquire(&self, signal: &Signal) -> Result<Daemon<E::Link>, ReacquireError> {
        let connection: &AppConnection<E> = &self.guard.connection;
        let listener = Arc::new(Notify::new());
        let _registered = {
            let mut s = connection.lock();
            let id = s.next_id();
            s.reacquiring.insert(id, Arc::clone(&listener));
            ListenerGuard { connection, id }
        };
        let mut remaining = REACQUIRE_WAIT;
        let mut previous = connection.env.now();
        let mut was_visible = connection.lock().visible;
        let mut probe: Option<BoxFuture<'_, bool>> = None;
        let mut probing: Option<(u64, Daemon<E::Link>)> = None;
        loop {
            // A check: time counts only while the app is in front.
            let now = connection.env.now();
            if was_visible {
                remaining = remaining.saturating_sub(now.saturating_duration_since(previous));
            }
            previous = now;
            let (visible, ready) = {
                let s = connection.lock();
                let ready = match &s.client {
                    Some(ready) if *s.status() == Connection::Ready => Some((
                        ready.id,
                        ready.daemon.clone(),
                        ready.raw.clone(),
                        s.is_remote(),
                    )),
                    _ => None,
                };
                (s.visible, ready)
            };
            was_visible = visible;
            if signal.aborted() || !self.guard.current() {
                return Err(ReacquireError::Cancelled);
            }
            if remaining.is_zero() {
                return Err(ReacquireError::Gone);
            }
            if visible
                && probe.is_none()
                && let Some((id, daemon, raw, remote)) = ready
            {
                probing = Some((id, daemon));
                probe = Some(
                    async move {
                        // A daemon on this PC needs no ping; another PC's file host has to answer.
                        !remote || connection.probe_link(&raw, Some(signal)).await
                    }
                    .boxed(),
                );
            }
            let tick = remaining.min(REACQUIRE_TICK);
            let finished = async {
                match probe.as_mut() {
                    Some(probe) => probe.await,
                    None => std::future::pending::<bool>().await,
                }
            };
            tokio::select! {
                biased;
                () = signal.cancelled() => return Err(ReacquireError::Cancelled),
                answered = finished => {
                    probe = None;
                    let Some((id, daemon)) = probing.take() else { continue };
                    let (same, visible_now) = {
                        let s = connection.lock();
                        (s.ready_id() == Some(id), s.visible)
                    };
                    if !signal.aborted() && self.guard.current() && answered && same && visible_now {
                        return Ok(daemon);
                    }
                    if !answered && same {
                        let mut s = connection.lock();
                        self.guard.connection.reconnect_now(&mut s);
                    }
                }
                () = listener.notified() => {}
                () = connection.sleep(tick), if visible => {}
            }
        }
    }
}

impl<E: Environment> Drop for AttachmentDownload<E> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use protocol::ErrorCode;
    use protocol::model::{Bot, Message};
    use protocol::relay::PushDataVersion;
    use protocol::remote::PushKind;
    use protocol::wire::ChatSend;
    use serde_json::{Value, json};

    use super::*;

    const HOST: &str = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11";
    const BOT: &str = "0d6ff97e-a44e-42f4-86a0-27ace431918d";
    const CAN_NOT_REACH: &str = "Can't reach your PC. Keep both devices on the same Wi-Fi, and keep Tjommie open on the PC.";
    const PC_OFFLINE: &str = "Your PC is offline. Check that it's awake with Tjommie running.";

    // The fake PC (the TypeScript's `remote-pc-fixture`): it answers the calls the connection makes, and can hold calls,
    // drop its wires or let them die (a socket that looks open and carries nothing).

    /// What a connection to the PC does. The LAN/relay race is the dialer's (`desktop-host`), so a leg is just the outcome.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum Reach {
        #[default]
        Open,
        /// Nothing answers at its addresses.
        Refused,
        /// The relay says the PC isn't connected.
        Offline,
        /// The PC says this device was removed.
        Revoked,
        /// The PC speaks another protocol version.
        Update,
    }

    struct WireState {
        closed: watch::Sender<Option<CloseReason>>,
        dead: AtomicBool,
        events: broadcast::Sender<Event>,
    }

    impl WireState {
        fn new(capacity: usize) -> Arc<Self> {
            Arc::new(Self {
                closed: watch::channel(None).0,
                dead: AtomicBool::new(false),
                events: broadcast::channel(capacity).0,
            })
        }

        fn end(&self, code: u16, reason: &str) {
            self.closed.send_if_modified(|current| {
                if current.is_some() {
                    return false;
                }
                *current = Some(CloseReason {
                    code,
                    reason: reason.to_owned(),
                    opened: true,
                });
                true
            });
        }

        /// The PC dropped the connection.
        fn drop_wire(&self) {
            self.end(1006, "");
        }

        /// The socket looks open but carries nothing.
        fn die(&self) {
            self.dead.store(true, Ordering::SeqCst);
        }

        fn is_closed(&self) -> bool {
            self.closed.borrow().is_some()
        }
    }

    async fn wait_closed(closed: &mut watch::Receiver<Option<CloseReason>>) -> CloseReason {
        loop {
            if let Some(reason) = closed.borrow_and_update().clone() {
                return reason;
            }
            if closed.changed().await.is_err() {
                return CloseReason {
                    code: 1006,
                    reason: "The fake wire is gone".to_owned(),
                    opened: true,
                };
            }
        }
    }

    #[derive(Default)]
    struct PcState {
        calls: Vec<&'static str>,
        held: HashSet<&'static str>,
        waiting: Vec<(&'static str, oneshot::Sender<()>)>,
        peak: HashMap<&'static str, usize>,
        opens: usize,
        wires: Vec<Arc<WireState>>,
        bots: Vec<Bot>,
        messages: HashMap<String, Vec<Message>>,
        failing: HashMap<&'static str, RpcError>,
        reach: Reach,
    }

    #[derive(Default)]
    struct Pc {
        state: Mutex<PcState>,
    }

    impl Pc {
        fn count(&self, method: &str) -> usize {
            locked(&self.state)
                .calls
                .iter()
                .filter(|called| **called == method)
                .count()
        }

        fn hold(&self, method: &'static str) {
            locked(&self.state).held.insert(method);
        }

        /// Answers the calls that wait now, and keeps holding the next ones.
        fn release(&self, method: &'static str) {
            let mut state = locked(&self.state);
            let mut index = 0;
            while index < state.waiting.len() {
                if state
                    .waiting
                    .get(index)
                    .is_some_and(|(name, _)| *name == method)
                {
                    let (_, release) = state.waiting.remove(index);
                    let _ = release.send(());
                } else {
                    index += 1;
                }
            }
        }

        /// Stops holding the method, and answers the calls that wait.
        fn unhold(&self, method: &'static str) {
            locked(&self.state).held.remove(method);
            self.release(method);
        }

        fn waiting(&self, method: &str) -> usize {
            locked(&self.state)
                .waiting
                .iter()
                .filter(|(name, release)| *name == method && !release.is_closed())
                .count()
        }

        fn peak(&self, method: &str) -> usize {
            locked(&self.state).peak.get(method).copied().unwrap_or(0)
        }

        fn opens(&self) -> usize {
            locked(&self.state).opens
        }

        fn wire(&self, index: usize) -> Arc<WireState> {
            Arc::clone(
                locked(&self.state)
                    .wires
                    .get(index)
                    .expect("that wire was opened"),
            )
        }

        fn last_wire(&self) -> Arc<WireState> {
            Arc::clone(locked(&self.state).wires.last().expect("a wire was opened"))
        }

        fn set_reach(&self, reach: Reach) {
            locked(&self.state).reach = reach;
        }

        fn set_bots(&self, bots: Vec<Bot>) {
            locked(&self.state).bots = bots;
        }

        fn add(&self, message: Message) {
            locked(&self.state)
                .messages
                .entry(message.conversation_id.clone())
                .or_default()
                .push(message);
        }

        fn fail(&self, method: &'static str, error: RpcError) {
            locked(&self.state).failing.insert(method, error);
        }

        /// The six startup lists fail with `error` (`None`: they answer).
        fn fail_lists(&self, error: Option<RpcError>) {
            for method in [
                "bots.list",
                "settings.get",
                "routines.list",
                "rooms.list",
                "accounts.list",
                "notifications.pending",
            ] {
                match &error {
                    Some(error) => self.fail(method, error.clone()),
                    None => {
                        locked(&self.state).failing.remove(method);
                    }
                }
            }
        }

        /// Pushes an event to the newest wire.
        fn emit(&self, event: Event) {
            let _ = self.last_wire().events.send(event);
        }

        fn open_wire(self: &Arc<Self>, capacity: usize) -> FakeLink {
            let wire = WireState::new(capacity);
            locked(&self.state).wires.push(Arc::clone(&wire));
            FakeLink {
                pc: Arc::clone(self),
                wire,
            }
        }

        fn respond(&self, name: &'static str, params: &Value) -> Result<Value, RpcError> {
            let state = locked(&self.state);
            if let Some(error) = state.failing.get(name) {
                return Err(error.clone());
            }
            Ok(match name {
                "bots.list" => json!(state.bots),
                "settings.get" => json!({ "chainLimit": 5, "relayUrl": null }),
                "routines.list"
                | "rooms.list"
                | "accounts.list"
                | "notifications.pending"
                | "setup.status" => json!([]),
                "remote.ping" | "device.registerPush" => json!({}),
                "exchanges.get" => json!({
                    "messages": [message("e1", "x1")], "hasMore": true,
                    "exchange": {
                        "id": "x1", "fromBotId": "b1", "toBotId": "b2", "request": "Check the build",
                        "reply": null, "status": "waiting", "createdAt": "2026-10-25T09:00:00.000Z",
                        "repliedAt": null
                    }
                }),
                "chat.history" => page(&state.messages, params, "botId"),
                "room.history" => page(&state.messages, params, "roomId"),
                "chat.send" => json!(message(
                    "sent",
                    params.get("botId").and_then(Value::as_str).unwrap_or("b1")
                )),
                _ => {
                    return Err(RpcError::Daemon {
                        code: ErrorCode::UnknownMethod,
                        message: format!("The fake PC has no answer for {name}"),
                    });
                }
            })
        }
    }

    /// A page of a conversation, newest last: the `limit` messages before `before`, or the newest.
    fn page(messages: &HashMap<String, Vec<Message>>, params: &Value, conversation: &str) -> Value {
        let all = params
            .get(conversation)
            .and_then(Value::as_str)
            .and_then(|id| messages.get(id))
            .cloned()
            .unwrap_or_default();
        let end = params
            .get("before")
            .and_then(Value::as_str)
            .and_then(|before| all.iter().position(|message| message.id == before))
            .unwrap_or(all.len());
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
        let start = end.saturating_sub(limit);
        json!({ "messages": &all[start..end], "hasMore": start > 0 })
    }

    fn message(id: &str, conversation: &str) -> Message {
        serde_json::from_value(json!({
            "id": id, "conversationId": conversation, "author": conversation, "kind": "text", "body": id,
            "data": null, "status": "done", "createdAt": "2026-10-25T09:00:00.000Z"
        }))
        .expect("a message")
    }

    fn bot(id: &str) -> Bot {
        serde_json::from_value(json!({
            "id": id, "name": id, "color": "blue", "shape": "circle", "instructions": "", "provider": "claude",
            "model": "sonnet", "autonomy": "folder", "homeFolder": format!("C:\\bots\\{id}"), "projectFolder": null,
            "browser": false, "paused": false, "canMessage": [], "state": "idle", "setupReason": null,
            "limitedUntil": null, "createdAt": "2026-10-25T08:00:00.000Z"
        }))
        .expect("a bot")
    }

    #[derive(Clone)]
    struct FakeLink {
        pc: Arc<Pc>,
        wire: Arc<WireState>,
    }

    impl FakeLink {
        async fn answer(&self, name: &'static str, params: Value) -> Result<Value, RpcError> {
            let gate = {
                let mut state = locked(&self.pc.state);
                state.calls.push(name);
                if state.held.contains(name) {
                    let (release, gate) = oneshot::channel();
                    state.waiting.push((name, release));
                    let waiting = state
                        .waiting
                        .iter()
                        .filter(|(held, release)| *held == name && !release.is_closed())
                        .count();
                    let peak = state.peak.entry(name).or_default();
                    *peak = (*peak).max(waiting);
                    Some(gate)
                } else {
                    None
                }
            };
            if self.wire.dead.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            if self.wire.dead.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            self.pc.respond(name, &params)
        }
    }

    impl Rpc for FakeLink {
        fn call<M: ClientMethod>(
            &self,
            params: M::Params,
        ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
            let link = self.clone();
            let params = serde_json::to_value(params)
                .map_err(|error| RpcError::BadParams(error.to_string()));
            async move {
                let params = params?;
                let mut closed = link.wire.closed.subscribe();
                let value = tokio::select! {
                    biased;
                    reason = wait_closed(&mut closed) => return Err(RpcError::Closed(reason)),
                    value = link.answer(M::NAME, params) => value?,
                };
                serde_json::from_value(value)
                    .map_err(|error| RpcError::BadResult(error.to_string()))
            }
        }
    }

    impl DaemonLink for FakeLink {
        fn events(&self) -> broadcast::Receiver<Event> {
            self.wire.events.subscribe()
        }

        fn closed(&self) -> impl Future<Output = CloseReason> + Send + 'static {
            let mut closed = self.wire.closed.subscribe();
            async move { wait_closed(&mut closed).await }
        }

        fn close(&self) -> impl Future<Output = ()> + Send {
            self.wire.end(1000, "Client closed");
            async {}
        }
    }

    /// A cleanup the test lets finish.
    struct Gate(watch::Sender<bool>);

    impl Gate {
        fn open(&self) {
            self.0.send_replace(true);
        }
    }

    enum LocalStep {
        Fail(Failure),
        /// The daemon answers when the test says so.
        Wait(oneshot::Receiver<()>),
    }

    #[derive(Default)]
    struct PushLog {
        stopped: usize,
        registered: usize,
        unregistered: usize,
    }

    struct EnvState {
        android: bool,
        clock: Mutex<Instant>,
        pc: Arc<Pc>,
        hosts: Mutex<Vec<SavedPc>>,
        keep_alive: AtomicBool,
        event_capacity: AtomicUsize,
        local_token: Mutex<String>,
        local_plan: Mutex<VecDeque<LocalStep>>,
        local_connects: AtomicUsize,
        restarts: AtomicUsize,
        forget_failures: Mutex<VecDeque<String>>,
        forgotten: Mutex<Vec<String>>,
        scopes: Mutex<Vec<FilesScope>>,
        gates_once: Mutex<VecDeque<watch::Receiver<bool>>>,
        gate_always: Mutex<Option<watch::Receiver<bool>>>,
        syncs: Mutex<Vec<(Connection, Option<String>)>>,
        pushes: Mutex<PushLog>,
    }

    #[derive(Clone)]
    struct FakeEnv(Arc<EnvState>);

    impl FakeEnv {
        fn new(android: bool, hosts: Vec<SavedPc>) -> Self {
            Self(Arc::new(EnvState {
                android,
                clock: Mutex::new(Instant::now()),
                pc: Arc::new(Pc::default()),
                hosts: Mutex::new(hosts),
                keep_alive: AtomicBool::new(false),
                event_capacity: AtomicUsize::new(64),
                local_token: Mutex::new("first".to_owned()),
                local_plan: Mutex::new(VecDeque::new()),
                local_connects: AtomicUsize::new(0),
                restarts: AtomicUsize::new(0),
                forget_failures: Mutex::new(VecDeque::new()),
                forgotten: Mutex::new(Vec::new()),
                scopes: Mutex::new(Vec::new()),
                gates_once: Mutex::new(VecDeque::new()),
                gate_always: Mutex::new(None),
                syncs: Mutex::new(Vec::new()),
                pushes: Mutex::new(PushLog::default()),
            }))
        }

        fn set_now(&self, at: Instant) {
            *locked(&self.0.clock) = at;
        }

        fn scopes(&self) -> Vec<FilesScope> {
            locked(&self.0.scopes).clone()
        }

        fn reset_scopes(&self) {
            locked(&self.0.scopes).clear();
        }

        /// Every file cleanup from now on waits for the gate to open.
        fn gate_every_cleanup(&self) -> Gate {
            let (gate, open) = watch::channel(false);
            *locked(&self.0.gate_always) = Some(open);
            Gate(gate)
        }

        /// The next file cleanup waits for the gate to open.
        fn gate_next_cleanup(&self) -> Gate {
            let (gate, open) = watch::channel(false);
            locked(&self.0.gates_once).push_back(open);
            Gate(gate)
        }

        fn local_connects(&self) -> usize {
            self.0.local_connects.load(Ordering::SeqCst)
        }

        fn syncs(&self) -> Vec<Connection> {
            locked(&self.0.syncs)
                .iter()
                .map(|(status, _)| status.clone())
                .collect()
        }
    }

    impl Environment for FakeEnv {
        type Link = FakeLink;

        fn is_android(&self) -> bool {
            self.0.android
        }

        fn now(&self) -> Instant {
            *locked(&self.0.clock)
        }

        fn spawn(&self, task: BoxFuture<'static, ()>) {
            tokio::spawn(task);
        }

        fn local_daemon(&self) -> impl Future<Output = Result<DesktopConnection, String>> + Send {
            let token = locked(&self.0.local_token).clone();
            async move { Ok(DesktopConnection { port: 47100, token }) }
        }

        fn restart_daemon(&self) {
            self.0.restarts.fetch_add(1, Ordering::SeqCst);
        }

        fn connect_local(
            &self,
            _connection: &DesktopConnection,
        ) -> impl Future<Output = Result<FakeLink, Failure>> + Send {
            self.0.local_connects.fetch_add(1, Ordering::SeqCst);
            let step = locked(&self.0.local_plan).pop_front();
            let pc = Arc::clone(&self.0.pc);
            let capacity = self.0.event_capacity.load(Ordering::SeqCst);
            async move {
                match step {
                    Some(LocalStep::Fail(failure)) => Err(failure),
                    Some(LocalStep::Wait(open)) => {
                        let _ = open.await;
                        Ok(pc.open_wire(capacity))
                    }
                    None => Ok(pc.open_wire(capacity)),
                }
            }
        }

        fn saved_hosts(&self) -> Vec<SavedPc> {
            locked(&self.0.hosts).clone()
        }

        fn open_remote(&self, id: &str) -> impl Future<Output = Result<FakeLink, Failure>> + Send {
            let known = locked(&self.0.hosts).iter().any(|host| host.id == id);
            let pc = Arc::clone(&self.0.pc);
            let capacity = self.0.event_capacity.load(Ordering::SeqCst);
            async move {
                let reach = {
                    let mut state = locked(&pc.state);
                    state.opens += 1;
                    state.reach
                };
                if !known {
                    return Err(Failure::Other(
                        "Pair this device with your PC first.".to_owned(),
                    ));
                }
                match reach {
                    Reach::Open => Ok(pc.open_wire(capacity)),
                    Reach::Refused => Err(Failure::Other(CAN_NOT_REACH.to_owned())),
                    Reach::Offline => Err(Failure::HostOffline(PC_OFFLINE.to_owned())),
                    Reach::Revoked => Err(Failure::Refused),
                    Reach::Update => Err(Failure::Update),
                }
            }
        }

        fn forget_host(&self, id: &str) -> impl Future<Output = Result<(), String>> + Send {
            let failure = locked(&self.0.forget_failures).pop_front();
            if failure.is_none() {
                locked(&self.0.hosts).retain(|host| host.id != id);
                locked(&self.0.forgotten).push(id.to_owned());
            }
            async move { failure.map_or(Ok(()), Err) }
        }

        fn invalidate_files(
            &self,
            scope: FilesScope,
        ) -> impl Future<Output = Result<(), String>> + Send + 'static {
            locked(&self.0.scopes).push(scope);
            let gate = locked(&self.0.gates_once)
                .pop_front()
                .or_else(|| locked(&self.0.gate_always).clone());
            async move {
                if let Some(mut gate) = gate {
                    while !*gate.borrow_and_update() {
                        if gate.changed().await.is_err() {
                            break;
                        }
                    }
                }
                Ok(())
            }
        }

        fn keep_alive_running(&self) -> bool {
            self.0.keep_alive.load(Ordering::SeqCst)
        }

        fn sync_keep_alive(&self, status: &Connection, host_name: Option<&str>) {
            locked(&self.0.syncs).push((status.clone(), host_name.map(str::to_owned)));
        }

        fn register_push(
            &self,
            _link: &FakeLink,
        ) -> impl Future<Output = Result<(), String>> + Send {
            locked(&self.0.pushes).registered += 1;
            async { Ok(()) }
        }

        fn stop_sending_push(&self) {
            locked(&self.0.pushes).stopped += 1;
        }

        fn unregister_push(&self) -> impl Future<Output = ()> + Send {
            locked(&self.0.pushes).unregistered += 1;
            async {}
        }
    }

    fn status_name(status: &Connection) -> &'static str {
        match status {
            Connection::Connecting => "connecting",
            Connection::Ready => "ready",
            Connection::Reconnecting => "reconnecting",
            Connection::Offline { .. } => "offline",
            Connection::Removed => "removed",
            Connection::Update => "update",
            Connection::Failed { .. } => "failed",
            Connection::Outside => "outside",
        }
    }

    struct World {
        env: FakeEnv,
        conn: Arc<AppConnection<FakeEnv>>,
        /// The connection status after every change of state, from the start.
        statuses: Arc<Mutex<Vec<&'static str>>>,
    }

    /// A phone paired with the synthetic PC (the TypeScript's `boot`), or the desktop window (`android: false`).
    fn world(android: bool) -> World {
        let name = if android { "Synthetic PC" } else { "Home PC" };
        let env = FakeEnv::new(
            android,
            vec![SavedPc {
                id: HOST.to_owned(),
                name: name.to_owned(),
            }],
        );
        let conn = AppConnection::new(env.clone());
        let statuses = Arc::new(Mutex::new(vec!["connecting"]));
        conn.subscribe({
            let statuses = Arc::clone(&statuses);
            move |state| locked(&statuses).push(status_name(&state.connection))
        });
        World {
            env,
            conn,
            statuses,
        }
    }

    fn phone() -> World {
        world(true)
    }

    fn desktop() -> World {
        world(false)
    }

    async fn started_phone() -> World {
        let w = phone();
        w.start().await;
        w
    }

    impl World {
        fn pc(&self) -> &Arc<Pc> {
            &self.env.0.pc
        }

        fn status(&self) -> Connection {
            self.conn.read(|state| state.connection.clone())
        }

        /// The statuses the connection went through, each once in a row.
        fn changes(&self) -> Vec<&'static str> {
            let mut changes: Vec<&'static str> = Vec::new();
            for status in locked(&self.statuses).iter() {
                if changes.last() != Some(status) {
                    changes.push(status);
                }
            }
            changes
        }

        fn show(&self, visible: bool) {
            self.conn.on_visibility(visible);
        }

        /// Fires the timers that are due now.
        fn fire_due(&self) {
            while let Some(due) = self.conn.next_timer()
                && due <= self.env.now()
            {
                self.conn.on_timer(due);
            }
        }

        async fn turns(&self) {
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        }

        /// The TypeScript's `settle`: the work that is ready runs, and the timers that are due fire.
        async fn settle(&self) {
            for _ in 0..10 {
                self.fire_due();
                self.turns().await;
            }
        }

        /// `vi.advanceTimersByTimeAsync(ms)`: time moves to each timer in turn, and the work it starts runs.
        async fn advance(&self, ms: u64) {
            let target = self.env.now() + Duration::from_millis(ms);
            self.settle().await;
            while let Some(due) = self.conn.next_timer() {
                if due > target {
                    break;
                }
                if due > self.env.now() {
                    self.env.set_now(due);
                }
                self.conn.on_timer(due);
                self.turns().await;
            }
            self.env.set_now(target);
            self.settle().await;
        }

        async fn start(&self) {
            self.conn.start_connection();
            self.settle().await;
        }

        fn bot_ids(&self) -> Vec<String> {
            self.conn
                .read(|state| state.bots.iter().map(|bot| bot.id.clone()).collect())
        }

        fn conversation(&self, id: &str) -> Option<super::super::state::Conversation> {
            self.conn.read(|state| state.conversations.get(id).cloned())
        }
    }

    fn locked_database() -> RpcError {
        RpcError::Daemon {
            code: ErrorCode::Internal,
            message: "The database is locked.".to_owned(),
        }
    }

    // connection.test.ts

    #[tokio::test]
    async fn rejects_a_call_made_while_tjommie_isnt_connected_instead_of_throwing() {
        let w = desktop();
        let error = w
            .conn
            .request::<method::BotsList>(Empty {})
            .await
            .expect_err("not connected");
        assert_eq!(error, RequestError::NotConnected);
        assert_eq!(error.to_string(), "Tjommie isn't connected right now.");
    }

    #[tokio::test]
    async fn rejects_a_chats_history_while_tjommie_isnt_connected_as_the_rooms_does() {
        let w = desktop();
        let chat = w.conn.bot_history("b1");
        let room = w.conn.room_history("team");
        assert_eq!(
            chat(None).await.expect_err("not connected").to_string(),
            "Tjommie isn't connected right now."
        );
        assert_eq!(
            room(None).await.expect_err("not connected").to_string(),
            "Tjommie isn't connected right now."
        );
    }

    #[tokio::test]
    async fn loads_a_chat_whose_history_failed_to_load_again_on_retry() {
        let w = desktop();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetch: FetchPage = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_| {
                let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                async move {
                    if first {
                        Err(RequestError::Rpc(RpcError::BadResult(
                            "History unavailable".to_owned(),
                        )))
                    } else {
                        Ok(Page {
                            messages: Vec::new(),
                            has_more: true,
                        })
                    }
                }
                .boxed()
            })
        };
        w.conn.load_conversation("b1", Arc::clone(&fetch)).await;
        assert_eq!(
            w.conversation("b1").map(|conversation| conversation.status),
            Some(ConversationStatus::Failed)
        );
        w.conn.retry_conversation("b1", Arc::clone(&fetch)).await;
        let loaded = w.conversation("b1").expect("the chat is loaded");
        assert_eq!(loaded.status, ConversationStatus::Ready);
        assert!(loaded.has_more);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn forgets_a_closed_exchange_so_its_messages_stop_arriving_and_a_reopen_loads_it_again() {
        let w = desktop();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetch: FetchPage = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Page {
                        messages: Vec::new(),
                        has_more: false,
                    })
                }
                .boxed()
            })
        };
        w.conn.load_conversation("x1", Arc::clone(&fetch)).await;
        w.conn.close_conversation("x1");
        assert_eq!(w.conversation("x1"), None);
        let reply = Event::MessageAdded(message("m1", "x1"));
        w.conn.update(|state| apply_event(state, &reply));
        assert_eq!(w.conversation("x1"), None);
        w.conn.load_conversation("x1", Arc::clone(&fetch)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    // connection-start.test.ts

    #[tokio::test]
    async fn says_why_after_a_few_failed_tries_and_keeps_trying_until_the_daemon_answers() {
        let w = desktop();
        w.pc().fail_lists(Some(locked_database()));
        w.conn.start_connection();
        w.advance(0).await;
        assert_eq!(w.status(), Connection::Connecting);
        w.advance(2_000).await;
        assert_eq!(w.status(), Connection::Connecting);
        w.advance(2_000).await;
        assert_eq!(w.env.local_connects(), 3);
        assert_eq!(
            w.status(),
            Connection::Failed {
                reason: "The database is locked.".to_owned()
            }
        );

        w.pc().fail_lists(None);
        w.advance(2_000).await;
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn says_in_plain_words_when_the_daemon_cant_be_reached() {
        let w = desktop();
        for _ in 0..3 {
            locked(&w.env.0.local_plan).push_back(LocalStep::Fail(Failure::Unreachable(
                "WebSocket closed (code 1006, reason: none)".to_owned(),
            )));
        }
        w.conn.start_connection();
        w.advance(4_000).await;
        assert_eq!(w.env.local_connects(), 3);
        assert_eq!(
            w.status(),
            Connection::Failed {
                reason: "Tjommie couldn't reach its background service.".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn shows_reconnecting_while_it_replaces_a_live_one() {
        let w = desktop();
        w.conn.start_connection();
        w.advance(0).await;
        assert_eq!(w.status(), Connection::Ready);
        let first = w.pc().wire(0);

        let (open, opened) = oneshot::channel();
        locked(&w.env.0.local_plan).push_back(LocalStep::Wait(opened));
        w.conn.on_connection(DesktopConnection {
            port: 47101,
            token: "second".to_owned(),
        });
        w.advance(0).await;
        assert_eq!(w.status(), Connection::Reconnecting);
        assert!(first.is_closed());

        let _ = open.send(());
        w.advance(0).await;
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn keeps_at_most_16_calls_outstanding_so_a_burst_cant_trip_the_phone_hosts_limit_of_32_and_answers_every_call()
     {
        let w = desktop();
        // A room's history waits until the test lets it answer; the startup lists answer at once.
        w.pc().hold("room.history");
        w.conn.start_connection();
        w.advance(0).await;
        assert_eq!(w.status(), Connection::Ready);

        let answered = Arc::new(AtomicUsize::new(0));
        let burst: Vec<_> = (0..40)
            .map(|_| {
                let conn = Arc::clone(&w.conn);
                let answered = Arc::clone(&answered);
                tokio::spawn(async move {
                    conn.request::<method::RoomHistory>(RoomHistory {
                        before: None,
                        limit: 10,
                        room_id: "team".to_owned(),
                    })
                    .await
                    .expect("answered");
                    answered.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        while answered.load(Ordering::SeqCst) < 40 {
            w.settle().await;
            assert!(w.pc().waiting("room.history") <= 16);
            w.pc().release("room.history");
        }
        for call in burst {
            call.await.expect("the call finished");
        }
        assert_eq!(w.pc().peak("room.history"), 16);
    }

    // connection.heartbeat.test.ts

    #[tokio::test]
    async fn pings_every_20_s_while_in_front_and_replaces_a_link_whose_ping_gets_no_reply_within_10_s()
     {
        let w = started_phone().await;
        assert_eq!(w.status(), Connection::Ready);
        w.advance(20_000).await;
        assert_eq!(w.pc().count("remote.ping"), 1); // answered: the link stays
        assert_eq!(w.pc().opens(), 1);

        w.pc().wire(0).die();
        w.advance(20_000).await; // the next ping goes out into the dead socket
        w.advance(9_999).await;
        assert_eq!(w.pc().opens(), 1);
        w.advance(1).await;
        w.settle().await;
        assert_eq!(w.pc().opens(), 2);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn opens_a_new_wire_the_moment_the_network_changes() {
        let w = started_phone().await;
        w.conn.network_changed();
        w.settle().await;
        assert_eq!(w.pc().opens(), 2);
        assert!(w.pc().wire(0).is_closed());
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn rejects_a_chat_send_that_was_in_flight_when_the_link_was_lost_and_never_sends_it_again()
     {
        let w = started_phone().await;
        w.pc().hold("chat.send");
        let outcome = tokio::spawn({
            let conn = Arc::clone(&w.conn);
            async move {
                conn.request::<method::ChatSend>(ChatSend {
                    bot_id: "b1".to_owned(),
                    text: "Ship it".to_owned(),
                    files: Vec::new(),
                })
                .await
                .map_or("rejected", |_| "sent")
            }
        });
        w.settle().await;
        assert_eq!(w.pc().count("chat.send"), 1);

        w.pc().wire(0).die(); // Wi-Fi to mobile data: the old socket is a zombie
        w.conn.network_changed();
        assert_eq!(outcome.await.expect("the request finished"), "rejected");
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.pc().count("chat.send"), 1);
    }

    #[tokio::test]
    async fn stops_heartbeat_while_hidden_and_checks_the_link_on_return_without_a_foreground_service()
     {
        let w = started_phone().await;
        w.show(false);
        w.advance(120_000).await;
        assert_eq!(w.pc().count("remote.ping"), 0);
        assert_eq!(w.pc().opens(), 1);
        w.show(true);
        w.settle().await;
        assert_eq!(w.pc().opens(), 1);
        assert_eq!(w.pc().count("remote.ping"), 1);
    }

    // connection.resume.test.ts

    #[tokio::test]
    async fn locked_30_min_then_visible_the_dead_link_is_replaced_after_a_bounded_ping_without_a_failed_screen_and_the_open_chat_catches_up()
     {
        let w = phone();
        for at in 1..=60 {
            w.pc().add(message(&format!("m{at:02}"), "b1"));
        }
        w.start().await;
        assert_eq!(w.status(), Connection::Ready);
        w.conn
            .load_conversation("b1", w.conn.bot_history("b1"))
            .await;
        w.conn
            .load_older("b1", w.conn.bot_history("b1"))
            .await
            .expect("the older page");
        // All 60 loaded: the newest page and the one before it.
        let loaded = w.conversation("b1").expect("the chat is loaded");
        assert_eq!(loaded.status, ConversationStatus::Ready);
        assert!(!loaded.has_more);

        w.show(false);
        w.pc().wire(0).die(); // Doze froze the app: the socket looks open but carries nothing
        w.pc().add(message("m61", "b1")); // and the PC moved on meanwhile
        w.advance(30 * 60_000).await;
        assert_eq!(w.pc().opens(), 1);

        w.show(true);
        w.settle().await;
        assert_eq!(w.pc().opens(), 1);
        w.advance(10_000).await;
        w.settle().await;
        assert_eq!(w.pc().opens(), 2);
        assert_eq!(w.status(), Connection::Ready);
        let chat = w.conversation("b1").expect("the chat is still loaded");
        let ids: Vec<&str> = chat
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(ids.len(), 61); // the older page stayed, and the new message is there
        assert_eq!(ids.get(..2), Some(["m01", "m02"].as_slice()));
        assert_eq!(ids.last(), Some(&"m61"));
        assert!(!chat.has_more);
        assert_eq!(
            w.changes(),
            ["connecting", "ready", "reconnecting", "ready"]
        );
    }

    #[tokio::test]
    async fn keeps_the_link_through_a_quick_trip_away_checking_it_with_a_ping_instead_of_reconnecting()
     {
        let w = started_phone().await;
        w.show(false);
        w.advance(5_000).await;
        w.show(true);
        w.settle().await;
        assert_eq!(w.pc().opens(), 1);
        assert_eq!(w.pc().count("remote.ping"), 1);
        assert_eq!(w.changes(), ["connecting", "ready"]);
        assert_eq!(w.status(), Connection::Ready);
    }

    // The thumbnail queue's own assertions (queueThumbnail and pauseThumbnailWork) are not ported (file_thumbnails.rs says
    // why); the connection's part is that no new link opens before the file cleanup has finished.
    #[tokio::test]
    async fn admits_fresh_file_actions_after_a_disconnect_only_when_native_thumbnail_cleanup_is_confirmed()
     {
        let w = started_phone().await;
        let cleaned = w.env.gate_every_cleanup();
        w.pc().wire(0).drop_wire();
        w.settle().await;
        assert!(w.env.scopes().contains(&FilesScope::Transfers));
        // The cleanup hasn't been confirmed: the new link is not opened.
        assert_eq!(w.pc().opens(), 1);
        assert_ne!(w.status(), Connection::Ready);

        cleaned.open();
        w.advance(2_000).await;
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.pc().opens(), 2);
    }

    #[tokio::test]
    async fn keeps_authorized_native_save_data_when_returning_from_a_picker_after_two_minutes_on_a_healthy_link()
     {
        let w = started_phone().await;
        w.env.reset_scopes();
        w.show(false);
        w.advance(120_000).await;
        w.show(true);
        w.settle().await;
        assert_eq!(w.pc().opens(), 1);
        assert_eq!(w.pc().count("remote.ping"), 1);
        assert_eq!(w.env.scopes().len(), 0);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn keeps_only_a_byte_free_pending_picker_through_a_ten_minute_hidden_disconnect_and_reacquires_a_fresh_client()
     {
        let w = started_phone().await;
        let abort = Signal::new();
        let context = w
            .conn
            .capture_attachment_download(abort.clone())
            .expect("connected");
        let original = context.client().clone();
        w.show(false);
        w.pc().wire(0).drop_wire();
        w.settle().await;
        w.advance(600_000).await;
        assert!(!abort.aborted());
        assert_eq!(w.pc().count("remote.ping"), 0);

        w.show(true);
        w.settle().await;
        let (current, ()) = tokio::join!(context.reacquire(&abort), w.settle());
        let current = current.expect("the file host answered");
        assert_ne!(current, original);
        assert_eq!(current, w.conn.daemon().expect("connected"));
        context.transferring(&current).expect("still current");
        w.pc().last_wire().drop_wire();
        w.settle().await;
        assert!(abort.aborted());
        context.close();
    }

    #[tokio::test]
    async fn fences_selection_a_to_b_to_a_while_allowing_a_same_selection_reconnect() {
        let w = started_phone().await;
        let guard = w.conn.capture_attachment_save().expect("connected");
        let active = guard.activate().expect("ready");
        let abort = Signal::new();
        let context = w
            .conn
            .capture_attachment_download(abort.clone())
            .expect("connected");
        w.pc().wire(0).drop_wire();
        w.settle().await;
        assert!(guard.current());
        assert!(!active.current());
        assert!(!abort.aborted());
        w.conn.connect_remote(Some("another-pc"));
        w.conn.connect_remote(Some(HOST));
        w.settle().await;
        assert!(!guard.current());
        assert!(abort.aborted());
        context.close();
    }

    #[tokio::test]
    async fn bounds_fresh_authorization_to_thirty_visible_seconds_and_leaves_no_active_transfer() {
        let w = started_phone().await;
        let abort = Signal::new();
        let context = w
            .conn
            .capture_attachment_download(abort.clone())
            .expect("connected");
        w.pc().hold("remote.ping");
        let (outcome, ()) = tokio::join!(context.reacquire(&abort), w.advance(30_001));
        let error = outcome.expect_err("the file host never answered");
        assert_eq!(error, ReacquireError::Gone);
        assert!(error.to_string().contains("reconnect"));
        assert!(!abort.aborted());
        context.close();
    }

    #[tokio::test]
    async fn cancels_a_pending_authorization_promptly_without_a_late_ping_closing_a_healthy_link() {
        let w = started_phone().await;
        w.env.reset_scopes();
        let abort = Signal::new();
        let context = w
            .conn
            .capture_attachment_download(abort.clone())
            .expect("connected");
        w.pc().hold("remote.ping");
        let mut pending = Box::pin(context.reacquire(&abort));
        tokio::select! {
            biased;
            outcome = &mut pending => panic!("the held ping can't have answered: {outcome:?}"),
            () = w.settle() => {}
        }
        abort.abort();
        assert_eq!(pending.await, Err(ReacquireError::Cancelled));
        w.advance(11_000).await;
        assert!(!w.pc().wire(0).is_closed());
        assert_eq!(w.env.scopes().len(), 0);
        context.close();
    }

    #[tokio::test]
    async fn does_not_cancel_a_staged_native_save_when_a_foreground_ping_expires_while_the_picker_is_hidden()
     {
        let w = started_phone().await;
        w.env.reset_scopes();
        w.pc().hold("remote.ping");
        w.advance(20_000).await;
        assert_eq!(w.pc().count("remote.ping"), 1);
        w.show(false);
        w.advance(21_000).await;
        w.settle().await;
        assert_eq!(w.env.scopes().len(), 0);
        assert!(!w.pc().wire(0).is_closed());
        assert_eq!(w.status(), Connection::Ready);
        w.pc().unhold("remote.ping");
        w.show(true);
        w.settle().await;
        assert_eq!(w.pc().count("remote.ping"), 2);
        assert_eq!(w.pc().opens(), 1);
        assert_eq!(w.env.scopes().len(), 0);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn does_not_let_an_older_unanswered_ping_replace_a_link_verified_after_returning() {
        let w = started_phone().await;
        w.env.reset_scopes();
        w.pc().hold("remote.ping");
        w.advance(20_000).await;
        w.show(false);
        w.advance(1_000).await;
        w.pc().unhold("remote.ping");
        w.show(true);
        w.settle().await;
        assert_eq!(w.pc().count("remote.ping"), 2);
        w.advance(10_000).await;
        w.settle().await;
        assert_eq!(w.pc().opens(), 1);
        assert_eq!(w.env.scopes().len(), 0);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn fences_an_overdue_hidden_ping_before_the_queued_resume_check_runs() {
        let w = started_phone().await;
        w.env.reset_scopes();
        w.pc().hold("remote.ping");
        w.advance(20_000).await;
        // The ping's 10 s deadline: the only timer there is while the ping is out.
        let deadline = w.conn.next_timer().expect("the pong deadline is set");
        w.show(false);
        w.pc().unhold("remote.ping");
        w.show(true);
        w.conn.on_timer(deadline);
        w.turns().await;
        assert_eq!(w.env.scopes().len(), 0);
        assert_eq!(w.status(), Connection::Ready);
        w.settle().await;
        assert_eq!(w.pc().opens(), 1);
    }

    #[tokio::test]
    async fn host_offline_over_relay_the_relays_4404_says_the_pc_is_offline_over_the_bots_already_listed()
     {
        let w = phone();
        w.pc().set_bots(vec![bot("b1")]);
        w.start().await;
        assert_eq!(w.bot_ids(), ["b1"]);

        w.pc().set_reach(Reach::Offline);
        w.pc().wire(0).drop_wire(); // the PC went to sleep
        w.settle().await;
        w.advance(1_000).await;
        w.settle().await;
        assert_eq!(
            w.status(),
            Connection::Offline {
                reason: OfflineReason::HostOffline
            }
        );
        assert_eq!(w.bot_ids(), ["b1"]);
        assert!(!w.changes().contains(&"failed"));
    }

    #[tokio::test]
    async fn cant_reach_the_pc_a_saved_pc_that_stops_answering_shows_unreachable_over_the_cached_screens_never_the_failed_screen()
     {
        let w = started_phone().await;
        w.pc().set_reach(Reach::Refused);
        w.pc().wire(0).drop_wire();
        w.settle().await;
        assert_eq!(
            w.status(),
            Connection::Offline {
                reason: OfflineReason::Unreachable
            }
        );
        w.advance(60_000).await;
        assert!(!w.changes().contains(&"failed"));
    }

    #[tokio::test]
    async fn revoked_the_pcs_authenticated_refusal_is_terminal_removed_and_nothing_reopens() {
        let w = started_phone().await;
        w.env.reset_scopes();

        w.pc().set_reach(Reach::Revoked);
        w.pc().wire(0).drop_wire();
        w.settle().await;
        assert_eq!(w.status(), Connection::Removed);
        assert!(w.env.scopes().contains(&FilesScope::All));
        assert_eq!(w.env.syncs().last(), Some(&Connection::Removed));
        let opens = w.pc().opens();
        w.advance(60_000).await;
        w.show(false);
        w.show(true);
        w.conn.network_changed();
        w.advance(60_000).await;
        assert_eq!(w.pc().opens(), opens);
        assert_eq!(w.status(), Connection::Removed);
    }

    #[tokio::test]
    async fn backs_off_0_1_2_5_and_10_s_then_keeps_trying_every_10_s_while_the_app_is_in_front() {
        let w = started_phone().await;
        w.pc().set_reach(Reach::Refused);
        w.pc().wire(0).drop_wire();
        w.settle().await;
        assert_eq!(w.pc().opens(), 2); // straight away
        for wait in [1_000, 2_000, 5_000, 10_000, 10_000] {
            let opens = w.pc().opens();
            w.advance(wait - 1).await;
            assert_eq!(w.pc().opens(), opens);
            w.advance(1).await;
            assert_eq!(w.pc().opens(), opens + 1);
        }
    }

    #[tokio::test]
    async fn says_why_a_first_connect_failed_and_tries_again_on_return_without_taking_the_connect_screen_away()
     {
        let w = phone();
        w.pc().set_reach(Reach::Refused);
        w.conn.start_connection();
        w.settle().await;
        w.advance(1_000 + 2_000).await;
        assert_eq!(
            w.status(),
            Connection::Failed {
                reason: CAN_NOT_REACH.to_owned()
            }
        );

        w.show(false);
        w.show(true); // back from copying a pairing code, say
        w.settle().await;
        assert_eq!(w.pc().opens(), 4);
        assert_eq!(status_name(&w.status()), "failed");
        w.pc().set_reach(Reach::Open);
        w.conn.connect_remote(Some(HOST)); // Connect pressed
        assert_eq!(w.status(), Connection::Connecting);
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn stops_retrying_while_hidden_without_keep_alive_and_tries_again_the_moment_the_app_is_back()
     {
        let w = started_phone().await;
        w.env.reset_scopes();

        w.pc().set_reach(Reach::Refused);
        w.show(false);
        w.pc().wire(0).drop_wire();
        w.settle().await;
        assert!(!w.env.scopes().is_empty());
        w.advance(10 * 60_000).await;
        assert_eq!(w.pc().opens(), 1);
        assert_eq!(w.status(), Connection::Reconnecting);

        w.pc().set_reach(Reach::Open);
        w.show(true);
        w.settle().await;
        assert_eq!(w.pc().opens(), 2);
        assert_eq!(w.status(), Connection::Ready);
    }

    // connection-host.test.ts

    async fn hard_cancels_a_byte_free_picker_on(reason: &str) {
        let w = desktop();
        w.start().await;
        w.conn.connect_remote(Some(HOST));
        w.settle().await;
        let abort = Signal::new();
        let context = w
            .conn
            .capture_attachment_download(abort.clone())
            .expect("connected");
        let guard = w.conn.capture_attachment_save().expect("connected");
        if reason == "forget" {
            w.conn.forget_remote(HOST).await.expect("forgotten");
        } else {
            w.pc().set_reach(Reach::Revoked);
            w.conn.connect_remote(Some(HOST));
        }
        w.settle().await;
        assert!(abort.aborted());
        assert!(!guard.current());
        assert!(w.env.scopes().contains(&FilesScope::All));
        context.close();
    }

    #[tokio::test]
    async fn hard_cancels_a_byte_free_picker_on_forget_instead_of_rebinding_after_reconnect() {
        hard_cancels_a_byte_free_picker_on("forget").await;
    }

    #[tokio::test]
    async fn hard_cancels_a_byte_free_picker_on_refused_instead_of_rebinding_after_reconnect() {
        hard_cancels_a_byte_free_picker_on("refused").await;
    }

    #[tokio::test]
    async fn closes_previous_host_file_handles_before_a_new_host_becomes_ready() {
        let w = desktop();
        w.start().await;
        let finish = w.env.gate_next_cleanup();
        w.conn.connect_remote(Some(HOST));
        w.settle().await;
        assert_eq!(w.pc().opens(), 0);
        assert_ne!(w.status(), Connection::Ready);
        finish.open();
        w.settle().await;
        assert_eq!(w.pc().opens(), 1);
        let state = w.conn.state();
        assert_eq!(state.connection, Connection::Ready);
        assert_eq!(state.host.as_deref(), Some(HOST));
    }

    #[tokio::test]
    async fn remembers_which_saved_pc_the_desktop_window_is_on_from_the_moment_its_picked_until_this_pc_or_forget()
     {
        let w = desktop();
        w.start().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.conn.state().host, None);

        w.conn.connect_remote(Some(HOST));
        assert_eq!(w.conn.state().host.as_deref(), Some(HOST)); // at once, so the menu button names it while it connects
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.conn.state().host.as_deref(), Some(HOST));

        w.conn.connect_local();
        assert_eq!(w.conn.state().host, None);
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.conn.state().host, None);

        w.conn.connect_remote(Some(HOST));
        w.settle().await;
        w.conn.forget_remote(HOST).await.expect("forgotten");
        assert_eq!(w.conn.state().host, None);
    }

    #[tokio::test]
    async fn forgets_a_pc_that_removed_this_device_only_once_its_key_is_gone_a_failed_delete_leaves_the_removed_screen_the_pc_and_its_key_as_they_were()
     {
        let w = desktop();
        w.start().await;
        w.pc().set_reach(Reach::Revoked);
        w.conn.connect_remote(Some(HOST));
        w.settle().await;
        assert_eq!(w.status(), Connection::Removed);
        assert_eq!(w.conn.state().host.as_deref(), Some(HOST));
        w.env.0.local_connects.store(0, Ordering::SeqCst);

        locked(&w.env.0.forget_failures)
            .push_back("Windows couldn't delete this PC's key.".to_owned());
        let failure = w.conn.forget_remote(HOST).await.expect_err("the key stays");
        assert_eq!(failure, "Windows couldn't delete this PC's key.");
        w.settle().await;
        // Still on the removed screen, which says why.
        assert_eq!(w.status(), Connection::Removed);
        assert_eq!(w.conn.state().host.as_deref(), Some(HOST));
        assert_eq!(w.env.local_connects(), 0);

        // The delete works this time: this PC's own Tjommie comes back.
        w.conn.forget_remote(HOST).await.expect("forgotten");
        w.settle().await;
        assert_eq!(
            locked(&w.env.0.forgotten).last().map(String::as_str),
            Some(HOST)
        );
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.conn.state().host, None);
        assert_eq!(w.env.local_connects(), 1);
    }

    // What the TypeScript didn't decide, or didn't test.

    #[tokio::test]
    async fn a_daemon_that_speaks_another_protocol_says_update_and_is_not_retried() {
        let w = desktop();
        w.start().await;
        assert_eq!(w.status(), Connection::Ready);
        w.pc().wire(0).end(4003, "Protocol mismatch");
        w.settle().await;
        assert_eq!(w.status(), Connection::Update);
        let connects = w.env.local_connects();
        w.advance(60_000).await;
        assert_eq!(w.env.local_connects(), connects);

        // The same for an attempt the daemon refuses at once.
        let w = desktop();
        locked(&w.env.0.local_plan).push_back(LocalStep::Fail(Failure::Update));
        w.start().await;
        assert_eq!(w.status(), Connection::Update);
        w.advance(60_000).await;
        assert_eq!(w.env.local_connects(), 1);
    }

    #[tokio::test]
    async fn a_remote_link_that_ends_is_replaced_and_a_pc_on_another_protocol_says_update() {
        let w = started_phone().await;
        // The TypeScript replaces a remote link whatever the close code was.
        w.pc().wire(0).end(4003, "Protocol mismatch");
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.pc().opens(), 2);
        // What says update is the new attempt being refused for its protocol, and that is not retried.
        w.pc().set_reach(Reach::Update);
        w.pc().last_wire().drop_wire();
        w.settle().await;
        assert_eq!(w.status(), Connection::Update);
        let opens = w.pc().opens();
        w.advance(60_000).await;
        assert_eq!(w.pc().opens(), opens);
    }

    #[test]
    fn failures_read_the_close_codes_and_errors_the_typescript_told_apart() {
        let closed = |code| CloseReason {
            code,
            reason: String::new(),
            opened: true,
        };
        assert_eq!(Failure::from(&closed(4003)), Failure::Update);
        assert_eq!(
            Failure::from(&closed(1006)),
            Failure::Unreachable("WebSocket closed (code 1006, reason: none)".to_owned())
        );
        assert_eq!(
            Failure::from(&ConnectError::ProtocolMismatch),
            Failure::Update
        );
        assert_eq!(
            Failure::from(&ConnectError::Closed(closed(4003))),
            Failure::Update
        );
        assert!(matches!(
            Failure::from(&ConnectError::Unauthorized),
            Failure::Unreachable(_)
        ));
        assert_eq!(
            Failure::from(&RpcError::Closed(closed(4003))),
            Failure::Update
        );
        assert_eq!(
            Failure::from(&locked_database()),
            Failure::Other("internal: The database is locked.".to_owned())
        );
        assert_eq!(
            Failure::from(&locked_database()).to_string(),
            "The database is locked."
        );
    }

    #[test]
    fn the_desktop_connection_hides_its_token_when_printed() {
        let connection = DesktopConnection {
            port: 47100,
            token: "s3cret-token".to_owned(),
        };
        let text = format!("{connection:?}");
        assert!(text.contains("47100"));
        assert!(!text.contains("s3cret-token"));
    }

    #[tokio::test]
    async fn starting_twice_connects_once() {
        let w = desktop();
        w.conn.start_connection();
        w.conn.start_connection();
        w.settle().await;
        assert_eq!(w.env.local_connects(), 1);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn next_timer_names_the_heartbeat_and_nothing_while_hidden_or_idle() {
        let w = phone();
        assert_eq!(w.conn.next_timer(), None);
        w.start().await;
        let started = w.env.now();
        assert_eq!(
            w.conn.next_timer(),
            Some(started + Duration::from_millis(LINK.ping_foreground_ms))
        );
        w.show(false);
        assert_eq!(w.conn.next_timer(), None); // no keep-alive: a hidden app doesn't ping
        w.env.0.keep_alive.store(true, Ordering::SeqCst);
        w.show(true);
        w.show(false);
        assert_eq!(
            w.conn.next_timer(),
            Some(started + Duration::from_millis(LINK.ping_background_ms))
        ); // with keep-alive, every 60 s
    }

    #[tokio::test]
    async fn the_app_hears_when_a_timer_is_set() {
        let w = phone();
        let mut changes = w.conn.timers_changed();
        assert!(!changes.has_changed().expect("the connection is alive"));
        w.start().await;
        assert!(changes.has_changed().expect("the connection is alive")); // the heartbeat
        changes.mark_unchanged();
        w.show(false);
        w.show(true); // a ping now
        assert!(changes.has_changed().expect("the connection is alive"));
    }

    #[tokio::test]
    async fn a_timer_that_fires_twice_or_after_its_connection_was_replaced_starts_nothing() {
        let w = desktop();
        w.pc().fail_lists(Some(locked_database()));
        w.conn.start_connection();
        w.settle().await;
        let retry = w.conn.next_timer().expect("a retry in 2 s");
        assert_eq!(retry, w.env.now() + Duration::from_secs(2));
        w.env.set_now(retry);
        w.conn.on_timer(retry);
        w.conn.on_timer(retry);
        w.turns().await;
        assert_eq!(w.env.local_connects(), 2);

        // A retry that is waiting when the daemon sends a new connection: the new one replaces it.
        w.turns().await;
        let waiting = w.conn.next_timer().expect("the next retry");
        w.pc().fail_lists(None);
        w.conn.on_connection(DesktopConnection {
            port: 47100,
            token: "second".to_owned(),
        });
        w.conn.on_timer(waiting); // the old retry fires after the new connection took its place
        w.turns().await;
        assert_eq!(w.env.local_connects(), 3);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn a_dropped_call_gives_its_place_in_line_back() {
        let w = desktop();
        w.pc().hold("room.history");
        w.start().await;
        let calls: Vec<_> = (0..17)
            .map(|_| {
                let conn = Arc::clone(&w.conn);
                tokio::spawn(async move {
                    let _ = conn
                        .request::<method::RoomHistory>(RoomHistory {
                            before: None,
                            limit: 10,
                            room_id: "team".to_owned(),
                        })
                        .await;
                })
            })
            .collect();
        w.settle().await;
        assert_eq!(w.pc().waiting("room.history"), 16);
        let daemon = w.conn.daemon().expect("connected");
        assert_eq!(daemon.limited.slots.available_permits(), 0);
        // The call that is still waiting for a slot gives up.
        if let Some(last) = calls.last() {
            last.abort();
        }
        w.pc().unhold("room.history");
        w.settle().await;
        assert_eq!(daemon.limited.slots.available_permits(), CALLS);
    }

    #[tokio::test]
    async fn a_download_that_is_dropped_or_waiting_leaves_nothing_registered() {
        let w = started_phone().await;
        let abort = Signal::new();
        let context = w
            .conn
            .capture_attachment_download(abort.clone())
            .expect("connected");
        assert_eq!(locked(&w.conn.inner).downloads.len(), 1);
        // A wait for the file host that is dropped takes its visibility listener and its tick with it.
        w.pc().hold("remote.ping");
        {
            let mut waiting = Box::pin(context.reacquire(&abort));
            tokio::select! {
                biased;
                outcome = &mut waiting => panic!("the held ping can't have answered: {outcome:?}"),
                () = w.settle() => {}
            }
            assert_eq!(locked(&w.conn.inner).reacquiring.len(), 1);
            assert!(!locked(&w.conn.inner).sleeps.is_empty());
        }
        assert!(locked(&w.conn.inner).reacquiring.is_empty());
        assert!(locked(&w.conn.inner).sleeps.is_empty());
        drop(context);
        assert!(locked(&w.conn.inner).downloads.is_empty());
    }

    #[tokio::test]
    async fn a_download_needs_the_file_host_connected() {
        let w = phone();
        assert_eq!(
            w.conn
                .capture_attachment_download(Signal::new())
                .err()
                .map(|error| error.to_string()),
            Some("The file host is offline. Reconnect to download it.".to_owned())
        );
        assert!(w.conn.capture_attachment_save().is_none());
    }

    #[tokio::test]
    async fn a_ping_that_gets_an_error_answer_counts_as_alive() {
        let w = started_phone().await;
        // A PC from before remote.ping says unknown_method: still there.
        w.pc().fail(
            "remote.ping",
            RpcError::Daemon {
                code: ErrorCode::UnknownMethod,
                message: "unknown method".to_owned(),
            },
        );
        w.advance(20_000).await;
        w.advance(10_001).await;
        assert_eq!(w.pc().opens(), 1);
        assert_eq!(w.pc().count("remote.ping"), 1);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn an_event_that_arrives_while_the_lists_load_is_applied_on_top_of_them() {
        let w = desktop();
        w.pc().hold("bots.list");
        w.conn.start_connection();
        w.settle().await;
        assert_ne!(w.status(), Connection::Ready);
        // The daemon pushes a new bot while the list is on its way: the list doesn't have it.
        w.pc().emit(Event::BotUpdated(bot("b2")));
        w.settle().await;
        w.pc().unhold("bots.list");
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.bot_ids(), ["b2"]);
        // And from then on events go straight into the state.
        w.pc().emit(Event::BotUpdated(bot("b3")));
        w.settle().await;
        assert_eq!(w.bot_ids(), ["b2", "b3"]);
    }

    #[tokio::test]
    async fn a_message_that_arrives_while_a_chat_catches_up_is_kept() {
        let w = phone();
        for at in 1..=3 {
            w.pc().add(message(&format!("m{at}"), "b1"));
        }
        w.start().await;
        w.conn
            .load_conversation("b1", w.conn.bot_history("b1"))
            .await;
        w.pc().hold("chat.history");
        w.conn.network_changed();
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.pc().waiting("chat.history"), 1); // the catch-up fetch
        assert!(locked(&w.conn.inner).arriving.contains_key("b1"));
        // The page was read before m4 existed; the event arrives while it is on its way.
        w.pc().emit(Event::MessageAdded(message("m4", "b1")));
        w.settle().await;
        w.pc().unhold("chat.history");
        w.settle().await;
        let ids: Vec<String> = w
            .conversation("b1")
            .expect("the chat is loaded")
            .messages
            .into_iter()
            .map(|message| message.id)
            .collect();
        assert_eq!(ids, ["m1", "m2", "m3", "m4"]);
        assert!(locked(&w.conn.inner).arriving.is_empty());
    }

    #[tokio::test]
    async fn an_older_daemon_without_accounts_leaves_the_list_unknown_and_setup_failing_says_why() {
        let w = desktop();
        w.pc().fail(
            "accounts.list",
            RpcError::Daemon {
                code: ErrorCode::UnknownMethod,
                message: "no such method".to_owned(),
            },
        );
        w.pc().fail("setup.status", locked_database());
        w.start().await;
        assert_eq!(w.status(), Connection::Ready);
        let state = w.conn.state();
        assert_eq!(state.accounts, None);
        let setup = state.setup.expect("the setup cards say why");
        assert_eq!(setup, failed_setup("The database is locked."));
    }

    #[tokio::test]
    async fn events_that_a_link_missed_end_it_and_start_a_reconnect() {
        let w = phone();
        w.env.0.event_capacity.store(2, Ordering::SeqCst);
        w.start().await;
        assert_eq!(w.pc().opens(), 1);
        // More events than the link keeps, with no turn for the connection in between.
        for at in 0..5 {
            w.pc().emit(Event::BotUpdated(bot(&format!("b{at}"))));
        }
        w.settle().await;
        assert_eq!(w.pc().opens(), 2);
        assert!(w.pc().wire(0).is_closed());
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn forgetting_the_pc_a_phone_is_on_lets_go_of_push_and_waits_for_the_pc_only_three_seconds()
     {
        let w = started_phone().await;
        w.pc().hold("device.registerPush"); // the PC doesn't answer
        let forgetting = tokio::spawn({
            let conn = Arc::clone(&w.conn);
            async move { conn.forget_remote(HOST).await }
        });
        w.advance(2_999).await;
        assert!(!forgetting.is_finished());
        w.advance(1).await;
        assert_eq!(forgetting.await.expect("it finished"), Ok(()));
        assert_eq!(locked(&w.env.0.forgotten).as_slice(), [HOST]);
        // No PC left: this device goes to the connect screen.
        w.settle().await;
        assert_eq!(w.status(), Connection::Outside);
        let log = locked(&w.env.0.pushes);
        assert!(log.stopped > 0);
        // Once by Forget itself, and once by the switch to no PC, as in the TypeScript.
        assert_eq!(log.unregistered, 2);
    }

    #[tokio::test]
    async fn a_forget_that_fails_on_a_phone_registers_push_again() {
        let w = started_phone().await;
        let registered = locked(&w.env.0.pushes).registered;
        locked(&w.env.0.forget_failures).push_back("Keystore is locked.".to_owned());
        let failure = w.conn.forget_remote(HOST).await.expect_err("the key stays");
        assert_eq!(failure, "Keystore is locked.");
        w.settle().await;
        assert_eq!(locked(&w.env.0.pushes).registered, registered + 1);
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn a_chat_load_whose_caller_gives_up_still_finishes() {
        let w = desktop();
        let (answer, answered) = oneshot::channel::<()>();
        let gate = Arc::new(Mutex::new(Some(answered)));
        let fetch: FetchPage = Arc::new(move |_| {
            let waiting = locked(&gate).take();
            async move {
                if let Some(waiting) = waiting {
                    let _ = waiting.await;
                }
                Ok(Page {
                    messages: vec![message("m1", "b1")],
                    has_more: false,
                })
            }
            .boxed()
        });
        let loading = tokio::spawn({
            let conn = Arc::clone(&w.conn);
            async move { conn.load_conversation("b1", fetch).await }
        });
        w.settle().await;
        assert_eq!(
            w.conversation("b1").map(|chat| chat.status),
            Some(ConversationStatus::Loading)
        );
        loading.abort(); // the screen's task is dropped while its chat loads
        w.settle().await;
        let _ = answer.send(());
        w.settle().await;
        let chat = w.conversation("b1").expect("the chat is loaded");
        assert_eq!(chat.status, ConversationStatus::Ready);
        assert_eq!(chat.messages.len(), 1);
    }

    #[tokio::test]
    async fn a_forget_whose_caller_gives_up_still_leaves_the_pc() {
        let w = started_phone().await;
        w.pc().hold("device.registerPush");
        let forgetting = tokio::spawn({
            let conn = Arc::clone(&w.conn);
            async move {
                let _ = conn.forget_remote(HOST).await;
            }
        });
        w.settle().await;
        forgetting.abort(); // the screen's task is dropped while Forget waits for the PC
        w.advance(3_000).await;
        assert_eq!(locked(&w.env.0.forgotten).as_slice(), [HOST]);
        assert_eq!(w.status(), Connection::Outside);
    }

    #[tokio::test]
    async fn switching_pc_with_none_saved_goes_to_the_connect_screen() {
        let w = phone();
        locked(&w.env.0.hosts).clear();
        w.conn.start_connection();
        w.settle().await;
        assert_eq!(w.status(), Connection::Outside);
        assert_eq!(locked(&w.env.0.pushes).unregistered, 1);
        assert_eq!(w.pc().opens(), 0);
    }

    #[tokio::test]
    async fn the_desktops_daemon_failing_stops_the_retries_and_try_again_asks_the_app() {
        let w = desktop();
        w.start().await;
        w.conn.on_daemon_failed("The daemon crashed.".to_owned());
        assert_eq!(
            w.status(),
            Connection::Failed {
                reason: "The daemon crashed.".to_owned()
            }
        );
        w.pc().wire(0).drop_wire(); // the old link going away starts no retry
        w.advance(60_000).await;
        assert_eq!(w.env.local_connects(), 1);

        w.conn.restart_daemon();
        assert_eq!(w.status(), Connection::Connecting);
        assert_eq!(w.env.0.restarts.load(Ordering::SeqCst), 1);
        w.conn.on_connection(DesktopConnection {
            port: 47101,
            token: "second".to_owned(),
        });
        w.settle().await;
        assert_eq!(w.status(), Connection::Ready);
    }

    #[tokio::test]
    async fn a_window_on_another_pc_ignores_the_desktops_daemon() {
        let w = desktop();
        w.start().await;
        w.conn.connect_remote(Some(HOST));
        w.settle().await;
        let connects = w.env.local_connects();
        w.conn.on_connection(DesktopConnection {
            port: 47101,
            token: "second".to_owned(),
        });
        w.conn.on_daemon_failed("The daemon crashed.".to_owned());
        w.settle().await;
        assert_eq!(w.env.local_connects(), connects);
        assert_eq!(w.status(), Connection::Ready);
        assert_eq!(w.conn.state().host.as_deref(), Some(HOST));
    }

    fn push(host: &str, bot: &str, room: Option<&str>) -> PushData {
        PushData {
            v: PushDataVersion::V1,
            host_id: host.to_owned(),
            kind: PushKind::Approval,
            bot_id: bot.to_owned(),
            room_id: room.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn a_tapped_push_goes_to_the_room_or_the_bots_chat_and_makes_its_pc_current() {
        let w = started_phone().await;
        assert_eq!(
            w.conn.route_push(&push(HOST, BOT, None)),
            Some(format!("/chat/?bot={BOT}"))
        );
        assert_eq!(w.pc().opens(), 1); // already on that PC
        assert_eq!(
            w.conn.route_push(&push(HOST, BOT, Some("team room/1"))),
            Some("/room/?id=team%20room%2F1".to_owned())
        );
        // A room id that is empty or too long is no room: the bot's chat.
        assert_eq!(
            w.conn.route_push(&push(HOST, BOT, Some(""))),
            Some(format!("/chat/?bot={BOT}"))
        );
        assert_eq!(
            w.conn.route_push(&push(HOST, BOT, Some(&"r".repeat(65)))),
            Some(format!("/chat/?bot={BOT}"))
        );
        // A PC that isn't saved here, or a bot id that isn't a uuid, goes nowhere.
        assert_eq!(w.conn.route_push(&push("other-pc", BOT, None)), None);
        assert_eq!(w.conn.route_push(&push(HOST, "b1", None)), None);
        // Another saved PC becomes the current one.
        locked(&w.env.0.hosts).push(SavedPc {
            id: "other-pc".to_owned(),
            name: "Other PC".to_owned(),
        });
        assert_eq!(
            w.conn.route_push(&push("other-pc", BOT, None)),
            Some(format!("/chat/?bot={BOT}"))
        );
        assert_eq!(w.conn.state().host.as_deref(), Some("other-pc"));
    }

    #[test]
    fn reads_uuids_the_way_zod_does() {
        assert!(is_uuid("0d6ff97e-a44e-42f4-86a0-27ace431918d"));
        assert!(is_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(is_uuid("FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF"));
        assert!(!is_uuid("b1"));
        assert!(!is_uuid("0d6ff97e-a44e-02f4-86a0-27ace431918d")); // version 0
        assert!(!is_uuid("0d6ff97e-a44e-42f4-06a0-27ace431918d")); // variant 0
        assert!(!is_uuid("0d6ff97e-a44e-42f4-86a0-27ace431918"));
        assert!(!is_uuid("0d6ff97e-a44e-42f4-86a0-27ace431918g"));
    }

    #[test]
    fn encodes_uri_components_the_way_javascript_does() {
        assert_eq!(encode_uri_component("team"), "team");
        assert_eq!(encode_uri_component("a b/c?d"), "a%20b%2Fc%3Fd");
        assert_eq!(encode_uri_component("it's (ok)!"), "it's%20(ok)!");
        assert_eq!(encode_uri_component("\u{e9}"), "%C3%A9");
    }

    #[tokio::test]
    async fn the_connection_is_reached_through_the_daemon_handle_screens_hold() {
        let w = desktop();
        w.start().await;
        let daemon = w.conn.daemon().expect("connected");
        let bots = daemon
            .call::<method::BotsList>(Empty {})
            .await
            .expect("answered");
        assert!(bots.is_empty());
        assert_eq!(w.conn.daemon().expect("connected"), daemon);
        // History through a fetcher reads the live connection.
        w.pc().add(message("m1", "b1"));
        let page = w.conn.bot_history("b1")(None).await.expect("a page");
        assert_eq!(page.messages.len(), 1);
        assert!(!page.has_more);
        assert_eq!(w.pc().count("chat.history"), 1);
        // A room's pages are asked for by the room, and an exchange has no older pages whatever the daemon says.
        w.pc().add(message("r1", "team"));
        w.pc().add(message("r2", "team"));
        let room = w.conn.room_history("team")(None).await.expect("a page");
        assert_eq!(room.messages.len(), 2);
        assert_eq!(w.pc().count("room.history"), 1);
        let exchange = w.conn.exchange_page("x1")(Some("ignored".to_owned()))
            .await
            .expect("a page");
        assert_eq!(exchange.messages.len(), 1);
        assert!(!exchange.has_more);
        assert_eq!(w.pc().count("exchanges.get"), 1);
    }
}
