//! The browser panel (Phase 3 plan T3.7; `right-panel.tsx`, `browser-panel.tsx`, `browser-logins.tsx`; Aegis
//! `browser-panel.html`, motion.html card 12): a bot's browser beside its chat, its chrome, and its motion.
//!
//! What the panel reads of the daemon comes through `BrowserData`, as the shell's comes through `ShellData`: T1.4's store
//! fills it once it is wired. This PC's browsers are the engine's (desktop-host), driven on the UI thread (`local`). A
//! window on another PC shows that PC's browser as T1.4's `BrowserSession` sees it (`BrowserView`), through `on_remote`.

pub mod chrome;
pub mod local;
pub mod logins;
pub mod motion;
pub mod panel;
pub mod panel_variant;
pub mod remote;
pub mod stream;

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Global, Window};
use protocol::browser::{BrowserControl, BrowserFrame, HistoryAction, TabAction};
use protocol::model::Bot;

use crate::guard::guarded;
use crate::keymap::{Dismiss, TogglePanel};
use crate::model::logins::LiveState as _;
use crate::model::state::{AppState, Connection};
use crate::motion::keyboard::Trigger;

/// What the panel shows of one bot's browser (T1.4): the state, a remote PC's page as a picture (this PC's page is
/// WebView2's own), what went wrong, and whether a command is busy.
pub use crate::model::browser_session::BrowserView;

/// What the person asks of a bot's browser in the panel (`BrowserControls`, browser-panel.tsx).
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserCommand {
    SetControl(BrowserControl),
    Navigate(String),
    History(HistoryAction),
    Dialog {
        accept: bool,
        prompt_text: Option<String>,
    },
    Tab {
        action: TabAction,
        page_id: Option<String>,
    },
    /// Open the browser again after it failed to ("Try again").
    Retry,
    /// A remote picture is on screen, so the live stream may send the next (`BrowserSession::drawn`).
    Drawn(BrowserFrame),
    /// The panel starts showing a remote PC's bot's browser (the Browser pane is shown).
    Show,
    /// The panel stops showing it (closed, another pane, another bot): the browser goes back to the bot.
    Hide,
}

/// Takes a remote panel's command for a bot to the store's session.
pub type OnRemote = Rc<dyn Fn(&str, BrowserCommand, &mut Window, &mut App)>;

/// What the panel shows that the daemon decides. Until the store is wired: no bots, so no panel.
#[derive(Clone, Default)]
pub struct BrowserData {
    /// The bots (`AppState.bots`): the panel reads its chat's bot here.
    pub bots: Vec<Bot>,
    /// A window on another PC: each bot's browser as T1.4's `BrowserSession` shows it, by bot id.
    pub remote: HashMap<String, BrowserView>,
    /// Takes the remote panel's commands to the store's sessions; `None` until it is wired.
    pub on_remote: Option<OnRemote>,
    /// How many times the remote sessions were wired afresh (a new PC or connection): the panel asks to show its bot again.
    pub epoch: u64,
    /// What the host's vault answered for each bot, by bot id.
    pub logins: HashMap<String, logins::LoginsView>,
    /// Takes the logins menu's actions to the host's vault (T1.4's logins client); `None` until it is wired.
    pub on_logins: Option<logins::OnLogins>,
}

impl Global for BrowserData {}

/// What the panel keeps of the app's connection: the PC and connection its remote sessions are wired to, if any.
#[derive(Default)]
struct Wired(Option<(String, u64)>);

impl Global for Wired {}

/// Feeds the panel from the app's state, after each copy of it: the agents it can show a browser for and, in a window on
/// another PC, the wiring of its sessions to that PC's connection (`remote::wire`), made again when the PC or its
/// connection changes and ended (`remote::unwire`) when the window leaves it or the connection ends.
pub fn feed(state: &AppState, cx: &mut App) {
    let data = cx.default_global::<BrowserData>();
    if data.bots != state.bots {
        data.bots.clone_from(&state.bots);
    }
    let want = state
        .host
        .clone()
        .filter(|_| state.connection == Connection::Ready)
        .zip(crate::ui::store::live_state(cx).daemon_generation());
    if cx.default_global::<Wired>().0 == want {
        return;
    }
    if cx.default_global::<Wired>().0.is_some() {
        remote::unwire(cx);
    }
    if want.is_some() {
        remote::wire(std::sync::Arc::new(crate::ui::store::rpc(cx)), true, cx);
    }
    cx.default_global::<Wired>().0 = want;
}

/// Ctrl+B opens or closes the panel, and Escape (outside an overlay) closes it; both are keys, so both snap. They are
/// app-level handlers, so they work wherever the focus is (keymap.rs, M2 review F4). An open exchange goes first:
/// Ctrl+B closes only it, as the chat header's panel button does (M4 review A6), and the shell's root takes Escape for
/// it.
pub fn init(cx: &mut App) {
    cx.default_global::<BrowserData>();
    cx.on_action(|_: &TogglePanel, cx| {
        guarded("toggle the browser panel", || {
            crate::ui::chat::exchange::toggle_panel(Trigger::Keyboard, cx)
        });
    });
    cx.on_action(|_: &Dismiss, cx| {
        guarded("close the browser panel", || {
            panel::BrowserPanel::dismiss(cx)
        });
    });
}
