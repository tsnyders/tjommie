//! The screens' view tests (Phase 3 plan M5): the app's shell on a route, over a fake store, read back through the
//! semantic tree the test driver serves (each `semantic()` element's id, role, text and focus, as UI Automation sees
//! them). Sample data is invented: no real names, paths or accounts.

use std::sync::Arc;

use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use protocol::model::{Autonomy, Bot, BotColor, BotShape, BotState, Provider, Room, Routine};

use crate::model::state::{AppState, Connection};
use crate::model::testing::FakeRpc;
use crate::test_driver::UiNode;
use crate::theme::Theme;
use crate::ui::shell::Shell;
use crate::ui::shell::data::{self, ShellData};
use crate::ui::shell::route::{Navigate as _, Route};

/// The app's window on `route`, connected, with `state` in the store and calls answered by `rpc`. Returns the window
/// and the handle that reads the calls made.
pub fn app(
    cx: &mut TestAppContext,
    route: Route,
    state: AppState,
    rpc: FakeRpc,
) -> (&mut VisualTestContext, Arc<FakeRpc>) {
    open(cx, route, state, rpc, false)
}

/// `app`, with the browser panel beside the main column, closed, as the app has it: the bots it reads, and its keys
/// (Ctrl+B, Escape).
pub fn app_with_panel(
    cx: &mut TestAppContext,
    route: Route,
    state: AppState,
    rpc: FakeRpc,
) -> (&mut VisualTestContext, Arc<FakeRpc>) {
    open(cx, route, state, rpc, true)
}

fn open(
    cx: &mut TestAppContext,
    route: Route,
    state: AppState,
    rpc: FakeRpc,
    with_panel: bool,
) -> (&mut VisualTestContext, Arc<FakeRpc>) {
    if with_panel {
        let bots = state.bots.clone();
        cx.update(|cx| {
            cx.set_global(crate::ui::browser::BrowserData {
                bots,
                ..Default::default()
            });
            crate::ui::browser::init(cx);
        });
    }
    let calls = cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(crate::keymap::field_tab(cx));
        cx.bind_keys(crate::keymap::bindings());
        cx.set_global(Theme::aegis().unwrap());
        crate::notices::init(cx);
        cx.set_global(ShellData {
            connection: data::Connection::Ready,
            ..ShellData::default()
        });
        cx.navigate(route);
        crate::motion::clock::fixed_step(1. / 60., cx);
        crate::ui::store::fake(cx, state, rpc)
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let panel = with_panel.then(|| crate::ui::browser::panel::BrowserPanel::create(window, cx));
        let shell = cx.new(|cx| {
            let mut shell = Shell::new(window, cx);
            shell.set_panel(panel.map(Into::into), cx);
            shell
        });
        gpui_kit::component::Root::new(shell, window, cx)
    });
    draw(cx);
    (cx, calls)
}

/// Presses Tab `times` times: each time the keyboard is on an element `inside` takes for the sheet's, and it moves between
/// more than one (a sheet over the content keeps Tab inside it: gpui-kit's focus trap).
pub fn tab_stays_in(cx: &mut VisualTestContext, times: usize, inside: impl Fn(&str) -> bool) {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..times {
        cx.simulate_keystrokes("tab");
        draw(cx);
        let focused: Vec<String> = crate::test_driver::nodes()
            .into_iter()
            .filter(|node| node.focused)
            .map(|node| node.id)
            .collect();
        assert!(
            !focused.is_empty() && focused.iter().all(|id| inside(id)),
            "Tab stays in the sheet: {focused:?}"
        );
        for id in focused {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
    }
    assert!(
        seen.len() > 1,
        "Tab moves between the sheet's controls: {seen:?}"
    );
}

/// Runs what's pending, then draws a frame, so the semantic tree is current.
pub fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The platform's next frame, only if the last one asked for it (`request_animation_frame`): a test then sees what the
/// app draws with no other event to prompt it. Whether a frame was asked for.
pub fn next_frame(cx: &mut VisualTestContext) -> bool {
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx)) > 0;
    if asked {
        draw(cx);
    }
    asked
}

