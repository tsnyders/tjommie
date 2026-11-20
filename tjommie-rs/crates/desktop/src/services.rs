//! The desktop services M6 built, wired into the app (Phase 3 plan T6.1 to T6.4; `main.ts:119-189`, `389-437`, `684-716`):
//!
//! - **The tray** (T6.1). Closing the window hides it and the app keeps running; the first time, a toast says so. A left
//!   click on the icon shows the window; its menu has Show, Restart to update and Quit. Quit hides the window at once, then
//!   runs the graceful quit. Without a tray icon to bring the window back, closing quits instead.
//! - **Notifications** (T6.2). The daemon's `notify` events and its pending routine failures become toasts, except while
//!   the window is shown and focused; a routine failure is acknowledged only after the OS took its toast. A click shows the
//!   window and goes to the notice's chat or room.
//! - **Start at sign-in** (T6.3). Set once on the first packaged run (production only: `LoginItem` writes nothing for any
//!   other identity), and the switch in Settings, General.
//! - **The updater** (T6.4). Production only (`resolve_feed`): the first check after 60 s, then every 6 h. At quit it
//!   installs the ready update after the daemon has stopped, or, if the daemon had to be forced, doesn't: the window comes
//!   back and says so.
//!
//! WebView2, COM and the tray's window never touch GPUI state: each of them pushes a typed event into a channel that a
//! foreground task applies here. Win32 show and hide go out as `ShowWindowAsync`, which posts the change to the window's
//! own message loop instead of re-entering GPUI's window procedure.
//!
//! An e2e build (the test driver, e2e identity) swaps the real tray, toasts and registry for in-memory fakes the driver can
//! read, so a test never puts an icon in the owner's tray, a toast on their desktop or a value in their registry.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::channel;

use client::Client;
use desktop_host::app_dirs::{AppDirs, DirsEnv};
use desktop_host::daemon_process::{DaemonError, QUIT_TIMEOUT, StopOutcome};
use desktop_host::identity::{Identity, Kind};
use desktop_host::shell::login_item::{LoginItem, LoginItemConfig};
use desktop_host::shell::notify::{
    Acknowledge, Notifications, Notifier, ToastBackend, WinRtToasts, WindowState, register_aumid,
};
use desktop_host::shell::registry::{Registry, Windows};
use desktop_host::shell::tray::{self, Tray, TrayConfig, TrayEvent};
use desktop_host::updater::{
    DaemonStop, DetachedLauncher, Http, Policy, QuitHost, QuitOutcome, ReqwestHttp, UpdateEnv,
    UpdateState, Updater, UpdaterConfig, UpdaterDeps, VersionResource, resolve_feed, run_quit,
    this_version,
};
use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::channel::mpsc::unbounded;
use futures::future::{BoxFuture, Either};
use gpui::{AnyWindowHandle, App, AsyncApp, Global, Window};
use protocol::wire::{NotificationsAck, Notify, PendingNotification, method};
use url::Url;

use crate::app_controls::{AppControls, DesktopApp};
use crate::guard::{guarded, guarded_async};
use crate::hosts::AppHosts;
use crate::notices::{self, Notice};
use crate::ui::browser::local::window_hwnd;
use crate::ui::shell::route::{Navigate as _, Route};

/// The tray's icon, the project's own 32 px mark.
const TRAY_ICON: &[u8] = include_bytes!("../../../design/tray-32.png");

// ---- the window ------------------------------------------------------------------------------------------------------

/// What a window needs to be shown or hidden from the tray.
fn show_command(window: &Window) {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SW_SHOW, ShowWindowAsync};
    if let Some(hwnd) = window_hwnd(window) {
        // SAFETY: the app's own window handle; both calls only post a request to its message loop.
        unsafe {
            let _ = ShowWindowAsync(hwnd, SW_SHOW);
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            }
        }
    }
    window.activate_window();
}

fn hide_command(window: &Window) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindowAsync};
    if let Some(hwnd) = window_hwnd(window) {
        // SAFETY: the app's own window handle; this only posts a request to its message loop.
        unsafe {
            let _ = ShowWindowAsync(hwnd, SW_HIDE);
        }
    }
}

/// Shows the window and gives it the keyboard (`showWindow`): a hidden one comes back, a minimized one is restored. Asked
/// from anywhere, even inside an update of the window itself, so it is done once that update has finished.
pub fn show_window(window: AnyWindowHandle, cx: &mut App) {
    cx.defer(move |cx| {
        guarded("show the window", || {
            let _ = window.update(cx, |_, window, _| show_command(window));
        });
    });
}

/// Hides the window; the app and its agents keep running. Deferred like `show_window`.
pub fn hide_window(window: AnyWindowHandle, cx: &mut App) {
    cx.defer(move |cx| {
        guarded("hide the window", || {
            let _ = window.update(cx, |_, window, _| hide_command(window));
        });
    });
}

