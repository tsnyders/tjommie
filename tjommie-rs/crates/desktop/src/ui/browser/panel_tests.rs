//! The panel's view tests (`panel::…`): its keys and motion, and `browser-panel.test.tsx` / `right-panel.test.tsx` ported
//! case by case where the GPUI panel has the behaviour.

use std::cell::RefCell;

use gpui::{InteractiveElement as _, Modifiers, VisualTestContext};
use protocol::browser::{
    BrowserControl, BrowserDialog, BrowserFrame, BrowserState, BrowserTab, FrameMime, HistoryAction,
};
use protocol::model::{Autonomy, BotColor, BotShape, BotState, Provider};

use super::*;
use crate::theme::Theme;

fn bot(id: &str, browser: bool) -> Bot {
    Bot {
        id: id.into(),
        name: "Scout".into(),
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
        browser,
        paused: false,
        can_message: Vec::new(),
        state: BotState::Idle,
        setup_reason: None,
        limited_until: None,
        created_at: String::new(),
    }
}

fn state(control: BrowserControl) -> BrowserState {
    BrowserState {
        bot_id: "b1".into(),
        generation: "g1".into(),
        control,
        control_epoch: Some(1),
        private_epoch: None,
        private_active: None,
        can_control: Some(true),
        revision: None,
        viewport_revision: None,
        busy: None,
        active_page_id: Some("p1".into()),
        tabs: vec![BrowserTab {
            id: "p1".into(),
            url: "https://example.com/".into(),
            title: "Example".into(),
        }],
        dialog: None,
    }
}

fn shown(state: BrowserState) -> BrowserView {
    BrowserView {
        state: Some(state),
        ..BrowserView::default()
    }
}

struct Host {
    panel: Entity<BrowserPanel>,
    focus: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        clock::begin_frame(cx);
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .child(div().flex_1())
            .child(self.panel.clone())
            .child(clock::painted())
    }
}

type Sent = Rc<RefCell<Vec<BrowserCommand>>>;

/// A window with the panel beside an empty column, on the bot's chat. With a remote view the window is on another PC,
/// and the commands it sends there are recorded.
fn panel_window(
    cx: &mut gpui::TestAppContext,
    bot: Bot,
    remote: Option<BrowserView>,
) -> (Entity<BrowserPanel>, Sent, &mut VisualTestContext) {
    let sent: Sent = Rc::default();
    let record = sent.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(crate::keymap::bindings());
        cx.set_global(Theme::aegis().unwrap());
        crate::notices::init(cx);
        super::super::init(cx);
        let mut data = BrowserData {
            bots: vec![bot.clone()],
            ..BrowserData::default()
        };
        if let Some(view) = remote {
            data.remote.insert(bot.id.clone(), view);
            data.on_remote = Some(Rc::new(move |_, command, _, _| {
                record.borrow_mut().push(command)
            }));
            cx.set_global(ShellData {
                host: Some("studio".into()),
                ..ShellData::default()
            });
        }
        cx.set_global(data);
        cx.navigate(Route::Chat(bot.id.clone()));
    });
    let made = Rc::new(RefCell::new(None));
    let keep = made.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        let panel = BrowserPanel::create(window, cx);
        *keep.borrow_mut() = Some(panel.clone());
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let host = cx.new(|_| Host { panel, focus });
        gpui_kit::component::Root::new(host, window, cx)
    });
    draw(cx);
    let panel = made.borrow().clone().unwrap();
    (panel, sent, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Opens the panel at once on the Browser pane.
fn open_browser(panel: &Entity<BrowserPanel>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            let bot = chat_bot(cx).unwrap().id;
            panel.view = choose_pane(&panel.view, &bot, Pane::Browser);
            panel.set_open(true, Trigger::Keyboard, cx);
        })
    });
    draw(cx);
    draw(cx);
}

fn set_remote(view: BrowserView, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        cx.global_mut::<BrowserData>()
            .remote
            .insert("b1".into(), view);
    });
    draw(cx);
}

