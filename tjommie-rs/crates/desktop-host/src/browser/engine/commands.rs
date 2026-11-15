//! The commands a bot's browser takes (`native-browser.ts`: `execute`, `act`, `control`, `remote`, `resize`, `mount`):
//! the person's own clicks, the phone's and other PCs' remote control, and the bot's page actions. Every command is
//! fenced by the access epochs, the browser generation, and a per-bot FIFO queue, as in TypeScript.

use std::future::Future;

use protocol::browser::{
    BrowserControl, BrowserFrameResult, BrowserInput, BrowserState, BrowserTab, HistoryAction,
    KeyAction, MouseAction, TabAction,
};
use protocol::native_browser::{
    NativeBrowserCommand, NativeBrowserHostCommand, NativeBrowserRemoteAction,
    NativeBrowserRemoteCommand, NativeBrowserUiCommand,
};
use serde::Serialize;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC, ICoreWebView2Deferral,
    ICoreWebView2NewWindowRequestedEventArgs,
};
use windows::core::{BOOL, HSTRING};

use super::*;
use crate::browser::cdp::{
    self, DispatchKeyParams, DispatchMouseParams, EmulationClearDeviceMetricsOverride,
    EmulationSetDeviceMetricsOverride, InputDispatchKeyEvent, InputDispatchMouseEvent,
    InputInsertText, InsertTextParams, MouseEventType, NoParams, PointerButton,
    SetDeviceMetricsParams,
};
use crate::browser::keyboard::FocusDirection;
use crate::browser::keys;
use crate::browser::tools::ToolReply;
use crate::browser::tools::protocol::{
    PageSetInterceptFileChooserDialog, SetInterceptFileChooserParams,
};

/// The person at this PC, as a controller. Remote controllers are named by their connection.
pub const LOCAL: &str = "local";

/// The longest address and title of a tab in a state: a page chooses both (history.pushState, document.title), and
/// the state goes to the daemon and every watching device on each change.
const URL_CHARS: usize = 8192;
const TITLE_CHARS: usize = 1024;

/// What a person is told when a deleted bot's profile was cleared but Windows still holds its folder: the same words the
/// TypeScript host uses.
const LOCKED_PROFILE_NOTICE: &str = "Browser sign-in and storage were cleared. Locked profile files will be removed on a later startup.";

/// A browser may have this many tabs open at once.
const TAB_LIMIT: usize = 30;
/// How long a disconnected remote controller is remembered, so its late commands are refused.
const CONTROLLER_MEMORY: Duration = Duration::from_secs(60);

/// A re-check made at each step of a long command: it fails once the caller's authority is gone.
pub type Check<'a> = &'a dyn Fn() -> Result<(), EngineError>;

/// Who a page action is for: the person's own click, or the bot's tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Human,
    Bot,
}

/// What a command answers: a browser state, or nothing (`{}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Reply {
    State(Box<BrowserState>),
    /// A browser tool's answer: text and images, with an error flag.
    Tool(Box<ToolReply>),
    /// A picture of the page, with the state it was taken in.
    Frame(Box<BrowserFrameResult>),
    Empty(Empty),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Empty {}

impl Reply {
    pub fn empty() -> Self {
        Self::Empty(Empty {})
    }

    pub fn state(state: BrowserState) -> Self {
        Self::State(Box::new(state))
    }

    pub fn tool(reply: ToolReply) -> Self {
        Self::Tool(Box::new(reply))
    }
}

/// A remote controller seen by this engine.
pub(super) struct ControllerLife {
    closed: bool,
    expires: Option<Instant>,
}

/// What the daemon says about the bots, which decides whether a bot's browser may exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotEvent {
    Deleted { id: String },
    Updated { id: String, browser: bool },
}

pub(super) fn run_check(check: Option<Check<'_>>) -> Result<(), EngineError> {
    match check {
        Some(check) => check(),
        None => Ok(()),
    }
}

fn private_input_refused() -> EngineError {
    EngineError::refused("Remote input is disabled during a private interaction.")
}

impl Engine {
    // ---- state -----------------------------------------------------------------------------------------------

    /// The browser's state as `owner` sees it (`can_control` is about them).
    pub fn state(&self, bot_id: &str, owner: &str) -> Result<BrowserState, EngineError> {
        let bot = self.bot(bot_id)?;
        Ok(self.browser_state(&bot, owner))
    }

    pub(super) fn browser_state(&self, bot: &Bot, owner: &str) -> BrowserState {
        let state = bot.state.borrow();
        let private = state.access.private_active();
        let tabs = state
            .tabs
            .iter()
            .map(|tab| BrowserTab {
                id: tab.id.clone(),
                url: if private {
                    "about:blank".to_string()
                } else {
                    com::source(&tab.webview)
                        .ok()
                        .filter(|url| !url.is_empty())
                        .map(|url| url.chars().take(URL_CHARS).collect())
                        .unwrap_or_else(|| "about:blank".to_string())
                },
                title: if private {
                    "Private interaction".to_string()
                } else {
                    com::title(&tab.webview)
                        .unwrap_or_default()
                        .chars()
                        .take(TITLE_CHARS)
                        .collect()
                },
            })
            .collect();
        BrowserState {
            bot_id: bot.identity.id.clone(),
            generation: state.generation.clone(),
            control: state.access.control(),
            control_epoch: Some(state.access.control_epoch()),
            private_epoch: Some(state.access.private_epoch()),
            private_active: Some(private),
            can_control: Some(state.access.owner() == Some(owner)),
            revision: Some(state.revision),
            viewport_revision: Some(state.viewport_revision),
            busy: Some(state.busy > 0 || state.handover > 0),
            active_page_id: state.active.clone(),
            tabs,
            dialog: if private { None } else { state.dialog.clone() },
        }
    }

    pub(super) fn notify_state(&self, bot: &Rc<Bot>) {
        if bot.state.borrow().dead {
            return;
        }
        let state = self.browser_state(bot, LOCAL);
        let _ = self.notify.try_send(Notification::State(Box::new(state)));
    }

    /// Something about the browser changed: a new revision, a state for the host to draw, and the page placed anew.
    pub(super) fn changed(&self, bot: &Rc<Bot>) {
        bot.state.borrow_mut().revision += 1;
        self.notify_state(bot);
        self.apply(bot);
        // A page switch moves the stream to the new page; a private interaction ends it.
        self.kick_stream(bot);
    }

