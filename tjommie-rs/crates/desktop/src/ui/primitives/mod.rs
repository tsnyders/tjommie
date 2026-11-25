//! The Aegis primitives (Phase 3 plan T2.2). Their handlers (`on_press`, `on_toggle`, a menu row's) are guarded
//! inside, so screens pass plain closures. Every overlay (dialog, sheet, menu) raises the browser shield while open.

pub mod avatar;
pub mod button;
pub mod dialog;
pub mod menu;
pub mod scroll;
pub mod settings_row;
pub mod sheet;
pub mod shield;
pub mod toast;
pub mod toggle;

use std::collections::HashMap;

use gpui::{App, FocusHandle, Global, SharedString, WeakFocusHandle, Window};

/// The controls' focus handles by id, as last drawn, so a control can be focused from outside it (`focus_by_id`): a
/// component's own keyed state lives in a namespace of its type, out of reach of the screen that draws it.
/// ponytail: an id drawn once stays in the map (a weak handle, so it holds nothing); bounded by the ids ever drawn.
#[derive(Default)]
struct Drawn(HashMap<SharedString, WeakFocusHandle>);

impl Global for Drawn {}

/// A control's focus handle, kept across renders under its id, and a tab stop.
pub(crate) fn focus_handle(id: &str, window: &mut Window, cx: &mut App) -> FocusHandle {
    let key = SharedString::from(format!("{id}.focus"));
    let handle = window
        .use_keyed_state(key, cx, |_, cx| cx.focus_handle().tab_stop(true))
        .read(cx)
        .clone();
    cx.default_global::<Drawn>()
        .0
        .insert(SharedString::from(id.to_owned()), handle.downgrade());
    handle
}

/// Keeps `handle`, a control's own (a text field's, made by gpui-kit), under `id` for [`focus_by_id`].
pub(crate) fn remember_focus(id: &str, handle: &FocusHandle, cx: &mut App) {
    cx.default_global::<Drawn>()
        .0
        .insert(SharedString::from(id.to_owned()), handle.downgrade());
}

/// Focuses the control last drawn under `id` (a link's wish, such as Accounts' Sign in); false if it isn't there.
pub(crate) fn focus_by_id(id: &str, window: &mut Window, cx: &mut App) -> bool {
    let handle = cx
        .try_global::<Drawn>()
        .and_then(|drawn| drawn.0.get(id))
        .and_then(WeakFocusHandle::upgrade);
    match handle {
        Some(handle) => {
            window.focus(&handle, cx);
            true
        }
        None => false,
    }
}

/// A window with gpui-kit's root and the Aegis theme, showing what `content` draws. Nothing has the focus.
#[cfg(test)]
pub(crate) fn test_window(
    cx: &mut gpui::TestAppContext,
    content: impl Fn(&mut Window, &mut App) -> gpui::AnyElement + 'static,
) -> &mut gpui::VisualTestContext {
    window_with(cx, content, false)
}

/// `test_window` whose root holds the focus, as the shell's does, so keys reach the keymap from the start (Tab, the
/// arrows).
#[cfg(test)]
pub(crate) fn keyboard_window(
    cx: &mut gpui::TestAppContext,
    content: impl Fn(&mut Window, &mut App) -> gpui::AnyElement + 'static,
) -> &mut gpui::VisualTestContext {
    window_with(cx, content, true)
}

#[cfg(test)]
fn window_with(
    cx: &mut gpui::TestAppContext,
    content: impl Fn(&mut Window, &mut App) -> gpui::AnyElement + 'static,
    focused: bool,
) -> &mut gpui::VisualTestContext {
    use gpui::AppContext as _;
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(crate::keymap::field_tab(cx));
        cx.bind_keys(crate::keymap::bindings());
        cx.set_global(crate::theme::Theme::aegis().unwrap());
        crate::motion::clock::fixed_step(1. / 60., cx);
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let focus = cx.focus_handle();
            if focused {
                focus.focus(window, cx);
            }
            TestHost(Box::new(content), focus)
        });
        gpui_kit::component::Root::new(view, window, cx)
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx
}

#[cfg(test)]
struct TestHost(Content, gpui::FocusHandle);

#[cfg(test)]
type Content = Box<dyn Fn(&mut Window, &mut App) -> gpui::AnyElement>;

#[cfg(test)]
impl gpui::Render for TestHost {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement as _, ParentElement as _, Styled as _};
        // Each frame on the motion clock, and a fresh semantic tree, as the shell's are.
        crate::motion::clock::begin_frame(cx);
        crate::test_driver::begin_frame();
        gpui::div()
            .track_focus(&self.1)
            .size_full()
            .child((self.0)(window, cx))
            .child(crate::motion::clock::painted())
    }
}
