//! The Aegis primitives (Phase 3 plan T2.2). Their handlers (`on_press`, `on_toggle`, a menu row's) are guarded
//! inside, so screens pass plain closures. Every overlay (dialog, sheet, menu) raises the browser shield while open.

pub mod avatar;
pub mod button;
pub mod dialog;
pub mod menu;
pub mod settings_row;
pub mod sheet;
pub mod shield;
pub mod toast;
pub mod toggle;

use gpui::{App, FocusHandle, SharedString, Window};

/// A control's focus handle, kept across renders under its id, and a tab stop.
pub(crate) fn focus_handle(id: &str, window: &mut Window, cx: &mut App) -> FocusHandle {
    let key = SharedString::from(format!("{id}.focus"));
    window
        .use_keyed_state(key, cx, |_, cx| cx.focus_handle().tab_stop(true))
        .read(cx)
        .clone()
}

/// A window with gpui-kit's root and the Aegis theme, showing what `content` draws.
#[cfg(test)]
pub(crate) fn test_window(
    cx: &mut gpui::TestAppContext,
    content: impl Fn(&mut Window, &mut App) -> gpui::AnyElement + 'static,
) -> &mut gpui::VisualTestContext {
    use gpui::AppContext as _;
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::theme::Theme::aegis().unwrap());
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| TestHost(Box::new(content)));
        gpui_kit::component::Root::new(view, window, cx)
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx
}

#[cfg(test)]
struct TestHost(Content);

#[cfg(test)]
type Content = Box<dyn Fn(&mut Window, &mut App) -> gpui::AnyElement>;

#[cfg(test)]
impl gpui::Render for TestHost {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        gpui::div().size_full().child((self.0)(window, cx))
    }
}
