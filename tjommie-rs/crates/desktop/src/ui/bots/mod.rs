//! Agents: New agent and Agent settings (Phase 3 plan T5.1; `app/bots/new`, `app/bots/settings`, `bot-form/*`). Both
//! pages share the frame here: a header with Back and the agent's chip, the form with an aside beside it, and the
//! action bar.

pub mod form;
pub mod new;
pub mod rows;
pub mod settings;

use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::ui::fields::Link;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// The page: the header, the form and its aside (the aside moves under the form when they don't fit side by side),
/// and the action bar.
fn page(
    header: AnyElement,
    form: AnyElement,
    aside: AnyElement,
    footer: AnyElement,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(header)
        .child(
            div()
                .id("bot_form.scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_between()
                        .gap(r(theme.space.s40))
                        .pb(r(theme.space.s24))
                        .px(r(theme.space.s40))
                        .pt(r(theme.space.s8))
                        .child(div().w(r(640.)).min_w(r(532.)).child(form))
                        .child(div().w(r(340.)).flex_none().child(aside)),
                ),
        )
        .child(footer)
        .into_any_element()
}

/// The header: Back (to Agents) and the chip.
fn header(back: &'static str, chip: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let theme = theme(cx);
    div()
        .h(r(56.))
        .flex_none()
        .flex()
        .items_center()
        .gap(r(theme.space.s16))
        .px(r(theme.space.s40))
        .pt(r(theme.space.s8))
        .child(
            Link::new(back, "Back")
                .quiet()
                .icon("arrow-left")
                .on_press(|_, cx| cx.navigate(Route::Agents)),
        )
        .child(div().flex_1().flex().justify_center().child(chip))
        .into_any_element()
}

/// The agent's chip: its avatar, its name (the page's heading) and, for an agent that exists, its state.
pub(crate) fn chip(avatar: AnyElement, name: AnyElement, status: Option<String>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .h(r(40.))
        .min_w_0()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .px(r(theme.space.s14))
        .rounded_full()
        .bg(hsla(colors.surface.sunk))
        .type_style(&theme.typography.desktop.ui_strong)
        .text_color(hsla(colors.text.primary))
        .child(avatar)
        .child(name)
        .children(status.map(|status| {
            div()
                .flex_none()
                .type_style(&theme.typography.desktop.meta)
                .text_color(hsla(colors.text.tertiary))
                .child(status)
        }))
        .into_any_element()
}

/// The action bar: a problem at the left, the actions at the right.
fn footer(problem: Option<AnyElement>, actions: Vec<AnyElement>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .h(r(64.))
        .flex_none()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .px(r(theme.space.s24))
        .border_t_1()
        .border_color(hsla(palette(cx).line.base))
        .child(div().flex_1().min_w_0().children(problem))
        .children(actions)
        .into_any_element()
}

#[cfg(test)]
mod tests;
