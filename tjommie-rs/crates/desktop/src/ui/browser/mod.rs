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
pub mod stream;

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Global, Window};
use protocol::browser::{BrowserControl, BrowserFrame, BrowserState, HistoryAction, TabAction};
use protocol::model::Bot;

use crate::guard::guarded;
use crate::keymap::{Dismiss, TogglePanel};
use crate::motion::keyboard::Trigger;

/// What the panel shows of one bot's browser: T1.4's `model::browser_session::BrowserView`, field for field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserView {
    pub state: Option<BrowserState>,
    /// A remote PC's page as a picture (`browser.streamFrame`, else `browser.frame`); this PC's page is WebView2's own.
    pub frame: Option<BrowserFrame>,
    pub error: Option<String>,
    pub busy: bool,
}

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
    /// What the host's vault answered for each bot, by bot id.
    pub logins: HashMap<String, logins::LoginsView>,
    /// Takes the logins menu's actions to the host's vault (T1.4's logins client); `None` until it is wired.
    pub on_logins: Option<logins::OnLogins>,
}

impl Global for BrowserData {}

/// Ctrl+B opens or closes the panel, and Escape (outside an overlay) closes it; both are keys, so both snap. They are
/// app-level handlers, so they work wherever the focus is (keymap.rs, M2 review F4).
pub fn init(cx: &mut App) {
    cx.default_global::<BrowserData>();
    cx.on_action(|_: &TogglePanel, cx| {
        guarded("toggle the browser panel", || {
            panel::BrowserPanel::toggle(Trigger::Keyboard, cx)
        });
    });
    cx.on_action(|_: &Dismiss, cx| {
        guarded("close the browser panel", || {
            panel::BrowserPanel::dismiss(cx)
        });
    });
}

/// An e2e run's stand-in for the store (`TJOMMIE_E2E_FIXTURE=panel`): connected, with one bot in the Recents, so the
/// panel has a chat to open beside. Test-driver builds only.
#[cfg(feature = "test-driver")]
pub fn e2e_fixture(cx: &mut App) {
    use protocol::model::{Autonomy, BotColor, BotShape, BotState, Provider};

    use crate::ui::shell::data::{Connection, Recent, RecentKind, ShellData};

    if std::env::var("TJOMMIE_E2E_FIXTURE").as_deref() != Ok("panel") {
        return;
    }
    let forge = Bot {
        id: "bot-1".into(),
        name: "Forge".into(),
        color: BotColor::Blue,
        shape: BotShape::Hexagon,
        instructions: String::new(),
        provider: Provider::Claude,
        model: "claude-haiku".into(),
        account_id: None,
        effective_account_id: None,
        using_fallback: None,
        autonomy: Autonomy::Ask,
        home_folder: String::new(),
        project_folder: None,
        // The panel's state only: no page, so no WebView2 in the run.
        browser: false,
        paused: false,
        can_message: Vec::new(),
        state: BotState::Idle,
        setup_reason: None,
        limited_until: None,
        created_at: String::new(),
    };
    let shell = cx.default_global::<ShellData>();
    shell.connection = Connection::Ready;
    shell.recents = vec![Recent {
        id: forge.id.clone().into(),
        name: forge.name.clone().into(),
        kind: RecentKind::Bot {
            color: forge.color,
            shape: forge.shape,
            state: forge.state,
        },
    }];
    cx.default_global::<BrowserData>().bots = vec![forge];
    cx.refresh_windows();
}
