//! The app's live connection (Phase 3 plan M5, the store's wiring): the desktop `Environment` that T1.3's
//! `AppConnection` runs on, the tokio runtime its work and timers run on, and the feed of its state into `ui::store`
//! and the shell.
//!
//! - This PC's daemon is started and kept running by `daemon_process::Supervisor` (T1.2): the bundled exe in a
//!   production build, else `TJOMMIE_DAEMON_EXE` or the debug build beside this exe. A dev build connects instead to a
//!   daemon someone started with `--dev`, when its token file is there.
//! - Saved PCs are `remote_hosts` (T1.3) over `desktop-ui.json` and the device keys.
//! - The connection's state reaches GPUI as a signal: a foreground task copies it into the store once per change it
//!   sees, and redraws.
//!
//! An e2e build's daemon gets port 0 and listens for phones on loopback only (no `0.0.0.0` in tests).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use client::{Client, ConnectError, Endpoint};
use desktop_host::daemon_process::{self, DaemonLaunch, DaemonState, Env, Supervisor};
use desktop_host::device_keys::DeviceKeys;
use desktop_host::device_settings::{self, DeviceSettings, RemoteHost};
use desktop_host::hosts::Local;
use desktop_host::identity::{Identity, Kind};
use desktop_host::remote_hosts::{DialError, HostError, HostStore, Hosts, NetworkDialer};
use desktop_host::updater::DaemonStop;
use desktop_host::vault::host::HostToken;
use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use gpui::{App, SharedString};
use tokio::sync::watch;

use crate::guard::{guarded, guarded_async};
use crate::hosts::HostControl;
use crate::model::connection::{
    AppConnection, DesktopConnection, Environment, Failure, FilesScope, SavedPc,
};
use crate::model::state::{self, AppState};
use crate::ui::shell::data::{self, Command, Recent, RecentKind, SavedHost, ShellData};

/// The app's connection to a daemon.
pub type Live = AppConnection<DesktopEnv>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the daemon is started with: the variables this run adds to its own environment. The data root always rides
/// along; an e2e build's daemon takes any free port and keeps the phones' listener on loopback.
pub fn launch_env(identity: &Identity, root: &Path, base: &[(String, String)]) -> Env {
    let disposable = matches!(identity.kind, Kind::E2e | Kind::UpdateTest);
    let mut env =
        daemon_process::daemon_launch_env(base, root, disposable, identity.kind == Kind::Dev);
    if disposable {
        // The phones' listener too takes any free port, and on loopback only: nothing in a test listens on the network.
        for (name, value) in [("TJOMMIE_LAN_PORT", "0"), ("TJOMMIE_LAN_BIND", "127.0.0.1")] {
            env.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            env.push((name.to_owned(), value.to_owned()));
        }
    }
    env
}

/// What `AppConnection` hears when a saved PC can't be opened (`openRemoteClient`'s errors).
pub fn failure_of(error: &HostError) -> Failure {
    match error {
        HostError::Dial(DialError::Refused(_)) => Failure::Refused,
        HostError::Dial(DialError::HostOffline) => Failure::HostOffline(error.to_string()),
        // ponytail: the dialer hands a client error on as its text, so 4003 is known by that text; a typed variant in
        // `DialError` would replace this (asked of the Client lane).
        HostError::Dial(DialError::Failed(text))
            if *text == ConnectError::ProtocolMismatch.to_string() =>
        {
            Failure::Update
        }
        other => Failure::Other(other.to_string()),
    }
}

/// What the shell shows of the app state: the connection, the PC, the saved PCs and the Recents (bots in the list's
/// order, then rooms). Requests go to `on_command`.
pub fn shell_data(state: &AppState, saved: &[SavedPc], base: &ShellData) -> ShellData {
    let connection = match &state.connection {
        state::Connection::Connecting => data::Connection::Connecting,
        state::Connection::Ready => data::Connection::Ready,
        state::Connection::Reconnecting => data::Connection::Reconnecting,
        state::Connection::Offline { reason } => data::Connection::Offline {
            reason: match reason {
                state::OfflineReason::HostOffline => data::OfflineReason::HostOffline,
                state::OfflineReason::Unreachable => data::OfflineReason::Unreachable,
            },
        },
        state::Connection::Removed => data::Connection::Removed,
        state::Connection::Update => data::Connection::Update,
        state::Connection::Failed { reason } => data::Connection::Failed {
            reason: reason.clone().into(),
        },
        state::Connection::Outside => data::Connection::Outside,
    };
    let bots = state.bots.iter().map(|bot| Recent {
        id: bot.id.clone().into(),
        name: bot.name.clone().into(),
        kind: RecentKind::Bot {
            color: bot.color,
            shape: bot.shape,
            state: bot.state,
        },
    });
    // The desktop keeps no unread marks for rooms (the TypeScript had none either).
    let rooms = state.rooms.iter().map(|room| Recent {
        id: room.id.clone().into(),
        name: room.name.clone().into(),
        kind: RecentKind::Room { unread: false },
    });
    ShellData {
        connection,
        host: state.host.clone().map(SharedString::from),
        saved: saved
            .iter()
            .map(|pc| SavedHost {
                id: pc.id.clone().into(),
                name: pc.name.clone().into(),
            })
            .collect(),
        recents: bots.chain(rooms).collect(),
        ..base.clone()
    }
}

/// `desktop-ui.json`, the one copy this run reads and writes (T1.5): the saved PCs here, appearance in Settings.
pub struct LocalSettings {
    root: PathBuf,
    settings: Mutex<DeviceSettings>,
}

impl LocalSettings {
    pub fn load(root: &Path) -> Self {
        LocalSettings {
            root: root.to_owned(),
            settings: Mutex::new(device_settings::load_or_import(root)),
        }
    }

    pub fn read(&self) -> DeviceSettings {
        locked(&self.settings).clone()
    }