    /// A notice for the person, unless a private interaction is on (nothing leaks out of it).
    pub(super) fn notice(&self, bot: &Rc<Bot>, message: &str) {
        if bot.state.borrow().access.private_active() {
            return;
        }
        let message: String = message.chars().take(2000).collect();
        let _ = self.notify.try_send(Notification::Notice {
            bot_id: bot.identity.id.clone(),
            message,
        });
    }

    pub(super) fn check_generation(&self, bot: &Bot, generation: &str) -> Result<(), EngineError> {
        if bot.state.borrow().generation == generation {
            Ok(())
        } else {
            Err(EngineError::refused(
                "The browser session changed; refresh its state.",
            ))
        }
    }

    pub(super) fn require_control(&self, bot: &Bot, actor: Actor) -> Result<(), EngineError> {
        let state = bot.state.borrow();
        if state.dead {
            return Err(EngineError::refused("The agent browser is closing."));
        }
        let wrong = match actor {
            Actor::Bot => {
                state.access.control() != BrowserControl::Bot
                    || state.handover > 0
                    || state.access.private_active()
            }
            Actor::Human => state.access.control() != BrowserControl::Human || state.handover > 0,
        };
        if !wrong {
            return Ok(());
        }
        Err(EngineError::refused(match actor {
            Actor::Bot => "The person is controlling this browser. Wait until they return control.",
            Actor::Human => "Take control of the browser first.",
        }))
    }

    pub(super) fn tab(&self, bot: &Bot, id: Option<&str>) -> Result<Rc<Tab>, EngineError> {
        let state = bot.state.borrow();
        let wanted = id.or(state.active.as_deref());
        wanted
            .and_then(|wanted| state.tabs.iter().find(|tab| tab.id == wanted).cloned())
            .ok_or_else(|| EngineError::refused("That browser tab is closed."))
    }

    pub(super) fn tabs_of(&self, bot: &Bot) -> Vec<Rc<Tab>> {
        bot.state.borrow().tabs.clone()
    }

    /// Runs `task` as the bot's next command: one at a time, in arrival order, and counted as `busy` meanwhile.
    pub(super) async fn enqueue<T>(
        &self,
        bot: &Rc<Bot>,
        task: impl Future<Output = Result<T, EngineError>>,
    ) -> Result<T, EngineError> {
        let _turn = bot.queue.lock().await;
        if bot.state.borrow().dead {
            return Err(EngineError::refused("The agent browser is closing."));
        }
        bot.state.borrow_mut().busy += 1;
        self.changed(bot);
        let result = task.await;
        {
            let mut state = bot.state.borrow_mut();
            state.busy = state.busy.saturating_sub(1);
        }
        if !bot.state.borrow().dead {
            self.changed(bot);
        }
        result
    }

    // ---- execute ---------------------------------------------------------------------------------------------

    /// What `/browser-host` hands the desktop for a bot, except tools (T3.3) and the live view (T3.4).
    pub async fn execute(
        self: &Rc<Self>,
        bot: &BotIdentity,
        command: NativeBrowserCommand,
    ) -> Result<Reply, EngineError> {
        let reply = match command {
            NativeBrowserCommand::Host(NativeBrowserHostCommand::Close) => {
                self.close_bot(&bot.id).await?;
                return Ok(Reply::empty());
            }
            NativeBrowserCommand::Host(NativeBrowserHostCommand::DeleteProfile) => {
                // The daemon deletes the bot's row only when this succeeds, so the outcome must be told truthfully:
                // a profile that could not be cleared and still stands is a failure, never a quiet success.
                match self.delete_profile(&bot.id).await? {
                    DeleteOutcome::Removed => {}
                    DeleteOutcome::ClearedButLocked => {
                        // The bot's entry is gone by now, so this goes out directly (as the TypeScript host's notice does).
                        let _ = self.notify.try_send(Notification::Notice {
                            bot_id: bot.id.clone(),
                            message: LOCKED_PROFILE_NOTICE.to_string(),
                        });
                    }
                    DeleteOutcome::Remains => {
                        return Err(EngineError::refused(
                            "This agent's browser profile could not be cleared, so its sign-in and storage remain.",
                        ));
                    }
                }
                return Ok(Reply::empty());
            }
            NativeBrowserCommand::Host(NativeBrowserHostCommand::CancelTools) => {
                return Err(EngineError::refused(
                    "Tool cancellation belongs to the private desktop host bridge.",
                ));
            }
            NativeBrowserCommand::Host(NativeBrowserHostCommand::Tool(_)) => {
                return Err(EngineError::refused(
                    "Native browser automation is unavailable.",
                ));
            }
            NativeBrowserCommand::Host(NativeBrowserHostCommand::Remote(remote)) => {
                self.remote(bot, &remote).await?
            }
            NativeBrowserCommand::Ui(ui) => {
                Reply::state(self.act(bot, &ui, Actor::Human, None, LOCAL).await?)
            }
        };
        // A bot deleted or switched off while its command ran: the browser closes, the command fails.
        if self.disabled.borrow().contains(&bot.id) {
            self.close_bot(&bot.id).await?;
            return Err(EngineError::refused("This agent's browser is off."));
        }
        Ok(reply)
    }

    /// The daemon says a bot was deleted, or its browser switched on or off.
    pub async fn on_bot_event(self: &Rc<Self>, event: BotEvent) {
        match event {
            BotEvent::Deleted { id } => {
                self.disabled.borrow_mut().insert(id.clone());
                let _ = self.close_bot(&id).await;
            }
            BotEvent::Updated { id, browser: true } => {
                self.disabled.borrow_mut().remove(&id);
            }
            BotEvent::Updated { id, browser: false } => {
                self.disabled.borrow_mut().insert(id.clone());
                let _ = self.close_bot(&id).await;
            }
        }
    }

