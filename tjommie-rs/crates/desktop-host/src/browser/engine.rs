//! The WebView2 engine (Phase 3 plan T3.1): one environment, created on the first `open`, with one page controller
//! per tab in a profile per bot. A bot's page sits in the panel, parked off-screen, or hidden and asleep, as
//! [`lifecycle`](super::lifecycle) decides; nothing exists until a bot is opened.
//!
//! Everything here runs on the UI thread. WebView2 callbacks hand their results to channels and never touch the
//! engine's state; the state changes only in the engine's own methods and in [`Engine::apply_event`].

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;
use std::time::{Duration, Instant};

use futures::future::LocalBoxFuture;
use futures::lock::Mutex as AsyncMutex;
use protocol::browser::{BROWSER_START_URL, BrowserDialog, MouseButton};
use tokio::sync::Mutex as QueueMutex;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2Settings3,
    ICoreWebView2Settings4,
};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, MapWindowPoints};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, IsChild, IsWindowVisible,
    RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos,
    ShowWindow, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOPARENTNOTIFY,
};
use windows::core::{BOOL, Interface as _, w};

use super::access::{Access, AccessError};
use super::cdp::CdpError;
use super::com::{self, ComError};
use super::lifecycle::{Lifecycle, MemoryLevel, Need, Placement, Plan, Timeouts};
use super::policy::{self, PolicyError};
use super::profiles::{self, ProfileError};
use super::{BotIdentity, flags};

mod commands;
mod events;

pub use commands::{Actor, BotEvent, Check, LOCAL, Reply};
pub use events::{HostEvent, Notification};

/// Where a page that a tool or a stream keeps awake sits while no panel shows it: outside the window's client
/// area, still "visible" to WebView2 (hiding it would stop it rendering).
const PARK_ORIGIN: i32 = -32000;
/// A parked page keeps the size of the panel it last had; this is its size before it ever had one.
const DEFAULT_VIEWPORT: (i32, i32) = (1280, 800);
/// How long, and how often, `delete_profile` waits for Windows to let go of a closed profile's files.
const UNLOCK_ATTEMPTS: u32 = 20;
const UNLOCK_INTERVAL: Duration = Duration::from_millis(250);

/// Why an engine operation failed. The text is shown to the person and the bot.
#[derive(Debug)]
pub enum EngineError {
    Closed,
    Deleting,
    HomeChanged,
    NotOpen,
    Policy(PolicyError),
    Profile(ProfileError),
    Com(ComError),
    Window(String),
    Access(AccessError),
    Cdp(CdpError),
    /// A refusal in the TypeScript engine's own words.
    Refused(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("The desktop browser host is closed."),
            Self::Deleting => f.write_str(
                "The agent browser profile is being deleted or needs cleanup to be retried.",
            ),
            Self::HomeChanged => f.write_str(
                "The browser agent home changed; close its browser before reopening it.",
            ),
            Self::NotOpen => f.write_str("The agent browser is not open."),
            Self::Policy(error) => write!(f, "{error}"),
            Self::Profile(error) => write!(f, "{error}"),
            Self::Com(error) => write!(f, "{error}"),
            Self::Window(message) | Self::Refused(message) => f.write_str(message),
            Self::Access(error) => write!(f, "{error}"),
            Self::Cdp(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<PolicyError> for EngineError {
    fn from(error: PolicyError) -> Self {
        Self::Policy(error)
    }
}
impl From<ProfileError> for EngineError {
    fn from(error: ProfileError) -> Self {
        Self::Profile(error)
    }
}
impl From<ComError> for EngineError {
    fn from(error: ComError) -> Self {
        Self::Com(error)
    }
}
impl From<AccessError> for EngineError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}
impl From<CdpError> for EngineError {
    fn from(error: CdpError) -> Self {
        Self::Cdp(error)
    }
}
impl EngineError {
    pub fn refused(message: &str) -> Self {
        Self::Refused(message.to_string())
    }
}
impl From<windows::core::Error> for EngineError {
    fn from(error: windows::core::Error) -> Self {
        Self::Com(error.into())
    }
}

/// What the engine needs from its host. Fields are public so a test can set the seams.
pub struct EngineConfig {
    /// The app's window. Each bot's pages live in a child of it.
    pub parent: HWND,
    /// `<root>/data/webview2`.
    pub user_data_folder: PathBuf,
    /// The address a bot's first page opens.
    pub start_url: String,
    pub timeouts: Timeouts,
    /// The time source. A test moves it.
    pub clock: Rc<dyn Fn() -> Instant>,
    /// Waits without blocking. The default uses a short-lived thread; the app can supply its executor's timer.
    pub sleep: Rc<dyn Fn(Duration) -> LocalBoxFuture<'static, ()>>,
}

impl EngineConfig {
    pub fn new(parent: HWND, user_data_folder: PathBuf) -> Self {
        Self {
            parent,
            user_data_folder,
            start_url: BROWSER_START_URL.to_string(),
            timeouts: Timeouts::default(),
            clock: Rc::new(Instant::now),
            sleep: Rc::new(thread_sleep),
        }
    }
}

fn thread_sleep(duration: Duration) -> LocalBoxFuture<'static, ()> {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let _ = tx.send(());
    });
    Box::pin(async move {
        let _ = rx.await;
    })
}