    /// Changes the settings and writes the file; a write that fails is logged and the change kept for this run.
    pub fn update(&self, change: impl FnOnce(&mut DeviceSettings)) {
        if let Err(error) = self.try_update(change) {
            log::warn!("device settings: couldn't write them ({error})");
        }
    }

    /// `update`, saying whether the file was written: the change is kept for this run either way.
    pub fn try_update(&self, change: impl FnOnce(&mut DeviceSettings)) -> std::io::Result<()> {
        let mut settings = locked(&self.settings);
        change(&mut settings);
        device_settings::save(&self.root, &settings)
    }
}

impl HostStore for LocalSettings {
    fn hosts(&self) -> Vec<RemoteHost> {
        locked(&self.settings).hosts.clone()
    }

    fn pending(&self) -> Vec<RemoteHost> {
        locked(&self.settings).pending.clone()
    }

    fn set_hosts(&self, hosts: Vec<RemoteHost>) {
        self.update(|settings| settings.hosts = hosts);
    }

    fn set_pending(&self, hosts: Vec<RemoteHost>) {
        self.update(|settings| settings.pending = hosts);
    }
}

/// How this run reaches this PC's daemon.
enum Daemon {
    /// Started and kept running here.
    Supervised {
        launch: Box<dyn Fn() -> DaemonLaunch + Send + Sync>,
        supervisor: Mutex<Option<Supervisor>>,
        state: Mutex<watch::Receiver<DaemonState>>,
    },
    /// A development daemon started with `--dev`: its token file.
    Development { env: Env },
    /// There is no daemon to start; the text says why.
    Unavailable(String),
}

/// The desktop's `Environment`. Its parts are shared, so the app reaches them after the connection owns it.
pub struct DesktopEnv(Arc<Shared>);

struct Shared {
    runtime: tokio::runtime::Handle,
    root: PathBuf,
    daemon: Daemon,
    settings: Arc<LocalSettings>,
    keys: Option<DeviceKeys>,
    dialer: NetworkDialer,
    /// The connection this environment serves: told when the daemon restarts or fails.
    connection: OnceLock<Weak<Live>>,
    /// The browser, Logins and worker clients of this PC's daemon: started for each daemon this run starts, never for a
    /// saved PC, and stopped before the daemon restarts or the app quits.
    hosts: HostControl,
    /// The stop "Try again" has begun: it takes the supervisor out of its slot, so a quit that arrives meanwhile finds the
    /// slot empty and has to wait for this instead of reading the empty slot as a daemon that is already stopped.
    stopping: watch::Sender<Stopping>,
}

/// Where a daemon stop that began on "Try again" stands.
#[derive(Debug, Clone, PartialEq)]
enum Stopping {
    /// None is running. What the slot holds (or does not) is the whole story.
    Idle,
    /// The clients and then the daemon are being stopped.
    InFlight,
    /// The stop is over, and how it went: `Ok(None)` is that there was nothing to stop.
    Done(Result<Option<DaemonStop>, String>),
}

type SavedHosts<'a> =
    Hosts<'a, LocalSettings, desktop_host::device_keys::WindowsCredentials, NetworkDialer>;

