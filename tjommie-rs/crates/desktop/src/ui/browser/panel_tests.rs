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
        // The semantic tree is this frame's, as the app's shell keeps it.
        crate::test_driver::begin_frame();
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
            // The person's commands; the panel's own Show and Hide are the remote tests' business.
            data.on_remote = Some(Rc::new(move |_, command, _, _| {
                if !matches!(command, BrowserCommand::Show | BrowserCommand::Hide) {
                    record.borrow_mut().push(command)
                }
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
fn a_key_mid_motion_lets_go_of_live(cx: &mut gpui::TestAppContext) {
    let (panel, _, cx) = panel_window(cx, bot("bot-1", true), None);
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.set_open(true, Trigger::Pointer, cx)));
    std::thread::sleep(std::time::Duration::from_millis(16));
    draw(cx);
    assert!(
        cx.update(|_, cx| Live::any(cx)),
        "the live opening holds Live"
    );
    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    assert!(
        !cx.update(|_, cx| panel.read(cx).motion.is_moving()),
        "the key snapped it shut"
    );
    assert!(
        !cx.update(|_, cx| Live::any(cx)),
        "and nothing live is moving, so an inactive window may throttle again"
    );
}

// The throttle rule (gate condition 5): a remote live stream showing in the panel keeps the display's rate.
#[gpui::test]
fn a_remote_stream_showing_holds_live(cx: &mut gpui::TestAppContext) {
    let mut view = shown(state(BrowserControl::Bot));
    view.frame = Some(jpeg_frame(1));
    let (panel, _, cx) = panel_window(cx, bot("b1", true), Some(view));
    assert!(!cx.update(|_, cx| Live::any(cx)), "closed: nothing live");
    open_browser(&panel, cx);
    assert!(cx.update(|_, cx| Live::any(cx)), "the stream shows");
    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    assert!(!cx.update(|_, cx| Live::any(cx)), "closed again");
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

// bot-panel.tsx `Summary`: the model, the state (when not idle), the autonomy, and the folders, from the bot.
#[gpui::test]
fn the_details_summary_says_the_bots_model_state_autonomy_and_folders(
    cx: &mut gpui::TestAppContext,
) {
    let mut working = bot("bot-1", true);
    working.state = BotState::Working;
    working.project_folder = Some("D:/work/site".into());
    let (_, _, cx) = panel_window(cx, working, None);
    cx.simulate_keystrokes("ctrl-b");
    draw(cx);
    for id in [
        "browser.details.model",
        "browser.details.state",
        "browser.details.autonomy",
        "browser.details.home",
        "browser.details.project",
    ] {
        assert!(cx.debug_bounds(id).is_some(), "{id}");
    }
    cx.update(|_, cx| {
        if let Some(bot) = cx.global_mut::<BrowserData>().bots.first_mut() {
            bot.state = BotState::Idle;
            bot.project_folder = None;
        }
    });
    draw(cx);
    assert!(
        cx.debug_bounds("browser.details.state").is_none(),
        "an idle bot shows no state"
    );
    assert!(cx.debug_bounds("browser.details.project").is_none());
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

// Escape in the page's dialog (its answer field, its buttons) cancels the dialog and keeps the panel, as
// `right-panel.tsx` kept Escape for an alertdialog (review F7). While the bot holds the page, the person can't answer:
// Escape there is the panel's, as everywhere else (T2.4).
#[gpui::test]
fn escape_in_a_page_dialog_cancels_it_and_keeps_the_panel(cx: &mut gpui::TestAppContext) {
    let dialog = BrowserDialog {
        page_id: "p1".into(),
        kind: "prompt".into(),
        message: "Your name?".into(),
        default_value: String::new(),
    };
    let mut mine = state(BrowserControl::Human);
    mine.dialog = Some(dialog.clone());
    let (panel, sent, cx) = panel_window(cx, bot("b1", true), Some(shown(mine)));
    open_browser(&panel, cx);
    let answer = cx.update(|_, cx| panel.read(cx).prompt.clone());
    let type_in_the_answer = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| answer.update(cx, |input, cx| input.focus(window, cx)));
        draw(cx);
    };
    type_in_the_answer(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(
        *sent.borrow(),
        [BrowserCommand::Dialog {
            accept: false,
            prompt_text: None
        }],
        "the dialog is cancelled"
    );
    assert!(
        cx.update(|_, cx| panel.read(cx).is_open()),
        "the panel stays"
    );

    let mut watching = state(BrowserControl::Bot);
    watching.dialog = Some(dialog);
    set_remote(shown(watching), cx);
    type_in_the_answer(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(sent.borrow().len(), 1, "nothing more sent");
    assert!(
        !cx.update(|_, cx| panel.read(cx).is_open()),
        "the panel closed"
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

// browser-panel.test.tsx: "suppresses private pixels even if an unexpected frame reaches the pane". The frame is a real
// picture that decodes and draws, so only the private filter keeps it off (review F2); the same frame shows once the
// interaction is over.
#[gpui::test]
fn suppresses_private_pixels_and_the_control_pill(cx: &mut gpui::TestAppContext) {
    let mut private = state(BrowserControl::Human);
    private.private_active = Some(true);
    let mut view = shown(private);
    view.frame = Some(jpeg_frame(1));
    let (panel, sent, cx) = panel_window(cx, bot("b1", true), Some(view.clone()));
    open_browser(&panel, cx);
    // As long as a drawn picture would take to be reported (bounded: it never is).
    draw_until_drawn(&sent, 0, cx);
    assert!(cx.debug_bounds("browser.control").is_none());
    assert!(
        cx.update(|_, cx| panel.read(cx).picture.frame().is_none()),
        "no picture while private"
    );
    assert!(drawn(&sent).is_empty(), "and none reported drawn");

    // The control: private off, the same frame is drawn and reported.
    if let Some(state) = view.state.as_mut() {
        state.private_active = None;
    }
    set_remote(view, cx);
    draw_until_drawn(&sent, 0, cx);
    assert_eq!(
        cx.update(|_, cx| panel.read(cx).picture.frame().map(|frame| frame.sequence)),
        Some(1),
        "the picture shows"
    );
    assert_eq!(drawn(&sent), [1]);
}

/// A 4x3 JPEG, base64 as a frame carries it.
const JPEG: &str = "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAADAAQDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDxSiiivzc/tU//2Q==";

fn jpeg_frame(sequence: u64) -> BrowserFrame {
    BrowserFrame {
        data: JPEG.into(),
        mime: FrameMime::Jpeg,
        width: 4,
        height: 3,
        sequence,
        viewport_width: None,
        viewport_height: None,
        page_id: Some("p1".into()),
        viewport_revision: None,
        stream_id: Some("s1".into()),
        control_epoch: None,
        private_epoch: None,
    }
}

fn drawn(sent: &Sent) -> Vec<u64> {
    sent.borrow()
        .iter()
        .filter_map(|command| match command {
            BrowserCommand::Drawn(frame) => Some(frame.sequence),
            _ => None,
        })
        .collect()
}

/// Draws until a frame has been reported drawn after `before`, while GPUI decodes the picture.
fn draw_until_drawn(sent: &Sent, before: usize, cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        draw(cx);
        if drawn(sent).len() > before {
            return;
        }
    }
}

// The remote live view (browser.streamFrame): each frame is reported drawn once it is on screen, once, in order, so the
// session acknowledges it (browser.streamAck) and the stream sends the next. These counts would also pass for a report
// sent when the frame arrives: that it follows the paint is by construction (`stream.rs`, the picture's paint defers the
// report), not by this test (review F12).
#[gpui::test]
fn reports_each_streamed_frame_once_it_is_drawn(cx: &mut gpui::TestAppContext) {
    let mut view = shown(state(BrowserControl::Bot));
    view.frame = Some(jpeg_frame(1));
    let (panel, sent, cx) = panel_window(cx, bot("b1", true), Some(view.clone()));
    open_browser(&panel, cx);
    draw_until_drawn(&sent, 0, cx);
    assert_eq!(drawn(&sent), [1]);
    assert_eq!(
        cx.update(|_, cx| panel.read(cx).picture.frame().map(|frame| frame.sequence)),
        Some(1)
    );
    draw(cx);
    draw(cx);
    assert_eq!(
        drawn(&sent),
        [1],
        "once per frame, however often the panel draws"
    );
    view.frame = Some(jpeg_frame(2));
    set_remote(view, cx);
    draw_until_drawn(&sent, 1, cx);
    assert_eq!(drawn(&sent), [1, 2]);
}

/// That PC's daemon, as T1.4's test double: a bot's browser with one page and its picture, and the live stream when
/// `streams` (without it, `browser.stream` answers `unknown_method`, as an older daemon does).
fn remote_daemon(streams: bool) -> crate::model::testing::FakeRpc {
    use protocol::browser::BrowserFrameResult;
    use protocol::wire::{Empty, method};
    let page = state(BrowserControl::Bot);
    let (open, pulled, streamed) = (page.clone(), page.clone(), page);
    let rpc = crate::model::testing::FakeRpc::new()
        .on::<method::BrowserOpen>(move |_| Ok(open.clone()))
        .on::<method::BrowserFrame>(move |_| {
            Ok(BrowserFrameResult {
                state: pulled.clone(),
                frame: Some(jpeg_frame(1)),
                unchanged: None,
            })
        })
        .on::<method::BrowserStreamAck>(|_| Ok(Empty {}));
    if streams {
        rpc.on::<method::BrowserStream>(move |_| Ok(streamed.clone()))
    } else {
        rpc
    }
}

/// A window on another PC, its panel wired to that PC's daemon through `remote::wire`.
fn remote_window(
    cx: &mut gpui::TestAppContext,
    rpc: std::sync::Arc<crate::model::testing::FakeRpc>,
) -> (Entity<BrowserPanel>, &mut VisualTestContext) {
    let (panel, _, cx) = panel_window(cx, bot("b1", true), Some(BrowserView::default()));
    cx.update(|_, cx| {
        let data = cx.global_mut::<BrowserData>();
        data.remote.clear();
        data.on_remote = None;
        super::super::remote::wire(rpc, true, cx);
    });
    (panel, cx)
}

/// Lets the sessions' calls, timers and the picture's decoding run, drawing between them.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..10 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1));
        cx.run_until_parked();
        draw(cx);
    }
}

fn asked(rpc: &crate::model::testing::FakeRpc, name: &str) -> Vec<serde_json::Value> {
    rpc.calls()
        .into_iter()
        .filter(|(method, _)| *method == name)
        .map(|(_, params)| params)
        .collect()
}

// The remote live view (T3.7): T1.4's session opens when the Browser pane shows, asks for the stream after its first
// picture, and each streamed frame is acknowledged once it is drawn; closing the panel stops the stream.
#[gpui::test]
fn a_remote_stream_is_drawn_and_each_drawn_frame_acknowledged(cx: &mut gpui::TestAppContext) {
    let rpc = std::sync::Arc::new(remote_daemon(true));
    let (panel, cx) = remote_window(cx, rpc.clone());
    open_browser(&panel, cx);
    settle(cx);
    assert!(
        !asked(&rpc, "browser.open").is_empty(),
        "the session opened"
    );
    assert!(
        asked(&rpc, "browser.stream")
            .iter()
            .any(|params| params["on"] == true),
        "and asked for the stream after its first picture"
    );
    cx.update(|_, cx| {
        super::super::remote::stream_frame(
            protocol::browser::BrowserStreamFrame {
                bot_id: "b1".into(),
                frame: Some(jpeg_frame(7)),
            },
            cx,
        )
    });
    for _ in 0..5 {
        settle(cx);
    }
    assert_eq!(
        cx.update(|_, cx| panel.read(cx).picture.frame().map(|frame| frame.sequence)),
        Some(7),
        "the streamed frame is on screen"
    );
    let acks: Vec<_> = asked(&rpc, "browser.streamAck")
        .into_iter()
        .filter(|params| params["sequence"] == 7)
        .collect();
    assert_eq!(acks.len(), 1, "acknowledged once, after it was drawn");
    cx.simulate_keystrokes("ctrl-b");
    settle(cx);
    assert!(
        asked(&rpc, "browser.stream")
            .iter()
            .any(|params| params["on"] == false),
        "closing the panel stops the stream"
    );
}

// An older daemon has no browser.stream (unknown_method): the session falls back to polling browser.frame, and never
// asks for the stream again.
#[gpui::test]
fn an_older_daemon_without_the_stream_is_polled(cx: &mut gpui::TestAppContext) {
    let rpc = std::sync::Arc::new(remote_daemon(false));
    let (panel, cx) = remote_window(cx, rpc.clone());
    open_browser(&panel, cx);
    settle(cx);
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(300));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        settle(cx);
    }
    assert_eq!(
        asked(&rpc, "browser.stream").len(),
        1,
        "asked once, refused"
    );
    assert!(
        asked(&rpc, "browser.frame").len() >= 2,
        "pictures are polled instead: {:?}",
        rpc.calls()
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        cx.update(|_, cx| panel.read(cx).picture.frame().map(|frame| frame.sequence)),
        Some(1),
        "the polled picture is on screen"
    );
}