    /// The daemon restarted (a new port and host token). Its connections are gone with it, and so is every remote
    /// controller's hold on a browser: the daemon never got to send their `disconnect` jobs, and the new daemon names
    /// its connections afresh. Each such hold is handed back to the bot, the old controllers' late commands are
    /// refused, and every ticket and frame from before the restart goes stale. The person's own hold is kept.
    ///
    /// The `/browser-host` bridge calls this itself when it loses the daemon (`Executor::daemon_lost`, queued to the UI
    /// thread by `host::JobQueue` and run by `host::serve`): an app that uses that pair has nothing to wire. Calling it
    /// again is harmless.
    pub async fn daemon_restarted(self: &Rc<Self>) {
        let now = self.now();
        for life in self.controllers.borrow_mut().values_mut() {
            life.closed = true;
            life.expires = Some(now + CONTROLLER_MEMORY);
        }
        let bots: Vec<Rc<Bot>> = self.bots.borrow().values().cloned().collect();
        for bot in bots {
            if !bot.state.borrow().open {
                continue;
            }
            let owner = bot.state.borrow().access.owner().map(str::to_string);
            if let Some(owner) = owner.filter(|owner| owner != LOCAL) {
                let identity = bot.identity.clone();
                if let Err(error) = self
                    .control(&identity, BrowserControl::Bot, &owner, false, None)
                    .await
                {
                    self.notice(&bot, &format!("Browser handback failed: {error}"));
                }
            }
            bot.state.borrow_mut().access.changed_page();
            self.drop_remote_streams(&bot);
            self.changed(&bot);
        }
    }

    /// A bot whose browser is on, as the daemon lists it: it may open.
    pub fn allow_bot(&self, id: &str) {
        self.disabled.borrow_mut().remove(id);
    }

    // ---- act -------------------------------------------------------------------------------------------------

    /// A page action for the person's click or the bot's tool. `check` is run before every step and fails the
    /// command the moment its caller's authority goes.
    pub async fn act(
        self: &Rc<Self>,
        bot: &BotIdentity,
        command: &NativeBrowserUiCommand,
        actor: Actor,
        check: Option<Check<'_>>,
        owner: &str,
    ) -> Result<BrowserState, EngineError> {
        run_check(check)?;
        if matches!(
            command,
            NativeBrowserUiCommand::Open | NativeBrowserUiCommand::State
        ) {
            self.open(bot).await?;
            run_check(check)?;
            return self.state(&bot.id, owner);
        }
        // The browser stays awake and parked while a command runs on it, even with no panel open.
        let guard = self.begin_tool(bot).await?;
        let rc = guard.bot.clone();
        let result = self.act_open(&rc, command, actor, check, owner).await;
        guard.end();
        result
    }