/// The panel's page frame, in physical pixels in the parent's client coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Panel {
    pub rect: RECT,
    /// A menu or dialog is over the page: its window is out of the way, but it stays awake.
    pub occluded: bool,
}

/// What a test or a status line can read about a bot's browser: the engine's own record and WebView2's truth.
#[derive(Debug, Clone)]
pub struct BotSnapshot {
    pub open: bool,
    pub tabs: usize,
    pub need: Need,
    pub placement: Placement,
    /// WebView2's `IsVisible` for the active page.
    pub visible: bool,
    /// WebView2's `IsSuspended` for the active page.
    pub suspended: bool,
    /// Whether the bot's container window is shown.
    pub container_shown: bool,
    /// The container's rectangle in the parent's client coordinates.
    pub container: Option<RECT>,
    pub memory: MemoryLevel,
    pub generation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// The profile's folder is gone.
    Removed,
    /// Its sign-in and storage are cleared, but Windows still holds some files: they go at the next start.
    Locked,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub removed: Vec<String>,
    pub locked: Vec<String>,
}

struct Tab {
    id: String,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    /// Set while a CDP override holds the page at this CSS size; a panel showing the page gives it back.
    fixed_viewport: Cell<Option<(u32, u32)>>,
    /// Keys and buttons held down by injected input, released when control changes.
    keys: RefCell<Vec<String>>,
    buttons: RefCell<Vec<MouseButton>>,
    point: Cell<(f64, f64)>,
    /// A navigation is in flight. A suspended page makes no progress, so a loading page is left awake.
    loading: Cell<bool>,
    /// The JavaScript dialog the page is blocked on, until it is answered.
    dialog: RefCell<Option<events::PendingDialog>>,
}

/// What `apply` last did, so an unchanged plan costs no COM calls.
#[derive(Debug, Clone, PartialEq)]
struct Applied {
    plan: Plan,
    rect: Option<RECT>,
    occluded: bool,
    active: Option<String>,
    tabs: usize,
    memory: MemoryLevel,
    interactive: bool,
}

struct BotState {
    generation: String,
    open: bool,
    /// Closed for good: an `open` that was waiting for it must start over with a new entry.
    dead: bool,
    container: Option<HWND>,
    tabs: Vec<Rc<Tab>>,
    active: Option<String>,
    life: Lifecycle,
    panel: Option<Panel>,
    viewport: (i32, i32),
    applied: Option<Applied>,
    /// The active page's address when its controller was closed for idleness.
    last_url: Option<String>,
    access: Access,
    revision: u64,
    viewport_revision: u64,
    /// Commands running or queued (`busy` in the state).
    busy: u32,
    /// A hand-over of control is in progress.
    handover: u32,
    dialog: Option<BrowserDialog>,
}

struct Bot {
    identity: BotIdentity,
    profile: String,
    state: RefCell<BotState>,
    /// Held while the bot's controllers are created or closed.
    open_lock: AsyncMutex<()>,
    /// A bot's commands run one at a time, in the order they arrived.
    queue: QueueMutex<()>,
}

pub struct Engine {
    config: EngineConfig,
    events: async_channel::Sender<HostEvent>,
    notify: async_channel::Sender<Notification>,
    /// A menu or dialog is over the page area for every bot: pages stay awake but out of the way.
    occluded: Cell<bool>,
    /// Remote controllers seen: a disconnected one stays remembered for a minute, so a late command is refused.
    controllers: RefCell<HashMap<String, commands::ControllerLife>>,
    /// Bots whose browser is deleted or switched off: a command that finishes for one of them closes it.
    disabled: RefCell<HashSet<String>>,
    environment: AsyncMutex<Option<ICoreWebView2Environment>>,
    bots: RefCell<HashMap<String, Rc<Bot>>>,
    /// The one bot whose panel is open.
    mounted: RefCell<Option<String>>,
    window_visible: Cell<bool>,
    closed: Cell<bool>,
    /// Profiles being deleted: no `open` can race the cleanup.
    deleting: RefCell<HashSet<String>>,
}

/// The tab a tool or a test talks to. It keeps the page's COM object alive; use it only on the UI thread.
#[derive(Clone)]
pub struct TabHandle {
    id: String,
    webview: ICoreWebView2,
}

impl TabHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// One DevTools protocol command; `params` and the answer are JSON text.
    pub async fn call(&self, method: &str, params: &str) -> Result<String, ComError> {
        com::call_devtools(&self.webview, method, params).await
    }

    /// Navigates and waits for the navigation to end; `Ok(false)` is a failed navigation.
    pub async fn navigate(&self, url: &str) -> Result<bool, ComError> {
        com::navigate(&self.webview, url).await
    }

    pub fn source(&self) -> Result<String, ComError> {
        com::source(&self.webview)
    }
}

