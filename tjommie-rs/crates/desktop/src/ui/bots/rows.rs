//! The bot form's settings rows (`bot-form/rows.tsx`; WF3): a row that opens in place, and the Folder, Autonomy,
//! Browser, Can message and Routines rows built on it. The room form uses Can message for a room's agents.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use protocol::model::{Autonomy, Bot, Provider};

use crate::model::bot_form::{AUTONOMY_LEVELS, autonomy_help};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Look, RadioGroup};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::pressable;
use crate::ui::primitives::dialog::open_confirm;
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::toggle::Toggle;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

/// Whether a row is open, kept while the form shows it.
fn open_state(id: &str, default: bool, window: &mut Window, cx: &mut App) -> gpui::Entity<bool> {
    window.use_keyed_state(SharedString::from(format!("{id}.open")), cx, move |_, _| {
        default
    })
}

/// A settings row (`Row`, `ROW_BUTTON`): 56 px on the card, an icon, the title and the value at the right, and, while
/// open, its controls under the title. Its header is a button that says whether it is open.
#[allow(clippy::too_many_arguments)]
pub fn row(
    id: &str,
    icon: &'static str,
    title: &str,
    value: Option<AnyElement>,
    default_open: bool,
    controls: impl FnOnce(&mut Window, &mut App) -> AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let open = open_state(id, default_open, window, cx);
    let is_open = *open.read(cx);
    let header = header(
        id,
        icon,
        title,
        value,
        Some(if is_open {
            "chevron-down"
        } else {
            "chevron-right"
        }),
        Some(is_open),
        window,
        cx,
    );
    let header = pressable(
        header,
        Some(Rc::new(move |_, cx: &mut App| {
            open.update(cx, |open, cx| {
                *open = !*open;
                cx.notify();
            })
        })),
    );
    let body = is_open.then(|| controls(window, cx));
    let theme = theme(cx);
    div()
        .rounded(r(12.))
        .bg(hsla(palette(cx).surface.raised))
        .child(header)
        .when_some(body, |section, body| {
            section.child(
                div()
                    .mt(r(-8.))
                    .min_w_0()
                    .px(r(theme.space.s16))
                    .pl(r(46.))
                    .pb(r(theme.space.s14))
                    .child(body),
            )
        })
        .into_any_element()
}

/// A row's header (`RowHeader`): a button while it opens or leads somewhere (`expanded` says which).
#[allow(clippy::too_many_arguments)]
pub fn header(
    id: &str,
    icon: &'static str,
    title: &str,
    value: Option<AnyElement>,
    chevron: Option<&'static str>,
    expanded: Option<bool>,
    window: &mut Window,
    cx: &mut App,
) -> crate::semantic::Semantic<gpui::Stateful<gpui::Div>> {
    let focus = focus_handle(id, window, cx);
    let ring = focus_visible(&focus, window);
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(SharedString::from(id.to_owned()), Role::Button)
        .aria_label(SharedString::from(title.to_owned()))
        .when_some(expanded, |header, expanded| header.aria_expanded(expanded))
        .track_focus(&focus)
        .relative()
        .flex()
        .items_center()
        .gap(r(theme.space.s12))
        .h(r(56.))
        .w_full()
        .px(r(theme.space.s16))
        .rounded(r(12.))
        .cursor_pointer()
        .child(
            svg()
                .path(format!("icons/i/175/{icon}.svg"))
                .size(r(18.))
                .flex_none()
                .text_color(hsla(colors.icon.base)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.primary))
                .child(SharedString::from(title.to_owned())),
        )
        .when_some(value, |header, value| {
            header.child(
                div()
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s6))
                    .min_w_0()
                    .max_w(gpui::relative(0.6))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .type_style(&theme.typography.desktop.ui)
                    .text_color(hsla(colors.text.secondary))
                    .child(value),
            )
        })
        .when_some(chevron, |header, chevron| {
            header.child(
                svg()
                    .path(format!("icons/i/175/{chevron}.svg"))
                    .size(r(16.))
                    .flex_none()
                    .text_color(hsla(colors.icon.base)),
            )
        })
        .when(ring, |header| header.child(focus_ring(12., cx)))
}

/// Quiet help under a row's controls.
pub fn help(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .mt(r(theme.space.s8))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(palette(cx).text.secondary))
        .child(text.into())
        .into_any_element()
}

/// Agents to choose, each with a switch (`CanMessageRow`): an agent's Can message, and a room's agents. `self_id`
/// leaves the agent itself out; `label` is the row's title and, before each name, its switch's name.
#[allow(clippy::too_many_arguments)]
pub fn can_message(
    id: &str,
    label: &str,
    summary: String,
    self_id: Option<&str>,
    ids: &[String],
    bots: &[Bot],
    default_open: bool,
    on_change: impl Fn(Vec<String>, &mut Window, &mut App) + 'static,
    note: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let others: Vec<Bot> = bots
        .iter()
        .filter(|bot| Some(bot.id.as_str()) != self_id)
        .cloned()
        .collect();
    let chosen: Vec<&Bot> = others.iter().filter(|bot| ids.contains(&bot.id)).collect();
    let value = div()
        .flex()
        .items_center()
        .gap(r(theme(cx).space.s6))
        .children(
            chosen
                .iter()
                .take(3)
                .map(|bot| BotAvatar::new(bot.color, bot.shape, 18.).into_any_element()),
        )
        .child(summary)
        .into_any_element();
    let ids = ids.to_vec();
    let on_change = Rc::new(on_change);
    let label_owned = label.to_owned();
    let row_id = id.to_owned();
    row(
        id,
        "message-circle",
        label,
        Some(value),
        default_open,
        move |_, cx| {
            let theme = theme(cx);
            let colors = palette(cx);
            let list: AnyElement = if others.is_empty() {
                div()
                    .type_style(&theme.typography.desktop.ui)
                    .text_color(hsla(colors.text.secondary))
                    .child("No other agents yet.")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .children(others.iter().map(|bot| {
                        let on = ids.contains(&bot.id);
                        let (ids, on_change, bot_id) =
                            (ids.clone(), on_change.clone(), bot.id.clone());
                        div()
                            .flex()
                            .items_center()
                            .gap(r(theme.space.s12))
                            .h(r(40.))
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
                            .child(
                                Toggle::new(
                                    format!("{row_id}.{}", bot.id),
                                    format!("{label_owned} {}", bot.name),
                                    on,
                                )
                                .on_toggle(
                                    move |on, window, cx| {
                                        let next = if on {
                                            ids.iter().cloned().chain([bot_id.clone()]).collect()
                                        } else {
                                            ids.iter()
                                                .filter(|id| **id != bot_id)
                                                .cloned()
                                                .collect()
                                        };
                                        on_change(next, window, cx);
                                    },
                                ),
                            )
                            .into_any_element()
                    }))
                    .into_any_element()
            };
            div().child(list).child(help(note, cx)).into_any_element()
        },
        window,
        cx,
    )
}

