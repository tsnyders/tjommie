//! Toasts (navigation-shell `.toasts`, overlays `.toast`): the `Notices` global's notices, as near-black pills in a
//! column at the top centre, under the header. Each closes from its own button.

use gpui::{
    App, IntoElement, ParentElement as _, Role, StatefulInteractiveElement as _, Styled as _, div,
};

use super::button::{IconButton, IconVariant};
use crate::notices::{self, Notices};
use crate::semantic::SemanticExt as _;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// The toast layer, laid over the window; empty when nothing is raised.
pub fn toasts(cx: &App) -> impl IntoElement + use<> {
    let theme = theme(cx);
    let colors = palette(cx);
    let notices = cx
        .try_global::<Notices>()
        .map(|notices| notices.0.clone())
        .unwrap_or_default();
    div()
        .absolute()
        .top(r(theme.size.titlebar_height + theme.space.s12))
        .left_0()
        .right_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(r(theme.space.s8))
        .children(notices.into_iter().enumerate().map(|(index, notice)| {
            // A status, as the design's toast is, so Windows reads it out (M2 review F8).
            div()
                .semantic(format!("toast.{index}"), Role::Status)
                .aria_label(notice.text())
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .pl(r(theme.space.s16))
                .pr(r(theme.space.s4))
                .py(r(theme.space.s4))
                .rounded_full()
                .bg(hsla(colors.action.primary))
                .text_color(hsla(colors.on.primary))
                .shadow(shadows(&theme.shadow.light.raised))
                .type_style(&theme.typography.desktop.ui)
                .child(notice.text())
                .child(
                    IconButton::new(format!("toast.close.{index}"), "x", "Close")
                        .variant(IconVariant::OnDark)
                        .on_press(move |_, cx| notices::dismiss(index, cx)),
                )
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notices::Notice;
    use crate::ui::primitives::test_window;

    #[gpui::test]
    fn a_notice_is_a_status_toast(cx: &mut gpui::TestAppContext) {
        let cx = test_window(cx, |_, cx| toasts(cx).into_any_element());
        cx.update(|_, cx| cx.set_global(Notices(vec![Notice::SomethingWentWrong])));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("toast.0").is_some(),
            "found by its id, as UI Automation finds it"
        );
    }
}
