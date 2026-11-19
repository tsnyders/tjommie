//! The overview's cards (`agents/cards.tsx`; desktop-agents.html `.card.bot`, `.need`, `.new-tile`): an agent's card,
//! a Needs you card, a room's card, and the New agent and New room tiles.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Div, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, div, px, svg,
};
use jiff::Zoned;
use protocol::model::{
    Bot, BotState, Message, MessageData, MessageKind, Room, Routine, TextData, message_data,
};
use protocol::wire::{BotsPause, method};

use super::AgentsScreen;
use crate::guard::guarded_async;
use crate::model::labels::{DELETED_BOT, error_text, short_state};
use crate::model::overview::{next_routine_line, room_line, talking_with, task_line};
use crate::model::setup::provider_name;
use crate::semantic::{Semantic, SemanticExt as _};
use crate::ui::fields::Link;
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::primitives::button::{IconButton, Pill, PillVariant, pressable};
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, shadows, theme};

/// The raised card every card is (`.card`).
fn card(
    id: impl Into<SharedString>,
    role: Role,
    label: impl Into<SharedString>,
    cx: &App,
) -> Semantic<Stateful<Div>> {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(id, role)
        .aria_label(label)
        .relative()
        .flex()
        .flex_col()
        .bg(hsla(colors.surface.raised))
        .rounded(r(theme.radius.slot))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .shadow(shadows(&theme.shadow.light.card))
}

/// The eyes for an agent's state.
pub(crate) fn pose(state: BotState) -> Pose {
    match state {
        BotState::Waiting => Pose::Waiting,
        BotState::Working => Pose::Working,
        BotState::Limited => Pose::Limited,
        BotState::Paused => Pose::Paused,
        BotState::Idle | BotState::NeedsSetup => Pose::Idle,
    }
}

/// The chip beside the avatar (`StateChip`; `.pill-needs`, `.pill-working`, `.pill-quiet`, `.pill-idle`).
pub(crate) fn state_pill(bot: &Bot, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let (label, icon): (&str, Option<&str>) = match bot.state {
        BotState::Waiting => ("Waiting for you", None),
        BotState::Working => (short_state(bot.state), Some("loader-circle")),
        BotState::Paused => (short_state(bot.state), Some("pause")),
        BotState::Limited => (short_state(bot.state), Some("hourglass")),
        BotState::NeedsSetup => (short_state(bot.state), Some("plug")),
        BotState::Idle => (short_state(bot.state), None),
    };
    let pill = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(r(theme.space.s6))
        .h(r(24.))
        .px(r(theme.space.s10))
        .rounded_full()
        .type_style(&theme.typography.desktop.label);
    let pill = match bot.state {
        BotState::Waiting => pill
            .bg(hsla(colors.accent.tint))
            .text_color(hsla(colors.accent.text))
            .child(
                div()
                    .size(px(7.))
                    .rounded_full()
                    .bg(hsla(colors.accent.marker)),
            ),
        BotState::Working => pill
            .bg(hsla(colors.surface.panel))
            .text_color(hsla(colors.text.primary)),
        BotState::Idle => pill
            .px(r(theme.space.s4))
            .text_color(hsla(colors.text.tertiary)),
        _ => pill
            .border_1()
            .border_color(hsla(colors.line.soft))
            .text_color(hsla(colors.text.secondary)),
    };
    pill.when_some(icon, |pill, icon| {
        pill.child(
            svg()
                .path(format!("icons/i/175/{icon}.svg"))
                .size(r(13.))
                .text_color(hsla(colors.icon.base)),
        )
    })
    .child(label)
    .into_any_element()
}

/// An avatar dimmed as the TypeScript dims a quiet agent: idle or needing setup 35%, paused or limited 50%.
fn avatar(bot: &Bot, size: f32) -> AnyElement {
    let opacity = match bot.state {
        BotState::Idle | BotState::NeedsSetup => 0.35,
        BotState::Paused | BotState::Limited => 0.5,
        BotState::Working | BotState::Waiting => 1.0,
    };
    div()
        .flex_none()
        .opacity(opacity)
        .child(BotAvatar::new(bot.color, bot.shape, size).pose(pose(bot.state)))
        .into_any_element()
}

