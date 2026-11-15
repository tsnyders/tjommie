//! A room's members panel (`members-panel.tsx`; WF5): its agents with their state, each opening its chat; Edit members
//! (not in Everyone, which always holds every agent); and the chain limit with Change, which opens Settings at Agents.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use protocol::model::{Bot, Room, Settings};

use crate::semantic::SemanticExt as _;
use crate::ui::agents::cards::state_pill;
use crate::ui::fields::Link;
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{IconButton, pressable};
use crate::ui::primitives::focus_handle;
use crate::ui::shell::route::{Navigate as _, Route, Section};
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

/// "Agents can ask each other up to 3 times per chain."
pub fn chain_line(limit: u32) -> String {
    let times = if limit == 1 { "time" } else { "times" };
    format!("Agents can ask each other up to {limit} {times} per chain.")
}

/// The panel. `on_edit` opens the room form, `on_hide` closes the panel.
pub fn panel(
    room: &Room,
    members: &[&Bot],
    settings: Option<&Settings>,
    on_edit: impl Fn(&mut Window, &mut App) + 'static,
    on_hide: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let rows: Vec<AnyElement> = members
        .iter()
        .map(|bot| {
            let id: SharedString = format!("room.members.{}", bot.id).into();
            let focus = focus_handle(&id, window, cx);
            let ring = focus_visible(&focus, window);
            let theme = theme(cx);
            let colors = palette(cx);
            let chat = Route::Chat(bot.id.clone());
            let row = div()
                .semantic(id, Role::Link)
                .aria_label(bot.name.clone())
                .track_focus(&focus)
                .relative()
                .h(r(44.))
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .px(r(theme.space.s8))
                .rounded(r(theme.radius.field))
                .cursor_pointer()
                .hover(|row| row.bg(hsla(colors.state.hover)))
                .child(BotAvatar::new(bot.color, bot.shape, 24.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .type_style(&theme.typography.desktop.ui)
                        .text_color(hsla(colors.text.primary))
                        .child(bot.name.clone()),
                )
                .child(state_pill(bot, cx))
                .when(ring, |row| row.child(focus_ring(theme.radius.field, cx)));
            pressable(
                row,
                Some(Rc::new(move |_: &mut Window, cx: &mut App| {
                    cx.navigate(chat.clone())
                })),
            )
            .into_any_element()
        })
        .collect();
    let bar = div()
        .h(r(56.))
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .px(r(theme(cx).space.s16))
        .child(
            Link::new("room.members.settings", "Agents settings")
                .quiet()
                .icon("settings")
                .on_press(|_, cx| cx.navigate(Route::Settings(Section::Bots))),
        )
        .child(IconButton::new("room.members.hide", "x", "Hide the members").on_press(on_hide));
    let edit = (!room.everyone)
        .then(|| Link::new("room.members.edit", "Edit members").on_press(on_edit));
    let chain = settings.map(|settings| {
        let theme = theme(cx);
        div()
            .mt(r(theme.space.s24))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(r(theme.space.s8))
            .type_style(&theme.typography.desktop.small)
            .text_color(hsla(palette(cx).text.secondary))
            .child(
                svg()
                    .path("icons/i/175/info.svg")
                    .size(r(14.))
                    .flex_none()
                    .text_color(hsla(palette(cx).icon.base)),
            )
            .child(
                div()
                    .semantic("room.members.chain", Role::Paragraph)
                    .aria_label(chain_line(settings.chain_limit))
                    .child(chain_line(settings.chain_limit)),
            )
            .child(
                Link::new("room.members.change", "Change")
                    .on_press(|_, cx| cx.navigate(Route::Settings(Section::Bots))),
            )
    });
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic("room.members", Role::Complementary)
        .aria_label(format!("{} members", room.name))
        .w(r(320.))
        .flex_none()
        .flex()
        .flex_col()
        .border_l_1()
        .border_color(hsla(colors.line.base))
        .child(bar)
        .child(
            div()
                .id("room.members.scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(r(theme.space.s16))
                .pb(r(theme.space.s16))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(r(theme.space.s12))
                        .mb(r(theme.space.s6))
                        .child(
                            div()
                                .type_style(&theme.typography.desktop.label)
                                .text_color(hsla(colors.text.tertiary))
                                .child("Members"),
                        )
                        .children(edit),
                )
                .child(div().flex().flex_col().children(rows))
                .children(chain),
        )
        .into_any_element()
}
