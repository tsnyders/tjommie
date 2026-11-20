//! One chat message, drawn by its kind (`message-view.tsx`), under its turn's meta line (chat-messages.html `.meta`):
//! who and when, once per turn. A payload that doesn't match its kind shows as a notice with the message's text.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, svg,
};
use jiff::tz::TimeZone;
use protocol::model::{
    Bot, Message, MessageData, MessageKind, MessageStatus, TextData, message_data,
};

use super::{approval, bubbles, notices, rows};
use crate::model::format::clock;
use crate::model::labels::DELETED_BOT;
use crate::ui::files::card::FileFacts;
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// Opens an exchange beside the chat.
pub type OpenExchange = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// What a message's view needs besides the message: the bots, the chat it's in, and the exchange open beside it.
#[derive(Clone)]
pub struct MessageContext {
    pub bots: Rc<Vec<Bot>>,
    pub conversation_id: SharedString,
    pub open_exchange: Option<SharedString>,
    pub on_open_exchange: OpenExchange,
}

/// A bot reply that ended blank shows nothing; a list drops these before its timeline, so they leave no time label and
/// no empty row. A reply still streaming always shows (a chat opened mid-reply gets it with an empty body).
pub fn shown(message: &Message) -> bool {
    message.kind != MessageKind::Text
        || message.author == "you"
        || message.status == MessageStatus::Streaming
        || !message.body.trim().is_empty()
}

/// A thread row: the turn's meta line when this message starts a turn, then the message. Your own rows sit on the
/// right.
pub fn row(
    message: &Message,
    starts_turn: bool,
    context: &MessageContext,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let you = message.author == "you";
    let system = message.author == "system";
    let theme = theme(cx);
    let gap = theme.space.s6;
    // A queued message says so on its own meta line.
    let meta = (!system && (starts_turn || message.status == MessageStatus::Queued))
        .then(|| meta(message, context, cx));
    let body = view(message, context, window, cx);
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(r(gap))
        .when(you, |row| row.items_end())
        .when_some(meta, |row, meta| row.child(meta))
        .child(body)
        .into_any_element()
}

/// The turn's meta line: "You 10:42", or the agent's avatar, name and time.
fn meta(message: &Message, context: &MessageContext, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let time = SharedString::from(clock(&message.created_at, &TimeZone::system()));
    let who = |name: SharedString| {
        div()
            .type_style(&theme.typography.desktop.ui_strong)
            .text_color(hsla(colors.text.primary))
            .child(name)
    };
    let line = div()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .type_style(&theme.typography.desktop.meta)
        .text_color(hsla(colors.text.tertiary));
    if message.author == "you" {
        let queued = message.status == MessageStatus::Queued;
        return line
            .when(queued, |line| {
                line.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s6))
                        .child(
                            svg()
                                .path("icons/i/175/clock.svg")
                                .size(r(13.))
                                .flex_none()
                                .text_color(hsla(colors.icon.muted)),
                        )
                        .child("Queued"),
                )
            })
            .child(who("You".into()))
            .child(time)
            .into_any_element();
    }
    let bot = context.bots.iter().find(|bot| bot.id == message.author);
    let avatar = match bot {
        Some(bot) => BotAvatar::new(bot.color, bot.shape, theme.size.avatar20)
            .pose(Pose::of(bot.state))
            .into_any_element(),
        None => div()
            .size(r(theme.size.avatar20))
            .flex_none()
            .rounded_full()
            .bg(hsla(colors.surface.sunk))
            .into_any_element(),
    };
    let name = bot.map_or(DELETED_BOT, |bot| bot.name.as_str());
    line.child(avatar)
        .child(who(SharedString::from(name.to_owned())))
        .child(time)
        .into_any_element()
}

/// The message itself, by its kind.
fn view(
    message: &Message,
    context: &MessageContext,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Ok(data) = message_data(message) else {
        return notices::notice(&message.id, &message.body, cx);
    };
    match data {
        MessageData::Text(data) => {
            if message.author != "you" {
                return bubbles::bot(message, window, cx);
            }
            // An exchange post (`exchangeId`) carries the recipient's files, not yours.
            let files = match data {
                Some(TextData::User(user)) => user.files,
                _ => Vec::new(),
            };
            bubbles::you(message, &files, cx)
        }
        MessageData::Activity(activity) => rows::tools(&message.id, &activity.items, window, cx),
        MessageData::Approval(card) => {
            let bot = context
                .bots
                .iter()
                .find(|bot| bot.id == message.author)
                .cloned();
            approval::card(message, &card, bot.as_ref(), window, cx)
        }
        MessageData::Notice(_) => notices::notice(&message.id, &message.body, cx),
        MessageData::Error(retry) => notices::error(&message.id, &message.body, retry, window, cx),
        MessageData::Handoff(handoff) => {
            rows::handoff(&message.id, &message.body, &handoff, context, window, cx)
        }
        MessageData::Routine(routine) => notices::routine(&message.id, &message.body, &routine, cx),
        MessageData::File(file) => crate::ui::files::card::card(
            &message.id,
            0,
            file.attachment_id,
            FileFacts {
                name: file.name,
                path: file.path,
                size: Some(file.size),
                content_type: file.content_type,
            },
            window,
            cx,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::timeline::timeline;
    use protocol::model::MessageKind;

    pub(crate) fn message(
        id: &str,
        kind: MessageKind,
        author: &str,
        body: &str,
        status: MessageStatus,
    ) -> Message {
        Message {
            id: id.into(),
            conversation_id: "b1".into(),
            author: author.into(),
            kind,
            body: body.into(),
            data: None,
            status,
            created_at: "2026-09-27T07:14:00.000Z".into(),
        }
    }

    #[test]
    fn drops_bot_replies_that_ended_blank_so_they_leave_no_time_label() {
        let blank = message("m1", MessageKind::Text, "b1", " \n\n", MessageStatus::Done);
        // A chat opened mid-reply: it shows.
        let typing = message("m2", MessageKind::Text, "b1", "", MessageStatus::Streaming);
        let yours = message(
            "m3",
            MessageKind::Text,
            "you",
            "Skip senior roles.",
            MessageStatus::Done,
        );
        let file = message("m4", MessageKind::File, "b1", "", MessageStatus::Done);
        let all = [blank.clone(), typing, yours, file];
        let ids: Vec<&str> = all
            .iter()
            .filter(|m| shown(m))
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["m2", "m3", "m4"]);
        let kept: Vec<Message> = [blank].into_iter().filter(shown).collect();
        assert!(timeline(&kept, &jiff::Zoned::now()).is_empty());
    }
}
