//! A turn's rows (chat-messages.html `.tools`, chat-cards.html `.exchange`; `activity-row.tsx`, `handoff-row.tsx`):
//! the tool log, one row per step with its state, and work with another agent: an exchange's card, which opens the
//! exchange beside the chat, or a room answer's row, which opens the room.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use protocol::model::{
    ActivityItem, ActivityStatus, Bot, ExchangeHandoffData, ExchangeStatus, HandoffData,
    RoomHandoffData, TEAM_ROOM_ID,
};

use super::message_view::MessageContext;
use crate::model::labels::DELETED_BOT;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::Link;
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::primitives::button::pressable;
use crate::ui::shell::connection::spinner;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// The icon for a step, by the tool's name: reading, editing, running, the web, searching, or any other tool.
pub(crate) fn tool_icon(name: &str) -> &'static str {
    let name = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| name.contains(word));
    if has(&["edit", "write", "patch", "file_change"]) {
        "file-pen"
    } else if has(&["read", "view", "notebook"]) {
        "file-text"
    } else if has(&["bash", "command", "shell", "exec", "terminal"]) {
        "terminal"
    } else if has(&["grep", "glob", "search", "find"]) {
        "search"
    } else if has(&["web", "fetch", "browser", "url"]) {
        "globe"
    } else {
        "workflow"
    }
}

/// What a step's state says to a screen reader.
fn state_word(status: ActivityStatus) -> &'static str {
    match status {
        ActivityStatus::Running => "Running",
        ActivityStatus::Done => "Done",
        ActivityStatus::Failed => "Failed",
    }
}

/// A turn's tool log: every step, its icon, the daemon's one-line summary, and how it went.
pub fn tools(id: &str, items: &[ActivityItem], window: &mut Window, cx: &mut App) -> AnyElement {
    let rows: Vec<AnyElement> = items
        .iter()
        .map(|item| step(id, item, window, cx))
        .collect();
    div()
        .semantic(format!("tools.{id}"), Role::List)
        .w_full()
        .max_w(r(600.))
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

fn step(id: &str, item: &ActivityItem, window: &mut Window, cx: &mut App) -> AnyElement {
    let running = item.status == ActivityStatus::Running;
    let result = match item.status {
        ActivityStatus::Running => spinner(14., window, cx),
        ActivityStatus::Done | ActivityStatus::Failed => {
            let icon = if item.status == ActivityStatus::Done {
                "check"
            } else {
                "x"
            };
            svg()
                .path(format!("icons/i/220/{icon}.svg"))
                .size(r(14.))
                .flex_none()
                .text_color(hsla(palette(cx).text.primary))
                .into_any_element()
        }
    };
    let theme = theme(cx);
    let colors = palette(cx);
    let label = SharedString::from(format!("{}, {}", item.summary, state_word(item.status)));
    div()
        .semantic(format!("tools.{id}.{}", item.id), Role::ListItem)
        .aria_label(label)
        .h(r(28.))
        .flex()
        .items_center()
        .gap(r(theme.space.s10))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(if running {
            colors.text.primary
        } else {
            colors.text.secondary
        }))
        .child(
            svg()
                .path(format!("icons/i/175/{}.svg", tool_icon(&item.name)))
                .size(r(16.))
                .flex_none()
                .text_color(hsla(if running {
                    colors.text.primary
                } else {
                    colors.icon.muted
                })),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(SharedString::from(item.summary.clone())),
        )
        .child(div().flex_none().flex().items_center().child(result))
        .into_any_element()
}

/// Work with another agent: an exchange's card, or a room answer's row.
pub fn handoff(
    id: &str,
    body: &str,
    data: &HandoffData,
    context: &MessageContext,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match data {
        HandoffData::Room(room) => room_row(id, body, room, cx),
        HandoffData::Exchange(exchange) => exchange_card(body, exchange, context, window, cx),
    }
}

fn avatar(bot: Option<&Bot>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    match bot {
        Some(bot) => BotAvatar::new(bot.color, bot.shape, theme.size.avatar20)
            .pose(Pose::of(bot.state))
            .into_any_element(),
        // A deleted agent: an empty stone disc where its avatar was.
        None => div()
            .size(r(theme.size.avatar20))
            .flex_none()
            .rounded_full()
            .bg(hsla(palette(cx).surface.sunk))
            .into_any_element(),
    }
}

/// The room a room answer went to: rows from before there were other rooms have no room and open Everyone.
pub fn answer_room(room: &RoomHandoffData) -> String {
    room.room_id
        .clone()
        .unwrap_or_else(|| TEAM_ROOM_ID.to_owned())
}

/// A room answer's row ("Answered in Growth"): it opens that room.
fn room_row(id: &str, body: &str, room: &RoomHandoffData, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let target = answer_room(room);
    let row = div()
        .semantic(format!("handoff.{id}"), Role::Link)
        .aria_label(SharedString::from(body.to_owned()))
        .max_w_full()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .px(r(theme.space.s12))
        .py(r(theme.space.s6))
        .rounded_full()
        .border_1()
        .border_color(hsla(colors.line.base))
        .cursor_pointer()
        .hover(|style| style.bg(hsla(colors.state.hover)))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(colors.text.secondary))
        .child(
            svg()
                .path("icons/i/175/messages-square.svg")
                .size(r(14.))
                .flex_none()
                .text_color(hsla(colors.icon.base)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(SharedString::from(body.to_owned())),
        )
        .child(
            svg()
                .path("icons/i/175/chevron-right.svg")
                .size(r(14.))
                .flex_none()
                .text_color(hsla(colors.icon.muted)),
        );
    pressable(
        row,
        Some(Rc::new(move |_: &mut Window, cx: &mut App| {
            cx.navigate(Route::Room(target.clone()));
        })),
    )
    .into_any_element()
}

