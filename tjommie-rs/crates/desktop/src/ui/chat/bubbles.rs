//! Messages (chat-messages.html `.you`, `.bubble`, `.prose`; `bubbles.tsx`). Yours sit in a stone bubble on the right,
//! exactly as typed, with the files they carry above it. An agent's reply is prose, not a bubble: Markdown, its words
//! fading in as they stream. A reply still waiting for its first word says "Thinking"; one that ended blank shows
//! nothing.

use gpui::{
    AnyElement, App, IntoElement, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, svg,
};
use protocol::model::{Message, MessageStatus};

use crate::model::attachments::file_name;
use crate::semantic::SemanticExt as _;
use crate::theme::Theme;
use crate::ui::files::markdown;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// Your bubble's corners, top left, top right, bottom right and bottom left: round but for the corner nearest you.
pub fn corners(theme: &Theme) -> [f32; 4] {
    let radius = &theme.radius;
    [radius.slot, radius.slot, radius.xs, radius.slot]
}

/// Your message, exactly as typed, under the names of the files it carries.
pub fn you(message: &Message, files: &[String], cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let [top_left, top_right, bottom_right, bottom_left] = corners(theme);
    let chips = files.iter().map(|path| {
        let name = SharedString::from(file_name(path).to_owned());
        div()
            .h(r(28.))
            .max_w(r(240.))
            .flex()
            .items_center()
            .gap(r(theme.space.s6))
            .px(r(theme.space.s10))
            .rounded_full()
            .bg(hsla(colors.surface.sunk))
            .type_style(&theme.typography.desktop.small)
            .text_color(hsla(colors.text.primary))
            .child(
                svg()
                    .path("icons/i/175/file-text.svg")
                    .size(r(15.))
                    .flex_none()
                    .text_color(hsla(colors.icon.base)),
            )
            .child(div().min_w_0().truncate().child(name))
    });
    div()
        .max_w(gpui::relative(0.9))
        .flex()
        .flex_col()
        .items_end()
        .gap(r(theme.space.s4))
        .when_children(files, |column| {
            column.child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(r(theme.space.s6))
                    .children(chips),
            )
        })
        .child(
            div()
                .semantic(format!("bubble.{}", message.id), Role::Paragraph)
                .aria_label(SharedString::from(message.body.clone()))
                .bg(hsla(colors.surface.sunk))
                .rounded_tl(r(top_left))
                .rounded_tr(r(top_right))
                .rounded_br(r(bottom_right))
                .rounded_bl(r(bottom_left))
                .px(r(theme.space.s16))
                .py(r(theme.space.s8))
                .type_style(&theme.typography.desktop.body)
                .text_color(hsla(colors.text.primary))
                .child(SharedString::from(message.body.clone())),
        )
        .into_any_element()
}

/// An agent's reply: Markdown prose, or "Thinking" until its first word.
pub fn bot(message: &Message, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let streaming = message.status == MessageStatus::Streaming;
    if message.body.trim().is_empty() {
        if !streaming {
            return div().into_any_element();
        }
        return div()
            .semantic(format!("thinking.{}", message.id), Role::Status)
            .aria_label("Thinking")
            .type_style(&theme.typography.desktop.small)
            .text_color(hsla(colors.text.secondary))
            .child("Thinking")
            .into_any_element();
    }
    let text = SharedString::from(message.body.clone());
    div()
        .semantic(format!("prose.{}", message.id), Role::Article)
        .w_full()
        .max_w(r(600.))
        .child(markdown::reply(
            &format!("prose.{}.md", message.id),
            &text,
            window,
            cx,
        ))
        .into_any_element()
}

trait WhenChildren: Sized {
    fn when_children<T>(self, items: &[T], then: impl FnOnce(Self) -> Self) -> Self {
        if items.is_empty() { self } else { then(self) }
    }
}

impl<E: ParentElement> WhenChildren for E {}

use gpui::ParentElement;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_bubble_is_round_but_for_the_corner_nearest_you() {
        let theme = Theme::aegis().unwrap();
        assert_eq!(corners(&theme), [20., 20., 6., 20.]);
    }
}