impl Shared {
    fn hosts(&self) -> Option<SavedHosts<'_>> {
        self.keys.as_ref().map(|keys| Hosts {
            store: &*self.settings,
            keys,
            dialer: &self.dialer,
        })
    }

    /// Starts a new supervisor (the first, or after "Try again"), and tells the connection whatever it reports. A Try
    /// again comes from the UI thread, which has no Tokio runtime (`Supervisor::start` spawns on one): the start runs on
    /// the live runtime, and only after the last supervisor and its daemon have stopped, so the new daemon never races
    /// the old one for its port.
    fn supervise(self: &Arc<Self>) {
        let Daemon::Supervised { supervisor, .. } = &self.daemon else {
            return;
        };
        // A quit that began first stops the daemon itself (`Stopper::stop`): it must find it in its slot.
        if self.hosts.quitting() {
            return;
        }
        let old = locked(supervisor).take();
        // The stop in flight is this one, whether or not there is a daemon to stop: an outcome left by an earlier stop, which
        // a quit may have read already, must not be read again by the next quit.
        self.stopping.send_replace(Stopping::InFlight);
        let shared = Arc::clone(self);
        // unguarded: a tokio task, off the UI thread, where a panic ends only that task
        self.runtime.spawn(async move {
            // The clients of the daemon that is going stop first: the vault locks and nothing polls a dying daemon.
            let _ =
                tokio::time::timeout(crate::hosts::STOP_TIMEOUT, shared.hosts.stop(false)).await;
            let outcome = match old {
                Some(old) => crate::services::daemon_stop(
                    old.stop_before_quit(daemon_process::QUIT_TIMEOUT).await,
                ),
                None => Ok(None),
            };
            // A quit that arrived meanwhile is waiting for exactly this; it is the last word if no daemon starts below.
            shared.stopping.send_replace(Stopping::Done(outcome));
            // A quit during the stops above: no new daemon starts while the app exits.
            if shared.start_supervisor() {
                shared.stopping.send_replace(Stopping::Idle);
            }
        });
    }

    /// The supervisor's start, and the task that follows what it reports. Runs on the live runtime. `false` when none was
    /// started: the daemon is not supervised here, or the app is quitting.
    fn start_supervisor(&self) -> bool {
        let Daemon::Supervised {
            launch,
            supervisor,
            state,
        } = &self.daemon
        else {
            return false;
        };
        // The check and the store are one step under the slot's lock. A quit sets `quitting` and then takes the slot, so it
        // either finds this supervisor in it, or the app was already quitting here and none starts: it can no longer take an
        // empty slot while a daemon is being made that lands in it a moment later.
        let mut slot = locked(supervisor);
        if self.hosts.quitting() {
            return false;
        }
        let started = Supervisor::start(launch(), Arc::new(log_daemon_output));
        let mut watching = started.state();
        *locked(state) = started.state();
        *slot = Some(started);
        drop(slot);
        let connection = self.connection.get().cloned();
        let hosts = self.hosts.clone();
        // unguarded: a tokio task, off the UI thread, where a panic ends only that task
        self.runtime.spawn(async move {
            guarded_async("follow the daemon", async move {
                let mut told: Option<String> = None;
                // The daemon the clients were started for, by its token.
                let mut hosted: Option<String> = None;
                loop {
                    let (report, local) = match &*watching.borrow_and_update() {
                        DaemonState::Running {
                            connection,
                            desktop_host_token,
                        } => (
                            Some(Ok(DesktopConnection {
                                port: connection.port,
                                token: connection.token.expose().to_owned(),
                            })),
                            Some(Local {
                                port: connection.port,
                                token: connection.token.expose().to_owned(),
                                host_token: HostToken::new(desktop_host_token.expose()),
                            }),
                        ),
                        DaemonState::Failed(reason) => (Some(Err(reason.clone())), None),
                        DaemonState::Starting => (None, None),
                    };
                    match local {
                        // A restart gives new tokens, so new clients; the UI thread stops the old ones first.
                        Some(local) if hosted.as_deref() != Some(&local.token) => {
                            hosted = Some(local.token.clone());
                            hosts.start(local);
                        }
                        Some(_) => {}
                        // The daemon stopped or failed: nothing is left to serve, and the vault locks.
                        None => {
                            if hosted.take().is_some() {
                                hosts.stop_now();
                            }
                        }
                    }
                    let live = connection.as_ref().and_then(Weak::upgrade);
                    match (report, live) {
                        // A restart gives a new token; the first start's was already asked for (`local_daemon`).
                        (Some(Ok(next)), Some(live)) if told.as_deref() != Some(&next.token) => {
                            told = Some(next.token.clone());
                            // unguarded: the connection's own method, not a handler
                            live.on_connection(next);
                        }
                        (Some(Err(reason)), Some(live)) => {
                            // unguarded: the connection's own method, not a handler
                            live.on_daemon_failed(reason);
                        }
                        _ => {}
                    }
                    if watching.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        });
        true
    }
}

fn log_daemon_output(text: &str) {
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        log::info!("daemon: {line}");
    }
}

impl Environment for DesktopEnv {
    type Link = Client;

    fn is_android(&self) -> bool {
        false
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn spawn(&self, task: BoxFuture<'static, ()>) {
        // unguarded: a tokio task, off the UI thread, where a panic ends only that task
        self.0.runtime.spawn(task);
    }

    fn local_daemon(&self) -> impl Future<Output = Result<DesktopConnection, String>> + Send {
        self.0.local_daemon()
    }

    fn restart_daemon(&self) {
        self.0.restart_daemon();
    }

    fn connect_local(
        &self,
        connection: &DesktopConnection,
    ) -> impl Future<Output = Result<Client, Failure>> + Send {
        let (port, token) = (connection.port, connection.token.clone());
        async move {
            Client::connect(Endpoint::Local { port }, &token)
                .await
                .map_err(|error| Failure::from(&error))
        }
    }

    fn saved_hosts(&self) -> Vec<SavedPc> {
        self.0.saved_hosts()
    }

    async fn open_remote(&self, id: &str) -> Result<Client, Failure> {
        match self.0.hosts() {
            Some(hosts) => hosts.open(id).await.map_err(|error| failure_of(&error)),
            None => Err(Failure::Other(NO_KEYS.to_owned())),
        }
    }

    fn forget_host(&self, id: &str) -> impl Future<Output = Result<(), String>> + Send {
        let forgotten = match self.0.hosts() {
            Some(hosts) => hosts.forget(id).map_err(|error| error.to_string()),
            None => Err(NO_KEYS.to_owned()),
        };
        async move { forgotten }
    }

    fn invalidate_files(
        &self,
        _scope: FilesScope,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        // The desktop has no native file host yet (T4.4 reads attachments straight from the daemon): nothing to revoke.
        std::future::ready(Ok(()))
    }

    fn keep_alive_running(&self) -> bool {
        false
    }

    fn sync_keep_alive(&self, _status: &state::Connection, _host_name: Option<&str>) {}

    fn register_push(&self, _link: &Client) -> impl Future<Output = Result<(), String>> + Send {
        std::future::ready(Ok(()))
    }

    fn stop_sending_push(&self) {}

    fn unregister_push(&self) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

const NO_KEYS: &str = "This PC's saved connection keys can't be read.";

impl Shared {
    fn saved_hosts(&self) -> Vec<SavedPc> {
        self.settings
            .hosts()
            .into_iter()
            .map(|host| SavedPc {
                id: host.id,
                name: host.name.as_str().to_owned(),
            })
            .collect()
    }

    fn local_daemon(
        &self,
    ) -> impl Future<Output = Result<DesktopConnection, String>> + Send + 'static {
        let found = match &self.daemon {
            Daemon::Supervised { state, .. } => Ok(locked(state).clone()),
            Daemon::Development { env } => {
                Err(daemon_process::development_connection(env, &self.root)
                    .map(|connection| DesktopConnection {
                        port: connection.port,
                        token: connection.token.expose().to_owned(),
                    })
                    .map_err(|error| error.to_string()))
            }
            Daemon::Unavailable(reason) => Err(Err(reason.clone())),
        };
        async move {
            let mut state = match found {
                Ok(state) => state,
                Err(answer) => return answer,
            };
            loop {
                let answer = match &*state.borrow_and_update() {
                    DaemonState::Running { connection, .. } => Some(Ok(DesktopConnection {
                        port: connection.port,
                        token: connection.token.expose().to_owned(),
                    })),
                    DaemonState::Failed(reason) => Some(Err(reason.clone())),
                    DaemonState::Starting => None,
                };
                if let Some(answer) = answer {
                    return answer;
                }
                if state.changed().await.is_err() {
                    return Err(daemon_process::KEEPS_STOPPING.to_owned());
                }
            }
        }
    }

    fn restart_daemon(self: &Arc<Self>) {
        match &self.daemon {
            Daemon::Supervised { .. } => self.supervise(),
            Daemon::Development { .. } | Daemon::Unavailable(_) => {
                // Nothing to restart here: ask again, which reads the token file (or the reason) afresh.
                let Some(live) = self.connection.get().and_then(Weak::upgrade) else {
                    return;
                };
                let answer = self.local_daemon();
                // unguarded: a tokio task, off the UI thread, where a panic ends only that task
                self.runtime.spawn(async move {
                    match answer.await {
                        // unguarded: the connection's own method, not a handler
                        Ok(next) => live.on_connection(next),
                        // unguarded: the connection's own method, not a handler
                        Err(reason) => live.on_daemon_failed(reason),
                    }
                });
            }
        }
    }
}

/// The exe the daemon runs from, or why there is none: the bundled one in a production build, otherwise
/// `TJOMMIE_DAEMON_EXE` or the debug build in this exe's target folder.
fn daemon_exe(identity: &Identity, env: &[(String, String)]) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let folder = exe.parent().unwrap_or(Path::new("."));
    if identity.kind == Kind::Production {
        return Ok(daemon_process::packaged_daemon_path(
            &folder.join("resources"),
        ));
    }
    // target\debug\tjommie-desktop.exe: the repo is three folders up from the exe's.
    let repo = folder.ancestors().nth(3).unwrap_or(folder);
    daemon_process::daemon_command(env, None, repo, Path::exists).map_err(|error| error.to_string())
}

/// Starts the live connection: the runtime, the daemon (or the dev daemon), the connection, its timers, and the feed into
/// the store and the shell. Without a runtime the app stays on "Starting Tjommie", says why in the log, and still runs.
pub fn start(identity: Identity, root: PathBuf, cx: &mut App) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tjommie-live")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            log::error!("No runtime for the connection: {error}");
            return;
        }
    };
    // This device's local dictation runs on the same runtime.
    crate::speech::start(&identity, &root, runtime.handle(), cx);
    // The browser panel's logins menu reaches the selected PC's vault through the app's logins client.
    cx.default_global::<crate::ui::browser::BrowserData>()
        .on_logins = Some(std::rc::Rc::new(crate::ui::logins::browser_logins));
    let base: Env = std::env::vars().collect();
    let env = launch_env(&identity, &root, &base);
    let daemon = if identity.kind == Kind::Dev && root.join("daemon-token").exists() {
        Daemon::Development { env }
    } else {
        match daemon_exe(&identity, &env) {
            Ok(exe) => {
                let (_, idle) = watch::channel(DaemonState::Starting);
                Daemon::Supervised {
                    launch: Box::new(move || DaemonLaunch::new(exe.clone(), env.clone())),
                    supervisor: Mutex::new(None),
                    state: Mutex::new(idle),
                }
            }
            Err(reason) => Daemon::Unavailable(reason),
        }
    };
    cx.set_global(DataRoot(root.clone()));
    let keys = DeviceKeys::new(&root)
        .inspect_err(|error| log::warn!("No device keys: {error}"))
        .ok();
    let settings = Arc::new(LocalSettings::load(&root));
    // This device's appearance shows from the first frame; Settings, Appearance saves its changes through the file.
    let stored = settings.read();
    crate::ui::style::show_appearance(
        crate::ui::style::Appearance {
            mode: stored.theme,
            text: stored.text_size,
            density: stored.density,
        },
        cx,
    );
    cx.set_global(DeviceFile(Arc::clone(&settings)));
    // The login vault, its gate and the control of the browser, Logins and worker clients of this PC's daemon. They start
    // when the daemon is running with its host token (`start_supervisor`), on the UI thread once the window has its engine.
    let (app_hosts, orders, requests) = crate::hosts::create(&root, runtime.handle(), cx);
    let shared = Arc::new(Shared {
        runtime: runtime.handle().clone(),
        settings,
        root,
        daemon,
        keys,
        dialer: NetworkDialer::new(),
        connection: OnceLock::new(),
        hosts: app_hosts.control.clone(),
        stopping: watch::channel(Stopping::Idle).0,
    });
    cx.set_global(DaemonOwner(Arc::new(Stopper(Arc::clone(&shared)))));
    crate::hosts::serve(app_hosts.clone(), orders, requests, cx);
    cx.set_global(app_hosts);
    let live = AppConnection::new(DesktopEnv(Arc::clone(&shared)));
    let _ = shared.connection.set(Arc::downgrade(&live));
    feed(&live, &shared, cx);
    {
        let _context = runtime.enter();
        // The first start is synchronous, inside the runtime: its state receiver is in place before the connection's
        // first read, which otherwise could read the placeholder (its sender long gone) and say the service keeps
        // stopping (M5 re-review finding 5). `supervise` is for Try again.
        shared.start_supervisor();
        timers(&live, runtime.handle());
        live.start_connection();
    }
    crate::ui::store::connect(cx, live);
    cx.set_global(Tokio(runtime.handle().clone()));
    cx.set_global(Runtime(Some(runtime)));
}