/// A tool, a vault fill or a UI command in progress on a bot: while it lives the bot's page is awake and visible.
pub struct ToolGuard {
    engine: Rc<Engine>,
    bot: Rc<Bot>,
    ended: Cell<bool>,
}

impl ToolGuard {
    /// The bot's active tab. A tab can only be reached through a guard: the guard keeps the page awake, visible and
    /// parked (or in the panel) for as long as it lives, because a suspended page answers nothing.
    pub fn tab(&self) -> Result<TabHandle, EngineError> {
        let tab = self.engine.tab(&self.bot, None)?;
        Ok(TabHandle {
            id: tab.id.clone(),
            webview: tab.webview.clone(),
        })
    }

    /// A tab by id (a tool's `pageId`).
    pub fn tab_by_id(&self, id: &str) -> Result<TabHandle, EngineError> {
        let tab = self.engine.tab(&self.bot, Some(id))?;
        Ok(TabHandle {
            id: tab.id.clone(),
            webview: tab.webview.clone(),
        })
    }

    /// Ends the tool: a hidden bot goes back to sleep at once.
    pub fn end(self) {
        self.ended.set(true);
        let now = self.engine.now();
        self.bot.state.borrow_mut().life.end_tool(now);
        self.engine.apply(&self.bot);
    }
}

impl Drop for ToolGuard {
    fn drop(&mut self) {
        if self.ended.get() {
            return;
        }
        // Dropped without `end` (a cancelled task): the need ends now, and the page is put to sleep by the host's
        // next event, never from inside whatever dropped this.
        let now = self.engine.now();
        self.bot.state.borrow_mut().life.end_tool(now);
        let _ = self.engine.events.try_send(HostEvent::Reapply {
            bot_id: self.bot.identity.id.clone(),
        });
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the default window procedure; the container does nothing of its own.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_container(parent: HWND) -> Result<HWND, EngineError> {
    static REGISTER: Once = Once::new();
    // SAFETY: plain Win32 calls on the UI thread. The class and its brush live as long as the process.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        REGISTER.call_once(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(host_proc),
                hInstance: instance.into(),
                lpszClassName: w!("TjommieBrowserHost"),
                hbrBackground: CreateSolidBrush(COLORREF(0x00FF_FFFF)),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
        CreateWindowExW(
            WS_EX_NOPARENTNOTIFY,
            w!("TjommieBrowserHost"),
            None,
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|error| EngineError::Window(error.message()))
    }
}

fn rect_size(rect: &RECT) -> (i32, i32) {
    (rect.right - rect.left, rect.bottom - rect.top)
}

fn parked_rect(viewport: (i32, i32)) -> RECT {
    RECT {
        left: PARK_ORIGIN,
        top: PARK_ORIGIN,
        right: PARK_ORIGIN + viewport.0,
        bottom: PARK_ORIGIN + viewport.1,
    }
}

/// Settings of a bot's page: no native dialogs, menus, autofill or browser shortcuts. It is a page a bot drives and
/// the person watches, not a browser (the same lockdown as `native-browser.ts`'s `preferences`).
fn configure(webview: &ICoreWebView2) {
    // SAFETY: plain COM calls on interfaces we hold; a setting an older runtime lacks is skipped.
    unsafe {
        let Ok(settings) = webview.Settings() else {
            return;
        };
        let _ = settings.SetAreDefaultScriptDialogsEnabled(false);
        let _ = settings.SetAreDefaultContextMenusEnabled(false);
        let _ = settings.SetAreDevToolsEnabled(false);
        let _ = settings.SetIsStatusBarEnabled(false);
        let _ = settings.SetIsZoomControlEnabled(false);
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings3>() {
            let _ = settings.SetAreBrowserAcceleratorKeysEnabled(false);
        }
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings4>() {
            let _ = settings.SetIsPasswordAutosaveEnabled(false);
            let _ = settings.SetIsGeneralAutofillEnabled(false);
        }
    }
}

impl Engine {
    /// Nothing starts here: the environment waits for the first [`open`](Self::open).
    pub fn new(
        config: EngineConfig,
        events: async_channel::Sender<HostEvent>,
        notify: async_channel::Sender<Notification>,
    ) -> Rc<Self> {
        Rc::new(Self {
            config,
            events,
            notify,
            occluded: Cell::new(false),
            controllers: RefCell::new(HashMap::new()),
            disabled: RefCell::new(HashSet::new()),
            environment: AsyncMutex::new(None),
            bots: RefCell::new(HashMap::new()),
            mounted: RefCell::new(None),
            window_visible: Cell::new(true),
            closed: Cell::new(false),
            deleting: RefCell::new(HashSet::new()),
        })
    }