/// Autonomy (`AutonomyRow`): Full auto asks first, because it lets the agent do anything the Windows account can.
pub fn autonomy(
    id: &str,
    autonomy: Autonomy,
    provider: Provider,
    bot_name: &str,
    on_change: impl Fn(Autonomy, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let open = *open_state(id, true, window, cx).read(cx);
    let label = AUTONOMY_LEVELS
        .iter()
        .find(|(level, _)| *level == autonomy)
        .map(|(_, label)| *label)
        .unwrap_or_default();
    let value = (!open).then(|| div().child(label).into_any_element());
    let name = match bot_name.trim() {
        "" => "this agent".to_owned(),
        name => name.to_owned(),
    };
    let on_change = Rc::new(on_change);
    let group_id = format!("{id}.levels");
    row(
        id,
        "shield-check",
        "Autonomy",
        value,
        true,
        move |_, cx| {
            let levels = AUTONOMY_LEVELS.iter().fold(
                RadioGroup::new(group_id, "Autonomy", autonomy).look(Look::Segmented),
                |group, (level, label)| group.choice(*level, level_key(*level), *label),
            );
            let on_change = on_change.clone();
            let name = name.clone();
            div()
                .child(levels.on_choose(move |next, window, cx| {
                    if next == Autonomy::Full {
                        let on_change = on_change.clone();
                        open_confirm(
                            window,
                            cx,
                            format!("Let {name} work without asking?"),
                            format!(
                                "In Full auto, {name} can run any command and change any file your Windows \
                                 account can, without checking with you first. Only choose it for agents you \
                                 trust with that."
                            ),
                            "Use Full auto",
                            false,
                            move |window, cx| on_change(Autonomy::Full, window, cx),
                        );
                    } else {
                        on_change(next, window, cx);
                    }
                }))
                .child(help(autonomy_help(autonomy, provider), cx))
                .into_any_element()
        },
        window,
        cx,
    )
}

fn level_key(level: Autonomy) -> &'static str {
    match level {
        Autonomy::Ask => "ask",
        Autonomy::Folder => "folder",
        Autonomy::Full => "full",
    }
}

/// Browser (`BrowserRow`): a switch, never a row that opens.
pub fn browser(
    id: &str,
    on: bool,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let _ = window;
    div()
        .flex()
        .items_center()
        .gap(r(theme.space.s12))
        .h(r(56.))
        .px(r(theme.space.s16))
        .rounded(r(12.))
        .bg(hsla(colors.surface.raised))
        .child(
            svg()
                .path("icons/i/175/globe.svg")
                .size(r(18.))
                .flex_none()
                .text_color(hsla(colors.icon.base)),
        )
        .child(
            div()
                .flex_1()
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.primary))
                .child("Browser"),
        )
        .child(
            div()
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.secondary))
                .child("Built into Tjommie"),
        )
        .child(Toggle::new(id.to_owned(), "Browser", on).on_toggle(on_change))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::primitives::keyboard_window;
    use crate::ui::testing::{bot, draw, node};
    use protocol::model::BotState;
    use std::cell::RefCell;

    #[gpui::test]
    fn a_row_opens_from_the_keyboard_and_its_switches_choose_agents(cx: &mut gpui::TestAppContext) {
        let chosen = Rc::new(RefCell::new(Vec::<String>::new()));
        let shown = chosen.clone();
        let cx = keyboard_window(cx, move |window, cx| {
            let bots = vec![
                bot("b1", "Forge", BotState::Idle),
                bot("b2", "Scout", BotState::Idle),
            ];
            let set = shown.clone();
            let ids = shown.borrow().clone();
            can_message(
                "test.members",
                "In this room",
                format!("{} agents", ids.len()),
                None,
                &ids,
                &bots,
                false,
                move |next, _, _| *set.borrow_mut() = next,
                "The agents you mention answer here.",
                window,
                cx,
            )
        });
        draw(cx);
        let header = node("test.members").expect("the row's header");
        assert_eq!(header.role, "Button");
        assert!(node("test.members.b1").is_none(), "closed");
        // Tab to the header, Enter opens it, Tab to Forge's switch, Space turns it on.
        cx.simulate_keystrokes("tab enter");
        draw(cx);
        assert_eq!(node("test.members.b1").unwrap().text, "In this room Forge");
        cx.simulate_keystrokes("tab space");
        draw(cx);
        assert_eq!(*chosen.borrow(), ["b1"]);
    }
}