/// Whether a notice is already in front of the person: the window is shown and focused (`win.isVisible() && win.isFocused()`).
struct Up(Arc<desktop_host::hosts::Hosts>);

impl WindowState for Up {
    fn visible_and_focused(&self) -> bool {
        self.0.window().visible_and_focused()
    }
}

// ---- what the app holds ------------------------------------------------------------------------------------------------

/// The services of one run, for the UI thread: a global.
#[derive(Clone)]
pub struct Services {
    window: AnyWindowHandle,
    hosts: AppHosts,
    user_data: PathBuf,
    notifier: Arc<Notifier>,
    notifications: Arc<Notifications>,
    updater: Updater,
    login_item: Arc<LoginItem>,
    /// The tray, when this run has one. Without one nothing could bring a hidden window back.
    tray: Option<Arc<Tray>>,
    runtime: tokio::runtime::Handle,
    /// The connection the notices came in on: what acknowledges them.
    client: Rc<RefCell<Option<Arc<Client>>>>,
    #[cfg(any(test, feature = "test-driver"))]
    fakes: Option<Arc<fakes::Fakes>>,
}

impl Global for Services {}

/// The app is closing: its clients and daemon are being stopped.
struct Quit {
    done: bool,
}

impl Global for Quit {}

/// Whether this run uses in-memory stand-ins for the tray, the toasts and the registry: an e2e build.
fn uses_fakes(identity: &Identity) -> bool {
    cfg!(feature = "test-driver") && identity.kind == Kind::E2e
}

/// The start-at-sign-in setting of this run: allowed for an identity that has one, never for a run with its own
/// `TJOMMIE_USER_DATA`. It writes nothing until `sync` or `set` is called, and `LoginItem` refuses both for any other
/// identity, so a dev, e2e or update-test run never touches the registry.
fn login_item_for(
    identity: &Identity,
    user_data: &Path,
    own_user_data: Option<&str>,
    registry: Arc<dyn Registry>,
) -> std::io::Result<LoginItem> {
    let config = LoginItemConfig::for_this_run(identity, user_data, own_user_data)?;
    Ok(LoginItem::new(registry, config))
}

/// A stand-in for the updater's HTTP when there is no feed (nothing is ever fetched then).
struct NoFeed;

impl Http for NoFeed {
    fn get_text<'a>(&'a self, _: &'a Url, _: usize) -> BoxFuture<'a, Result<String, String>> {
        async { Err("This build has no update feed.".to_owned()) }.boxed()
    }

    fn download<'a>(
        &'a self,
        _: &'a Url,
        _: &'a Path,
        _: u64,
        _: desktop_host::updater::Progress<'a>,
    ) -> BoxFuture<'a, Result<(), String>> {
        async { Err("This build has no update feed.".to_owned()) }.boxed()
    }
}

/// This run's updater: its feed is the compiled-in one in a production build, a local one in an update test, and none
/// otherwise; `UpdateState::Unsupported` says so. `quit` asks the app to run its graceful quit (Restart to update).
fn updater_for(
    identity: &Identity,
    dirs: &AppDirs,
    runtime: &tokio::runtime::Handle,
    quit: impl Fn() + Send + Sync + 'static,
) -> Updater {
    let updates = std::env::var("TJOMMIE_UPDATES").ok();
    let update_feed = std::env::var("TJOMMIE_UPDATE_FEED").ok();
    let user_data = std::env::var("TJOMMIE_USER_DATA").ok();
    let env = UpdateEnv {
        updates: updates.as_deref(),
        update_feed: update_feed.as_deref(),
        user_data: user_data.as_deref(),
    };
    let feed = resolve_feed(identity, &env).unwrap_or_else(|error| {
        log::warn!("Updates are off: {error}");
        None
    });
    let http: Arc<dyn Http> = match feed.as_ref().map(Policy::for_feed) {
        Some(Ok(policy)) => match ReqwestHttp::new(policy) {
            Ok(http) => Arc::new(http),
            Err(error) => {
                log::warn!("Updates can't be fetched: {error}");
                Arc::new(NoFeed)
            }
        },
        _ => Arc::new(NoFeed),
    };
    let updater = Updater::new(
        UpdaterConfig {
            app_id: identity.app_id.to_owned(),
            current: this_version(),
            feed,
            download_root: dirs.download_root.clone(),
        },
        UpdaterDeps {
            http,
            inspector: Arc::new(VersionResource),
            launcher: Arc::new(DetachedLauncher),
            quit: Arc::new(quit),
            before_restart: None,
        },
        runtime.clone(),
    );
    updater.start();
    updater
}