    fn now(&self) -> Instant {
        (self.config.clock)()
    }

    /// `<user data folder>/EBWebView`: WebView2 keeps each profile's folder here.
    pub fn profile_root(&self) -> PathBuf {
        self.config.user_data_folder.join("EBWebView")
    }

    /// The profile folders on disk, by bot-id hash.
    pub fn profile_folders(&self) -> Result<Vec<String>, EngineError> {
        Ok(profiles::profile_directories(&self.profile_root())?)
    }

    // ---- opening -------------------------------------------------------------------------------------------

    /// Opens a bot's browser if it is not open: the environment (first time only), then one page at the start
    /// address, or at the last address if its controller was closed for idleness.
    pub async fn open(self: &Rc<Self>, bot: &BotIdentity) -> Result<(), EngineError> {
        self.ensure_open(bot, false).await.map(|_| ())
    }

    /// A tool starts on a bot: its page is created or resumed, visible and parked if no panel shows it, before this
    /// returns, so the tool's first command never waits on a sleeping or missing page.
    pub async fn begin_tool(self: &Rc<Self>, bot: &BotIdentity) -> Result<ToolGuard, EngineError> {
        let bot = self.ensure_open(bot, true).await?;
        let guard = ToolGuard {
            engine: self.clone(),
            bot,
            ended: Cell::new(false),
        };
        self.apply(&guard.bot);
        Ok(guard)
    }

    fn bot_entry(&self, identity: &BotIdentity) -> Result<Rc<Bot>, EngineError> {
        let profile = policy::profile_name(&identity.id)?;
        if self.deleting.borrow().contains(&profile) {
            return Err(EngineError::Deleting);
        }
        let mut bots = self.bots.borrow_mut();
        if let Some(bot) = bots.get(&identity.id) {
            let same_home =
                bot.identity.home_folder.to_lowercase() == identity.home_folder.to_lowercase();
            return if same_home {
                Ok(bot.clone())
            } else {
                Err(EngineError::HomeChanged)
            };
        }
        let now = self.now();
        let bot = Rc::new(Bot {
            identity: identity.clone(),
            profile,
            state: RefCell::new(BotState {
                generation: String::new(),
                open: false,
                dead: false,
                container: None,
                tabs: Vec::new(),
                active: None,
                life: Lifecycle::new(self.config.timeouts, now),
                panel: None,
                viewport: DEFAULT_VIEWPORT,
                applied: None,
                last_url: None,
                access: Access::default(),
                revision: 0,
                viewport_revision: 0,
                busy: 0,
                handover: 0,
                dialog: None,
            }),
            open_lock: AsyncMutex::new(()),
            queue: QueueMutex::new(()),
        });
        bots.insert(identity.id.clone(), bot.clone());
        Ok(bot)
    }

    async fn ensure_open(
        self: &Rc<Self>,
        identity: &BotIdentity,
        with_tool: bool,
    ) -> Result<Rc<Bot>, EngineError> {
        loop {
            if self.closed.get() {
                return Err(EngineError::Closed);
            }
            if self.disabled.borrow().contains(&identity.id) {
                return Err(EngineError::refused(
                    "This agent's browser is off or the agent no longer exists.",
                ));
            }
            let bot = self.bot_entry(identity)?;
            let turn = bot.open_lock.lock().await;
            if bot.state.borrow().dead {
                drop(turn);
                continue; // closed while this waited: start over with a new entry
            }
            if self.closed.get() {
                return Err(EngineError::Closed);
            }
            let now = self.now();
            // The need is marked before the page exists, so a page created for a tool is born awake and parked.
            if with_tool {
                bot.state.borrow_mut().life.begin_tool(now);
            }
            let opened = if bot.state.borrow().open {
                Ok(())
            } else {
                self.create_tabs(&bot).await
            };
            if let Err(error) = opened {
                if with_tool {
                    bot.state.borrow_mut().life.end_tool(self.now());
                }
                return Err(error);
            }
            drop(turn);
            return Ok(bot);
        }
    }

    async fn environment(&self) -> Result<ICoreWebView2Environment, EngineError> {
        let mut slot = self.environment.lock().await;
        if let Some(environment) = slot.as_ref() {
            return Ok(environment.clone());
        }
        if self.closed.get() {
            return Err(EngineError::Closed);
        }
        std::fs::create_dir_all(&self.config.user_data_folder)
            .map_err(|error| EngineError::Window(error.to_string()))?;
        let environment =
            com::create_environment(&self.config.user_data_folder, &flags::browser_arguments())
                .await?;
        *slot = Some(environment.clone());
        Ok(environment)
    }

