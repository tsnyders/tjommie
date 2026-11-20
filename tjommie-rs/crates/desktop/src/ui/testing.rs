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
        crate::ui::store::fake(cx, state, rpc)
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let shell = cx.new(|cx| Shell::new(window, cx));
        gpui_kit::component::Root::new(shell, window, cx)
    });
    draw(cx);
    (cx, calls)
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
