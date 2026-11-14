//! Toasts (navigation-shell `.toasts`, overlays `.toast`): the `Notices` global's notices, as near-black pills in a
//! column at the top centre, under the header. Each closes from its own button.

use gpui::{App, IntoElement, ParentElement as _, Styled as _, div};

use super::button::{IconButton, IconVariant};
use crate::notices::{self, Notices};
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
            div()
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