// Leaving the PC, or its connection ending, ends the sessions the panel opened there: the stream stops and the browsers go
// back to their bots, and the panel stops reading that PC's views and sending it commands (host wiring).
#[gpui::test]
fn leaving_a_remote_pc_ends_its_sessions(cx: &mut gpui::TestAppContext) {
    let rpc = std::sync::Arc::new(remote_daemon(true));
    let (panel, cx) = remote_window(cx, rpc.clone());
    open_browser(&panel, cx);
    settle(cx);
    assert!(
        asked(&rpc, "browser.stream")
            .iter()
            .any(|params| params["on"] == true),
        "the stream was on"
    );
    cx.update(|_, cx| super::super::remote::unwire(cx));
    settle(cx);
    assert!(
        asked(&rpc, "browser.stream")
            .iter()
            .any(|params| params["on"] == false),
        "leaving the PC stopped the stream"
    );
    cx.update(|_, cx| {
        let data = cx.global::<BrowserData>();
        assert!(data.on_remote.is_none(), "commands still go to the old PC");
        assert!(data.remote.is_empty(), "the old PC's views are still shown");
    });
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

/// An icon that isn't baked draws nothing: every icon the panel and the Logins windows name is in `design/icons`.
#[test]
fn every_icon_the_panel_draws_is_baked() {
    let sources = [
        include_str!("chrome.rs"),
        include_str!("panel.rs"),
        include_str!("logins.rs"),
        include_str!("../vault_entry.rs"),
    ];
    let named = regex::Regex::new(
        r#"icons/i/175/([a-z0-9-]+)\.svg|IconButton::new\(\s*"[^"]+",\s*"([a-z0-9-]+)"|\.icon\("([a-z0-9-]+)"\)"#,
    )
    .unwrap();
    let icons: Vec<String> = sources
        .iter()
        .flat_map(|source| named.captures_iter(source))
        .filter_map(|found| {
            found
                .iter()
                .skip(1)
                .flatten()
                .next()
                .map(|name| name.as_str().to_owned())
        })
        .collect();
    assert!(icons.len() >= 10, "the panel's icons were found: {icons:?}");
    for icon in icons {
        let path = format!("icons/i/175/{icon}.svg");
        assert!(crate::assets::icon(&path).is_some(), "{path} is not baked");
    }
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

/// The dialog fit (every dialog's check): Saved logins, its Manage logins in the window at 1000 x 600 and at 360 x 600.
#[gpui::test]
fn dialog_fits_saved_logins(cx: &mut gpui::TestAppContext) {
    let (panel, _, cx) = panel_window(
        cx,
        bot("b1", true),
        Some(shown(state(BrowserControl::Human))),
    );
    open_browser(&panel, cx);
    click("browser.logins", cx);
    draw(cx);
    click("browser.logins.saved", cx);
    crate::ui::testing::draw(cx);
    crate::ui::testing::dialog_fits(cx, &["logins.manage"]);
}

/// The dialog fit for a page's own dialog: a long message scrolls in its bar, so Cancel and OK stay whole in a 600 high
/// window, not clipped by the panel. (Its width is the shell's to set; this host keeps the panel's own.)
#[gpui::test]
fn dialog_fits_a_page_dialog_with_a_long_message(cx: &mut gpui::TestAppContext) {
    let mut mine = state(BrowserControl::Human);
    mine.dialog = Some(BrowserDialog {
        page_id: "p1".into(),
        kind: "confirm".into(),
        message: "Leave this page? The changes you made on it are not saved yet. ".repeat(40),
        default_value: String::new(),
    });
    let (panel, _, cx) = panel_window(cx, bot("b1", true), Some(shown(mine)));
    open_browser(&panel, cx);
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(600.)));
    crate::ui::testing::draw(cx);
    for id in ["browser.dialog.cancel", "browser.dialog.ok"] {
        let control = crate::ui::testing::node(id).unwrap_or_else(|| panic!("{id} is drawn"));
        let bounds = control.bounds;
        assert!(
            bounds.y >= 0. && bounds.y + bounds.height <= 600.,
            "{id} sits inside the window: {bounds:?}"
        );
        assert_eq!(control.visible, Some(bounds), "{id} is whole, not clipped");
    }
}