/// Starts the tray, the notifications, the start-at-sign-in setting and the updater, for the window just opened. A part
/// that can't start is logged and the app goes on without it.
pub fn start(identity: &Identity, window: &mut Window, cx: &mut App) {
    let Some(hosts) = cx.try_global::<AppHosts>().cloned() else {
        return;
    };
    let Some(runtime) = cx
        .try_global::<crate::live::Tokio>()
        .map(|tokio| tokio.0.clone())
    else {
        return;
    };
    let Some(root) = cx
        .try_global::<crate::live::DataRoot>()
        .map(|root| root.0.clone())
    else {
        return;
    };
    let own_user_data = std::env::var("TJOMMIE_USER_DATA").ok();
    let user_data_env = own_user_data.as_deref().map(Path::new);
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let dirs = match AppDirs::resolve(
        identity,
        &root,
        &DirsEnv {
            user_data: user_data_env,
            roaming: roaming.as_deref(),
        },
    ) {
        Ok(dirs) => dirs,
        Err(refused) => {
            log::warn!(
                "No tray, toasts, login item or updater: a folder was refused ({refused:?})"
            );
            return;
        }
    };
    let handle = window.window_handle();
    let fake = uses_fakes(identity);
    #[cfg(any(test, feature = "test-driver"))]
    let fakes = fake.then(|| Arc::new(fakes::Fakes::default()));

    // Start at sign-in.
    #[cfg(any(test, feature = "test-driver"))]
    let registry: Arc<dyn Registry> = match &fakes {
        Some(fakes) => fakes.registry.clone(),
        None => Arc::new(Windows),
    };
    #[cfg(not(any(test, feature = "test-driver")))]
    let registry: Arc<dyn Registry> = Arc::new(Windows);
    let login_item = match login_item_for(
        identity,
        &dirs.user_data,
        own_user_data.as_deref(),
        registry.clone(),
    ) {
        Ok(item) => Arc::new(item),
        Err(error) => {
            log::warn!("No start-at-sign-in setting: {error}");
            return;
        }
    };
    if let Err(error) = login_item.sync() {
        log::warn!("Could not set start at sign-in: {error}");
    }

    // Toasts.
    let (clicks, clicked) = channel::<String>();
    #[cfg(any(test, feature = "test-driver"))]
    let backend: Arc<dyn ToastBackend> = match &fakes {
        Some(fakes) => fakes.toasts.clone(),
        None => real_toasts(identity, &*registry, clicks),
    };
    #[cfg(not(any(test, feature = "test-driver")))]
    let backend: Arc<dyn ToastBackend> = real_toasts(identity, &*registry, clicks);
    let notifier = Arc::new(Notifier::new(backend, Arc::new(Up(hosts.hosts.clone()))));
    let notifications = Arc::new(Notifications::new(notifier.clone()));

    // The updater, and the quits it asks for.
    let (quit_asked, quit_requests) = unbounded::<()>();
    let updater = updater_for(identity, &dirs, &runtime, move || {
        let _ = quit_asked.unbounded_send(());
    });

    // The tray.
    let (picked, tray_events) = channel::<TrayEvent>();
    let tray = if fake {
        None
    } else {
        match Tray::start(
            TrayConfig {
                tooltip: identity.product.to_owned(),
                icon_png: Some(TRAY_ICON),
            },
            picked,
        ) {
            Ok(tray) => Some(Arc::new(tray)),
            Err(error) => {
                log::warn!("No tray icon: {error}");
                None
            }
        }
    };
    if let Some(tray) = &tray {
        let showing = tray.clone();
        // unguarded: the updater's own listener, run on its thread, not a GPUI handler; it only tells the tray
        let _ = updater.on_state(move |state| showing.set_update_state(state.clone()));
    }

    let services = Services {
        window: handle,
        hosts: hosts.clone(),
        user_data: dirs.user_data.clone(),
        notifier,
        notifications,
        updater: updater.clone(),
        login_item,
        tray,
        runtime: runtime.clone(),
        client: Rc::default(),
        #[cfg(any(test, feature = "test-driver"))]
        fakes,
    };
    cx.set_global(AppControls(Arc::new(RealApp {
        login_item: services.login_item.clone(),
        updater: updater.clone(),
    })));
    cx.set_global(services);

    // What the tray, the toasts and the updater tell the app comes in through channels, and is applied here.
    let (state_changed, state_changes) = unbounded::<()>();
    // unguarded: the updater's own listener, run on its thread, not a GPUI handler; it only sends a signal
    let _ = updater.on_state(move |_| {
        let _ = state_changed.unbounded_send(());
    });
    pump(
        "follow the updater",
        state_changes,
        |(), cx| cx.refresh_windows(),
        cx,
    );
    pump(
        "quit when asked",
        quit_requests,
        |(), cx| begin_quit(cx),
        cx,
    );
    pump("follow the tray", forward(tray_events), tray_event, cx);
    pump(
        "follow the toasts",
        forward(clicked),
        |link, cx| clicked_toast(&link, cx),
        cx,
    );
}