/// What a quit asks of whoever owns this PC's daemon: stop it, and start it again when the quit is given up. The app's
/// owner is [`Stopper`]; a test answers for itself.
pub trait DaemonControl: Send + Sync {
    /// Stops the daemon and says how it went: `Ok(Some(..))` gracefully or only by force (`stopDaemonBeforeQuit`),
    /// `Ok(None)` when there was nothing to stop, `Err` when its stop could not be confirmed.
    fn stop(&self) -> BoxFuture<'static, Result<Option<DaemonStop>, String>>;
    /// Starts the daemon again after a stop the app gave up on (an update that was not installed).
    fn restart_daemon(&self);
}

/// The daemon's owner, for the quit.
pub struct DaemonOwner(pub Arc<dyn DaemonControl>);

impl gpui::Global for DaemonOwner {}

/// Stops this PC's daemon when the app quits: the supervisor stops it within the quit timeout, then by force.
#[derive(Clone)]
pub struct Stopper(Arc<Shared>);

impl DaemonControl for Stopper {
    fn stop(&self) -> BoxFuture<'static, Result<Option<DaemonStop>, String>> {
        Stopper::stop(self).boxed()
    }

    fn restart_daemon(&self) {
        Stopper::restart_daemon(self);
    }
}

/// Waits for the stop "Try again" has in flight, and says how it went. With none in flight there is nothing to stop.
async fn wait_for_stop(
    mut stopping: watch::Receiver<Stopping>,
) -> Result<Option<DaemonStop>, String> {
    loop {
        match &*stopping.borrow_and_update() {
            Stopping::Idle => return Ok(None),
            Stopping::Done(outcome) => return outcome.clone(),
            Stopping::InFlight => {}
        }
        if stopping.changed().await.is_err() {
            return Ok(None);
        }
    }
}

