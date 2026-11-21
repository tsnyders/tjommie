//! A conversation's thread (Phase 3 plan T4.1; `message-list.tsx`): its messages with their time labels, in a
//! virtualized GPUI `list()` anchored to the bottom (variable heights measured lazily, 400 px overdraw). A short thread
//! sits by the composer. The thread follows the newest message while you are at the bottom, and your own new message
//! always brings it there. An older page lands above what you are reading, which stays where it was.
//!
//! The list measures what it draws each frame, so a message that grows while it streams needs nothing here. The rows
//! are rebuilt only when the conversation's shape changes (a message added or dropped, an older page, the newest one
//! ending, a new day); the list is then told which rows were replaced.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, FollowMode, IntoElement, ListAlignment, ListState, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, list, px,
};
use jiff::Zoned;
use jiff::civil::Date;
use protocol::model::{Message, MessageStatus};

use super::message_view::{self, MessageContext, shown};
use super::notices;
use crate::model::timeline::{TimelineItem, timeline};
use crate::semantic::SemanticExt as _;
use crate::ui::store;
use crate::ui::style::{r, theme};

/// How much the list draws beyond what shows (spike criterion 1).
const OVERDRAW: f32 = 400.;

/// One row of the thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Stamp {
        key: SharedString,
        label: SharedString,
    },
    /// A message, by its place in the conversation. `starts_turn`: its meta line shows.
    Message {
        index: usize,
        id: SharedString,
        starts_turn: bool,
    },
}

impl Row {
    fn key(&self) -> &SharedString {
        match self {
            Row::Stamp { key, .. } => key,
            Row::Message { id, .. } => id,
        }
    }
}

/// What the rows are built from: any change to it rebuilds them.
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    len: usize,
    first: Option<String>,
    last: Option<String>,
    last_status: Option<MessageStatus>,
    last_blank: bool,
    today: Date,
}

impl Shape {
    fn of(messages: &[Message], now: &Zoned) -> Shape {
        let last = messages.last();
        Shape {
            len: messages.len(),
            first: messages.first().map(|message| message.id.clone()),
            last: last.map(|message| message.id.clone()),
            last_status: last.map(|message| message.status),
            last_blank: last.is_some_and(|message| message.body.trim().is_empty()),
            today: now.date(),
        }
    }
}

/// The rows for `messages`: a time label before each group, and each shown message, marked when it starts a turn (the
/// first after a label, or another author than the message before).
pub fn rows(messages: &[Message], now: &Zoned) -> Vec<Row> {
    let shown: Vec<(usize, &Message)> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| shown(message))
        .collect();
    // The timeline reads only ids and times, so it gets light copies.
    let light: Vec<Message> = shown
        .iter()
        .map(|(_, message)| Message {
            id: message.id.clone(),
            conversation_id: String::new(),
            author: String::new(),
            kind: message.kind,
            body: String::new(),
            data: None,
            status: message.status,
            created_at: message.created_at.clone(),
        })
        .collect();
    let mut places = shown.iter();
    let mut previous: Option<&str> = None;
    let mut rows = Vec::with_capacity(light.len() + 8);
    for item in timeline(&light, now) {
        match item {
            TimelineItem::Stamp { key, label } => {
                rows.push(Row::Stamp {
                    key: key.into(),
                    label: label.into(),
                });
                previous = None;
            }
            TimelineItem::Message { .. } => {
                let Some((index, message)) = places.next() else {
                    break;
                };
                let author = message.author.as_str();
                rows.push(Row::Message {
                    index: *index,
                    id: message.id.clone().into(),
                    starts_turn: previous != Some(author),
                });
                previous = Some(author);
            }
        }
    }
    rows
}

pub struct Thread {
    conversation_id: SharedString,
    list: ListState,
    rows: Rc<Vec<Row>>,
    shape: Option<Shape>,
    /// The newest shown message, to see your own new one arrive.
    newest: Option<SharedString>,
    /// Whether the last frame drew the oldest row (the list draws only what shows and its overdraw); cleared when an
    /// older row takes its place.
    top_drawn: Rc<Cell<bool>>,
}