/// The real toasts, under this build's own id (a dev or e2e-less build registers it; an installed one has its shortcut).
fn real_toasts(
    identity: &Identity,
    registry: &dyn Registry,
    clicks: std::sync::mpsc::Sender<String>,
) -> Arc<dyn ToastBackend> {
    if let Err(error) = register_aumid(registry, identity) {
        log::warn!("The notification id was not registered: {error}");
    }
    Arc::new(WinRtToasts::new(identity.app_id, clicks))
}

/// Moves what a std channel's receiver gets onto an async stream: a thread of its own waits, since the tray's and the
/// toasts' senders live on threads that must never touch GPUI.
fn forward<T: Send + 'static>(
    events: std::sync::mpsc::Receiver<T>,
) -> futures::channel::mpsc::UnboundedReceiver<T> {
    let (sender, receiver) = unbounded();
    // unguarded: a plain thread that only moves values from one channel to another
    std::thread::spawn(move || {
        while let Ok(event) = events.recv() {
            if sender.unbounded_send(event).is_err() {
                break;
            }
        }
    });
    receiver
}

/// Runs `apply` on the UI thread for each value that arrives, guarded.
fn pump<T: 'static>(
    what: &'static str,
    mut values: futures::channel::mpsc::UnboundedReceiver<T>,
    apply: impl Fn(T, &mut App) + Clone + 'static,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        guarded_async(what, async move {
            while let Some(value) = values.next().await {
                let apply = apply.clone();
                cx.update(|cx| {
                    guarded(what, || apply(value, cx));
                });
            }
        })
        .await;
    })
    .detach();
}

// ---- the tray, closing and quitting --------------------------------------------------------------------------------------

fn services(cx: &App) -> Option<Services> {
    cx.try_global::<Services>().cloned()
}

/// What the tray's icon and menu ask for.
pub fn tray_event(event: TrayEvent, cx: &mut App) {
    let Some(services) = services(cx) else {
        return;
    };
    match event {
        TrayEvent::Show => show_window(services.window, cx),
        TrayEvent::Restart => services.updater.restart(),
        TrayEvent::Quit => begin_quit(cx),
    }
}

/// The window's close button (`window.on("close")`): with a tray icon to bring it back, it only hides, and the first time
/// says that Tjommie keeps running; without one it quits. The app's own graceful quit is what finally lets it close.
pub fn on_close_requested(window: &mut Window, cx: &mut App) -> bool {
    if let Some(quit) = cx.try_global::<Quit>() {
        return quit.done;
    }
    let Some(services) = services(cx) else {
        // No services (a view test): closing ends the app as it always did.
        return true;
    };
    if !services.has_tray() {
        begin_quit(cx);
        return false;
    }
    // The window is being updated now (this is its close hook), so it is hidden through the handle we were given.
    hide_command(window);
    tray::hint_once(&services.user_data, |title, body| {
        services.notifier.show_plain(title, body)
    });
    false
}

impl Services {
    fn has_tray(&self) -> bool {
        #[cfg(any(test, feature = "test-driver"))]
        if self.fakes.is_some() {
            return true;
        }
        self.tray.as_ref().is_some_and(|tray| tray.icon_added())
    }
}

/// How the daemon's stop reads to the quit (`stopDaemonBeforeQuit`'s "forced" or not). No daemon to stop is a plain stop.
pub fn daemon_stop(
    outcome: Option<Result<StopOutcome, DaemonError>>,
) -> Result<DaemonStop, String> {
    match outcome {
        None | Some(Ok(StopOutcome::Stopped)) => Ok(DaemonStop::Graceful),
        Some(Ok(StopOutcome::Forced)) => Ok(DaemonStop::Forced),
        Some(Err(error)) => Err(error.to_string()),
    }
}