impl Stopper {
    /// Stops the daemon and says how it went (see [`DaemonControl::stop`]). Nothing to stop for a development daemon or
    /// none. A daemon that "Try again" took out of its slot is being stopped already: this waits for that stop and reports
    /// how it went, so a quit never goes on (and an installer never starts) beside a daemon that is still stopping.
    pub fn stop(&self) -> impl Future<Output = Result<Option<DaemonStop>, String>> + Send + use<> {
        // From here no daemon restarts (a "Try again" in flight, a supervisor's own restart) and no client starts.
        self.0.hosts.begin_quit();
        let old = match &self.0.daemon {
            Daemon::Supervised { supervisor, .. } => locked(supervisor).take(),
            Daemon::Development { .. } | Daemon::Unavailable(_) => None,
        };
        let in_flight = self.0.stopping.subscribe();
        // unguarded: a tokio task, off the UI thread, where a panic ends only that task
        let stopped = self.0.runtime.spawn(async move {
            match old {
                Some(old) => crate::services::daemon_stop(
                    old.stop_before_quit(daemon_process::QUIT_TIMEOUT).await,
                ),
                None => wait_for_stop(in_flight).await,
            }
        });
        async move {
            stopped
                .await
                .unwrap_or_else(|_| Err("The background service's stop did not finish.".to_owned()))
        }
    }

    /// Starts the daemon again after a stop the app gave up on (an update that was not installed): the connection asks for
    /// it as "Try again" does, and the clients start with the new daemon.
    pub fn restart_daemon(&self) {
        if let Some(live) = self.0.connection.get().and_then(Weak::upgrade) {
            live.restart_daemon();
        }
    }
}

/// The daemon's owner, once the live connection is started.
pub fn stopper(cx: &App) -> Option<Arc<dyn DaemonControl>> {
    cx.try_global::<DaemonOwner>().map(|owner| owner.0.clone())
}

/// The live runtime's handle, for UI code that calls a desktop-host function which spawns on Tokio while it also
/// awaits the UI (Diagnostics' Save report: its write runs on `spawn_blocking`, its picker on GPUI). See [`in_runtime`].
pub struct Tokio(pub tokio::runtime::Handle);

impl gpui::Global for Tokio {}

/// Polls `future` with the live runtime entered, so what it spawns on Tokio finds a runtime, while it stays on the
/// thread that polls it (the UI's, for a future that awaits the UI). With no runtime it polls the future as it is.
pub async fn in_runtime<T>(
    handle: Option<tokio::runtime::Handle>,
    future: impl Future<Output = T>,
) -> T {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        let _context = handle.as_ref().map(tokio::runtime::Handle::enter);
        future.as_mut().poll(cx)
    })
    .await
}

/// This run's data root (`main`'s, checked with `owner_safety`): where an agent's notes.md lives (`<root>/bots`).
pub struct DataRoot(pub PathBuf);

/// This device's settings file (`desktop-ui.json`), for the sections that change it (Appearance).
pub struct DeviceFile(pub Arc<LocalSettings>);

impl gpui::Global for DeviceFile {}

impl gpui::Global for DataRoot {}

