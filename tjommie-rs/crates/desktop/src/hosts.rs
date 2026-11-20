//! The app's side of the daemon's three host routes (Phase 3 host wiring; `main.ts` `launchDaemon`, `onDaemonExit`,
//! `will-quit`). `desktop_host::hosts` is the clients and the vault; this is what only the app can add:
//!
//! - **Orders.** The daemon's state is followed on the Tokio runtime ([`HostControl`]); the clients need the UI thread (the
//!   engine's jobs and the Logins windows live there), so each start or stop is an order to a task on the UI thread
//!   ([`serve`]). WebView2 and COM callbacks never touch GPUI state: a job crosses as a typed value, as `host` and `login` do.
//! - **Only this PC's daemon.** The follower in `live` calls [`HostControl::start`] for the daemon this run started, and
//!   never for a saved PC. Every new `Running` (a new port and token) starts new clients; the old ones stop first.
//! - **The bots.** A second connection to the daemon (the TypeScript's `nativeClient`) tells the engine which bots were
//!   deleted or had their browser switched off, and lists them so orphaned profiles are pruned.
//! - **The window.** The vault trusts a request from the window only while it is shown, its document is ready and the app
//!   is not quitting (`main.ts:457`): the window's facts are kept true here, and the Logins screen's own vault is the
//!   gate's local controller ([`Bridge`]).
//! - **Quitting.** Closing the window stops the clients, closes the bots' browsers and stops the daemon within the quit
//!   timeout before the app exits (GPUI's own quit hook gives only 200 ms).

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use client::{Client, Endpoint};
use desktop_host::browser::engine::{BotEvent, Engine};
use desktop_host::browser::host::{JobQueue, serve as serve_jobs};
use desktop_host::browser::login::{Request, serve as serve_logins};
use desktop_host::hosts::{
    BotDirectory, BotInfo, DaemonBots, DirectoryError, Hosts, Local, Parts, Running,
};
use desktop_host::vault::credential::LoginCredentials;
use desktop_host::vault::login::SecretStore;
use futures::channel::oneshot;
use futures::future::{BoxFuture, LocalBoxFuture};
use gpui::{App, Global, Window};
use protocol::Event;
use protocol::vault::VaultHostAction;
use protocol::wire::{Empty, Id, Notify, PendingNotification, method};
use tokio::sync::{broadcast, watch};

use crate::guard::{guarded, guarded_async};
use crate::model::logins::{CommandFailure, Desktop, NativeVault, RawAnswer};
use crate::ui::browser::local::LocalBrowser;
use crate::ui::vault_entry::EntryWindows;

/// How long a stop before a daemon restart waits for the UI thread to wind the clients down before it goes on without it.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the bot watcher waits before it connects to the daemon again.
const RETRY: Duration = Duration::from_secs(1);

// ---- orders ----------------------------------------------------------------------------------------------------------

/// What the watcher of this PC's daemon tells the app: the engine what happened to the bots (the TypeScript's
/// `nativeClient` subscription), and the services the notices to show (its `notifier` client).
pub enum Notice {
    /// The daemon's bots now: each id, and whether its browser is on.
    Listed(Vec<(String, bool)>),
    Deleted(String),
    Updated {
        id: String,
        browser: bool,
    },
    /// The watcher lost the daemon: the vault locks, as the TypeScript's `retry` does.
    Lost,
    /// The connection the notices come in on, which acknowledges them.
    Connected(Arc<Client>),
    /// A `notify` event.
    Notify(Box<Notify>),
    /// `notifications.pending`: what the daemon still holds because nothing acknowledged it.
    Pending(Vec<PendingNotification>),
}

/// An order to the UI thread's task.
pub enum Order {
    Start {
        local: Local,
        notices: Receiver<Notice>,
    },
    Stop {
        /// Also close every bot's browser (the app is quitting).
        browsers: bool,
        done: oneshot::Sender<()>,
    },
}

/// What starts and stops the clients: `Send + Sync`, so the daemon's follower on the Tokio runtime can order from there.
#[derive(Clone)]
pub struct HostControl {
    orders: Sender<Order>,
    runtime: tokio::runtime::Handle,
    /// Bumped by every order: the bot watcher of an older order ends.
    generation: Arc<watch::Sender<u64>>,
    /// The app is quitting: nothing starts the clients again (a daemon that dies mid-quit is restarted by its supervisor,
    /// and its new `Running` must not bring them back), and the daemon is not restarted either.
    quitting: Arc<AtomicBool>,
}