/// The app's side of `run_quit`.
struct AppQuit {
    cx: AsyncApp,
    hosts: AppHosts,
    stopper: Option<crate::live::Stopper>,
    window: AnyWindowHandle,
    /// An e2e run can hold the quit once the daemon has stopped, to look at the app while it is still alive.
    #[cfg(any(test, feature = "test-driver"))]
    held: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl QuitHost for AppQuit {
    async fn close_native(&self) {
        // The clients, the vault and the browsers go first, within the quit timeout.
        let timer = self.cx.background_executor().clone();
        let stop = self.hosts.control.stop(true);
        futures::pin_mut!(stop);
        // Which arm won is said: a quit that went on after the timeout is not one that stopped them.
        match futures::future::select(stop, timer.timer(QUIT_TIMEOUT)).await {
            Either::Left(_) => log::info!("The browser, Logins and worker clients stopped."),
            Either::Right(_) => log::warn!(
                "The browser, Logins and worker clients did not stop within {} s; the quit goes on.",
                QUIT_TIMEOUT.as_secs()
            ),
        }
    }

    async fn stop_daemon(&self) -> Result<DaemonStop, String> {
        let stopped = match &self.stopper {
            Some(stopper) => stopper.stop().await,
            None => Ok(DaemonStop::Graceful),
        };
        // A forced stop or one that could not be confirmed is warned about by `run_quit`.
        if stopped == Ok(DaemonStop::Graceful) {
            log::info!("The daemon stopped when asked.");
        }
        stopped
    }

    async fn close_native_host(&self) {
        // `close_native` stopped the clients already: there is no connection left to close.
        #[cfg(any(test, feature = "test-driver"))]
        if let Some(held) = &self.held {
            let timer = self.cx.background_executor().clone();
            while held.load(std::sync::atomic::Ordering::SeqCst) {
                timer.timer(std::time::Duration::from_millis(50)).await;
            }
        }
    }

    fn update_not_installed(&self, daemon_stopped: bool) {
        let (hosts, window) = (self.hosts.clone(), self.window);
        let restarting = daemon_stopped.then(|| self.stopper.clone()).flatten();
        self.cx.update(|cx| {
            hosts.hosts.cancel_quit();
            // The app runs on: the daemon may restart and its clients start again.
            hosts.control.resume();
            show_window(window, cx);
            notices::raise(Notice::UpdateNotInstalled { daemon_stopped });
            // The service did not stop in time and is gone now: it starts again, and its clients with it.
            if let Some(stopper) = restarting {
                stopper.restart_daemon();
            }
        });
    }
}

/// Quits the app, gracefully: the window hides at once (the stop can take a while), the browser, Logins and worker clients
/// stop, the daemon stops, and only then does a ready update's installer start (`will-quit`). If an update was waiting
/// and the daemon had to be forced, nothing is installed: the window comes back, says so, and the daemon starts again.
pub fn begin_quit(cx: &mut App) {
    if cx.has_global::<Quit>() {
        return;
    }
    let Some(services) = services(cx) else {
        cx.quit();
        return;
    };
    cx.set_global(Quit { done: false });
    hide_window(services.window, cx);
    services.hosts.hosts.begin_quit();
    log::info!("Closing: the browser, Logins and worker clients stop, then the daemon.");
    let (hosts, window, updater) = (
        services.hosts.clone(),
        services.window,
        services.updater.clone(),
    );
    let stopper = crate::live::stopper(cx);
    #[cfg(any(test, feature = "test-driver"))]
    let held = services.fakes.as_ref().map(|fakes| fakes.hold_quit.clone());
    cx.spawn(async move |cx| {
        guarded_async("quit", async move {
            let host = AppQuit {
                cx: cx.clone(),
                hosts,
                stopper,
                window,
                #[cfg(any(test, feature = "test-driver"))]
                held,
            };
            match run_quit(&host, &updater).await {
                QuitOutcome::Quit { installer_started } => {
                    log::info!(
                        "Closed: the app quits now{}.",
                        if installer_started {
                            ", and the update's installer started"
                        } else {
                            ""
                        }
                    );
                    cx.update(|cx| {
                        cx.global_mut::<Quit>().done = true;
                        cx.quit();
                    });
                }
                QuitOutcome::Aborted => cx.update(|cx| {
                    cx.remove_global::<Quit>();
                }),
            }
        })
        .await;
    })
    .detach();
}

// ---- notifications -----------------------------------------------------------------------------------------------------

/// Where the connection the notices came in on is kept, for acknowledging them.
pub fn connected(client: Arc<Client>, cx: &mut App) {
    if let Some(services) = services(cx) {
        *services.client.borrow_mut() = Some(client);
    }
}

impl Services {
    /// Tells the notifier whether the window is up front, as it is now.
    fn look_at_window(&self, cx: &mut App) {
        let facts = self.hosts.hosts.window();
        let pretend = self.pretended_focus();
        let _ = self.window.update(cx, |_, window, _| {
            facts.set_visible(window.is_visible());
            facts.set_focused(pretend.unwrap_or_else(|| window.is_window_active()));
        });
    }