impl Thread {
    pub fn new(conversation_id: SharedString) -> Self {
        let list = ListState::new(0, ListAlignment::Bottom, px(OVERDRAW));
        list.set_follow_mode(FollowMode::Tail);
        Thread {
            conversation_id,
            list,
            rows: Rc::default(),
            shape: None,
            newest: None,
            top_drawn: Rc::default(),
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Brings the rows up to date with `messages`.
    pub fn sync(&mut self, messages: &[Message], now: &Zoned) {
        let shape = Shape::of(messages, now);
        if self.shape.as_ref() == Some(&shape) {
            return;
        }
        self.shape = Some(shape);
        let next = rows(messages, now);
        let old = &self.rows;
        let same = |pair: (&Row, &Row)| pair.0.key() == pair.1.key();
        let prefix = old
            .iter()
            .zip(next.iter())
            .take_while(|pair| same(*pair))
            .count();
        let most = old.len().min(next.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(next.iter().rev())
            .take(most)
            .take_while(|pair| same(*pair))
            .count();
        if prefix + suffix < old.len().max(next.len()) {
            self.list
                .splice(prefix..old.len() - suffix, next.len() - prefix - suffix);
        }
        // A new oldest row (an older page landed above): the last frame drew the one before it, so the top is in reach
        // again only once this one is drawn, not on the strength of the last frame (M4 review A1).
        if prefix == 0 {
            self.top_drawn.set(false);
        }
        let newest = messages.iter().rev().find(|message| shown(message));
        let newest_id = newest.map(|message| SharedString::from(message.id.clone()));
        // Your own message always comes into view, even when you had scrolled up.
        if newest.is_some_and(|message| message.author == "you")
            && self.newest.is_some()
            && newest_id != self.newest
        {
            self.list.set_follow_mode(FollowMode::Tail);
        }
        self.newest = newest_id;
        self.rows = Rc::new(next);
    }

    /// Whether the last frame drew the oldest row, so the top is within the overdraw: time to load the page before
    /// it. (The list's own geometry can't say: while it follows the newest message it has no bounds above it.)
    pub fn top_in_reach(&self) -> bool {
        !self.rows.is_empty() && self.top_drawn.get()
    }

    pub fn render(&self, context: MessageContext, cx: &App) -> AnyElement {
        let rows = Rc::clone(&self.rows);
        let conversation = self.conversation_id.clone();
        let top_drawn = Rc::clone(&self.top_drawn);
        top_drawn.set(false);
        let theme = theme(cx);
        let (side, gap, bottom) = (theme.space.s28, theme.space.s10, theme.space.s12);
        let list = list(
            self.list.clone(),
            move |ix, window: &mut Window, cx: &mut App| {
                if ix == 0 {
                    top_drawn.set(true);
                }
                let content = match rows.get(ix) {
                    Some(Row::Stamp { label, .. }) => notices::stamp(label, cx),
                    Some(Row::Message {
                        index, starts_turn, ..
                    }) => {
                        let message = store::state(cx)
                            .conversations
                            .get(conversation.as_ref())
                            .and_then(|conversation| conversation.messages.get(*index))
                            .cloned();
                        match message {
                            Some(message) => {
                                message_view::row(&message, *starts_turn, &context, window, cx)
                            }
                            None => div().into_any_element(),
                        }
                    }
                    None => div().into_any_element(),
                };
                let last = ix + 1 == rows.len();
                div()
                    .w_full()
                    .px(r(side))
                    .pt(r(gap))
                    .when_last(last, r(bottom))
                    .child(content)
                    .into_any_element()
            },
        )
        .size_full();
        div()
            .semantic("chat.thread", Role::Log)
            .aria_label("Messages")
            .flex_1()
            .min_h_0()
            .w_full()
            .child(list)
            .into_any_element()
    }
}

trait LastPad: gpui::Styled + Sized {
    /// The thread's bottom padding, under its last row.
    fn when_last(self, last: bool, pad: gpui::Rems) -> Self {
        if last { self.pb(pad) } else { self }
    }
}

impl<E: gpui::Styled> LastPad for E {}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::model::MessageKind;

    fn message(id: &str, author: &str, at: &str) -> Message {
        Message {
            id: id.into(),
            conversation_id: "b1".into(),
            author: author.into(),
            kind: MessageKind::Text,
            body: "Hello".into(),
            data: None,
            status: MessageStatus::Done,
            created_at: at.into(),
        }
    }

    fn keys(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Stamp { .. } => "stamp".into(),
                Row::Message {
                    id, starts_turn, ..
                } => format!("{id}{}", if *starts_turn { "*" } else { "" }),
            })
            .collect()
    }

    #[test]
    fn a_turn_starts_after_a_label_or_with_another_author() {
        let now = Zoned::now();
        let messages = [
            message("m1", "you", "2026-09-27T07:14:00.000Z"),
            message("m2", "b1", "2026-09-27T07:14:30.000Z"),
            message("m3", "b1", "2026-09-27T07:15:00.000Z"),
            message("m4", "you", "2026-09-27T07:16:00.000Z"),
            // Fifteen minutes later: a new label, and a new turn even for the same author.
            message("m5", "you", "2026-09-27T07:31:00.000Z"),
        ];
        assert_eq!(
            keys(&rows(&messages, &now)),
            ["stamp", "m1*", "m2*", "m3", "m4*", "stamp", "m5*"]
        );
    }

    #[test]
    fn rows_keep_their_place_in_the_conversation() {
        let now = Zoned::now();
        let mut blank = message("m2", "b1", "2026-09-27T07:14:30.000Z");
        blank.body = "  ".into();
        let messages = [
            message("m1", "you", "2026-09-27T07:14:00.000Z"),
            blank,
            message("m3", "b1", "2026-09-27T07:15:00.000Z"),
        ];
        let places: Vec<usize> = rows(&messages, &now)
            .iter()
            .filter_map(|row| match row {
                Row::Message { index, .. } => Some(*index),
                Row::Stamp { .. } => None,
            })
            .collect();
        assert_eq!(
            places,
            [0, 2],
            "a blank reply has no row, and the rest keep their index"
        );
    }
}