fn click(id: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is not on screen"));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

fn moved_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| {
        clock::frames(cx)
            .iter()
            .filter(|frame| frame.moving)
            .count()
    })
}

#[gpui::test]
fn keyboard_toggle_snaps(cx: &mut gpui::TestAppContext) {
    let (panel, _, cx) = panel_window(cx, bot("bot-1", true), None);
    assert!(
        cx.debug_bounds("browser.panel").is_none(),
        "closed at the start"
    );
    let before = moved_frames(cx);

    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    let (open, width, open_width, moving) = cx.update(|_, cx| {
        let panel = panel.read(cx);
        (
            panel.is_open(),
            panel.width(),
            panel.motion.open_width(),
            panel.motion.is_moving(),
        )
    });
    assert!(open && !moving, "Ctrl+B opens the panel at once");
    assert_eq!(width, open_width);
    assert!(cx.debug_bounds("browser.panel").is_some());

    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    assert!(cx.update(|_, cx| !panel.read(cx).is_open() && panel.read(cx).width() == 0.));
    assert!(
        cx.debug_bounds("browser.panel").is_none(),
        "and closes it at once"
    );

    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    assert!(cx.debug_bounds("browser.panel").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(
        cx.debug_bounds("browser.panel").is_none(),
        "Escape closes it too"
    );
    assert_eq!(moved_frames(cx), before, "0 frames in between");
}

#[gpui::test]
fn the_pointer_moves_the_live_variant_on_the_spring_and_holds_live_until_rest(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, _, cx) = panel_window(cx, bot("bot-1", true), None);
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.set_open(true, Trigger::Pointer, cx)));
    assert_eq!(
        cx.update(|_, cx| panel.read(cx).motion.variant()),
        PanelVariant::Live
    );
    assert!(
        cx.update(|_, cx| Live::any(cx)),
        "a live motion keeps the display's rate"
    );
    let mut frames = 0;
    let mut widths = Vec::new();
    while cx.update(|_, cx| panel.read(cx).motion.is_moving()) {
        std::thread::sleep(std::time::Duration::from_millis(16));
        draw(cx);
        widths.push(cx.update(|_, cx| panel.read(cx).width()));
        frames += 1;
        assert!(frames < 200, "the panel never came to rest");
    }
    assert!(frames > 2, "it moved over several frames: {widths:?}");
    assert!(
        widths.windows(2).all(|pair| pair[1] >= pair[0]),
        "it opens without turning back: {widths:?}"
    );
    assert!(
        !cx.update(|_, cx| Live::any(cx)),
        "at rest it lets go of Live"
    );
}

#[gpui::test]
fn opens_only_beside_a_chat(cx: &mut gpui::TestAppContext) {
    let (panel, _, cx) = panel_window(cx, bot("bot-1", true), None);
    cx.update(|_, cx| cx.navigate(Route::Agents));
    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    assert!(!cx.update(|_, cx| panel.read(cx).is_open()));
    assert!(cx.debug_bounds("browser.panel").is_none());
}

// right-panel.test.tsx: "has no switch, and never the browser, for a bot without one"
#[gpui::test]
fn has_no_switch_and_never_the_browser_for_a_bot_without_one(cx: &mut gpui::TestAppContext) {
    let (panel, _, cx) = panel_window(cx, bot("bot-1", false), None);
    open_browser(&panel, cx);
    assert!(cx.debug_bounds("browser.panel").is_some());
    assert!(cx.debug_bounds("browser.panes").is_none());
    assert!(cx.debug_bounds("browser.address").is_none(), "Details only");
    assert!(cx.debug_bounds("browser.details.name").is_some());
}