    /// An e2e run decides for itself whether the window has the keyboard: the real answer depends on the desktop it runs on.
    #[cfg(any(test, feature = "test-driver"))]
    fn pretended_focus(&self) -> Option<bool> {
        self.fakes
            .as_ref()
            .map(|fakes| fakes.focused.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[cfg(not(any(test, feature = "test-driver")))]
    fn pretended_focus(&self) -> Option<bool> {
        None
    }

    /// The daemon's acknowledgement of a routine failure, on the live connection.
    fn acknowledge(&self) -> Acknowledge {
        let (client, runtime) = (self.client.borrow().clone(), self.runtime.clone());
        Arc::new(move |run_id| {
            let (client, runtime) = (client.clone(), runtime.clone());
            async move {
                let Some(client) = client else {
                    return Err("Tjommie isn't connected right now.".to_owned());
                };
                // unguarded: a tokio task on the live runtime, not a GPUI handler; a panic in it ends that task alone
                let sent = runtime.spawn(async move {
                    client
                        .call::<method::NotificationsAck>(NotificationsAck { run_id })
                        .await
                        .map(drop)
                        .map_err(|error| error.to_string())
                });
                sent.await
                    .unwrap_or_else(|_| Err("The acknowledgement did not finish.".to_owned()))
            }
            .boxed()
        })
    }

    /// Shows `notice` as a toast (or not, while the window is up front) and acknowledges it once shown.
    fn deliver(&self, notice: Notify, acknowledge: Acknowledge, cx: &mut App) {
        self.look_at_window(cx);
        let notifications = self.notifications.clone();
        cx.spawn(async move |_| {
            guarded_async("show a notice", async move {
                if let Err(error) = notifications.deliver(notice, acknowledge).await {
                    log::warn!("A notice could not be acknowledged: {error}");
                }
            })
            .await;
        })
        .detach();
    }
}

/// A live `notify` event from the daemon.
pub fn notify(notice: Notify, cx: &mut App) {
    if let Some(services) = services(cx) {
        let acknowledge = services.acknowledge();
        services.deliver(notice, acknowledge, cx);
    }
}

/// `notifications.pending`, fetched after subscribing: what the daemon still holds because nothing acknowledged it.
pub fn replay(pending: Vec<PendingNotification>, cx: &mut App) {
    let Some(services) = services(cx) else {
        return;
    };
    services.look_at_window(cx);
    let (notifications, acknowledge) = (services.notifications.clone(), services.acknowledge());
    cx.spawn(async move |_| {
        guarded_async("show the pending notices", async move {
            if let Err(error) = notifications.replay(pending, &acknowledge).await {
                log::warn!("Notifications are off until the next start: {error}");
            }
        })
        .await;
    })
    .detach();
}

/// A click on a toast: the window comes up and goes where the notice says.
fn clicked_toast(link: &str, cx: &mut App) {
    let Some(services) = services(cx) else {
        return;
    };
    show_window(services.window, cx);
    if let Some(route) = Route::from_notification_link(link) {
        cx.navigate(route);
    }
}

// ---- Settings, General -------------------------------------------------------------------------------------------------

/// The real controls General shows: the login item and the updater.
struct RealApp {
    login_item: Arc<LoginItem>,
    updater: Updater,
}

impl DesktopApp for RealApp {
    fn login_item(&self) -> bool {
        self.login_item.get()
    }

    fn set_login_item(&self, on: bool) -> Result<(), String> {
        self.login_item.set(on)
    }

    fn update_state(&self) -> UpdateState {
        self.updater.state()
    }

    fn check_for_updates(&self) {
        let updater = self.updater.clone();
        // Its own thread: the check is a Tokio future and this is the UI thread, which has no runtime.
        // unguarded: a plain thread; a panic in it ends only itself
        std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(updater.check());
            }
        });
    }

    fn restart_to_update(&self) {
        self.updater.restart();
    }
}

// ---- the e2e stand-ins and the test driver's reach into them -----------------------------------------------------------

/// In-memory stand-ins for the tray, the toasts and the registry, so an e2e run touches none of the owner's desktop.
#[cfg(any(test, feature = "test-driver"))]
mod fakes {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use desktop_host::shell::notify::{Toast, ToastError};
    use desktop_host::shell::registry::Hive;

    /// A toast backend that records what it was asked to show, and says "off" while notifications are disabled.
    pub struct FakeToasts {
        pub shown: Mutex<Vec<Toast>>,
        pub enabled: AtomicBool,
    }

    impl Default for FakeToasts {
        fn default() -> Self {
            FakeToasts {
                shown: Mutex::default(),
                enabled: AtomicBool::new(true),
            }
        }
    }

    impl ToastBackend for FakeToasts {
        fn show(&self, toast: &Toast) -> Result<(), ToastError> {
            if !self.enabled.load(Ordering::SeqCst) {
                return Err(ToastError::Disabled("Disabled".into()));
            }
            if let Ok(mut shown) = self.shown.lock() {
                shown.push(toast.clone());
            }
            Ok(())
        }
    }

    /// A registry in memory that counts what is written to it.
    #[derive(Default)]
    pub struct RecordingRegistry {
        values: Mutex<HashMap<(String, String), String>>,
        pub writes: AtomicUsize,
    }

    impl Registry for RecordingRegistry {
        fn read_string(&self, _: Hive, key: &str, name: &str) -> Option<String> {
            self.values
                .lock()
                .ok()?
                .get(&(key.to_owned(), name.to_owned()))
                .cloned()
        }

        fn read_binary(&self, _: Hive, _: &str, _: &str) -> Option<Vec<u8>> {
            None
        }