    async fn create_tabs(&self, bot: &Rc<Bot>) -> Result<(), EngineError> {
        let environment = self.environment().await?;
        let (url, container) = {
            let mut state = bot.state.borrow_mut();
            let url = state
                .last_url
                .take()
                .unwrap_or_else(|| self.config.start_url.clone());
            let container = match state.container {
                Some(container) => container,
                None => {
                    let container = create_container(self.config.parent)?;
                    state.container = Some(container);
                    container
                }
            };
            (url, container)
        };
        let created = self.new_tab(bot, &environment, container, Some(&url)).await;
        let tab = match created {
            Ok(tab) => tab,
            Err(error) => {
                self.destroy_container(bot);
                return Err(error);
            }
        };
        if self.closed.get() || bot.state.borrow().dead {
            // SAFETY: closing the controller this function just created.
            let _ = unsafe { tab.controller.Close() };
            self.destroy_container(bot);
            return Err(EngineError::Closed);
        }
        {
            let mut state = bot.state.borrow_mut();
            state.active = Some(tab.id.clone());
            state.tabs = vec![tab];
            state.generation = uuid::Uuid::new_v4().to_string();
            state.open = true;
            state.applied = None;
        }
        self.apply(bot);
        Ok(())
    }

    /// A page in `profile`. With a `url` it opens that address; without one (a popup, whose contents the page
    /// supplies) it stays blank.
    async fn new_tab(
        &self,
        bot: &Bot,
        environment: &ICoreWebView2Environment,
        container: HWND,
        url: Option<&str>,
    ) -> Result<Rc<Tab>, EngineError> {
        let (bot_id, profile) = (bot.identity.id.as_str(), bot.profile.as_str());
        let controller = com::create_controller(environment, container, profile).await?;
        // SAFETY: plain COM calls on the controller this function just created.
        let webview = unsafe { controller.CoreWebView2()? };
        configure(&webview);
        let id = uuid::Uuid::new_v4().to_string();
        events::watch(
            &self.events,
            bot_id,
            &id,
            &bot.identity.home_folder,
            &webview,
        )?;
        // SAFETY: as above. The page stays hidden until the lifecycle plan says where it goes.
        unsafe {
            controller.SetIsVisible(false)?;
            if let Some(url) = url {
                webview.Navigate(&windows::core::HSTRING::from(url))?;
            }
        }
        Ok(Rc::new(Tab {
            id,
            controller,
            webview,
            fixed_viewport: Cell::new(None),
            keys: RefCell::new(Vec::new()),
            buttons: RefCell::new(Vec::new()),
            point: Cell::new((0.0, 0.0)),
            loading: Cell::new(url.is_some()),
            dialog: RefCell::new(None),
        }))
    }

    // ---- what a bot needs ----------------------------------------------------------------------------------

    fn is_shown(&self, id: &str, state: &BotState) -> bool {
        self.window_visible.get()
            && self.mounted.borrow().as_deref() == Some(id)
            && state.panel.is_some_and(|panel| {
                let (width, height) = rect_size(&panel.rect);
                width > 0 && height > 0
            })
    }

    /// Opens or moves the panel over a bot's page, or closes it (`None`). One bot's panel is open at a time.
    pub fn set_panel(&self, id: &str, panel: Option<Panel>) -> Result<(), EngineError> {
        let bot = self.bot(id)?;
        let previous = self.mounted.borrow().clone();
        match panel {
            Some(panel) => {
                bot.state.borrow_mut().panel = Some(panel);
                *self.mounted.borrow_mut() = Some(id.to_string());
            }
            None => {
                bot.state.borrow_mut().panel = None;
                if previous.as_deref() == Some(id) {
                    *self.mounted.borrow_mut() = None;
                }
            }
        }
        if let Some(previous) = previous.filter(|previous| previous != id)
            && let Ok(other) = self.bot(&previous)
        {
            other.state.borrow_mut().panel = None;
            self.apply(&other);
        }
        self.apply(&bot);
        Ok(())
    }

    /// The app window is shown (true), or hidden to the tray or minimized (false): no panel is visible then, but a
    /// stream or a tool still keeps its bot's page awake.
    pub fn set_window_visible(&self, visible: bool) {
        self.window_visible.set(visible);
        let bots: Vec<Rc<Bot>> = self.bots.borrow().values().cloned().collect();
        for bot in bots {
            self.apply(&bot);
        }
    }

    /// A `browser.stream` or `browser.frame` capture is on (true) or off for a bot.
    pub fn set_streaming(&self, id: &str, on: bool) -> Result<(), EngineError> {
        let bot = self.bot(id)?;
        let now = self.now();
        bot.state.borrow_mut().life.set_streaming(on, now);
        self.apply(&bot);
        Ok(())
    }

    fn bot(&self, id: &str) -> Result<Rc<Bot>, EngineError> {
        let bot = self
            .bots
            .borrow()
            .get(id)
            .cloned()
            .ok_or(EngineError::NotOpen)?;
        if bot.state.borrow().open {
            Ok(bot)
        } else {
            Err(EngineError::NotOpen)
        }
    }