/// An agent's card (`BotCard`): who it is and what it's doing, its next routine, who it talked with today, and Open
/// chat, Pause or Resume, and More (Open chat, Pause or Resume, Agent settings).
#[allow(clippy::too_many_arguments)]
pub(super) fn bot_card(
    bot: &Bot,
    bots: &[Bot],
    messages: &[Message],
    routines: &[Routine],
    problem: Option<SharedString>,
    now: &Zoned,
    screen: Entity<AgentsScreen>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = format!("agents.card.{}", bot.id);
    let more = focus_handle(&format!("{id}.more"), window, cx);
    let theme = theme(cx);
    let colors = palette(cx);
    let partners: Vec<&Bot> = talking_with(&bot.id, messages, now)
        .iter()
        .filter_map(|id| bots.iter().find(|each| each.id == *id))
        .collect();
    let name: SharedString = bot.name.clone().into();
    let doing = match problem {
        Some(problem) => div()
            .semantic(format!("{id}.problem"), Role::Alert)
            .aria_label(problem.clone())
            .mt(r(theme.space.s6))
            .truncate()
            .type_style(&theme.typography.desktop.ui)
            .text_color(hsla(colors.danger.text))
            .child(problem)
            .into_any_element(),
        None => div()
            .mt(r(theme.space.s6))
            .truncate()
            .type_style(&theme.typography.desktop.ui)
            .text_color(hsla(colors.text.secondary))
            .child(task_line(bot, messages, now))
            .into_any_element(),
    };
    let footer = if partners.is_empty() {
        div().child("No agent chats today").into_any_element()
    } else {
        div()
            .flex()
            .items_center()
            .gap(r(theme.space.s8))
            .child("Talking with")
            .child(div().flex().children(partners.iter().map(|partner| {
                div()
                    .ml(px(-4.))
                    .child(BotAvatar::new(partner.color, partner.shape, 18.))
                    .into_any_element()
            })))
            .into_any_element()
    };
    let chat = Route::Chat(bot.id.clone());
    let pause_label = if bot.paused {
        "Resume agent"
    } else {
        "Pause agent"
    };
    let paused = bot.paused;
    let bot_id = bot.id.clone();
    let toggle = move |cx: &mut App| pause(&bot_id, paused, screen.clone(), cx);
    let toggle_again = toggle.clone();
    let open = chat.clone();
    let settings = Route::BotSettings(bot.id.clone());
    let menu = Menu::new(SharedString::from(format!("{id}.menu")), {
        let label: SharedString = format!("Menu for {}", bot.name).into();
        move |open, window, cx| more_trigger(format!("{id}.more"), label, open, &more, window, cx)
    })
    .anchor(gpui::Anchor::TopRight)
    .offset(px(6.))
    .item(
        MenuItem::new(
            format!("agents.menu.{}.chat", bot.id),
            "Open chat",
            move |_, cx| cx.navigate(open.clone()),
        )
        .icon("message-circle"),
    )
    .item(
        MenuItem::new(
            format!("agents.menu.{}.pause", bot.id),
            pause_label,
            move |_, cx| toggle_again(cx),
        )
        .icon(if paused { "play" } else { "pause" }),
    )
    .item(
        MenuItem::new(
            format!("agents.menu.{}.settings", bot.id),
            "Agent settings",
            move |_, cx| cx.navigate(settings.clone()),
        )
        .icon("settings"),
    );
    let card_id = format!("agents.card.{}", bot.id);
    card(card_id.clone(), Role::Article, name.clone(), cx)
        .min_h(r(196.))
        .pt(r(theme.space.s16))
        .px(r(theme.space.s16))
        .pb(r(theme.space.s12))
        .when(bot.state == BotState::Waiting, |card| {
            card.border_1().border_color(hsla(colors.accent.base))
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .child(avatar(bot, theme.size.avatar40))
                .child(state_pill(bot, cx)),
        )
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(r(theme.space.s8))
                .mt(r(theme.space.s14))
                .min_w_0()
                .child(
                    div()
                        .semantic(format!("{card_id}.name"), Role::Heading)
                        .aria_label(name.clone())
                        .min_w_0()
                        .truncate()
                        .type_style(&theme.typography.desktop.display_s)
                        .text_color(hsla(colors.text.primary))
                        .child(name),
                )
                .child(
                    div()
                        .flex_none()
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.tertiary))
                        .child(provider_name(bot.provider)),
                ),
        )
        .child(doing)
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s8))
                .mt(r(theme.space.s8))
                .type_style(&theme.typography.desktop.meta)
                .text_color(hsla(colors.text.tertiary))
                .child(
                    svg()
                        .path("icons/i/175/clock.svg")
                        .size(r(14.))
                        .flex_none()
                        .text_color(hsla(colors.icon.muted)),
                )
                .child(
                    div()
                        .truncate()
                        .child(next_routine_line(bot, routines, now)),
                ),
        )
        .child(
            div()
                .mt(r(theme.space.s8))
                .type_style(&theme.typography.desktop.meta)
                .text_color(hsla(colors.text.tertiary))
                .child(footer),
        )
        .child(
            div()
                .mt_auto()
                .pt(r(theme.space.s12))
                .flex()
                .items_center()
                .gap(r(theme.space.s4))
                .child(
                    div().mr_auto().child(
                        Link::new(format!("{card_id}.open"), "Open chat")
                            .on_press(move |_, cx| cx.navigate(chat.clone())),
                    ),
                )
                .when(bot.state == BotState::Waiting, |acts| {
                    acts.child(
                        Pill::new(format!("{card_id}.review"), "Review")
                            .variant(PillVariant::Primary)
                            .small()
                            .on_press({
                                let chat = Route::Chat(bot.id.clone());
                                move |_, cx| cx.navigate(chat.clone())
                            }),
                    )
                })
                .child(
                    IconButton::new(
                        format!("{card_id}.pause"),
                        if paused { "play" } else { "pause" },
                        format!("{} {}", if paused { "Resume" } else { "Pause" }, bot.name),
                    )
                    .on_press(move |_, cx| toggle(cx)),
                )
                .child(menu),
        )
        .into_any_element()
}