/// The count, or how the exchange stands while there is no answer yet.
fn exchange_count(exchange: &ExchangeHandoffData) -> String {
    match exchange.status {
        ExchangeStatus::Waiting => "Waiting for a reply".into(),
        ExchangeStatus::Failed => "No reply".into(),
        ExchangeStatus::Replied => match exchange.messages {
            1 => "1 message".into(),
            count => format!("{count} messages"),
        },
    }
}

/// An exchange's card: who asked whom, the question, and View thread, which opens it beside the chat. While the
/// other agent answers, the card says so.
fn exchange_card(
    body: &str,
    exchange: &ExchangeHandoffData,
    context: &MessageContext,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let bots = &context.bots;
    let selected = context
        .open_exchange
        .as_ref()
        .is_some_and(|open| **open == exchange.exchange_id);
    let from = bots.iter().find(|bot| bot.id == exchange.from_bot_id);
    let to = bots.iter().find(|bot| bot.id == exchange.to_bot_id);
    let from_name = from.map_or(DELETED_BOT, |bot| bot.name.as_str());
    let to_name = to.map_or(DELETED_BOT, |bot| bot.name.as_str());
    let title = SharedString::from(format!("{from_name} asked {to_name}"));
    let answering = (exchange.status == ExchangeStatus::Waiting)
        .then(|| (format!("{to_name} is answering"), spinner(14., window, cx)));
    let from_avatar = avatar(from, cx);
    let to_avatar = avatar(to, cx);
    let theme = theme(cx);
    let colors = palette(cx);
    let id = exchange.exchange_id.clone();
    let open = context.on_open_exchange.clone();
    let open_id = SharedString::from(id.clone());
    div()
        .semantic(format!("exchange.{id}"), Role::Group)
        .aria_label(title.clone())
        .w_full()
        .max_w(r(600.))
        .flex()
        .flex_col()
        .gap(r(theme.space.s6))
        .px(r(theme.space.s16))
        .py(r(theme.space.s10))
        .bg(hsla(colors.surface.raised))
        .rounded(r(theme.radius.slot))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .shadow(shadows(&theme.shadow.light.card))
        // The open exchange's card is marked while the view beside the chat shows it.
        .when(selected, |card| {
            card.border_1().border_color(hsla(colors.ink.base))
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s8))
                .type_style(&theme.typography.desktop.ui_strong)
                .text_color(hsla(colors.text.primary))
                .child(
                    svg()
                        .path("icons/i/175/arrow-left-right.svg")
                        .size(r(14.))
                        .flex_none()
                        .text_color(hsla(colors.icon.muted)),
                )
                .child(div().min_w_0().truncate().child(title))
                .child(
                    div()
                        .ml_auto()
                        .flex_none()
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.tertiary))
                        .child(SharedString::from(exchange_count(exchange))),
                )
                .child(
                    div().ml(r(theme.space.s12)).flex_none().child(
                        Link::new(format!("exchange.{id}.open"), "View thread")
                            .on_press(move |window, cx| open(open_id.clone(), window, cx)),
                    ),
                ),
        )
        .child(
            div()
                .flex()
                .gap(r(theme.space.s10))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.primary))
                .child(from_avatar)
                .child(div().min_w_0().child(SharedString::from(body.to_owned()))),
        )
        .when_some(answering, |card, (text, spinner)| {
            card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s10))
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(colors.text.secondary))
                    .child(to_avatar)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(r(theme.space.s6))
                            .child(spinner)
                            .child(text),
                    ),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_answer_opens_its_room_or_everyone() {
        let growth = RoomHandoffData {
            room_message_id: "m1".into(),
            room_id: Some("r 1".into()),
        };
        assert_eq!(answer_room(&growth), "r 1");
        let old = RoomHandoffData {
            room_message_id: "m1".into(),
            room_id: None,
        };
        assert_eq!(answer_room(&old), "team");
    }

    #[test]
    fn steps_show_their_tool() {
        assert_eq!(tool_icon("Read"), "file-text");
        assert_eq!(tool_icon("MultiEdit"), "file-pen");
        assert_eq!(tool_icon("Bash"), "terminal");
        assert_eq!(tool_icon("command_execution"), "terminal");
        assert_eq!(tool_icon("Grep"), "search");
        assert_eq!(tool_icon("WebFetch"), "globe");
        assert_eq!(tool_icon("mcp__tjommie__message_bot"), "workflow");
    }

    #[test]
    fn an_exchange_counts_its_messages_until_it_has_an_answer() {
        let card = |status, messages| ExchangeHandoffData {
            exchange_id: "x1".into(),
            from_bot_id: "a".into(),
            to_bot_id: "b".into(),
            messages,
            status,
        };
        assert_eq!(
            exchange_count(&card(ExchangeStatus::Waiting, 1)),
            "Waiting for a reply"
        );
        assert_eq!(exchange_count(&card(ExchangeStatus::Failed, 1)), "No reply");
        assert_eq!(
            exchange_count(&card(ExchangeStatus::Replied, 1)),
            "1 message"
        );
        assert_eq!(
            exchange_count(&card(ExchangeStatus::Replied, 2)),
            "2 messages"
        );
    }
}
