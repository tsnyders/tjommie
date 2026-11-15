//! Port of `packages/ui/lib/timeline.ts`: a chat's messages with a time label before the first one, after a gap of 15
//! minutes or more, and at each new day. Days are calendar days in the zone of `now`, as `format::stamp` reads them.

use jiff::{Timestamp, Zoned};
use protocol::model::Message;

use super::format::stamp;

const GAP_MS: i64 = 15 * 60_000;

/// One row of the chat: a time label, or a message.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineItem<'a> {
    Stamp { key: String, label: String },
    Message { key: String, message: &'a Message },
}

/// The chat's rows: a label before each message that starts a group. A message starts one when it's the first, when it
/// comes 15 minutes or more after the one before, or when it's on a new day. A time that doesn't parse starts one too,
/// as JavaScript's `Invalid Date` never matches the day before.
pub fn timeline<'a>(messages: &'a [Message], now: &Zoned) -> Vec<TimelineItem<'a>> {
    let zone = now.time_zone().clone();
    let mut items = Vec::new();
    let mut previous: Option<Timestamp> = None;
    for message in messages {
        let at = message.created_at.parse::<Timestamp>().ok();
        let starts_group = match (previous, at) {
            (Some(before), Some(current)) => {
                current.as_millisecond() - before.as_millisecond() >= GAP_MS
                    || current.to_zoned(zone.clone()).date() != before.to_zoned(zone.clone()).date()
            }
            _ => true,
        };
        if starts_group {
            items.push(TimelineItem::Stamp {
                key: format!("stamp-{}", message.id),
                label: stamp(&message.created_at, now),
            });
        }
        items.push(TimelineItem::Message {
            key: message.id.clone(),
            message,
        });
        previous = at;
    }
    items
}

#[cfg(test)]
mod tests {
    use jiff::Zoned;
    use protocol::model::Message;
    use serde_json::{Value, json};

    use super::*;

    fn message(value: Value) -> Message {
        serde_json::from_value(value).unwrap()
    }

    fn text(id: &str, created_at: &str) -> Message {
        message(json!({
            "id": id, "conversationId": "b1", "author": "b1", "kind": "text", "body": id,
            "data": null, "status": "done", "createdAt": created_at
        }))
    }

    #[test]
    fn labels_the_first_message_gaps_of_15_minutes_or_more_and_each_new_day() {
        let now: Zoned = "2026-09-27T12:00:00[UTC]".parse().unwrap();
        let at = |day: u32, hour: u32, minute: u32| {
            format!("2026-09-{day:02}T{hour:02}:{minute:02}:00.000Z")
        };
        let messages = [
            text("a", &at(26, 9, 14)),
            text("b", &at(27, 9, 14)),
            text("c", &at(27, 9, 20)),
            text("d", &at(27, 9, 40)),
        ];
        let items = timeline(&messages, &now);
        let labels: Vec<String> = items
            .iter()
            .map(|item| match item {
                TimelineItem::Stamp { label, .. } => label.clone(),
                TimelineItem::Message { message, .. } => message.id.clone(),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Yesterday 9:14 AM",
                "a",
                "Today 9:14 AM",
                "b",
                "c",
                "Today 9:40 AM",
                "d"
            ]
        );
        assert!(
            matches!(&items[0], TimelineItem::Stamp { key, .. } if key == "stamp-a"),
            "a label's key is its message's id with a prefix"
        );
    }
}