/// The More button that opens a card's menu: a tab stop, so Enter or Space opens it.
pub(crate) fn more_trigger(
    id: String,
    label: SharedString,
    open: bool,
    focus: &FocusHandle,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let ring = focus_visible(focus, window);
    div()
        .semantic(id, Role::Button)
        .aria_label(label)
        .aria_expanded(open)
        .track_focus(focus)
        .relative()
        .size(r(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .when(open, |button| button.bg(hsla(colors.state.hover)))
        .hover(|style| style.bg(hsla(colors.state.hover)))
        .child(
            svg()
                .path("icons/i/175/ellipsis.svg")
                .size(r(theme.size.icon))
                .text_color(hsla(colors.icon.base)),
        )
        .when(ring, |button| {
            button.child(focus_ring(theme.radius.pill, cx))
        })
        .into_any_element()
}

/// Pause or Resume from a card; a failure says why in place of the task line (`CardMenu`'s `pause`).
fn pause(bot_id: &str, paused: bool, screen: Entity<AgentsScreen>, cx: &mut App) {
    screen.update(cx, |screen, cx| {
        screen.set_problem(bot_id, None);
        cx.notify();
    });
    let call = store::request::<method::BotsPause>(
        cx,
        BotsPause {
            id: bot_id.to_owned(),
            paused: !paused,
        },
    );
    let bot_id = bot_id.to_owned();
    cx.spawn(async move |cx| {
        guarded_async("pause or resume an agent", async move {
            if let Err(error) = call.await {
                cx.update(|cx| {
                    screen.update(cx, |screen, cx| {
                        screen.set_problem(&bot_id, Some(error_text(&error).into()));
                        cx.notify();
                    })
                });
            }
        })
        .await;
    })
    .detach();
}

/// A Needs you card (`NeedsYouCard`; `.need`): who waits, what it asks, and Review, which opens its chat.
pub(super) fn needs_you(bot: &Bot, messages: &[Message], now: &Zoned, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let id = format!("agents.need.{}", bot.id);
    let chat = Route::Chat(bot.id.clone());
    let open = chat.clone();
    card(
        id.clone(),
        Role::Article,
        format!("{} needs you", bot.name),
        cx,
    )
    .p(r(theme.space.s16))
    .pb(r(theme.space.s14))
    .gap(r(theme.space.s10))
    .child(
        div()
            .flex()
            .items_center()
            .gap(r(theme.space.s12))
            .child(BotAvatar::new(bot.color, bot.shape, theme.size.avatar40).pose(Pose::Waiting))
            .child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .type_style(&theme.typography.desktop.display_s)
                            .text_color(hsla(colors.text.primary))
                            .child(bot.name.clone()),
                    )
                    .child(
                        div()
                            .type_style(&theme.typography.desktop.meta)
                            .text_color(hsla(colors.text.tertiary))
                            .child(provider_name(bot.provider)),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .type_style(&theme.typography.desktop.meta)
                    .text_color(hsla(colors.accent.text))
                    .child("Needs you"),
            ),
    )
    .child(
        div()
            .type_style(&theme.typography.desktop.body)
            .text_color(hsla(colors.text.primary))
            .child(task_line(bot, messages, now)),
    )
    .child(
        div()
            .mt_auto()
            .flex()
            .items_center()
            .gap(r(theme.space.s20))
            .child(
                Pill::new(format!("{id}.review"), "Review")
                    .variant(PillVariant::Primary)
                    .small()
                    .on_press(move |_, cx| cx.navigate(chat.clone())),
            )
            .child(
                div().ml_auto().child(
                    Link::new(format!("{id}.open"), "Open chat")
                        .quiet()
                        .on_press(move |_, cx| cx.navigate(open.clone())),
                ),
            ),
    )
    .into_any_element()
}

/// A room's last two agent-to-agent posts (`TeamRoomCard`'s `posts`).
fn exchange_posts(messages: &[Message]) -> Vec<(&Message, String)> {
    let posts: Vec<(&Message, String)> = messages
        .iter()
        .filter(|message| message.kind == MessageKind::Text)
        .filter_map(|message| match message_data(message) {
            Ok(MessageData::Text(Some(TextData::ExchangePost(post)))) => {
                Some((message, post.to_bot_id))
            }
            _ => None,
        })
        .collect();
    let skip = posts.len().saturating_sub(2);
    posts.into_iter().skip(skip).collect()
}

/// A room's card (`TeamRoomCard`): its last two agent-to-agent posts and its newest post. While its posts are being read
/// (`None`) it says nothing of them. Its ids start with `prefix` ("agents.room" on Agents); with `members` (the rooms
/// list's), the room's agents are drawn beside its name, five and then "+N".
pub(crate) fn room_card(
    prefix: &str,
    room: &Room,
    messages: Option<&[Message]>,
    bots: &[Bot],
    members: Option<&[&Bot]>,
    now: &Zoned,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let id = format!("{prefix}.{}", room.id);
    let stack = members.map(|members| {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(r(theme.space.s6))
            .child(
                div()
                    .flex()
                    .pl(px(4.))
                    .children(members.iter().take(5).map(|bot| {
                        div()
                            .ml(px(-4.))
                            .child(BotAvatar::new(bot.color, bot.shape, 18.))
                    })),
            )
            .when(members.len() > 5, |stack| {
                stack.child(
                    div()
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.tertiary))
                        .child(format!("+{}", members.len() - 5)),
                )
            })
    });
    let posts = exchange_posts(messages.unwrap_or(&[]));
    let preview: AnyElement = if posts.is_empty() {
        match messages {
            // Still reading: nothing to say yet.
            None => div().into_any_element(),
            Some(_) => div()
                .semantic(format!("{id}.none"), Role::Paragraph)
                .aria_label("No agent chats yet")
                .text_center()
                .type_style(&theme.typography.desktop.meta)
                .text_color(hsla(colors.text.tertiary))
                .child("No agent chats yet")
                .into_any_element(),
        }
    } else {
        div()
            .flex()
            .flex_col()
            .gap(r(theme.space.s10))
            .children(posts.into_iter().map(|(post, to)| {
                let author = bots.iter().find(|bot| bot.id == post.author);
                let to = bots
                    .iter()
                    .find(|bot| bot.id == to)
                    .map_or(DELETED_BOT, |bot| bot.name.as_str());
                div()
                    .flex()
                    .gap(r(theme.space.s8))
                    .child(match author {
                        Some(author) => {
                            BotAvatar::new(author.color, author.shape, 14.).into_any_element()
                        }
                        None => div()
                            .size(r(14.))
                            .flex_none()
                            .rounded_full()
                            .bg(hsla(colors.surface.sunk))
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .min_w_0()
                            .type_style(&theme.typography.desktop.meta)
                            .child(div().text_color(hsla(colors.text.tertiary)).child(format!(
                                "{} to {to}",
                                author.map_or(DELETED_BOT, |author| author.name.as_str())
                            )))
                            .child(
                                div()
                                    .text_color(hsla(colors.text.secondary))
                                    .line_clamp(3)
                                    .child(post.body.clone()),
                            ),
                    )
                    .into_any_element()
            }))
            .into_any_element()
    };
    let room_route = Route::Room(room.id.clone());
    let open = room_route.clone();
    card(id.clone(), Role::Article, room.name.clone(), cx)
        .min_h(r(196.))
        .p(r(theme.space.s16))
        .pb(r(theme.space.s12))
        .gap(r(theme.space.s10))
        .child(
            div()
                .min_h(r(72.))
                .p(r(theme.space.s12))
                .rounded(r(theme.radius.field))
                .bg(hsla(colors.surface.panel))
                .flex()
                .flex_col()
                .justify_center()
                .child(preview),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s10))
                .child(
                    svg()
                        .path("icons/i/175/messages-square.svg")
                        .size(r(20.))
                        .flex_none()
                        .text_color(hsla(colors.icon.base)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .type_style(&theme.typography.desktop.display_s)
                        .text_color(hsla(colors.text.primary))
                        .child(room.name.clone()),
                )
                .children(stack),
        )
        .child(
            div()
                .semantic(format!("{id}.line"), Role::Paragraph)
                .aria_label(room_line(messages, bots, now))
                .truncate()
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.secondary))
                .child(room_line(messages, bots, now)),
        )
        .child(
            div().mt_auto().pt(r(theme.space.s4)).child(
                Link::new(format!("{id}.open"), "Open room")
                    .on_press(move |_, cx| cx.navigate(open.clone())),
            ),
        )
        .into_any_element()
}

