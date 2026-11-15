//! Projects (Phase 3 plan T5.3; `app/projects/page.tsx`, `projects/*`).

use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, Role, StatefulInteractiveElement as _,
    Styled as _, Window, div,
};

use crate::semantic::SemanticExt as _;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// The Projects screen.
pub fn screen(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let theme = theme(cx);
    div()
        .flex_1()
        .px(r(theme.space.s32))
        .pt(r(theme.space.s32))
        .child(
            div()
                .semantic("screen.projects", Role::Heading)
                .aria_label("Projects")
                .type_style(&theme.typography.desktop.display_l)
                .text_color(hsla(palette(cx).text.primary))
                .child("Projects"),
        )
        .into_any_element()
}