        fn write_string(&self, key: &str, name: &str, value: &str) -> Result<(), String> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut values) = self.values.lock() {
                values.insert((key.to_owned(), name.to_owned()), value.to_owned());
            }
            Ok(())
        }

        fn delete_value(&self, key: &str, name: &str) -> Result<(), String> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut values) = self.values.lock() {
                values.remove(&(key.to_owned(), name.to_owned()));
            }
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct Fakes {
        pub toasts: Arc<FakeToasts>,
        pub registry: Arc<RecordingRegistry>,
        /// The run ids the fake acknowledgement was asked for, in order.
        pub acked: Mutex<Vec<String>>,
        /// Whether the window counts as having the keyboard (a toast is skipped while it is shown and focused).
        pub focused: AtomicBool,
        /// A quit waits, once the daemon has stopped, for this to be cleared (the app is alive to be looked at).
        pub hold_quit: Arc<AtomicBool>,
    }
}

/// What the test driver reads of the services (`Request::Services`).
#[cfg(any(test, feature = "test-driver"))]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ServicesSnapshot {
    /// The app's window is shown (not hidden to the tray).
    pub visible: bool,
    /// The toasts the fake notifier took, as `(title, body, link)`.
    pub toasts: Vec<(String, String, Option<String>)>,
    /// The run ids the fake acknowledgement was asked for.
    pub acked: Vec<String>,
    /// How many times the start-at-sign-in setting wrote to the (in-memory) registry.
    pub registry_writes: usize,
    pub login_item: bool,
}

/// What an e2e test asks of the services through the driver.
#[cfg(any(test, feature = "test-driver"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Poke {
    /// The tray's icon or menu: `show`, `restart` or `quit`.
    Tray(String),
    /// A notice as the daemon would send it: a routine failure with this run id.
    RoutineFailed { run_id: String },
    /// Notifications turned on or off in Windows (the fake notifier refuses toasts while off).
    Notifications(bool),
    /// The window has the keyboard, or doesn't (a toast is skipped, but acknowledged, while it is shown and focused).
    Focus(bool),
    /// Clicks the nth toast the fake notifier took.
    ClickToast(usize),
    /// Holds a quit once the daemon has stopped, while on: the app is alive with its daemon gone.
    HoldQuit(bool),
}

#[cfg(any(test, feature = "test-driver"))]
pub fn snapshot(window: &Window, cx: &App) -> ServicesSnapshot {
    use std::sync::atomic::Ordering;
    let Some(services) = cx.try_global::<Services>() else {
        return ServicesSnapshot::default();
    };
    let mut snapshot = ServicesSnapshot {
        visible: window.is_visible(),
        login_item: services.login_item.get(),
        ..ServicesSnapshot::default()
    };
    if let Some(fakes) = &services.fakes {
        snapshot.toasts = fakes
            .toasts
            .shown
            .lock()
            .map(|shown| {
                shown
                    .iter()
                    .map(|toast| (toast.title.clone(), toast.body.clone(), toast.route.clone()))
                    .collect()
            })
            .unwrap_or_default();
        snapshot.acked = fakes
            .acked
            .lock()
            .map(|acked| acked.clone())
            .unwrap_or_default();
        snapshot.registry_writes = fakes.registry.writes.load(Ordering::SeqCst);
    }
    snapshot
}

