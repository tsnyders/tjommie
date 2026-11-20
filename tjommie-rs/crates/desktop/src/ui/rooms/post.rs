//! A post in a room (`room-post.tsx`; WF5), drawn as Aegis draws a chat's turns (chat-messages.html): yours at the
//! right, a "You" line over the bubble; an agent's at the left, its avatar, name, "to <agent>" for a post between
//! agents, and the time over the text. In your posts the mentions the room's agents answer are marked.

use std::ops::Range;

use gpui::{
    AnyElement, App, FontWeight, HighlightStyle, IntoElement, ParentElement as _, Role,
    StatefulInteractiveElement as _, Styled as _, StyledText, div,
};
use jiff::Zoned;
use protocol::mentions::mention_ranges;
use protocol::model::{Bot, Message, MessageData, MessageKind, TextData, message_data};

use crate::model::format::clock;
use crate::model::labels::DELETED_BOT;
use crate::semantic::SemanticExt as _;
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

/// A message in the room: a text message as a post; anything else (an agent at work, a notice, an error) as its text
/// on a line under the posts.
pub fn post(
    message: &Message,
    bots: &[Bot],
    members: &[&Bot],
    now: &Zoned,
    cx: &App,
) -> AnyElement {
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
    if message.kind != MessageKind::Text {
        // ponytail: activity rows, notices and errors as their text alone; the Chat lane's message view (T4.1) draws
        // them in full, with Retry, once it lands.
        return div()
            .semantic(id, Role::Paragraph)
            .aria_label(message.body.clone())
            .pl(r(28.))
            .type_style(&theme.typography.desktop.meta)
            .text_color(hsla(colors.text.tertiary))
            .child(message.body.clone())
            .into_any_element();
    }
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
    // A post between agents names the one it went to; its files and Open exchange come with the exchange view (T4.1).
    let to = match message_data(message) {
        Ok(MessageData::Text(Some(TextData::ExchangePost(post)))) => Some(
            bots.iter()
                .find(|bot| bot.id == post.to_bot_id)
                .map_or(DELETED_BOT, |bot| bot.name.as_str()),
        ),
        _ => None,
    };
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
        .child(
            div()
                .type_style(&theme.typography.desktop.body)
                .text_color(hsla(colors.text.primary))
                .child(message.body.clone()),
        )
        .into_any_element()
}