/// A dashed-free new tile (`.new-tile`): a round plus, a title and a line, the whole tile one button.
fn new_tile(
    id: &'static str,
    title: &'static str,
    line: &'static str,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let focus = focus_handle(id, window, cx);
    let ring = focus_visible(&focus, window);
    let theme = theme(cx);
    let colors = palette(cx);
    let tile = div()
        .semantic(id, Role::Button)
        .aria_label(title)
        .track_focus(&focus)
        .relative()
        .min_h(r(196.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(r(theme.space.s8))
        .p(r(theme.space.s16))
        .rounded(r(theme.radius.slot))
        .bg(hsla(colors.surface.sunk))
        .cursor_pointer()
        .hover(|style| style.bg(hsla(colors.state.sunk_hover)))
        .child(
            div()
                .size(r(44.))
                .mb(r(theme.space.s4))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(hsla(colors.surface.raised))
                .shadow(shadows(&theme.shadow.light.card))
                .child(
                    svg()
                        .path("icons/i/175/plus.svg")
                        .size(r(22.))
                        .text_color(hsla(colors.icon.base)),
                ),
        )
        .child(
            div()
                .type_style(&theme.typography.desktop.ui_strong)
                .text_color(hsla(colors.text.primary))
                .child(title),
        )
        .child(
            div()
                .max_w(r(190.))
                .text_center()
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.secondary))
                .child(line),
        )
        .when(ring, |tile| tile.child(focus_ring(theme.radius.slot, cx)));
    pressable(tile, Some(std::rc::Rc::new(on_press))).into_any_element()
}

pub(super) fn new_bot_tile(window: &mut Window, cx: &mut App) -> AnyElement {
    new_tile(
        "agents.new_bot",
        "New agent",
        "Start from a suggestion or from scratch",
        |_, cx| cx.navigate(Route::NewBot),
        window,
        cx,
    )
}

pub(crate) fn new_room_tile(
    id: &'static str,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    new_tile(
        id,
        "New room",
        "Choose which agents are in it",
        on_press,
        window,
        cx,
    )
}