// right-panel.test.tsx: "always offers Details and Browser, Hide panel ..."; bot-panel.test.tsx: the Computer card opens it.
#[gpui::test]
fn offers_details_and_browser_hide_and_settings(cx: &mut gpui::TestAppContext) {
    let (panel, _, cx) = panel_window(cx, bot("bot-1", true), None);
    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    for id in [
        "browser.pane.details",
        "browser.pane.browser",
        "browser.hide",
        "browser.settings",
        "browser.open",
    ] {
        assert!(cx.debug_bounds(id).is_some(), "{id}");
    }
    assert!(
        cx.debug_bounds("browser.expand").is_none(),
        "Expand is the browser's"
    );
    click("browser.open", cx);
    draw(cx);
    assert_eq!(cx.update(|_, cx| panel.read(cx).view.pane), Pane::Browser);
    assert!(cx.debug_bounds("browser.expand").is_some());
    click("browser.hide", cx);
    draw(cx);
    assert!(!cx.update(|_, cx| panel.read(cx).is_open()));
}

// browser-panel.test.tsx: "labels its parts and offers Take control while the bot drives", "shows the bot's controls
// disabled".
#[gpui::test]
fn labels_its_parts_and_offers_take_control_while_the_bot_drives(cx: &mut gpui::TestAppContext) {
    let (panel, sent, cx) =
        panel_window(cx, bot("b1", true), Some(shown(state(BrowserControl::Bot))));
    open_browser(&panel, cx);
    for id in [
        "browser.expand",
        "browser.address",
        "browser.viewport",
        "browser.tab.p1",
        "browser.back",
        "browser.forward",
        "browser.reload",
        "browser.logins",
        "browser.control",
        "browser.who",
    ] {
        assert!(cx.debug_bounds(id).is_some(), "{id}");
    }
    click("browser.reload", cx);
    assert!(sent.borrow().is_empty(), "the bot's controls are the bot's");
    click("browser.control", cx);
    assert_eq!(
        *sent.borrow(),
        [BrowserCommand::SetControl(BrowserControl::Human)]
    );
}

// browser-panel.test.tsx: "offers Resume bot while the user drives, and enables the controls"
#[gpui::test]
fn enables_the_controls_while_the_user_drives(cx: &mut gpui::TestAppContext) {
    let (panel, sent, cx) = panel_window(
        cx,
        bot("b1", true),
        Some(shown(state(BrowserControl::Human))),
    );
    open_browser(&panel, cx);
    click("browser.reload", cx);
    click("browser.back", cx);
    click("browser.control", cx);
    assert_eq!(
        *sent.borrow(),
        [
            BrowserCommand::History(HistoryAction::Reload),
            BrowserCommand::History(HistoryAction::Back),
            BrowserCommand::SetControl(BrowserControl::Bot),
        ]
    );
}

// browser-panel.test.tsx: "keeps another device's controls disabled while allowing viewing"
#[gpui::test]
fn keeps_another_devices_controls_disabled_while_allowing_viewing(cx: &mut gpui::TestAppContext) {
    let mut held = state(BrowserControl::Human);
    held.can_control = Some(false);
    let (panel, sent, cx) = panel_window(cx, bot("b1", true), Some(shown(held)));
    open_browser(&panel, cx);
    click("browser.back", cx);
    assert!(sent.borrow().is_empty());
    assert!(cx.debug_bounds("browser.viewport").is_some());
}

// browser-panel.test.tsx: "says Google may block sign-in only on Google's rejected page"
#[gpui::test]
fn says_google_may_block_sign_in_only_on_googles_rejected_page(cx: &mut gpui::TestAppContext) {
    let mut rejected = state(BrowserControl::Bot);
    if let Some(tab) = rejected.tabs.first_mut() {
        tab.url = "https://accounts.google.com/v3/signin/rejected?x=1".into();
    }
    let (panel, _, cx) = panel_window(cx, bot("b1", true), Some(shown(rejected)));
    open_browser(&panel, cx);
    assert!(cx.debug_bounds("browser.google").is_some());
    set_remote(shown(state(BrowserControl::Bot)), cx);
    assert!(cx.debug_bounds("browser.google").is_none());
}