/// Applies a poke; `Err` says why it can't be.
#[cfg(any(test, feature = "test-driver"))]
pub fn poke(poke: Poke, cx: &mut App) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let services = services(cx).ok_or("the app has no services")?;
    let fakes = services.fakes.clone().ok_or("this run has no stand-ins")?;
    match poke {
        Poke::Tray(item) => {
            let event = match item.as_str() {
                "show" => TrayEvent::Show,
                "restart" => TrayEvent::Restart,
                "quit" => TrayEvent::Quit,
                other => return Err(format!("no tray item {other:?}")),
            };
            tray_event(event, cx);
        }
        Poke::RoutineFailed { run_id } => {
            let notice = Notify::RoutineFailed {
                bot_id: "bot-1".into(),
                title: "Forge's routine failed".into(),
                body: "Morning check: the provider was down.".into(),
                routine_id: Some("routine-1".into()),
                run_id: Some(run_id),
            };
            // The acknowledgement is the fake's: it records the run id, so a test sees the order of toast and ack.
            let recorder = fakes.clone();
            let acknowledge: Acknowledge = Arc::new(move |run_id| {
                if let Ok(mut acked) = recorder.acked.lock() {
                    acked.push(run_id);
                }
                async { Ok(()) }.boxed()
            });
            services.deliver(notice, acknowledge, cx);
        }
        Poke::Notifications(on) => fakes.toasts.enabled.store(on, Ordering::SeqCst),
        Poke::HoldQuit(on) => fakes.hold_quit.store(on, Ordering::SeqCst),
        Poke::Focus(on) => {
            fakes.focused.store(on, Ordering::SeqCst);
            services.hosts.hosts.window().set_focused(on);
        }
        Poke::ClickToast(index) => {
            let link = fakes
                .toasts
                .shown
                .lock()
                .ok()
                .and_then(|shown| shown.get(index).and_then(|toast| toast.route.clone()))
                .ok_or("no such toast")?;
            clicked_toast(&link, cx);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_host::daemon_process::DaemonError;
    use desktop_host::identity::{DEV, E2E, PRODUCTION, UPDATE_TEST};
    use desktop_host::shell::registry::Hive;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A registry in memory that counts its writes.
    #[derive(Default)]
    struct Counting {
        values: Mutex<HashMap<(String, String), String>>,
        writes: AtomicUsize,
    }

    impl Registry for Counting {
        fn read_string(&self, _: Hive, key: &str, name: &str) -> Option<String> {
            self.values
                .lock()
                .ok()?
                .get(&(key.to_owned(), name.to_owned()))
                .cloned()
        }

        fn read_binary(&self, _: Hive, _: &str, _: &str) -> Option<Vec<u8>> {
            None
        }

        fn write_string(&self, key: &str, name: &str, value: &str) -> Result<(), String> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut values) = self.values.lock() {
                values.insert((key.to_owned(), name.to_owned()), value.to_owned());
            }
            Ok(())
        }

        fn delete_value(&self, key: &str, name: &str) -> Result<(), String> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut values) = self.values.lock() {
                values.remove(&(key.to_owned(), name.to_owned()));
            }
            Ok(())
        }
    }

    /// A folder of its own for one test, removed after.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Folder {
            let path = std::env::temp_dir()
                .join(format!("tjommie-services-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Folder(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// T6.3: a dev, e2e or update-test run never writes the login item, whatever it is asked.
    #[test]
    fn a_dev_e2e_or_update_test_run_never_writes_the_login_item() {
        for identity in [&DEV, &E2E, &UPDATE_TEST] {
            let folder = Folder::new(identity.name);
            let registry = Arc::new(Counting::default());
            let item = login_item_for(identity, &folder.0, None, registry.clone()).unwrap();
            item.sync().unwrap();
            item.set(true).unwrap();
            item.set(false).unwrap();
            assert!(!item.get(), "{}", identity.name);
            assert_eq!(
                registry.writes.load(Ordering::SeqCst),
                0,
                "{} wrote to the registry",
                identity.name
            );
            assert!(
                !folder.0.exists(),
                "{} left a marker or a folder",
                identity.name
            );
        }
    }

    /// T6.3: the first packaged run turns it on once, and a later choice is left alone.
    #[test]
    fn the_first_packaged_run_sets_start_at_sign_in_once() {
        let folder = Folder::new("production");
        let registry = Arc::new(Counting::default());
        let item = login_item_for(&PRODUCTION, &folder.0, None, registry.clone()).unwrap();
        item.sync().unwrap();
        assert_eq!(
            registry.writes.load(Ordering::SeqCst),
            2,
            "the entry, and the approval cleared"
        );
        assert!(item.get());
        // The person turned it off in Settings; the next start leaves that alone.
        item.set(false).unwrap();
        let writes = registry.writes.load(Ordering::SeqCst);
        item.sync().unwrap();
        assert_eq!(registry.writes.load(Ordering::SeqCst), writes);
        assert!(!item.get());
    }

    /// A run with its own user data (a smoke run beside a real install) never touches it either.
    #[test]
    fn a_run_with_its_own_user_data_never_writes_it() {
        let folder = Folder::new("smoke");
        let registry = Arc::new(Counting::default());
        let item = login_item_for(
            &PRODUCTION,
            &folder.0,
            Some("C:/Temp/own"),
            registry.clone(),
        )
        .unwrap();
        item.sync().unwrap();
        assert_eq!(registry.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_daemons_stop_reads_as_graceful_forced_or_unconfirmed() {
        assert_eq!(daemon_stop(None), Ok(DaemonStop::Graceful));
        assert_eq!(
            daemon_stop(Some(Ok(StopOutcome::Stopped))),
            Ok(DaemonStop::Graceful)
        );
        assert_eq!(
            daemon_stop(Some(Ok(StopOutcome::Forced))),
            Ok(DaemonStop::Forced)
        );
        assert!(daemon_stop(Some(Err(DaemonError::StopTimedOut))).is_err());
    }

    /// Dev and e2e builds have no update feed, so there is nothing to check and the settings say so.
    #[test]
    fn a_dev_build_has_no_updater_feed() {
        let env = UpdateEnv::default();
        for identity in [&DEV, &E2E] {
            assert_eq!(resolve_feed(identity, &env), Ok(None), "{}", identity.name);
        }
    }
}