    async fn act_open(
        self: &Rc<Self>,
        bot: &Rc<Bot>,
        command: &NativeBrowserUiCommand,
        actor: Actor,
        check: Option<Check<'_>>,
        owner: &str,
    ) -> Result<BrowserState, EngineError> {
        run_check(check)?;
        if let NativeBrowserUiCommand::Control { control } = command {
            return self.control_open(bot, *control, owner, true, check).await;
        }
        let ticket = bot.state.borrow().access.ticket();
        let authorized = || -> Result<(), EngineError> {
            run_check(check)?;
            let state = bot.state.borrow();
            match actor {
                Actor::Human => state.access.require(owner, ticket.control_epoch)?,
                Actor::Bot => state.access.check(ticket, true)?,
            }
            Ok(())
        };
        authorized()?;
        let generation = match command {
            NativeBrowserUiCommand::Navigate { generation, .. }
            | NativeBrowserUiCommand::History { generation, .. }
            | NativeBrowserUiCommand::Tab { generation, .. }
            | NativeBrowserUiCommand::Dialog { generation, .. } => generation,
            _ => return self.state(&bot.identity.id, owner),
        };
        self.check_generation(bot, generation)?;
        // Answering a dialog unblocks the page, so it never waits behind a command stuck on that same dialog.
        if let NativeBrowserUiCommand::Dialog {
            page_id,
            accept,
            prompt_text,
            ..
        } = command
        {
            authorized()?;
            let tab = self.tab(bot, Some(page_id))?;
            let open = bot
                .state
                .borrow()
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.page_id == tab.id);
            if !open {
                return Err(EngineError::refused("There is no dialog in that tab."));
            }
            if !Self::answer_dialog(&tab, *accept, prompt_text.as_deref()) {
                return Err(EngineError::refused("There is no dialog in that tab."));
            }
            bot.state.borrow_mut().dialog = None;
            self.changed(bot);
            authorized()?;
            return Ok(self.browser_state(bot, owner));
        }
        self.enqueue(bot, async {
            authorized()?;
            self.require_control(bot, actor)?;
            self.check_generation(bot, generation)?;
            match command {
                NativeBrowserUiCommand::Tab {
                    action, page_id, ..
                } => {
                    for tab in self.tabs_of(bot) {
                        self.release(bot, &tab).await;
                        run_check(check)?;
                    }
                    match action {
                        TabAction::New => {
                            self.add_tab(bot, true).await?;
                            if actor == Actor::Bot {
                                run_check(check)?;
                            } else {
                                let state = bot.state.borrow();
                                if state.access.owner() != Some(owner)
                                    || state.access.private_active()
                                {
                                    return Err(EngineError::refused(
                                        "Browser control changed while opening the tab.",
                                    ));
                                }
                            }
                        }
                        TabAction::Close => {
                            let tab = self.tab(bot, page_id.as_deref())?;
                            self.destroy_tab(bot, &tab).await;
                        }
                        TabAction::Select => {
                            let tab = self.tab(bot, page_id.as_deref())?;
                            bot.state.borrow_mut().active = Some(tab.id.clone());
                        }
                    }
                    {
                        let mut state = bot.state.borrow_mut();
                        if state.access.owner().is_some() {
                            state.access.changed_page();
                        }
                    }
                    self.changed(bot);
                }
                NativeBrowserUiCommand::Navigate { page_id, url, .. } => {
                    let tab = self.tab(bot, Some(page_id))?;
                    self.release(bot, &tab).await;
                    run_check(check)?;
                    // The navigation's own completion may wait for a dialog or a slow subresource: state events
                    // report its progress.
                    let url = policy::native_browser_url(url)?;
                    // The page is loading from here on: it is not put to sleep before it has finished.
                    tab.set_loading(true, self.now());
                    // SAFETY: plain COM call on a page this engine owns.
                    if let Err(error) = unsafe { tab.webview.Navigate(&HSTRING::from(url)) } {
                        tab.set_loading(false, self.now());
                        self.notice(bot, &error.message());
                    }
                    self.changed(bot);
                }
                NativeBrowserUiCommand::History {
                    page_id, action, ..
                } => {
                    let tab = self.tab(bot, Some(page_id))?;
                    self.release(bot, &tab).await;
                    run_check(check)?;
                    Self::history(&tab, *action, self.now());
                    self.changed(bot);
                }
                _ => {}
            }
            Ok(self.browser_state(bot, owner))
        })
        .await
    }

    fn history(tab: &Tab, action: HistoryAction, now: Instant) {
        // SAFETY: plain COM calls on a page this engine owns.
        unsafe {
            match action {
                HistoryAction::Reload => {
                    tab.set_loading(tab.webview.Reload().is_ok(), now);
                }
                HistoryAction::Back => {
                    let mut can = BOOL::default();
                    if tab.webview.CanGoBack(&mut can).is_ok() && can.as_bool() {
                        tab.set_loading(tab.webview.GoBack().is_ok(), now);
                    }
                }
                HistoryAction::Forward => {
                    let mut can = BOOL::default();
                    if tab.webview.CanGoForward(&mut can).is_ok() && can.as_bool() {
                        tab.set_loading(tab.webview.GoForward().is_ok(), now);
                    }
                }
            }
        }
    }

    /// Opens another tab for the bot. A start-address tab loads the start page; a popup's tab is filled by the page.
    pub(super) async fn add_tab(
        &self,
        bot: &Rc<Bot>,
        start_page: bool,
    ) -> Result<Rc<Tab>, EngineError> {
        let environment = self.environment().await?;
        let container = bot
            .state
            .borrow()
            .container
            .ok_or_else(|| EngineError::refused("The agent browser is closing."))?;
        let url = start_page.then(|| self.config.start_url.clone());
        let tab = self
            .new_tab(bot, &environment, container, url.as_deref())
            .await?;
        if bot.state.borrow().dead {
            // SAFETY: closing the controller this function just created.
            let _ = unsafe { tab.controller.Close() };
            return Err(EngineError::refused("The agent browser is closing."));
        }
        let mut state = bot.state.borrow_mut();
        state.tabs.push(tab.clone());
        state.active = Some(tab.id.clone());
        Ok(tab)
    }

    pub(super) async fn destroy_tab(&self, bot: &Rc<Bot>, tab: &Rc<Tab>) {
        let had_dialog = bot
            .state
            .borrow()
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.page_id == tab.id);
        if had_dialog {
            self.dismiss(bot);
        }
        // SAFETY: closing a controller this engine owns; its page ends with it.
        let _ = unsafe { tab.controller.Close() };
        let mut state = bot.state.borrow_mut();
        state.tabs.retain(|other| other.id != tab.id);
        if state.active.as_deref() == Some(tab.id.as_str()) {
            state.active = state.tabs.last().map(|last| last.id.clone());
        }
        if state
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.page_id == tab.id)
        {
            state.dialog = None;
        }
    }

    /// Releases whatever injected input is still held on a page: keys up, buttons up.
    pub(super) async fn release(&self, bot: &Rc<Bot>, tab: &Rc<Tab>) {
        let waiting_on_dialog = bot
            .state
            .borrow()
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.page_id == tab.id);
        if waiting_on_dialog {
            return;
        }
        let held_keys: Vec<String> = tab.keys.borrow_mut().drain(..).collect();
        let held_buttons: Vec<_> = tab.buttons.borrow_mut().drain(..).collect();
        for key in held_keys {
            let event = keys::key_event(&key, KeyAction::Up, 0);
            let _ = cdp::call::<InputDispatchKeyEvent>(&tab.webview, &event).await;
        }
        let (x, y) = tab.point.get();
        for button in held_buttons {
            let _ = cdp::call::<InputDispatchMouseEvent>(
                &tab.webview,
                &DispatchMouseParams {
                    kind: MouseEventType::MouseReleased,
                    x,
                    y,
                    modifiers: 0,
                    button: pointer(button),
                    buttons: 0,
                    click_count: 1,
                    delta_x: None,
                    delta_y: None,
                },
            )
            .await;
        }
    }

    /// Answers the page's open dialog: accepting it (with the prompt's text, if it asks for one) or declining it.
    /// `false` if the page was not blocked on one.
    fn answer_dialog(tab: &Tab, accept: bool, prompt_text: Option<&str>) -> bool {
        let pending = tab.dialog.borrow_mut().take();
        let Some(pending) = pending else {
            return false;
        };
        // SAFETY: the arguments and deferral COM handed to the dialog handler, each used once here.
        unsafe {
            if accept {
                if let Some(text) = prompt_text {
                    let _ = pending.args.SetResultText(&HSTRING::from(text));
                }
                let _ = pending.args.Accept();
            }
            let _ = pending.deferral.Complete();
        }
        true
    }

    /// Declines the dialog a bot's page is blocked on, if any.
    fn dismiss(&self, bot: &Rc<Bot>) {
        let page_id = bot
            .state
            .borrow()
            .dialog
            .as_ref()
            .map(|dialog| dialog.page_id.clone());
        if let Some(page_id) = page_id
            && let Ok(tab) = self.tab(bot, Some(&page_id))
        {
            Self::answer_dialog(&tab, false, None);
        }
        bot.state.borrow_mut().dialog = None;
    }

    // ---- control ---------------------------------------------------------------------------------------------

    /// The person takes the browser, or hands it back to the bot. `explicit` is a request from the person, not a
    /// side effect of closing the panel; it is refused where a side effect would just be skipped.
    pub async fn control(
        self: &Rc<Self>,
        bot: &BotIdentity,
        control: BrowserControl,
        owner: &str,
        explicit: bool,
        check: Option<Check<'_>>,
    ) -> Result<BrowserState, EngineError> {
        let guard = self.begin_tool(bot).await?;
        let rc = guard.bot.clone();
        let result = self
            .control_open(&rc, control, owner, explicit, check)
            .await;
        guard.end();
        result
    }

    async fn control_open(
        self: &Rc<Self>,
        bot: &Rc<Bot>,
        control: BrowserControl,
        owner: &str,
        explicit: bool,
        check: Option<Check<'_>>,
    ) -> Result<BrowserState, EngineError> {
        run_check(check)?;
        let handover = {
            let mut state = bot.state.borrow_mut();
            match control {
                BrowserControl::Human => state.access.take(owner)?,
                BrowserControl::Bot => {
                    if state.access.private_active() && explicit {
                        return Err(EngineError::refused(
                            "Finish the private interaction before explicitly resuming the agent.",
                        ));
                    }
                    if !state.access.release(owner) {
                        if explicit && state.access.owner().is_some() {
                            return Err(EngineError::refused(
                                "Another device is controlling this browser.",
                            ));
                        }
                        drop(state);
                        return Ok(self.browser_state(bot, owner));
                    }
                }
            }
            state.handover += 1;
            state.handover
        };
        self.changed(bot);
        let has_dialog = bot.state.borrow().dialog.is_some();
        if has_dialog && control == BrowserControl::Bot {
            self.dismiss(bot);
        }
        // Ownership and held-input release cannot wait behind a browser tool or a frame.
        for tab in self.tabs_of(bot) {
            self.release(bot, &tab).await;
            if control == BrowserControl::Human && tab.file_chooser_intercept.replace(false) {
                // The tools had file choosers report to them; the person's click opens the native picker. A page blocked
                // on a dialog answers nothing, so this never waits long.
                let off = cdp::call::<PageSetInterceptFileChooserDialog>(
                    &tab.webview,
                    &SetInterceptFileChooserParams { enabled: false },
                );
                futures::pin_mut!(off);
                let _ = futures::future::select(off, self.sleep(Duration::from_millis(500))).await;
            }
        }
        {
            let mut state = bot.state.borrow_mut();
            if state.handover == handover {
                state.handover = 0;
            }
        }
        self.changed(bot);
        if control == BrowserControl::Human {
            let focus = {
                let state = bot.state.borrow();
                let interactive = state
                    .applied
                    .as_ref()
                    .is_some_and(|applied| applied.interactive);
                (state.access.owner() == Some(owner) && state.handover == 0 && interactive)
                    .then(|| {
                        state.active.as_ref().and_then(|active| {
                            state.tabs.iter().find(|tab| &tab.id == active).cloned()
                        })
                    })
                    .flatten()
            };
            if let Some(tab) = focus {
                // SAFETY: plain COM call on a page this engine owns.
                let _ = unsafe {
                    tab.controller
                        .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
                };
            }
        }
        Ok(self.browser_state(bot, owner))
    }

    // ---- remote ----------------------------------------------------------------------------------------------

    /// What a phone or another PC may do. Only the authenticated daemon adds the controller id.
    pub async fn remote(
        self: &Rc<Self>,
        bot: &BotIdentity,
        command: &NativeBrowserRemoteCommand,
    ) -> Result<Reply, EngineError> {
        let owner = command.controller_id.as_str();
        self.controllers
            .borrow_mut()
            .entry(owner.to_string())
            .or_insert(ControllerLife {
                closed: false,
                expires: None,
            });
        if matches!(command.action, NativeBrowserRemoteAction::Disconnect) {
            if let Some(life) = self.controllers.borrow_mut().get_mut(owner) {
                life.closed = true;
                life.expires = Some(self.now() + CONTROLLER_MEMORY);
            }
            let open = self
                .bots
                .borrow()
                .get(&bot.id)
                .is_some_and(|rc| rc.state.borrow().open && !rc.state.borrow().dead);
            if open {
                // The stream follows its controllers: this one's is over, but the handback must not wait for the
                // screencast to stop.
                if let Ok(rc) = self.bot(&bot.id) {
                    self.drop_controller_stream(&rc, owner);
                }
                self.control(bot, BrowserControl::Bot, owner, false, None)
                    .await?;
            }
            return Ok(Reply::empty());
        }
        let connection = || -> Result<(), EngineError> {
            let closed = self
                .controllers
                .borrow()
                .get(owner)
                .is_some_and(|life| life.closed);
            if closed {
                Err(EngineError::refused(
                    "This browser controller disconnected.",
                ))
            } else {
                Ok(())
            }
        };
        connection()?;
        self.open(bot).await?;
        connection()?;
        let rc = self.bot(&bot.id)?;
        match &command.action {
            NativeBrowserRemoteAction::State => {
                return Ok(Reply::state(self.browser_state(&rc, owner)));
            }
            NativeBrowserRemoteAction::Frame { thumbnail } => {
                let frame = self
                    .remote_frame(bot, owner, &connection, thumbnail.unwrap_or(false))
                    .await?;
                return Ok(Reply::Frame(Box::new(frame)));
            }
            NativeBrowserRemoteAction::Stream { on, max_fps } => {
                if self.config.spawn.is_none() || self.config.on_stream.is_none() {
                    return Err(EngineError::refused(
                        "The live view of a bot's browser is not available.",
                    ));
                }
                let state = self
                    .remote_stream(&rc, owner, *on, *max_fps, &connection)
                    .await?;
                return Ok(Reply::state(state));
            }
            NativeBrowserRemoteAction::Control {
                generation: Some(generation),
                ..
            } => {
                self.check_generation(&rc, generation)?;
            }
            _ => {}
        }
        if let NativeBrowserRemoteAction::Control {
            control,
            control_epoch,
            release_only,
            ..
        } = &command.action
        {
            if *control == BrowserControl::Bot {
                let Some(epoch) = control_epoch else {
                    return Err(EngineError::refused(
                        "Use the current browser control epoch.",
                    ));
                };
                rc.state.borrow().access.require(owner, *epoch)?;
            }
            let explicit = !release_only.unwrap_or(false);
            return self
                .control(bot, *control, owner, explicit, Some(&connection))
                .await
                .map(Reply::state);
        }
        let (generation, control_epoch) = match &command.action {
            NativeBrowserRemoteAction::Navigate {
                generation,
                control_epoch,
                ..
            }
            | NativeBrowserRemoteAction::History {
                generation,
                control_epoch,
                ..
            }
            | NativeBrowserRemoteAction::Tab {
                generation,
                control_epoch,
                ..
            }
            | NativeBrowserRemoteAction::Dialog {
                generation,
                control_epoch,
                ..
            }
            | NativeBrowserRemoteAction::Resize {
                generation,
                control_epoch,
                ..
            }
            | NativeBrowserRemoteAction::Input {
                generation,
                control_epoch,
                ..
            } => (generation.as_str(), *control_epoch),
            _ => return Ok(Reply::state(self.browser_state(&rc, owner))),
        };
        let check = || -> Result<(), EngineError> {
            connection()?;
            self.check_generation(&rc, generation)?;
            let state = rc.state.borrow();
            state.access.require(owner, control_epoch)?;
            if state.access.private_active() {
                return Err(private_input_refused());
            }
            Ok(())
        };
        check()?;
        let ui = match &command.action {
            NativeBrowserRemoteAction::Navigate {
                generation,
                page_id,
                url,
                ..
            } => NativeBrowserUiCommand::Navigate {
                generation: generation.clone(),
                page_id: page_id.clone(),
                url: url.clone(),
            },
            NativeBrowserRemoteAction::History {
                generation,
                page_id,
                action,
                ..
            } => NativeBrowserUiCommand::History {
                generation: generation.clone(),
                page_id: page_id.clone(),
                action: *action,
            },
            NativeBrowserRemoteAction::Tab {
                generation,
                action,
                page_id,
                ..
            } => NativeBrowserUiCommand::Tab {
                generation: generation.clone(),
                action: *action,
                page_id: page_id.clone(),
            },
            NativeBrowserRemoteAction::Dialog {
                generation,
                page_id,
                accept,
                prompt_text,
                ..
            } => NativeBrowserUiCommand::Dialog {
                generation: generation.clone(),
                page_id: page_id.clone(),
                accept: *accept,
                prompt_text: prompt_text.clone(),
            },
            NativeBrowserRemoteAction::Resize {
                page_id,
                width,
                height,
                ..
            } => {
                return self
                    .remote_resize(bot, &rc, page_id, *width, *height, owner, &check)
                    .await;
            }
            NativeBrowserRemoteAction::Input {
                page_id,
                viewport_revision,
                input,
                ..
            } => {
                return self
                    .remote_input(bot, &rc, page_id, *viewport_revision, input, &check)
                    .await;
            }
            _ => return Ok(Reply::state(self.browser_state(&rc, owner))),
        };
        self.act(bot, &ui, Actor::Human, Some(&check), owner)
            .await
            .map(Reply::state)
    }

    #[allow(clippy::too_many_arguments)]
    async fn remote_resize(
        self: &Rc<Self>,
        bot: &BotIdentity,
        rc: &Rc<Bot>,
        page_id: &str,
        width: u32,
        height: u32,
        owner: &str,
        check: Check<'_>,
    ) -> Result<Reply, EngineError> {
        let guard = self.begin_tool(bot).await?;
        let result = self
            .enqueue(rc, async {
                check()?;
                let tab = self.tab(rc, Some(page_id))?;
                if rc.state.borrow().active.as_deref() != Some(tab.id.as_str()) {
                    return Err(EngineError::refused("The active browser page changed."));
                }
                if self.is_shown(&rc.identity.id, &rc.state.borrow()) {
                    return Err(EngineError::refused(
                        "The browser follows the local Computer panel size while that panel is visible.",
                    ));
                }
                self.release(rc, &tab).await;
                check()?;
                self.resize_tab(rc, &tab, width, height).await?;
                check()?;
                rc.state.borrow_mut().viewport_revision += 1;
                self.changed(rc);
                Ok(Reply::state(self.browser_state(rc, owner)))
            })
            .await;
        guard.end();
        result
    }

    async fn remote_input(
        self: &Rc<Self>,
        bot: &BotIdentity,
        rc: &Rc<Bot>,
        page_id: &str,
        viewport_revision: u64,
        input: &BrowserInput,
        check: Check<'_>,
    ) -> Result<Reply, EngineError> {
        let guard = self.begin_tool(bot).await?;
        let result = self
            .enqueue(rc, async {
                check()?;
                let tab = self.tab(rc, Some(page_id))?;
                if rc.state.borrow().active.as_deref() != Some(tab.id.as_str()) {
                    return Err(EngineError::refused("The active browser page changed."));
                }
                if viewport_revision != rc.state.borrow().viewport_revision {
                    return Err(EngineError::refused(
                        "The browser viewport changed; wait for its current frame.",
                    ));
                }
                let (width, height) = self.css_viewport(rc, &tab);
                if let BrowserInput::Mouse { x, y, .. } | BrowserInput::Wheel { x, y, .. } = input
                    && (*x >= width || *y >= height)
                {
                    return Err(EngineError::refused(
                        "Browser input must be inside the current viewport.",
                    ));
                }
                self.inject(&tab, input).await?;
                check()?;
                Ok(Reply::empty())
            })
            .await;
        guard.end();
        result
    }

    /// The page's viewport in CSS pixels: the size it was fixed to, or its window's size at the display scale.
    fn css_viewport(&self, bot: &Rc<Bot>, tab: &Tab) -> (f64, f64) {
        if let Some((width, height)) = tab.fixed_viewport.get() {
            return (f64::from(width), f64::from(height));
        }
        let (width, height) = bot.state.borrow().viewport;
        let scale = com::rasterization_scale(&tab.controller).max(0.1);
        (f64::from(width) / scale, f64::from(height) / scale)
    }

    /// Input for a page, injected through the DevTools protocol: the same events a person's hands would make.
    async fn inject(&self, tab: &Rc<Tab>, input: &BrowserInput) -> Result<(), EngineError> {
        let modifiers = tab
            .keys
            .borrow()
            .iter()
            .fold(0, |held, key| held | keys::modifier_bit(key));
        match input {
            BrowserInput::Text { text } => {
                cdp::call::<InputInsertText>(
                    &tab.webview,
                    &InsertTextParams { text: text.clone() },
                )
                .await?;
            }
            BrowserInput::Key { action, key } => {
                let event: DispatchKeyParams = keys::key_event(key, *action, modifiers);
                cdp::call::<InputDispatchKeyEvent>(&tab.webview, &event).await?;
                let mut held = tab.keys.borrow_mut();
                match action {
                    KeyAction::Down => {
                        if !held.contains(key) {
                            held.push(key.clone());
                        }
                    }
                    KeyAction::Up => held.retain(|other| other != key),
                }
            }
            BrowserInput::Wheel {
                x,
                y,
                delta_x,
                delta_y,
            } => {
                cdp::call::<InputDispatchMouseEvent>(
                    &tab.webview,
                    &DispatchMouseParams {
                        kind: MouseEventType::MouseWheel,
                        x: x.round(),
                        y: y.round(),
                        modifiers,
                        button: PointerButton::None,
                        buttons: 0,
                        click_count: 0,
                        delta_x: Some(*delta_x),
                        delta_y: Some(*delta_y),
                    },
                )
                .await?;
            }
            BrowserInput::Mouse {
                action,
                x,
                y,
                button,
            } => {
                let point = (x.round(), y.round());
                tab.point.set(point);
                if *action != MouseAction::Move && button.is_none() {
                    return Err(EngineError::refused(
                        "A pointer press or release needs a button.",
                    ));
                }
                let kind = match action {
                    MouseAction::Move => MouseEventType::MouseMoved,
                    MouseAction::Down => MouseEventType::MousePressed,
                    MouseAction::Up => MouseEventType::MouseReleased,
                };
                if let Some(button) = button {
                    let mut held = tab.buttons.borrow_mut();
                    match action {
                        MouseAction::Down if !held.contains(button) => held.push(*button),
                        MouseAction::Up => held.retain(|other| other != button),
                        _ => {}
                    }
                }
                let buttons = tab
                    .buttons
                    .borrow()
                    .iter()
                    .fold(0, |mask, held| mask | button_bit(*held));
                cdp::call::<InputDispatchMouseEvent>(
                    &tab.webview,
                    &DispatchMouseParams {
                        kind,
                        x: point.0,
                        y: point.1,
                        modifiers,
                        button: button.map_or(PointerButton::None, pointer),
                        buttons,
                        click_count: u32::from(*action != MouseAction::Move),
                        delta_x: None,
                        delta_y: None,
                    },
                )
                .await?;
            }
        }
        Ok(())
    }

    // ---- sizes -----------------------------------------------------------------------------------------------

    /// A fixed agent viewport for a browser no panel is showing: 240-2560 by 180-1600 CSS pixels.
    pub async fn resize(
        self: &Rc<Self>,
        bot: &BotIdentity,
        width: u32,
        height: u32,
        check: Option<Check<'_>>,
    ) -> Result<BrowserState, EngineError> {
        run_check(check)?;
        if !(240..=2560).contains(&width) || !(180..=1600).contains(&height) {
            return Err(EngineError::refused(
                "Browser dimensions must be 240\u{2013}2560 by 180\u{2013}1600.",
            ));
        }
        let guard = self.begin_tool(bot).await?;
        let rc = guard.bot.clone();
        let result = self
            .enqueue(&rc, async {
                run_check(check)?;
                self.require_control(&rc, Actor::Bot)?;
                if self.is_shown(&rc.identity.id, &rc.state.borrow()) {
                    return Err(EngineError::refused(
                        "The visible browser follows its panel size. Resize the Tjommie panel, or hide it before requesting a fixed agent viewport.",
                    ));
                }
                let tab = self.tab(&rc, None)?;
                self.resize_tab(&rc, &tab, width, height).await?;
                run_check(check)?;
                rc.state.borrow_mut().viewport_revision += 1;
                self.changed(&rc);
                Ok(self.browser_state(&rc, LOCAL))
            })
            .await;
        guard.end();
        result
    }

    /// Fixes a page at `width` by `height` CSS pixels: the engine keeps hidden pages at the size they last had
    /// until told otherwise.
    async fn resize_tab(
        &self,
        bot: &Rc<Bot>,
        tab: &Rc<Tab>,
        width: u32,
        height: u32,
    ) -> Result<(), EngineError> {
        cdp::call::<EmulationSetDeviceMetricsOverride>(
            &tab.webview,
            &SetDeviceMetricsParams {
                width,
                height,
                device_scale_factor: 0.0,
                mobile: false,
            },
        )
        .await?;
        tab.fixed_viewport.set(Some((width, height)));
        let mut state = bot.state.borrow_mut();
        state.viewport = (
            i32::try_from(width).unwrap_or(i32::MAX),
            i32::try_from(height).unwrap_or(i32::MAX),
        );
        // The parked window takes the new size at the next placement.
        state.applied = None;
        drop(state);
        self.apply(bot);
        Ok(())
    }

    // ---- the panel -------------------------------------------------------------------------------------------

    /// Opens the panel over a bot's page (or closes it), as the person sees it, and hands the browser back to the
    /// bot when the panel goes: a closed panel leaves nobody holding a page they cannot see.
    pub async fn mount(
        self: &Rc<Self>,
        bot: &BotIdentity,
        panel: Option<Panel>,
    ) -> Result<BrowserState, EngineError> {
        self.open(bot).await?;
        let Some(panel) = panel else {
            self.set_panel(&bot.id, None)?;
            return self
                .control(bot, BrowserControl::Bot, LOCAL, false, None)
                .await;
        };
        let previous = self
            .mounted
            .borrow()
            .clone()
            .filter(|previous| previous != &bot.id)
            .and_then(|previous| self.bots.borrow().get(&previous).cloned());
        // Opening the panel moves it off the bot that had it (set_panel does that); that bot gets the browser back.
        self.set_panel(&bot.id, Some(panel))?;
        if let Some(previous) = previous {
            let identity = previous.identity.clone();
            if let Err(error) = self
                .control(&identity, BrowserControl::Bot, LOCAL, false, None)
                .await
            {
                self.notice(&previous, &format!("Browser handback failed: {error}"));
            }
        }
        let rc = self.bot(&bot.id)?;
        self.changed(&rc);
        Ok(self.browser_state(&rc, LOCAL))
    }

    /// The panel closed or moved to another bot: the page leaves, and the browser goes back to the bot.
    pub async fn hide_bot(self: &Rc<Self>, id: &str) {
        let Ok(bot) = self.bot(id) else {
            return;
        };
        let _ = self.set_panel(id, None);
        let identity = bot.identity.clone();
        if let Err(error) = self
            .control(&identity, BrowserControl::Bot, LOCAL, false, None)
            .await
        {
            self.notice(&bot, &format!("Browser handback failed: {error}"));
        }
    }

    /// A menu or dialog covers this bot's page (T3.5 draws the shield).
    pub fn occlude_bot(&self, id: &str) {
        let Ok(bot) = self.bot(id) else {
            return;
        };
        if let Some(panel) = bot.state.borrow_mut().panel.as_mut() {
            panel.occluded = true;
        }
        self.apply(&bot);
    }

    /// A menu or dialog covers the page area, whichever bot is in it.
    pub fn set_occluded(&self, value: bool) {
        self.occluded.set(value);
        let bots: Vec<Rc<Bot>> = self.bots.borrow().values().cloned().collect();
        for bot in bots {
            self.apply(&bot);
        }
    }

    // ---- events ----------------------------------------------------------------------------------------------

    /// Applies one typed message from WebView2's callbacks. The host calls this for every message, in order.
    pub async fn apply_event(self: &Rc<Self>, event: HostEvent) {
        match event {
            HostEvent::Reapply { bot_id } => {
                let bot = self.bots.borrow().get(&bot_id).cloned();
                if let Some(bot) = bot {
                    self.apply(&bot);
                }
            }
            HostEvent::PageChanged {
                bot_id,
                tab_id,
                navigation,
                loading,
            } => {
                let Some(bot) = self.live_bot(&bot_id) else {
                    return;
                };
                if let Some(loading) = loading {
                    let tab = bot
                        .state
                        .borrow()
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .cloned();
                    if let Some(tab) = tab {
                        tab.set_loading(loading, self.now());
                        if !loading {
                            // A page that has finished loading may go to sleep now: plan it again.
                            bot.state.borrow_mut().applied = None;
                        }
                    }
                }
                if navigation {
                    {
                        let mut state = bot.state.borrow_mut();
                        if state.access.owner().is_some() {
                            state.access.changed_page();
                        }
                    }
                    let tab = bot
                        .state
                        .borrow()
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .cloned();
                    if let Some(tab) = tab {
                        self.release(&bot, &tab).await;
                    }
                }
                self.changed(&bot);
            }
            HostEvent::TabClosed { bot_id, tab_id } => {
                let Some(bot) = self.live_bot(&bot_id) else {
                    return;
                };
                let tab = bot
                    .state
                    .borrow()
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .cloned();
                if let Some(tab) = tab {
                    self.destroy_tab(&bot, &tab).await;
                    self.changed(&bot);
                }
            }
            HostEvent::NewWindow {
                bot_id,
                args,
                deferral,
            } => {
                self.new_window(&bot_id, &args, &deferral).await;
            }
            HostEvent::Dialog {
                bot_id,
                tab_id,
                kind,
                message,
                default_value,
                args,
                deferral,
            } => {
                let tab = self.live_bot(&bot_id).and_then(|bot| {
                    bot.state
                        .borrow()
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .cloned()
                });
                let (Some(bot), Some(tab)) = (self.live_bot(&bot_id), tab) else {
                    // The page or the bot is gone: the dialog is declined so nothing waits on it.
                    // SAFETY: the deferral COM handed to the handler, completed once here.
                    let _ = unsafe { deferral.Complete() };
                    return;
                };
                *tab.dialog.borrow_mut() = Some(events::PendingDialog { args, deferral });
                let limit = |text: String| text.chars().take(10_000).collect::<String>();
                bot.state.borrow_mut().dialog = Some(protocol::browser::BrowserDialog {
                    page_id: tab_id,
                    kind,
                    message: limit(message),
                    default_value: limit(default_value),
                });
                self.changed(&bot);
            }
            HostEvent::Notice { bot_id, message } => {
                if let Some(bot) = self.live_bot(&bot_id) {
                    self.notice(&bot, &message);
                }
            }
            HostEvent::SuspendDone { bot_id } => {
                if let Some(bot) = self.live_bot(&bot_id) {
                    // The plan may be the same as before, but the page is not: look at it again.
                    bot.state.borrow_mut().applied = None;
                    self.apply(&bot);
                }
            }
            HostEvent::BindingCalled { bot_id, json } => {
                if let Some(bot) = self.live_bot(&bot_id) {
                    self.on_binding(&bot, &json);
                }
            }
            HostEvent::MoveFocus { bot_id, next } => {
                if let Some(bot) = self.live_bot(&bot_id) {
                    self.focus_leaves(
                        &bot,
                        if next {
                            FocusDirection::Next
                        } else {
                            FocusDirection::Previous
                        },
                    );
                }
            }
            HostEvent::Shortcut { bot_id, chord } => {
                if let Some(bot) = self.live_bot(&bot_id) {
                    self.on_shortcut(&bot, chord);
                }
            }
            HostEvent::ScreencastFrame {
                bot_id,
                tab_id,
                json,
            } => self.on_screencast_frame(&bot_id, &tab_id, &json),
            HostEvent::RestoreViewport { bot_id, tab_id } => {
                let Some(bot) = self.live_bot(&bot_id) else {
                    return;
                };
                let tab = bot
                    .state
                    .borrow()
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .cloned();
                if let Some(tab) = tab {
                    let cleared = cdp::call::<EmulationClearDeviceMetricsOverride>(
                        &tab.webview,
                        &NoParams {},
                    )
                    .await;
                    if cleared.is_err() {
                        self.notice(&bot, "The native browser viewport could not be restored.");
                    }
                    bot.state.borrow_mut().viewport_revision += 1;
                    self.changed(&bot);
                }
            }
        }
    }

    pub(super) fn live_bot(&self, id: &str) -> Option<Rc<Bot>> {
        let bot = self.bots.borrow().get(id).cloned()?;
        let live = bot.state.borrow().open && !bot.state.borrow().dead;
        live.then_some(bot)
    }

    /// A popup: its contents become a new tab of the same bot, up to the tab limit.
    async fn new_window(
        self: &Rc<Self>,
        bot_id: &str,
        args: &ICoreWebView2NewWindowRequestedEventArgs,
        deferral: &ICoreWebView2Deferral,
    ) {
        let refuse = |message: Option<&str>| {
            if let (Some(message), Some(bot)) = (message, self.live_bot(bot_id)) {
                self.notice(&bot, message);
            }
            // SAFETY: the arguments and deferral COM handed to the handler, completed exactly once here.
            unsafe {
                let _ = args.SetHandled(true);
                let _ = deferral.Complete();
            }
        };
        let Some(bot) = self.live_bot(bot_id) else {
            return refuse(None);
        };
        if bot.state.borrow().tabs.len() >= TAB_LIMIT {
            return refuse(Some("The browser tab limit is 30."));
        }
        match self.add_tab(&bot, false).await {
            Ok(tab) => {
                // SAFETY: as above; the new page becomes the popup's window.
                unsafe {
                    let _ = args.SetNewWindow(&tab.webview);
                    let _ = args.SetHandled(true);
                    let _ = deferral.Complete();
                }
                self.changed(&bot);
            }
            Err(error) => refuse(Some(&format!(
                "The browser could not open a new tab: {error}"
            ))),
        }
    }

    /// Drops what the engine remembers of remote controllers that disconnected a minute ago.
    pub(super) fn forget_controllers(&self, now: Instant) {
        self.controllers
            .borrow_mut()
            .retain(|_, life| life.expires.is_none_or(|expires| expires > now));
    }
}

fn pointer(button: protocol::browser::MouseButton) -> PointerButton {
    match button {
        protocol::browser::MouseButton::Left => PointerButton::Left,
        protocol::browser::MouseButton::Middle => PointerButton::Middle,
        protocol::browser::MouseButton::Right => PointerButton::Right,
    }
}

/// A button's bit in a CDP `buttons` mask: left 1, right 2, middle 4.
fn button_bit(button: protocol::browser::MouseButton) -> u32 {
    match button {
        protocol::browser::MouseButton::Left => 1,
        protocol::browser::MouseButton::Right => 2,
        protocol::browser::MouseButton::Middle => 4,
    }
}