impl HostControl {
    pub fn new(runtime: tokio::runtime::Handle) -> (HostControl, Receiver<Order>) {
        let (orders, inbox) = async_channel::unbounded();
        (
            HostControl {
                orders,
                runtime,
                generation: Arc::new(watch::channel(0).0),
                quitting: Arc::new(AtomicBool::new(false)),
            },
            inbox,
        )
    }

    /// The daemon this run started is running with new tokens: start the clients for it. The ones for the daemon before are
    /// stopped by the UI thread before the new ones start. Nothing starts once the app is quitting.
    pub fn start(&self, local: Local) {
        if self.quitting() {
            return;
        }
        let (notices, inbox) = async_channel::unbounded();
        // Read inside the change: a `stop` between the bump and a later read would hand this watcher the stop's generation.
        // One that lands between this and the subscription is seen by the watcher's first check.
        let mut mine = 0;
        self.generation.send_modify(|generation| {
            *generation += 1;
            mine = *generation;
        });
        let current = self.generation.subscribe();
        // unguarded: a tokio task, off the UI thread; guarded_async wraps its body
        self.runtime.spawn(guarded_async(
            "watch the daemon's bots",
            watch_bots(local.port, local.token.clone(), notices, current, mine),
        ));
        let _ = self.orders.try_send(Order::Start {
            local,
            notices: inbox,
        });
    }

    /// The daemon is going (it restarts, or the app quits): stop the clients. Resolves when the UI thread has stopped them.
    /// It holds no timer, so it runs on any executor (the UI's at quit, Tokio's at a restart): the caller bounds the wait,
    /// with [`STOP_TIMEOUT`] or the quit timeout.
    pub fn stop(&self, browsers: bool) -> impl Future<Output = ()> + Send + use<> {
        if browsers {
            self.begin_quit();
        }
        self.generation.send_modify(|generation| *generation += 1);
        let (done, stopped) = oneshot::channel();
        let sent = self.orders.try_send(Order::Stop { browsers, done });
        async move {
            if sent.is_ok() {
                let _ = stopped.await;
            }
        }
    }

    /// [`stop`](Self::stop) without waiting: the daemon is already gone.
    pub fn stop_now(&self) {
        drop(self.stop(false));
    }

    /// The app is quitting: no client starts and no daemon restarts from here (until [`resume`](Self::resume)).
    pub fn begin_quit(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }

    /// The quit was given up on (an update that did not install): the app runs on, and the daemon and its clients may start.
    pub fn resume(&self) {
        self.quitting.store(false, Ordering::SeqCst);
    }

    pub fn quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }
}