/// The runtime lives as long as the app.
struct Runtime(#[allow(dead_code, reason = "held, not read")] Option<tokio::runtime::Runtime>);

impl gpui::Global for Runtime {}

/// Runs the connection's timers (sans-IO: it says when, this waits and tells it).
fn timers(live: &Arc<Live>, runtime: &tokio::runtime::Handle) {
    let live = Arc::clone(live);
    // unguarded: a tokio task, off the UI thread, where a panic ends only that task
    runtime.spawn(async move {
        guarded_async("run the connection's timers", async move {
            let mut changed = live.timers_changed();
            loop {
                let next = live.next_timer();
                let due = async {
                    match next {
                        Some(at) => tokio::time::sleep_until(at.into()).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    // unguarded: the connection's own method, not a handler
                    () = due => live.on_timer(Instant::now()),
                    seen = changed.changed() => if seen.is_err() { break },
                }
            }
        })
        .await;
    });
}

/// How long the feed waits before it copies the state again: one frame, so a burst of events is one copy (M5 review M5).
const FRAME: Duration = Duration::from_millis(16);

/// How long to wait before the next copy of the state, given when the last one was made: the rest of the frame.
fn before_next_copy(last: Option<Instant>, now: Instant, frame: Duration) -> Duration {
    last.map_or(Duration::ZERO, |last| {
        frame.saturating_sub(now.saturating_duration_since(last))
    })
}

/// Copies the connection's state into the store, the shell and the browser panel after each change, at most once a frame
/// however many changes came (the copy is a pointer, and nothing is compared unless the connection's version moved), and
/// gives the shell its commands. A browser stream frame from another PC is handed to the panel's session for it.
fn feed(live: &Arc<Live>, shared: &Arc<Shared>, cx: &mut App) {
    let (changed, mut changes) = futures::channel::mpsc::unbounded::<()>();
    let (frames, mut streamed) =
        futures::channel::mpsc::unbounded::<protocol::browser::BrowserStreamFrame>();
    // unguarded: the connection's own listener, not a GPUI handler; it only sends a signal (inside the connection's lock)
    live.subscribe(move |_| {
        let _ = changed.unbounded_send(());
    });
    // unguarded: the connection's own listener, not a GPUI handler; it only sends the frame (inside the connection's lock)
    live.subscribe_events(move |event| {
        if let protocol::Event::BrowserStreamFrame(frame) = event {
            let _ = frames.unbounded_send(frame.clone());
        }
    });
    let commands = Arc::clone(live);
    let base = ShellData {
        on_command: Some(std::rc::Rc::new(move |command, _, cx| {
            guarded("run a shell command", || {
                command_for(&commands, command, cx)
            });
        })),
        ..ShellData::default()
    };
    cx.set_global(base);
    cx.spawn(async move |cx| {
        guarded_async("hand on the streamed pictures", async move {
            while let Some(frame) = streamed.next().await {
                cx.update(|cx| {
                    guarded("show a streamed picture", || {
                        crate::ui::browser::remote::stream_frame(frame, cx)
                    });
                });
            }
        })
        .await;
    })
    .detach();
    let live = Arc::clone(live);
    let shared = Arc::clone(shared);
    cx.spawn(async move |cx| {
        guarded_async("follow the connection", async move {
            // What the last copy showed: the state's version and the saved PCs, which the state doesn't hold.
            let mut shown: Option<(u64, Vec<(String, String)>)> = None;
            loop {
                let (version, state) = live.state_and_version();
                let saved: Vec<(String, String)> = shared
                    .saved_hosts()
                    .into_iter()
                    .map(|pc| (pc.id, pc.name))
                    .collect();
                if shown
                    .as_ref()
                    .is_none_or(|(seen, kept)| *seen != version || *kept != saved)
                {
                    cx.update(|cx| {
                        guarded("show the connection's state", || {
                            let base = cx.try_global::<ShellData>().cloned().unwrap_or_default();
                            let pcs: Vec<SavedPc> = saved
                                .iter()
                                .map(|(id, name)| SavedPc {
                                    id: id.clone(),
                                    name: name.clone(),
                                })
                                .collect();
                            let shell = shell_data(&state, &pcs, &base);
                            cx.set_global(shell);
                            crate::ui::store::mirror(cx, version, Arc::clone(&state));
                            crate::ui::browser::feed(&state, cx);
                            cx.refresh_windows();
                        });
                    });
                    shown = Some((version, saved));
                }
                let last = Instant::now();
                if changes.next().await.is_none() {
                    break;
                }
                // Changes that came in this frame are all in the next copy.
                let wait = before_next_copy(Some(last), Instant::now(), FRAME);
                if !wait.is_zero() {
                    cx.background_executor().timer(wait).await;
                }
                while changes.try_recv().is_ok() {}
            }
        })
        .await;
    })
    .detach();
}

fn command_for(live: &Arc<Live>, command: Command, cx: &mut App) {
    match command {
        Command::Retry => live.restart_daemon(),
        Command::ConnectLocal => live.connect_local(),
        Command::ConnectRemote(id) => live.connect_remote(Some(&id)),
        Command::ConnectAnother | Command::PairAgain => {
            use crate::ui::shell::route::{Navigate as _, Route, Section};
            cx.navigate(Route::Settings(Section::Devices));
        }
        Command::Forget(id) => {
            let live = Arc::clone(live);
            cx.spawn(async move |cx| {
                guarded_async("forget a PC", async move {
                    let forgotten = live.forget_remote(&id).await;
                    cx.update(|cx| {
                        cx.default_global::<ShellData>().problem =
                            forgotten.err().map(SharedString::from);
                        cx.refresh_windows();
                    });
                })
                .await;
            })
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::ui::testing::{bot, room};
    use protocol::model::{BotColor, BotShape, BotState};

    fn value<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        daemon_process::env_get(env, name)
    }

    /// M5 review M5: the feed copies the state at most once a frame, so a stream of events is one copy per frame.
    #[test]
    fn the_feed_waits_out_the_rest_of_the_frame_before_it_copies_again() {
        let frame = Duration::from_millis(16);
        let now = Instant::now();
        assert_eq!(
            before_next_copy(None, now, frame),
            Duration::ZERO,
            "the first copy is at once"
        );
        assert_eq!(
            before_next_copy(Some(now), now + Duration::from_millis(4), frame),
            Duration::from_millis(12),
            "a change 4 ms after a copy waits for the frame's remaining 12 ms"
        );
        assert_eq!(
            before_next_copy(Some(now), now + Duration::from_millis(40), frame),
            Duration::ZERO,
            "a change after a quiet frame is copied at once"
        );
    }

    /// A supervised `Shared` whose daemon launch only counts: the exe is not there, so nothing really starts. The UI
    /// thread's side of the clients' stop is a task that answers each `Stop` order (while `answering` is open, so a test can
    /// hold a stop in flight), as the app's `serve` loop does.
    /// A held launch: it says it has begun (the sender), then waits to be let go (the receiver).
    type HeldLaunch = Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>;

    struct Fixture {
        shared: Arc<Shared>,
        launches: Arc<AtomicUsize>,
        answering: watch::Sender<bool>,
        /// The next launch, if a test holds it: it says it has begun, then waits to be let go.
        held: Arc<Mutex<HeldLaunch>>,
    }

    impl Fixture {
        /// Holds the next daemon launch: the first end hears that it has begun, the second lets it go.
        fn hold_the_next_launch(
            &self,
        ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
            let (begun, hears) = std::sync::mpsc::channel();
            let (lets_go, waits) = std::sync::mpsc::channel();
            *self.held.lock().unwrap() = Some((begun, waits));
            (hears, lets_go)
        }
    }

    fn counting_shared(root: &Path) -> Fixture {
        let launches = Arc::new(AtomicUsize::new(0));
        let counted = launches.clone();
        let held: Arc<Mutex<HeldLaunch>> = Arc::default();
        let holding = held.clone();
        let (hosts, orders) = HostControl::new(tokio::runtime::Handle::current());
        let (answering, mut open) = watch::channel(true);
        tokio::spawn(async move {
            while let Ok(order) = orders.recv().await {
                if let crate::hosts::Order::Stop { done, .. } = order {
                    let _ = open.wait_for(|open| *open).await;
                    let _ = done.send(());
                }
            }
        });
        let shared = Arc::new(Shared {
            runtime: tokio::runtime::Handle::current(),
            root: root.to_owned(),
            daemon: Daemon::Supervised {
                launch: Box::new(move || {
                    counted.fetch_add(1, Ordering::SeqCst);
                    let hold = holding.lock().unwrap().take();
                    if let Some((begun, waits)) = hold {
                        let _ = begun.send(());
                        let _ = waits.recv();
                    }
                    DaemonLaunch::new(root_exe(), Vec::new())
                }),
                supervisor: Mutex::new(None),
                state: Mutex::new(watch::channel(DaemonState::Starting).1),
            },
            settings: Arc::new(LocalSettings::load(root)),
            keys: None,
            dialer: NetworkDialer::new(),
            connection: OnceLock::new(),
            hosts,
            stopping: watch::channel(Stopping::Idle).0,
        });
        Fixture {
            shared,
            launches,
            answering,
            held,
        }
    }

    fn root_exe() -> PathBuf {
        std::env::temp_dir().join("tjommie-no-such-daemon.exe")
    }

    /// Review Minor 2: a quit during "Try again" must not leave a new daemon starting while the app exits.
    #[tokio::test]
    async fn no_daemon_starts_once_the_app_is_quitting() {
        let root = std::env::temp_dir().join(format!("tjommie-live-quit-{}", std::process::id()));
        let first = counting_shared(&root);
        let (shared, launches) = (first.shared.clone(), first.launches.clone());

        // The quit began before "Try again" was pressed.
        drop(shared.hosts.stop(true));
        shared.supervise();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            launches.load(Ordering::SeqCst),
            0,
            "a daemon started in a quit"
        );
        shared.hosts.resume();

        // The quit began after it: the stop of the old daemon is still running when the restart would start the new one.
        let second = counting_shared(&root);
        let (racing, raced) = (second.shared.clone(), second.launches.clone());
        racing.supervise();
        drop(racing.hosts.stop(true));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            raced.load(Ordering::SeqCst),
            0,
            "a daemon started while the quit stopped the old one"
        );

        // With no quit it starts, as it always did.
        shared.supervise();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(launches.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Re-review M2: a quit that begins during "Try again" finds the supervisor's slot empty (the Try again took it), and must
    /// not read that as a daemon that has stopped: it waits for the stop in flight and says how it went, so the installer
    /// can never start beside a daemon that is still stopping.
    #[tokio::test]
    async fn a_quit_during_try_again_waits_for_the_stop_in_flight() {
        let root =
            std::env::temp_dir().join(format!("tjommie-live-tryagain-{}", std::process::id()));
        let f = counting_shared(&root);
        // A daemon in the slot (its exe is not there, so it fails to start, which is enough to be stopped).
        f.shared.start_supervisor();
        assert_eq!(f.launches.load(Ordering::SeqCst), 1);
        // "Try again": the supervisor leaves its slot and its stop begins, held at the clients' stop (the UI thread is slow).
        f.answering.send_replace(false);
        f.shared.supervise();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let quitting = Stopper(f.shared.clone()).stop();
        tokio::pin!(quitting);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut quitting)
                .await
                .is_err(),
            "the quit read the empty slot as a daemon that had stopped, with its stop still in flight"
        );
        // The stop finishes; the quit hears how it went, and nothing started a daemon in a quit.
        f.answering.send_replace(true);
        let outcome = tokio::time::timeout(Duration::from_secs(10), quitting)
            .await
            .expect("the quit never heard that the stop had finished");
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(
            f.launches.load(Ordering::SeqCst),
            1,
            "a daemon started in a quit"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Check Nit 1: an outcome an earlier stop left, which a quit has already read, must not be read again by the next quit. A
    /// restart that has nothing to stop (after an aborted update, the daemon already gone) publishes its own stop first.
    #[tokio::test]
    async fn a_restart_does_not_leave_the_last_stops_outcome_for_the_next_quit() {
        let root = std::env::temp_dir().join(format!("tjommie-live-stale-{}", std::process::id()));
        let f = counting_shared(&root);
        // A quit read a forced stop and was given up; the daemon is restarted, and nothing is in the slot.
        f.shared
            .stopping
            .send_replace(Stopping::Done(Ok(Some(DaemonStop::Forced))));
        f.answering.send_replace(false);
        f.shared.supervise();
        let quitting = Stopper(f.shared.clone()).stop();
        tokio::pin!(quitting);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut quitting)
                .await
                .is_err(),
            "the quit read the outcome of a stop that was over, with the restart's own still going"
        );
        f.answering.send_replace(true);
        let outcome = tokio::time::timeout(Duration::from_secs(10), quitting)
            .await
            .expect("the quit never heard that the restart's stop had finished");
        assert_eq!(outcome, Ok(None), "the restart stopped nothing");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Check Nit 2: a daemon being started is either in the slot when a quit looks for it, or not started at all. The quit sets
    /// `quitting` and then takes the slot; the start checks `quitting` and stores the supervisor under the slot's lock, so the
    /// quit can no longer take an empty slot while a daemon is being made that lands in it a moment later.
    #[tokio::test]
    async fn a_quit_that_arrives_while_a_daemon_is_being_started_finds_it_in_the_slot() {
        let root = std::env::temp_dir().join(format!("tjommie-live-racing-{}", std::process::id()));
        let f = counting_shared(&root);
        let (begun, lets_go) = f.hold_the_next_launch();
        let handle = tokio::runtime::Handle::current();
        let shared = f.shared.clone();
        // The start is under way (its launch is held), on a thread of its own as it is on the live runtime.
        let starting = std::thread::spawn(move || {
            let _runtime = handle.enter();
            shared.start_supervisor();
        });
        begun
            .recv_timeout(Duration::from_secs(10))
            .expect("the daemon's launch never began");
        // The quit arrives meanwhile (on its own thread too, since looking for the daemon may have to wait for the start).
        let (stopping, quitting) = std::sync::mpsc::channel();
        let shared = f.shared.clone();
        let quit = std::thread::spawn(move || {
            let _ = stopping.send(Stopper(shared).stop());
        });
        while !f.shared.hosts.quitting() {
            std::thread::yield_now();
        }
        lets_go.send(()).unwrap();
        starting.join().unwrap();
        quit.join().unwrap();
        let quitting = quitting.recv().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), quitting)
                .await
                .expect("the quit never finished")
                .is_ok()
        );
        let Daemon::Supervised { supervisor, .. } = &f.shared.daemon else {
            panic!("the fixture daemon is supervised");
        };
        assert!(
            locked(supervisor).is_none(),
            "a daemon started under a quit was left running in the slot"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Re-review Nit 6: with nothing running and nothing in flight there is nothing to stop, and the quit is told so.
    #[tokio::test]
    async fn a_quit_with_no_daemon_has_nothing_to_stop() {
        let root =
            std::env::temp_dir().join(format!("tjommie-live-nothing-{}", std::process::id()));
        let f = counting_shared(&root);
        assert_eq!(Stopper(f.shared.clone()).stop().await, Ok(None));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_e2e_daemon_takes_any_port_and_keeps_phones_on_loopback() {
        let base = vec![("PATH".to_owned(), "C:/Windows".to_owned())];
        let env = launch_env(
            &desktop_host::identity::E2E,
            Path::new("C:/Temp/root"),
            &base,
        );
        assert_eq!(value(&env, "TJOMMIE_HOME"), Some("C:/Temp/root"));
        assert_eq!(value(&env, "TJOMMIE_PORT"), Some("0"));
        assert_eq!(value(&env, "TJOMMIE_LAN_PORT"), Some("0"));
        assert_eq!(value(&env, "TJOMMIE_LAN_BIND"), Some("127.0.0.1"));
        assert_eq!(value(&env, "PATH"), Some("C:/Windows"));
    }

    #[test]
    fn a_dev_daemon_gets_the_dev_ports_and_the_root() {
        let env = launch_env(&desktop_host::identity::DEV, Path::new("C:/Temp/dev"), &[]);
        assert_eq!(value(&env, "TJOMMIE_HOME"), Some("C:/Temp/dev"));
        assert_eq!(value(&env, "TJOMMIE_PORT"), Some("47200"));
        assert_eq!(value(&env, "TJOMMIE_LAN_PORT"), Some("47201"));
        assert_eq!(value(&env, "TJOMMIE_LAN_BIND"), None);
    }

    #[test]
    fn a_saved_pc_that_fails_says_why_as_the_connection_reads_it() {
        use protocol::remote::RefusedReason;
        assert_eq!(
            failure_of(&HostError::Dial(DialError::Refused(RefusedReason::Revoked))),
            Failure::Refused
        );
        assert_eq!(
            failure_of(&HostError::Dial(DialError::HostOffline)),
            Failure::HostOffline(DialError::HostOffline.to_string())
        );
        assert_eq!(
            failure_of(&HostError::Dial(DialError::Failed(
                ConnectError::ProtocolMismatch.to_string()
            ))),
            Failure::Update
        );
        assert_eq!(
            failure_of(&HostError::NotPaired),
            Failure::Other(HostError::NotPaired.to_string())
        );
    }

    #[test]
    fn the_shell_shows_the_connection_and_the_agents_and_rooms_as_recents() {
        let mut forge = bot("b1", "Forge", BotState::Waiting);
        forge.color = BotColor::Teal;
        forge.shape = BotShape::Cloud;
        let state = AppState {
            connection: state::Connection::Offline {
                reason: state::OfflineReason::HostOffline,
            },
            host: Some("pc-1".into()),
            bots: vec![forge, bot("b2", "Scout", BotState::Working)],
            rooms: vec![room("r1", "Launch crew", &["b1"])],
            ..AppState::default()
        };
        let saved = vec![SavedPc {
            id: "pc-1".into(),
            name: "Studio PC".into(),
        }];
        let shell = shell_data(&state, &saved, &ShellData::default());
        assert_eq!(
            shell.connection,
            data::Connection::Offline {
                reason: data::OfflineReason::HostOffline
            }
        );
        assert_eq!(shell.host.as_deref(), Some("pc-1"));
        assert_eq!(
            shell.saved,
            vec![SavedHost {
                id: "pc-1".into(),
                name: "Studio PC".into()
            }]
        );
        let recents: Vec<(&str, RecentKind)> = shell
            .recents
            .iter()
            .map(|recent| (recent.name.as_ref(), recent.kind))
            .collect();
        assert_eq!(
            recents,
            vec![
                (
                    "Forge",
                    RecentKind::Bot {
                        color: BotColor::Teal,
                        shape: BotShape::Cloud,
                        state: BotState::Waiting
                    }
                ),
                (
                    "Scout",
                    RecentKind::Bot {
                        color: BotColor::Blue,
                        shape: BotShape::Hexagon,
                        state: BotState::Working
                    }
                ),
                ("Launch crew", RecentKind::Room { unread: false }),
            ]
        );
        assert_eq!(shell.recents[0].id, "b1");
        // A failure keeps its words; the connecting states map one for one.
        let failed = AppState {
            connection: state::Connection::Failed {
                reason: "No daemon.".into(),
            },
            ..AppState::default()
        };
        assert_eq!(
            shell_data(&failed, &[], &ShellData::default()).connection,
            data::Connection::Failed {
                reason: "No daemon.".into()
            }
        );
    }
}