    /// Brings WebView2 in line with the bot's need: resume first, then place and show; or hide, then suspend.
    /// Synchronous, and safe to call as often as you like: a plan that has not changed costs nothing.
    fn apply(&self, bot: &Rc<Bot>) {
        let now = self.now();
        let (plan, applied, tabs, container) = {
            let mut state = bot.state.borrow_mut();
            if !state.open || state.dead {
                return;
            }
            let shown = self.is_shown(&bot.identity.id, &state);
            state.life.set_shown(shown, now);
            let plan = state.life.need().plan();
            let memory = if plan.placement == Placement::Parked {
                state.life.memory_level(now)
            } else {
                MemoryLevel::Normal
            };
            let panel = state.panel.filter(|_| plan.placement == Placement::Panel);
            let occluded = self.occluded.get() || panel.is_some_and(|panel| panel.occluded);
            // Real mouse and keyboard reach the page only while the person holds it and can see it.
            let interactive = plan.placement == Placement::Panel
                && !occluded
                && state.access.owner() == Some("local")
                && state.handover == 0;
            let rect = match plan.placement {
                Placement::Panel => panel.map(|panel| panel.rect),
                Placement::Parked => Some(parked_rect(state.viewport)),
                Placement::Hidden => None,
            };
            let applied = Applied {
                plan,
                rect,
                occluded,
                active: state.active.clone(),
                tabs: state.tabs.len(),
                memory,
                interactive,
            };
            if state.applied.as_ref() == Some(&applied) {
                return;
            }
            let mut resized = false;
            if let (Placement::Panel, Some(rect)) = (plan.placement, rect) {
                resized = state.viewport != rect_size(&rect);
                state.viewport = rect_size(&rect);
            }
            // A panel showing the page gives the page its own size back after a fixed one.
            let mut restore = Vec::new();
            if plan.placement == Placement::Panel {
                for tab in &state.tabs {
                    if tab.fixed_viewport.take().is_some() {
                        restore.push(tab.id.clone());
                    }
                }
            }
            if resized || !restore.is_empty() {
                state.viewport_revision += 1;
            }
            for tab_id in restore {
                let _ = self.events.try_send(HostEvent::RestoreViewport {
                    bot_id: bot.identity.id.clone(),
                    tab_id,
                });
            }
            state.applied = Some(applied.clone());
            (plan, applied, state.tabs.clone(), state.container)
        };
        let Some(container) = container else {
            return;
        };
        if plan.suspend {
            Self::place(container, &tabs, None, false);
            Self::set_visible(&tabs, None);
            for tab in &tabs {
                if !tab.loading.get()
                    && !com::is_suspended(&tab.webview).unwrap_or(false)
                    && let Err(error) = com::try_suspend(&tab.webview)
                {
                    log::debug!("browser page not suspended: {error}");
                }
            }
        } else {
            for tab in &tabs {
                if com::is_suspended(&tab.webview).unwrap_or(false)
                    && let Err(error) = com::resume(&tab.webview)
                {
                    log::warn!("browser page not resumed: {error}");
                }
                com::set_memory_level(&tab.webview, applied.memory);
            }
            Self::place(container, &tabs, applied.rect, applied.occluded);
            Self::set_visible(&tabs, applied.active.as_deref());
        }
        self.set_interactive(container, applied.interactive);
    }

    /// Real mouse and keyboard input reaches a page only while `interactive`: the container window is disabled
    /// otherwise, and a page that held the keyboard gives it back to the app.
    fn set_interactive(&self, container: HWND, interactive: bool) {
        // SAFETY: plain Win32 calls on windows this engine owns, on the UI thread.
        unsafe {
            let _ = EnableWindow(container, interactive);
            let focus = GetFocus();
            if !interactive
                && !focus.is_invalid()
                && focus != self.config.parent
                && IsChild(container, focus).as_bool()
            {
                let _ = SetFocus(Some(self.config.parent));
            }
        }
    }

    fn place(container: HWND, tabs: &[Rc<Tab>], rect: Option<RECT>, window_hidden: bool) {
        // SAFETY: plain Win32 and COM calls on windows and controllers this engine owns, on the UI thread.
        unsafe {
            match rect {
                Some(rect) => {
                    let (width, height) = rect_size(&rect);
                    let show = if window_hidden {
                        Default::default()
                    } else {
                        SWP_SHOWWINDOW
                    };
                    let _ = SetWindowPos(
                        container,
                        None,
                        rect.left,
                        rect.top,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_NOZORDER | show,
                    );
                    for tab in tabs {
                        let _ = tab.controller.SetBounds(RECT {
                            left: 0,
                            top: 0,
                            right: width,
                            bottom: height,
                        });
                    }
                    if window_hidden {
                        let _ = ShowWindow(container, SW_HIDE);
                    }
                }
                None => {
                    let _ = ShowWindow(container, SW_HIDE);
                }
            }
        }
    }