/// Follows the daemon's bots for the engine: lists them, then forwards each deletion and browser switch, and starts over
/// when the connection drops, until a newer order ends it. The first list also lets the engine prune what no bot owns.
async fn watch_bots(
    port: u16,
    token: String,
    notices: Sender<Notice>,
    mut current: watch::Receiver<u64>,
    mine: u64,
) {
    let ended = |current: &watch::Receiver<u64>| *current.borrow() != mine;
    while !ended(&current) {
        let connected = Client::connect(Endpoint::Local { port }, &token).await;
        let Ok(client) = connected else {
            tokio::select! {
                () = tokio::time::sleep(RETRY) => continue,
                _ = current.changed() => return,
            }
        };
        let client = Arc::new(client);
        let closed = client.closed();
        tokio::pin!(closed);
        // Subscribed before the list and the pending notices are asked for: what happens meanwhile is applied after them.
        let mut events = client.events();
        let _ = notices.send(Notice::Connected(client.clone())).await;
        let mut listed = false;
        loop {
            if !listed {
                match client.call::<method::BotsList>(Empty {}).await {
                    Ok(bots) => {
                        let bots = bots.into_iter().map(|bot| (bot.id, bot.browser)).collect();
                        let _ = notices.send(Notice::Listed(bots)).await;
                    }
                    Err(_) => break,
                }
                match client.call::<method::NotificationsPending>(Empty {}).await {
                    Ok(pending) => {
                        let _ = notices.send(Notice::Pending(pending)).await;
                    }
                    Err(_) => break,
                }
                listed = true;
            }
            tokio::select! {
                _ = current.changed() => {
                    client.close().await;
                    return;
                }
                _ = &mut closed => break,
                event = events.recv() => match event {
                    Ok(Event::BotDeleted(Id { id })) => {
                        let _ = notices.send(Notice::Deleted(id)).await;
                    }
                    Ok(Event::BotUpdated(bot)) => {
                        let _ = notices
                            .send(Notice::Updated { id: bot.id, browser: bot.browser })
                            .await;
                    }
                    Ok(Event::Notify(notice)) => {
                        let _ = notices.send(Notice::Notify(Box::new(notice))).await;
                    }
                    Ok(_) => {}
                    // Events were lost: ask for the list again, on a fresh subscription, so that the events the old one
                    // still holds (older than the list) are not applied after it.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        events = client.events();
                        listed = false;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        client.close().await;
        let _ = notices.send(Notice::Lost).await;
        tokio::select! {
            () = tokio::time::sleep(RETRY) => {}
            _ = current.changed() => return,
        }
    }
}

// ---- the UI thread -----------------------------------------------------------------------------------------------------

/// The app's Logins pieces and its clients' control, kept for the screens and the quit.
#[derive(Clone)]
pub struct AppHosts {
    pub hosts: Arc<Hosts>,
    pub control: HostControl,
    /// The live runtime: the daemon's bot lookups are Tokio work that the UI thread's tasks only await.
    runtime: tokio::runtime::Handle,
}

impl Global for AppHosts {}

/// What `live::start` builds: the vault, its gate and the clients' control. The Logins windows start here too. `requests`
/// are the vault's browser steps, which [`serve`] runs on the engine.
pub fn create(
    root: &Path,
    runtime: &tokio::runtime::Handle,
    cx: &mut App,
) -> (AppHosts, Receiver<Order>, Receiver<Request>) {
    let secrets: Option<Arc<dyn SecretStore>> = match LoginCredentials::new(root) {
        Ok(credentials) => Some(Arc::new(credentials)),
        Err(error) => {
            log::warn!("Logins has no secret store: {error:?}");
            None
        }
    };
    let (hosts, requests) = Hosts::new(Parts {
        root: root.to_owned(),
        secrets,
        entry: EntryWindows::start(cx),
        runtime: runtime.clone(),
    });
    let (control, orders) = HostControl::new(runtime.clone());
    (
        AppHosts {
            hosts: Arc::new(hosts),
            control,
            runtime: runtime.clone(),
        },
        orders,
        requests,
    )
}

/// Runs the orders on the UI thread, once the window and its engine exist: starts the clients for each daemon, stops them
/// before the next, and serves the vault's browser steps on the engine. A run without an engine (no window handle) still
/// stops and starts the vault's side and says why the browsers are off.
pub fn serve(app: AppHosts, orders: Receiver<Order>, requests: Receiver<Request>, cx: &mut App) {
    cx.spawn(async move |cx| {
        guarded_async("run the host clients", async move {
            let mut running: Option<Running> = None;
            let mut logins_served = false;
            while let Ok(order) = orders.recv().await {
                // The engine, if the window made one: asked for each order, since the first may come before it exists.
                let engine = cx.update(|cx| LocalBrowser::engine(cx));
                match order {
                    Order::Start { local, notices } => {
                        if let Some(old) = running.take() {
                            app.hosts.stop(old).await;
                            if let Some(engine) = &engine {
                                engine.daemon_restarted().await;
                            }
                        }
                        match &engine {
                            Some(engine) if !logins_served => {
                                logins_served = true;
                                cx.update(|cx| serve_login_steps(engine, &requests, cx));
                            }
                            Some(_) => {}
                            None => {
                                // The vault and the worker lease still run; only what needs a page is off, and a Logins
                                // step that needs one fails at once instead of waiting for an engine that isn't there.
                                log::error!(
                                    "The bots' browsers are off: there is no browser engine."
                                );
                                requests.close();
                            }
                        }
                        running = cx.update(|cx| start(&app, local, engine.as_ref(), notices, cx));
                    }
                    Order::Stop { browsers, done } => {
                        if let Some(old) = running.take() {
                            app.hosts.stop(old).await;
                        }
                        if let Some(engine) = &engine {
                            engine.daemon_restarted().await;
                            if browsers {
                                let _ = engine.close_all().await;
                            }
                        }
                        let _ = done.send(());
                    }
                }
            }
        })
        .await;
    })
    .detach();
}

/// The UI thread's spawner for the engine's tasks: each is a foreground task of the app, guarded.
fn spawner(cx: &App) -> impl Fn(LocalBoxFuture<'static, ()>) + 'static {
    let executor = cx.foreground_executor().clone();
    move |task| {
        executor
            // unguarded: a future, not a closure; guarded_async wraps it
            .spawn(guarded_async("a browser job", task))
            .detach();
    }
}

fn serve_login_steps(engine: &Rc<Engine>, requests: &Receiver<Request>, cx: &mut App) {
    let (engine, requests, spawn) = (engine.clone(), requests.clone(), spawner(cx));
    cx.foreground_executor()
        // unguarded: a future, not a closure; guarded_async wraps it
        .spawn(guarded_async(
            "run the vault's browser steps",
            serve_logins(engine, requests, spawn),
        ))
        .detach();
}

/// The bot directory for the browser's jobs, which the UI thread awaits: the lookup talks to the daemon with a Tokio client,
/// so it runs on the live runtime and the UI task only awaits its answer (as [`LocalVault`]'s commands do).
struct OnRuntime {
    bots: Arc<DaemonBots>,
    runtime: tokio::runtime::Handle,
}

impl BotDirectory for OnRuntime {
    fn find(&self, id: &str) -> BoxFuture<'_, Result<Option<BotInfo>, DirectoryError>> {
        let (bots, id) = (self.bots.clone(), id.to_owned());
        // unguarded: a tokio task, off the UI thread, where a panic ends only that task (and the lookup fails)
        let looked = self.runtime.spawn(async move { bots.find(&id).await });
        Box::pin(async move { looked.await.unwrap_or(Err(DirectoryError)) })
    }
}

/// The clients for one daemon: its browser jobs are served on the engine, the bot watcher's notices are applied to it, and
/// the three connections start. With no engine (`None`) the jobs answer "not ready" at once and the bots' list has nowhere
/// to go, but the vault, the worker lease and the notices still start. `None` back, with the reason in the log, when the
/// clients can't start.
fn start(
    app: &AppHosts,
    local: Local,
    engine: Option<&Rc<Engine>>,
    notices: Receiver<Notice>,
    cx: &mut App,
) -> Option<Running> {
    let hosts = &app.hosts;
    let (queue, jobs) = JobQueue::new();
    let directory = Arc::new(DaemonBots::new(&local));
    if let Some(engine) = engine {
        let (spawn, bots): (_, Arc<dyn BotDirectory>) = (
            spawner(cx),
            Arc::new(OnRuntime {
                bots: directory.clone(),
                runtime: app.runtime.clone(),
            }),
        );
        cx.foreground_executor()
            // unguarded: a future, not a closure; guarded_async wraps it
            .spawn(guarded_async(
                "serve the browser's jobs",
                serve_jobs(engine.clone(), jobs, bots, spawn),
            ))
            .detach();
    }
    let (applying, locking) = (engine.cloned(), hosts.clone());
    cx.spawn(async move |cx| {
        guarded_async("hear the daemon's bots", async move {
            while let Ok(notice) = notices.recv().await {
                match notice {
                    Notice::Listed(bots) => {
                        if let Some(engine) = &applying {
                            // Each bot's browser as listed, and what the list no longer has closed, as the events would have.
                            // unguarded: the engine's own method inside guarded_async, not a handler registration
                            engine.on_bot_list(&bots).await;
                            let ids: Vec<String> = bots.into_iter().map(|(id, _)| id).collect();
                            if let Err(error) = engine.prune_profiles(&ids).await {
                                log::warn!(
                                    "Some deleted browser profiles could not be cleaned up: {error}"
                                );
                            }
                        }
                    }
                    Notice::Deleted(id) => {
                        if let Some(engine) = &applying {
                            // unguarded: the engine's own method inside guarded_async, not a handler registration
                            engine.on_bot_event(BotEvent::Deleted { id }).await;
                        }
                    }
                    Notice::Updated { id, browser } => {
                        if let Some(engine) = &applying {
                            let updated = BotEvent::Updated { id, browser };
                            // unguarded: the engine's own method inside guarded_async, not a handler registration
                            engine.on_bot_event(updated).await;
                        }
                    }
                    Notice::Lost => locking.lock(),
                    Notice::Connected(client) => {
                        cx.update(|cx| crate::services::connected(client, cx));
                    }
                    Notice::Notify(notice) => cx.update(|cx| crate::services::notify(*notice, cx)),
                    Notice::Pending(pending) => {
                        cx.update(|cx| crate::services::replay(pending, cx));
                    }
                }
            }
        })
        .await;
    })
    .detach();
    match hosts.start(&local, directory, Arc::new(queue)) {
        Ok(running) => Some(running),
        Err(error) => {
            log::error!("The browser and Logins hosts could not start: {error}");
            None
        }
    }
}

// ---- the window ------------------------------------------------------------------------------------------------------

/// Keeps what the vault's gate reads of the window true (`main.ts:457`), and hands the close button to the services (it
/// hides to the tray, or quits).
pub fn watch_window(app: &AppHosts, window: &mut Window, cx: &mut App) {
    let hosts = app.hosts.clone();
    hosts.window().show();
    // A window opened hidden (`--hidden`, `--updated`) starts in the tray: nothing is trusted that it hasn't shown.
    hosts.window().set_visible(window.is_visible());
    let visible = hosts.clone();
    window
        .observe_window_visibility(move |visibility, _, _| {
            guarded("follow the window's visibility for Logins", || {
                visible.window().set_visible(visibility.is_visible())
            });
        })
        .detach();
    // unguarded: guarded() wraps the whole body; a panic in it answers "close", which is the way out of a stuck quit
    window.on_window_should_close(cx, move |window, cx| {
        guarded("close the app", || {
            crate::services::on_close_requested(window, cx)
        })
        .unwrap_or(true)
    });
}

// ---- the Logins screen's own vault ----------------------------------------------------------------------------------

/// This PC's own vault, reached directly by the Logins screen and the browser panel's logins menu (`window.tjommie.vault`
/// in the TypeScript): the gate's local controller, trusted by the window's state.
#[derive(Clone)]
pub struct LocalVault {
    hosts: Arc<Hosts>,
    runtime: tokio::runtime::Handle,
}

impl PartialEq for LocalVault {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.hosts, &other.hosts)
    }
}

impl NativeVault for LocalVault {
    fn command(
        &self,
        action: &VaultHostAction,
    ) -> impl Future<Output = Result<RawAnswer, CommandFailure>> {
        let (hosts, action) = (self.hosts.clone(), action.clone());
        // The vault's own work (the isolation lease's heartbeat) spawns on Tokio, so it runs on the live runtime, and the
        // UI task only awaits its answer.
        let answered = self
            .runtime
            // unguarded: a tokio task, off the UI thread, where a panic ends only that task
            .spawn(async move { hosts.execute_local(action).await });
        async move {
            let reply = answered.await.map_err(|_| CommandFailure::Failed)?;
            let reply = reply.map_err(|_| CommandFailure::Failed)?;
            serde_json::to_value(reply)
                .map(RawAnswer::new)
                .map_err(|_| CommandFailure::Failed)
        }
    }
}

/// The desktop's bridge as the Logins client reads it (`desktop()` in the TypeScript): the vault of this run, if its storage
/// opened.
#[derive(Clone)]
pub struct Bridge(Option<LocalVault>);

impl Bridge {
    pub fn of(cx: &App) -> Bridge {
        Bridge(cx.try_global::<AppHosts>().and_then(|app| {
            let runtime = cx.try_global::<crate::live::Tokio>()?.0.clone();
            app.hosts.has_vault().then(|| LocalVault {
                hosts: app.hosts.clone(),
                runtime,
            })
        }))
    }
}

impl Desktop for Bridge {
    type Vault = LocalVault;

    fn vault(&self) -> Option<LocalVault> {
        self.0.clone()
    }
}

/// The app's hosts over folders and stand-ins of a test's own: no engine, no daemon, no person at the Logins windows.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::PathBuf;

    use desktop_host::vault::entry::EntryInput;
    use desktop_host::vault::login::{Cancel, Confirmation, EntryWindow, LoginError};
    use protocol::vault::LoginEntry;

    use super::*;

    /// A stand-in for the Logins windows: nothing in a test asks the person anything.
    pub(crate) struct NoWindows;

    impl EntryWindow for NoWindows {
        fn entry(
            &self,
            _: String,
            _: Option<LoginEntry>,
            _: Cancel,
        ) -> BoxFuture<'_, Result<Option<EntryInput>, LoginError>> {
            Box::pin(async { Ok(None) })
        }

        fn confirm(&self, _: Confirmation, _: Cancel) -> BoxFuture<'_, Result<bool, LoginError>> {
            Box::pin(async { Ok(false) })
        }

        fn close(&self) {}
    }

    /// `AppHosts` with its orders and vault-step inboxes, and the folder it keeps its records in (the caller removes it).
    pub(crate) fn app_hosts(
        runtime: &tokio::runtime::Handle,
        name: &str,
    ) -> (AppHosts, Receiver<Order>, Receiver<Request>, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("tjommie-hosts-{name}-{}", std::process::id()));
        let (hosts, requests) = Hosts::new(Parts {
            root: root.clone(),
            secrets: None,
            entry: Arc::new(NoWindows),
            runtime: runtime.clone(),
        });
        let (control, orders) = HostControl::new(runtime.clone());
        let app = AppHosts {
            hosts: Arc::new(hosts),
            control,
            runtime: runtime.clone(),
        };
        (app, orders, requests, root)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::net::{TcpListener, TcpStream};
    use std::time::Instant;

    use desktop_host::vault::host::HostToken;

    use super::testing::app_hosts;
    use super::*;

    fn local() -> Local {
        Local {
            port: 1,
            token: "token".to_owned(),
            host_token: HostToken::new("host"),
        }
    }

    fn started(inbox: &Receiver<Order>) -> bool {
        matches!(inbox.try_recv(), Ok(Order::Start { .. }))
    }

    /// Review Minor 3: a daemon that dies while the app quits is restarted by its supervisor, and its new `Running` must
    /// not start clients the quit just stopped.
    #[tokio::test]
    async fn a_start_after_the_quit_stopped_the_clients_is_ignored() {
        let (control, inbox) = HostControl::new(tokio::runtime::Handle::current());
        drop(control.stop(true));
        assert!(matches!(inbox.try_recv(), Ok(Order::Stop { .. })));
        control.start(local());
        assert!(
            inbox.try_recv().is_err(),
            "a client start followed the quit"
        );
        // The quit was given up on (an update that did not install): the clients start again.
        control.resume();
        control.start(local());
        assert!(started(&inbox));
    }

    /// What a connection to the stand-in daemon asked for first, if it has sent anything yet.
    fn first_request(mut stream: TcpStream) -> String {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        let mut buffer = [0_u8; 512];
        let read = stream.read(&mut buffer).unwrap_or(0);
        String::from_utf8_lossy(buffer.get(..read).unwrap_or_default()).into_owned()
    }

    /// Review Minor 1: with no browser engine (no window handle, a refused data root) a daemon's start still starts what
    /// does not need one: the Logins host connects to the daemon's `/vault-host` route. A listener stands in for the daemon
    /// and says what the clients ask it for.
    #[gpui::test]
    fn a_start_with_no_engine_still_starts_the_vaults_side(cx: &mut gpui::TestAppContext) {
        // The clients do real network work on their own threads, which wakes the app's tasks: not a deterministic test.
        cx.executor().allow_parking();
        let daemon = TcpListener::bind("127.0.0.1:0").unwrap();
        daemon.set_nonblocking(true).unwrap();
        let port = daemon.local_addr().unwrap().port();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (app, orders, requests, root) = app_hosts(runtime.handle(), "minor1");
        let control = app.control.clone();
        cx.update(|cx| serve(app, orders, requests, cx));
        control.start(Local {
            port,
            token: "t".repeat(43),
            host_token: HostToken::new("h".repeat(43)),
        });

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut asked = Vec::new();
        while !asked
            .iter()
            .any(|request: &String| request.contains("/vault-host"))
        {
            cx.run_until_parked();
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
            while let Ok((stream, _)) = daemon.accept() {
                asked.push(first_request(stream));
            }
            assert!(
                Instant::now() < deadline,
                "no Logins host connected to the daemon with no engine; the clients asked for: {asked:?}"
            );
        }

        // Stopped as a quit stops them, and the stand-in is let go of.
        let mut stopped = Box::pin(control.stop(true));
        while futures::FutureExt::now_or_never(&mut stopped).is_none() {
            cx.run_until_parked();
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            assert!(Instant::now() < deadline, "the clients did not stop");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stop before a daemon restart is not a quit: the new daemon's clients start.
    #[tokio::test]
    async fn a_start_after_a_restarts_stop_is_served() {
        let (control, inbox) = HostControl::new(tokio::runtime::Handle::current());
        drop(control.stop(false));
        assert!(matches!(inbox.try_recv(), Ok(Order::Stop { .. })));
        control.start(local());
        assert!(started(&inbox));
        control.stop_now();
    }
}
