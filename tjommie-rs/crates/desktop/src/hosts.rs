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
use std::time::Duration;

use async_channel::{Receiver, Sender};
use client::{Client, Endpoint};
use desktop_host::browser::engine::{BotEvent, Engine};
use desktop_host::browser::host::{JobQueue, serve as serve_jobs};
use desktop_host::browser::login::{Request, serve as serve_logins};
use desktop_host::daemon_process::QUIT_TIMEOUT;
use desktop_host::hosts::{DaemonBots, Hosts, Local, Parts, Running};
use desktop_host::vault::credential::LoginCredentials;
use desktop_host::vault::login::SecretStore;
use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use gpui::{App, Global, Window};
use protocol::Event;
use protocol::vault::VaultHostAction;
use protocol::wire::{Empty, Id, method};
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

/// What the bot watcher tells the engine (the TypeScript's `nativeClient` subscription).
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

impl HostControl {
    pub fn new(runtime: tokio::runtime::Handle) -> (HostControl, Receiver<Order>) {
        let (orders, inbox) = async_channel::unbounded();
        (
            HostControl {
                orders,
                runtime,
                generation: Arc::new(watch::channel(0).0),
            },
            inbox,
        )
    }

    /// The daemon this run started is running with new tokens: start the clients for it. The ones for the daemon before are
    /// stopped by the UI thread before the new ones start.
    pub fn start(&self, local: Local) {
        let (notices, inbox) = async_channel::unbounded();
        self.generation.send_modify(|generation| *generation += 1);
        let (current, mine) = (self.generation.subscribe(), *self.generation.borrow());
        let watching = local.clone();
        // unguarded: a tokio task, off the UI thread; guarded_async wraps its body
        self.runtime.spawn(guarded_async(
            "watch the daemon's bots",
            watch_bots(watching, notices, current, mine),
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
}

/// Follows the daemon's bots for the engine: lists them, then forwards each deletion and browser switch, and starts over
/// when the connection drops, until a newer order ends it. The first list also lets the engine prune what no bot owns.
async fn watch_bots(
    local: Local,
    notices: Sender<Notice>,
    mut current: watch::Receiver<u64>,
    mine: u64,
) {
    let ended = |current: &watch::Receiver<u64>| *current.borrow() != mine;
    while !ended(&current) {
        let connected = Client::connect(Endpoint::Local { port: local.port }, &local.token).await;
        let Ok(client) = connected else {
            tokio::select! {
                () = tokio::time::sleep(RETRY) => continue,
                _ = current.changed() => return,
            }
        };
        let closed = client.closed();
        tokio::pin!(closed);
        // Subscribed before the list is asked for: what happens meanwhile is applied after it.
        let mut events = client.events();
        let mut listed = false;
        loop {
            if !listed {
                match client.call::<method::BotsList>(Empty {}).await {
                    Ok(bots) => {
                        let bots = bots.into_iter().map(|bot| (bot.id, bot.browser)).collect();
                        let _ = notices.send(Notice::Listed(bots)).await;
                        listed = true;
                    }
                    Err(_) => break,
                }
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
                    Ok(_) => {}
                    // Events were lost: ask for the list again.
                    Err(broadcast::error::RecvError::Lagged(_)) => listed = false,
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
                        let Some(engine) = engine else {
                            log::error!("The bots' browsers are off: there is no browser engine.");
                            continue;
                        };
                        if !logins_served {
                            logins_served = true;
                            cx.update(|cx| serve_login_steps(&engine, &requests, cx));
                        }
                        running = cx.update(|cx| start(&app.hosts, local, &engine, notices, cx));
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

/// The clients for one daemon: its browser jobs are served on the engine, the bot watcher's notices are applied to it, and
/// the three connections start. `None`, with the reason in the log, when they can't.
fn start(
    hosts: &Arc<Hosts>,
    local: Local,
    engine: &Rc<Engine>,
    notices: Receiver<Notice>,
    cx: &mut App,
) -> Option<Running> {
    let (queue, jobs) = JobQueue::new();
    let spawn = spawner(cx);
    cx.foreground_executor()
        // unguarded: a future, not a closure; guarded_async wraps it
        .spawn(guarded_async(
            "serve the browser's jobs",
            serve_jobs(engine.clone(), jobs, spawn),
        ))
        .detach();
    let (applying, locking) = (engine.clone(), hosts.clone());
    cx.foreground_executor()
        // unguarded: a future, not a closure; guarded_async wraps it
        .spawn(guarded_async("hear the daemon's bots", async move {
            while let Ok(notice) = notices.recv().await {
                match notice {
                    Notice::Listed(bots) => {
                        for (id, browser) in &bots {
                            if *browser {
                                applying.allow_bot(id);
                            }
                        }
                        let ids: Vec<String> = bots.into_iter().map(|(id, _)| id).collect();
                        if let Err(error) = applying.prune_profiles(&ids).await {
                            log::warn!(
                                "Some deleted browser profiles could not be cleaned up: {error}"
                            );
                        }
                    }
                    // unguarded: the engine's own method inside guarded_async, not a handler registration
                    Notice::Deleted(id) => applying.on_bot_event(BotEvent::Deleted { id }).await,
                    Notice::Updated { id, browser } => {
                        let updated = BotEvent::Updated { id, browser };
                        // unguarded: the engine's own method inside guarded_async, not a handler registration
                        applying.on_bot_event(updated).await;
                    }
                    Notice::Lost => locking.lock(),
                }
            }
        }))
        .detach();
    let directory = Arc::new(DaemonBots::new(local.clone()));
    match hosts.start(&local, directory, Arc::new(queue)) {
        Ok(running) => Some(running),
        Err(error) => {
            log::error!("The browser and Logins hosts could not start: {error}");
            None
        }
    }
}

// ---- the window ------------------------------------------------------------------------------------------------------

/// Keeps what the vault's gate reads of the window true (`main.ts:457`), and turns a close into a graceful quit.
pub fn watch_window(app: &AppHosts, window: &mut Window, cx: &mut App) {
    let hosts = app.hosts.clone();
    hosts.window().show();
    let visible = hosts.clone();
    window
        .observe_window_visibility(move |visibility, _, _| {
            guarded("follow the window's visibility for Logins", || {
                visible.window().set_visible(visibility.is_visible())
            });
        })
        .detach();
    let quitting = app.clone();
    // unguarded: guarded() wraps the whole body; a panic in it answers "close", which is the way out of a stuck quit
    window.on_window_should_close(cx, move |_, cx| {
        guarded("close the app", || quit(&quitting, cx)).unwrap_or(true)
    });
}

/// Whether the app may close now. The first request starts the graceful quit and says not yet; the app exits itself when it
/// is done. A second request while that runs changes nothing.
fn quit(app: &AppHosts, cx: &mut App) -> bool {
    if cx.has_global::<Quit>() {
        return cx.global::<Quit>().done;
    }
    cx.set_global(Quit { done: false });
    app.hosts.begin_quit();
    log::info!("Closing: the browser, Logins and worker clients stop, then the daemon.");
    let (hosts, stopper) = (app.control.clone(), crate::live::stopper(cx));
    cx.spawn(async move |cx| {
        guarded_async("quit", async move {
            // The clients first (the vault locks and the browsers close), then the daemon, each within the quit timeout.
            let timeout = cx.background_executor().clone();
            let stop = hosts.stop(true);
            futures::pin_mut!(stop);
            let _ = futures::future::select(stop, timeout.timer(QUIT_TIMEOUT)).await;
            if let Some(stopper) = stopper {
                let daemon = stopper.stop();
                futures::pin_mut!(daemon);
                let _ = futures::future::select(daemon, timeout.timer(QUIT_TIMEOUT)).await;
            }
            log::info!("Closed: the clients and the daemon have stopped.");
            cx.update(|cx| {
                cx.global_mut::<Quit>().done = true;
                cx.quit();
            });
        })
        .await;
    })
    .detach();
    false
}

/// The app is closing: its clients and daemon are being stopped.
struct Quit {
    done: bool,
}

impl Global for Quit {}

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