/// The element with `id` in the last frame, as the test driver's snapshot has it.
pub fn node(id: &str) -> Option<UiNode> {
    crate::test_driver::nodes()
        .into_iter()
        .find(|node| node.id == id)
}

/// The text of the element with `id`; panics when it isn't on screen.
pub fn text(id: &str) -> String {
    node(id)
        .unwrap_or_else(|| panic!("{id} is not on screen"))
        .text
}

/// The ids on screen that start with `prefix`, in paint order.
pub fn ids(prefix: &str) -> Vec<String> {
    crate::test_driver::nodes()
        .into_iter()
        .map(|node| node.id)
        .filter(|id| id.starts_with(prefix))
        .collect()
}

/// Clicks the element with `id` and draws the next frame.
pub fn click(cx: &mut VisualTestContext, id: &'static str) {
    let bounds = cx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is not on screen"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    draw(cx);
}

/// The dialog fit, which every dialog's view tests run (the README pictures' routine editor ran its Save off a 1000 x
/// 700 window): with the dialog open, at 1000 x 600 and at 360 x 600 (the window's smallest), its title and each control
/// in `footer` (its actions) sit whole inside the window, not clipped by the body that scrolls between them. The window
/// then goes back to its size.
pub fn dialog_fits(cx: &mut VisualTestContext, footer: &[&str]) {
    let before = cx.update(|window, _| window.viewport_size());
    for (width, height) in [(1000., 600.), (360., 600.)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        draw(cx);
        for id in ["dialog.title"].iter().chain(footer) {
            let control = node(id).unwrap_or_else(|| panic!("{id} is drawn at {width} x {height}"));
            let bounds = control.bounds;
            assert!(
                bounds.x >= 0.
                    && bounds.y >= 0.
                    && bounds.x + bounds.width <= width
                    && bounds.y + bounds.height <= height,
                "{id} sits inside the {width} x {height} window: {bounds:?}"
            );
            assert_eq!(
                control.visible,
                Some(bounds),
                "{id} is whole at {width} x {height}, not clipped"
            );
        }
    }
    cx.simulate_resize(before);
    draw(cx);
}

/// A connected state with these bots, rooms and routines.
pub fn state(bots: Vec<Bot>, rooms: Vec<Room>, routines: Vec<Routine>) -> AppState {
    AppState {
        connection: Connection::Ready,
        bots,
        rooms,
        routines,
        ..AppState::default()
    }
}

/// A sample agent. Created in id order, so lists keep the order given.
pub fn bot(id: &str, name: &str, state: BotState) -> Bot {
    Bot {
        id: id.into(),
        name: name.into(),
        color: BotColor::Blue,
        shape: BotShape::Hexagon,
        instructions: String::new(),
        provider: Provider::Claude,
        model: "sonnet".into(),
        account_id: None,
        effective_account_id: None,
        using_fallback: None,
        autonomy: Autonomy::Ask,
        home_folder: format!("C:/Agents/{name}"),
        project_folder: None,
        browser: false,
        paused: state == BotState::Paused,
        can_message: Vec::new(),
        state,
        setup_reason: None,
        limited_until: None,
        created_at: format!("2026-11-01T09:00:0{}.000Z", id.len() % 10),
    }
}

/// A sample room with these agents.
pub fn room(id: &str, name: &str, bot_ids: &[&str]) -> Room {
    Room {
        id: id.into(),
        name: name.into(),
        bot_ids: bot_ids.iter().map(|id| (*id).to_owned()).collect(),
        everyone: false,
        created_at: Some("2026-11-02T09:00:00.000Z".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::primitives::button::Pill;
    use crate::ui::primitives::test_window;
    use gpui::IntoElement as _;

    #[gpui::test]
    fn the_semantic_tree_is_readable_in_view_tests(cx: &mut TestAppContext) {
        let cx = test_window(cx, |_, _| Pill::new("test.pill", "Save").into_any_element());
        draw(cx);
        let pill = node("test.pill").expect("the pill is in the tree");
        assert_eq!((pill.role.as_str(), pill.text.as_str()), ("Button", "Save"));
    }
}