// browser-panel.test.tsx: "shows a page dialog, answerable only by the user"
#[gpui::test]
fn shows_a_page_dialog_answerable_only_by_the_user(cx: &mut gpui::TestAppContext) {
    let dialog = BrowserDialog {
        page_id: "p1".into(),
        kind: "confirm".into(),
        message: "Delete it?".into(),
        default_value: String::new(),
    };
    let mut watching = state(BrowserControl::Bot);
    watching.dialog = Some(dialog.clone());
    let (panel, sent, cx) = panel_window(cx, bot("b1", true), Some(shown(watching)));
    open_browser(&panel, cx);
    assert!(cx.debug_bounds("browser.dialog").is_some());
    assert!(
        cx.debug_bounds("browser.dialog.take").is_some(),
        "Take control to answer."
    );
    assert!(cx.debug_bounds("browser.dialog.ok").is_none());
    let mut mine = state(BrowserControl::Human);
    mine.dialog = Some(dialog);
    set_remote(shown(mine), cx);
    assert!(cx.debug_bounds("browser.dialog.cancel").is_some());
    click("browser.dialog.ok", cx);
    assert_eq!(
        *sent.borrow(),
        [BrowserCommand::Dialog {
            accept: true,
            prompt_text: None
        }]
    );
}

// browser-panel.test.tsx: "says why it can't open, with a way to try again"
#[gpui::test]
fn says_why_it_cant_open_with_a_way_to_try_again(cx: &mut gpui::TestAppContext) {
    let view = BrowserView {
        error: Some("This agent's browser is off".into()),
        ..BrowserView::default()
    };
    let (panel, sent, cx) = panel_window(cx, bot("b1", true), Some(view));
    open_browser(&panel, cx);
    assert!(cx.debug_bounds("browser.error").is_some());
    click("browser.retry", cx);
    assert_eq!(*sent.borrow(), [BrowserCommand::Retry]);
}

// browser-panel.test.tsx: "suppresses private pixels even if an unexpected frame reaches the pane"
#[gpui::test]
fn suppresses_private_pixels_and_the_control_pill(cx: &mut gpui::TestAppContext) {
    let mut private = state(BrowserControl::Human);
    private.private_active = Some(true);
    let frame = BrowserFrame {
        data: "AAAA".into(),
        mime: FrameMime::Jpeg,
        width: 800,
        height: 600,
        sequence: 1,
        viewport_width: None,
        viewport_height: None,
        page_id: None,
        viewport_revision: None,
        stream_id: None,
        control_epoch: None,
        private_epoch: None,
    };
    let view = BrowserView {
        state: Some(private),
        frame: Some(frame),
        ..BrowserView::default()
    };
    let (panel, _, cx) = panel_window(cx, bot("b1", true), Some(view));
    open_browser(&panel, cx);
    assert!(cx.debug_bounds("browser.control").is_none());
    assert!(
        cx.update(|_, cx| panel.read(cx).picture.frame().is_none()),
        "no picture while private"
    );
}

// browser-panel.test.tsx: "explains an offline remote host without denying the remote Computer pane"
#[gpui::test]
fn explains_an_offline_remote_host(cx: &mut gpui::TestAppContext) {
    let (panel, _, cx) = panel_window(cx, bot("b1", true), Some(BrowserView::default()));
    cx.update(|_, cx| cx.global_mut::<BrowserData>().remote.clear());
    open_browser(&panel, cx);
    assert!(cx.debug_bounds("browser.error").is_some());
    assert!(cx.debug_bounds("browser.viewport").is_some());
}

#[test]
fn words_for_who_is_in_control() {
    let mut state = state(BrowserControl::Bot);
    assert_eq!(chrome::who(&state, "Scout"), "Scout is using this browser");
    state.control = BrowserControl::Human;
    assert_eq!(
        chrome::who(&state, "Scout"),
        "You're in control. Shift+Tab leaves the page."
    );
    state.can_control = Some(false);
    assert_eq!(
        chrome::who(&state, "Scout"),
        "Another device is controlling this browser. You can watch."
    );
    state.private_active = Some(true);
    assert_eq!(
        chrome::who(&state, "Scout"),
        "Private interaction. Automation and pictures are paused."
    );
}
