//! A post in a room (`room-post.tsx`; WF5), drawn as Aegis draws a chat's turns (chat-messages.html): yours at the
//! right, a "You" line over the bubble; an agent's at the left, its avatar, name, "to <agent>" for a post between
//! agents, and the time over the text. In your posts the mentions the room's agents answer are marked.

use std::ops::Range;

use gpui::{
    AnyElement, App, FontWeight, HighlightStyle, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Window, div,
};
use jiff::Zoned;
use protocol::mentions::mention_ranges;
use protocol::model::{Bot, Message, MessageData, MessageKind, TextData, message_data};

use crate::model::format::clock;
use crate::model::labels::DELETED_BOT;
use crate::semantic::SemanticExt as _;
use crate::ui::chat::bubbles;
use crate::ui::chat::message_view::{self, MessageContext};
use crate::ui::fields::Link;
use crate::ui::files::card::{FileFacts, card};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// Where `text`'s mentions are that the room answers: each @everyone, and each @name of one of its agents. Names are
/// read against every agent's, as the daemon reads them, so "@Bot 2" is never taken for "@Bot" in a room without Bot 2.
pub fn marked(text: &str, bots: &[Bot], members: &[&Bot]) -> Vec<Range<usize>> {
    let names: Vec<&str> = bots.iter().map(|bot| bot.name.as_str()).collect();
    mention_ranges(text, &names)
        .into_iter()
        .filter(|range| match &range.name {
            None => true,
            Some(name) => members.iter().any(|bot| &bot.name == name),
        })
        .filter_map(|range| Some(in_text(text, range.start)?..in_text(text, range.end)?))
        .collect()
}

/// `mention_ranges` reads the lowercased text: one of its offsets, in `text`. A character whose lowercase is longer
/// (İ) moves the offsets after it.
fn in_text(text: &str, lowered: usize) -> Option<usize> {
    let mut at = 0;
    for (index, c) in text.char_indices() {
        if at == lowered {
            return Some(index);
        }
        at += c.to_lowercase().map(char::len_utf8).sum::<usize>();
    }
    (at == lowered).then_some(text.len())
}

/// A message in the room: a text message as a post; anything else (an agent at work, a notice, an error) as a chat
/// draws it, in line with the posts' text (`MessageView` in `room-screen.tsx`).
pub fn post(
    message: &Message,
    bots: &[Bot],
    members: &[&Bot],
    context: &MessageContext,
    now: &Zoned,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if message.kind != MessageKind::Text {
        return div()
            .w_full()
            .pl(r(28.))
            .child(message_view::row(message, false, context, window, cx))
            .into_any_element();
    }
    // An agent's post: its Markdown, then, for a post between agents, its files and Open exchange (`room-post.tsx`).
    let exchange = match message_data(message) {
        Ok(MessageData::Text(Some(TextData::ExchangePost(post)))) => Some(post),
        _ => None,
    };
    let (bubble, files) = if message.author == "you" {
        (None, Vec::new())
    } else {
        let files: Vec<AnyElement> = exchange
            .iter()
            .flat_map(|post| post.files.iter().flatten().enumerate())
            .map(|(index, file)| {
                card(
                    &message.id,
                    index as u64,
                    file.attachment_id.clone(),
                    FileFacts {
                        name: file.name.clone(),
                        path: file.path.clone(),
                        size: file.size,
                        content_type: file.content_type.clone(),
                    },
                )
            })
            .collect();
        (Some(bubbles::bot(message, window, cx)), files)
    };
    let theme = theme(cx);
    let colors = palette(cx);
    let id = format!("room.post.{}", message.id);
    let time = clock(&message.created_at, now.time_zone());
    let meta = || {
        div()
            .flex()
            .items_center()
            .gap(r(theme.space.s8))
            .type_style(&theme.typography.desktop.meta)
            .text_color(hsla(colors.text.tertiary))
    };
    let who = |name: &str| {
        div()
            .type_style(&theme.typography.desktop.ui_strong)
            .text_color(hsla(colors.text.primary))
            .child(name.to_owned())
    };
    if message.author == "you" {
        let mention = HighlightStyle {
            color: Some(hsla(colors.accent.text)),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..HighlightStyle::default()
        };
        let highlights = marked(&message.body, bots, members)
            .into_iter()
            .map(|range| (range, mention));
        return div()
            .semantic(id, Role::Article)
            .aria_label(format!("You: {}", message.body))
            .self_end()
            .max_w(r(560.))
            .flex()
            .flex_col()
            .items_end()
            .gap(r(theme.space.s4))
            .child(meta().child(who("You")).child(time))
            .child(
                div()
                    .rounded_tl(r(theme.radius.slot))
                    .rounded_tr(r(theme.radius.slot))
                    .rounded_br(r(theme.radius.xs))
                    .rounded_bl(r(theme.radius.slot))
                    .bg(hsla(colors.surface.sunk))
                    .px(r(theme.space.s16))
                    .py(r(theme.space.s8))
                    .type_style(&theme.typography.desktop.body)
                    .text_color(hsla(colors.text.primary))
                    .child(StyledText::new(message.body.clone()).with_highlights(highlights)),
            )
            .into_any_element();
    }
    let author = bots.iter().find(|bot| bot.id == message.author);
    let name = author.map_or(DELETED_BOT, |bot| bot.name.as_str());
    // A post between agents names the one it went to.
    let to = exchange.as_ref().map(|post| {
        bots.iter()
            .find(|bot| bot.id == post.to_bot_id)
            .map_or(DELETED_BOT, |bot| bot.name.as_str())
    });
    let open = exchange.map(|post| {
        let (exchange_id, on_open) = (
            SharedString::from(post.exchange_id),
            context.on_open_exchange.clone(),
        );
        Link::new(format!("{id}.exchange"), "Open exchange")
            .on_press(move |window, cx| on_open(exchange_id.clone(), window, cx))
    });
    let avatar = match author {
        Some(bot) => BotAvatar::new(bot.color, bot.shape, 20.).into_any_element(),
        None => div()
            .size(r(20.))
            .flex_none()
            .rounded_full()
            .bg(hsla(colors.surface.sunk))
            .into_any_element(),
    };
    div()
        .semantic(id, Role::Article)
        .aria_label(format!("{name}: {}", message.body))
        .max_w(r(640.))
        .flex()
        .flex_col()
        .gap(r(theme.space.s6))
        .child(
            meta()
                .child(avatar)
                .child(who(name))
                .children(to.map(|to| format!("to {to}")))
                .child(time),
        )
        .children(bubble)
        .children(files)
        .children(open)
        .into_any_element()
}