    /// Only `active` renders; every other tab of the bot is hidden.
    fn set_visible(tabs: &[Rc<Tab>], active: Option<&str>) {
        for tab in tabs {
            // SAFETY: plain COM call on a controller this engine owns.
            let _ = unsafe { tab.controller.SetIsVisible(active == Some(tab.id.as_str())) };
        }
    }

    // ---- timers --------------------------------------------------------------------------------------------

    /// When [`tick`](Self::tick) next has something to do.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        self.bots
            .borrow()
            .values()
            .filter_map(|bot| bot.state.borrow().life.next_deadline(now))
            .min()
    }

    /// Runs what is due at `now`: a hidden bot idle for 10 minutes has its controller closed, and a bot kept awake
    /// out of sight for 30 seconds gets the Low memory target. The host calls this at each deadline.
    pub async fn tick(&self, now: Instant) {
        self.forget_controllers(now);
        let bots: Vec<Rc<Bot>> = self.bots.borrow().values().cloned().collect();
        for bot in bots {
            // An open or close in progress is not interrupted; the next tick sees the bot again.
            let Some(_turn) = bot.open_lock.try_lock() else {
                continue;
            };
            let due = {
                let state = bot.state.borrow();
                state.open && !state.dead && state.life.idle_close_due(now)
            };
            if due {
                self.teardown(&bot, true);
                continue;
            }
            self.apply(&bot);
            let level = {
                let state = bot.state.borrow();
                (state.open && state.life.need().awake() && !state.life.need().shown)
                    .then(|| state.life.memory_level(now))
            };
            if let Some(level) = level {
                self.set_memory(&bot, level);
            }
        }
    }

    fn set_memory(&self, bot: &Rc<Bot>, level: MemoryLevel) {
        let tabs = {
            let mut state = bot.state.borrow_mut();
            let Some(applied) = state.applied.as_mut() else {
                return;
            };
            if applied.memory == level {
                return;
            }
            applied.memory = level;
            state.tabs.clone()
        };
        for tab in &tabs {
            com::set_memory_level(&tab.webview, level);
        }
    }

    // ---- closing -------------------------------------------------------------------------------------------

    /// Closes the controllers and the container of a bot. With `remember`, the page's address is kept so the next
    /// `open` returns to it: this is the idle close, and the profile stays on disk.
    fn teardown(&self, bot: &Rc<Bot>, remember: bool) {
        let (tabs, active) = {
            let mut state = bot.state.borrow_mut();
            state.open = false;
            state.applied = None;
            state.panel = None;
            (std::mem::take(&mut state.tabs), state.active.take())
        };
        if remember {
            let last = tabs
                .iter()
                .find(|tab| Some(&tab.id) == active.as_ref())
                .and_then(|tab| com::source(&tab.webview).ok())
                .filter(|url| policy::native_browser_url(url).is_ok());
            bot.state.borrow_mut().last_url = last;
        }
        for tab in &tabs {
            // SAFETY: closing a controller this engine created and owns; its page ends with it.
            if let Err(error) = unsafe { tab.controller.Close() } {
                log::warn!("browser controller close failed: {}", error.message());
            }
        }
        drop(tabs);
        self.destroy_container(bot);
        if self.mounted.borrow().as_deref() == Some(bot.identity.id.as_str()) {
            *self.mounted.borrow_mut() = None;
        }
    }

    fn destroy_container(&self, bot: &Rc<Bot>) {
        let container = bot.state.borrow_mut().container.take();
        if let Some(container) = container {
            // SAFETY: a window this engine created, on its own thread.
            let _ = unsafe { DestroyWindow(container) };
        }
    }

    /// Closes a bot's browser for good: its pages, its container, its entry. The profile stays on disk.
    pub async fn close_bot(self: &Rc<Self>, id: &str) -> Result<(), EngineError> {
        let Some(bot) = self.bots.borrow().get(id).cloned() else {
            return Ok(());
        };
        let _turn = bot.open_lock.lock().await; // an open in progress finishes first
        self.teardown(&bot, false);
        bot.state.borrow_mut().dead = true;
        let mut bots = self.bots.borrow_mut();
        if bots
            .get(id)
            .is_some_and(|current| Rc::ptr_eq(current, &bot))
        {
            bots.remove(id);
        }
        Ok(())
    }

    /// Closes every bot's browser and the environment. The engine does not open again.
    pub async fn close_all(self: &Rc<Self>) -> Result<(), EngineError> {
        self.closed.set(true);
        let ids: Vec<String> = self.bots.borrow().keys().cloned().collect();
        for id in ids {
            self.close_bot(&id).await?;
        }
        *self.environment.lock().await = None;
        Ok(())
    }

    // ---- profiles ------------------------------------------------------------------------------------------

    /// Deletes a bot's profile: closes its browser, clears its sign-in and storage while the page still exists, then
    /// removes the folder, waiting a moment for Windows to release it.
    pub async fn delete_profile(self: &Rc<Self>, id: &str) -> Result<DeleteOutcome, EngineError> {
        let profile = policy::profile_name(id)?;
        if !self.deleting.borrow_mut().insert(profile.clone()) {
            return Err(EngineError::Deleting);
        }
        let outcome = self.delete_profile_reserved(id, &profile).await;
        self.deleting.borrow_mut().remove(&profile);
        outcome
    }

    async fn delete_profile_reserved(
        self: &Rc<Self>,
        id: &str,
        profile: &str,
    ) -> Result<DeleteOutcome, EngineError> {
        // Fail closed: sign-in is cleared from the live profile first, so a folder that stays locked holds none.
        let bot = self.bots.borrow().get(id).cloned();
        if let Some(bot) = bot {
            let webview = bot
                .state
                .borrow()
                .tabs
                .first()
                .map(|tab| tab.webview.clone());
            if let Some(webview) = webview {
                com::clear_profile(&webview).await?;
            }
        }
        self.close_bot(id).await?;
        let root = self.profile_root();
        for attempt in 0..UNLOCK_ATTEMPTS {
            if profiles::remove_profile(&root, profile)? {
                return Ok(DeleteOutcome::Removed);
            }
            if attempt + 1 < UNLOCK_ATTEMPTS {
                (self.config.sleep)(UNLOCK_INTERVAL).await;
            }
        }
        Ok(DeleteOutcome::Locked)
    }

    /// Removes profile folders whose bot no longer exists (those in `valid_ids` and the open ones are kept).
    /// Electron's old profiles in `data/native-browser` are not this engine's and are never touched.
    pub async fn prune_profiles(
        self: &Rc<Self>,
        valid_ids: &[String],
    ) -> Result<PruneReport, EngineError> {
        let mut keep: HashSet<String> = HashSet::new();
        for id in valid_ids {
            keep.insert(policy::profile_name(id)?);
        }
        let mut report = PruneReport::default();
        let mut errors: Vec<String> = Vec::new();
        for hash in profiles::profile_directories(&self.profile_root())? {
            // Rechecked at the last moment: a bot opened during the scan is never pruned.
            let open = self.bots.borrow().values().any(|bot| bot.profile == hash);
            if keep.contains(&hash) || open || self.deleting.borrow().contains(&hash) {
                continue;
            }
            self.deleting.borrow_mut().insert(hash.clone());
            let removed = profiles::remove_profile(&self.profile_root(), &hash);
            self.deleting.borrow_mut().remove(&hash);
            match removed {
                Ok(true) => report.removed.push(hash),
                Ok(false) => report.locked.push(hash),
                Err(error) => errors.push(error.to_string()),
            }
        }
        if errors.is_empty() {
            Ok(report)
        } else {
            Err(EngineError::Window(format!(
                "Could not safely prune all orphaned browser profiles: {}",
                errors.join("; ")
            )))
        }
    }

    // ---- reading -------------------------------------------------------------------------------------------

    /// The engine's record of a bot, and what WebView2 says about its active page. `None` for a bot never opened.
    pub fn snapshot(&self, id: &str) -> Option<BotSnapshot> {
        let bot = self.bots.borrow().get(id).cloned()?;
        let state = bot.state.borrow();
        let active = state
            .active
            .as_ref()
            .and_then(|active| state.tabs.iter().find(|tab| &tab.id == active));
        let mut visible = BOOL::default();
        if let Some(tab) = active {
            // SAFETY: plain COM call writing one BOOL.
            let _ = unsafe { tab.controller.IsVisible(&mut visible) };
        }
        let container = state.container;
        let container_shown = container.is_some_and(|window| {
            // SAFETY: a window this engine owns.
            unsafe { IsWindowVisible(window).as_bool() }
        });
        let container_rect = container.and_then(|window| {
            let mut corners = [POINT::default(); 2];
            let mut screen = RECT::default();
            // SAFETY: a window this engine owns; MapWindowPoints reads and writes the two points.
            unsafe {
                GetWindowRect(window, &mut screen).ok()?;
                corners[0] = POINT {
                    x: screen.left,
                    y: screen.top,
                };
                corners[1] = POINT {
                    x: screen.right,
                    y: screen.bottom,
                };
                MapWindowPoints(None, Some(self.config.parent), &mut corners);
            }
            Some(RECT {
                left: corners[0].x,
                top: corners[0].y,
                right: corners[1].x,
                bottom: corners[1].y,
            })
        });
        Some(BotSnapshot {
            open: state.open,
            tabs: state.tabs.len(),
            need: state.life.need(),
            placement: state
                .applied
                .as_ref()
                .map_or(Placement::Hidden, |applied| applied.plan.placement),
            visible: visible.as_bool(),
            suspended: active.is_some_and(|tab| com::is_suspended(&tab.webview).unwrap_or(false)),
            container_shown,
            container: container_rect,
            memory: state
                .applied
                .as_ref()
                .map_or(MemoryLevel::Normal, |applied| applied.memory),
            generation: state.generation.clone(),
        })
    }
}
