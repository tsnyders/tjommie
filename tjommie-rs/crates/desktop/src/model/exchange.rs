//! Port of `packages/ui/lib/exchange.ts`: the exchange view's meta line and caption, and an exchange's status as the chat's
//! handoff row shows it.

use jiff::{Timestamp, Zoned};
use protocol::model::{
    Bot, Exchange, ExchangeStatus, HandoffData, Message, MessageData, MessageKind, message_data,
};

use super::format::stamp;
use super::labels::bot_name;

/// Milliseconds since the epoch, or `None` for a time that doesn't parse.
fn millis(value: &str) -> Option<i64> {
    value
        .parse::<Timestamp>()
        .ok()
        .map(|at| at.as_millisecond())
}

/// "1 minute", "2 minutes".
fn counted(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("{count} {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// How long the answer took, in words: "less than a minute", "1 minute", "2 hours". JavaScript's `Math.round` halves
/// go up, so 30 seconds is a minute.
fn waited(milliseconds: i64) -> String {
    let minutes = (milliseconds + 30_000).div_euclid(60_000);
    if minutes < 1 {
        return "less than a minute".to_owned();
    }
    if minutes < 60 {
        return counted(minutes, "minute");
    }
    counted((minutes + 30).div_euclid(60), "hour")
}

/// The exchange view's meta line: "Today 9:31 AM · Replied after 2 minutes".
pub fn exchange_meta(exchange: &Exchange, now: &Zoned) -> String {
    let asked = stamp(&exchange.created_at, now);
    match exchange.status {
        ExchangeStatus::Waiting => format!("{asked} · Waiting for a reply"),
        ExchangeStatus::Failed => format!(
            "{asked} · No reply: {}",
            exchange.reply.as_deref().unwrap_or("something went wrong")
        ),
        ExchangeStatus::Replied => {
            // No duration when a time doesn't parse (JavaScript would show NaN).
            let took = match (
                exchange.replied_at.as_deref().and_then(millis),
                millis(&exchange.created_at),
            ) {
                (Some(replied), Some(created)) => replied - created,
                _ => 0,
            };
            format!("{asked} · Replied after {}", waited(took))
        }
    }
}

/// The line under a replied exchange: where the answer, and any files, went. `None` until it's replied.
pub fn exchange_caption(exchange: &Exchange, messages: &[Message], bots: &[Bot]) -> Option<String> {
    if exchange.status != ExchangeStatus::Replied {
        return None;
    }
    let from = bot_name(bots, &exchange.from_bot_id);
    let files = messages
        .iter()
        .filter(|message| message.kind == MessageKind::File)
        .count();
    Some(match files {
        0 => format!("Sent back to {from}."),
        1 => format!("Sent back to {from}. The file is in {from}'s inbox."),
        _ => format!("Sent back to {from}. The files are in {from}'s inbox."),
    })
}

/// An exchange's status from its chat's handoff row. The daemon updates the row when the exchange is answered or fails,
/// after the exchange's own last message, so an open exchange view fetches the exchange again when this changes.
pub fn handoff_status(messages: &[Message], exchange_id: &str) -> Option<ExchangeStatus> {
    messages
        .iter()
        .find_map(|message| match message_data(message) {
            Ok(MessageData::Handoff(HandoffData::Exchange(row)))
                if row.exchange_id == exchange_id =>
            {
                Some(row.status)
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use jiff::Zoned;
    use protocol::model::{Bot, Exchange, ExchangeStatus, Message};
    use serde_json::{Value, json};

    use super::*;

    fn now() -> Zoned {
        "2026-09-27T14:00:00[UTC]".parse().unwrap()
    }

    fn message(value: Value) -> Message {
        serde_json::from_value(value).unwrap()
    }

    fn bot(id: &str, name: &str) -> Bot {
        serde_json::from_value(json!({
            "id": id, "name": name, "color": "orange", "shape": "circle", "instructions": "",
            "provider": "claude", "model": "sonnet", "autonomy": "ask", "homeFolder": "C:/b",
            "projectFolder": null, "browser": false, "paused": false, "canMessage": [],
            "state": "idle", "setupReason": null, "limitedUntil": null, "createdAt": "2026-09-27T00:00:00Z"
        }))
        .unwrap()
    }

    fn bots() -> Vec<Bot> {
        vec![bot("b1", "Pnet apply"), bot("b2", "CV Writer")]
    }

    /// An exchange from Pnet apply to CV Writer, asked at 9:31.
    fn exchange(status: ExchangeStatus, reply: Option<&str>, replied_at: Option<&str>) -> Exchange {
        Exchange {
            id: "x1".to_owned(),
            from_bot_id: "b1".to_owned(),
            to_bot_id: "b2".to_owned(),
            request: "Tailor the CV for listing PN-48213".to_owned(),
            reply: reply.map(str::to_owned),
            status,
            created_at: "2026-09-27T09:31:00.000Z".to_owned(),
            replied_at: replied_at.map(str::to_owned),
        }
    }

    fn file_message() -> Message {
        message(json!({
            "id": "f1", "conversationId": "x1", "author": "b2", "kind": "file",
            "body": "CV-Sabenza-Angular.pdf",
            "data": {"path": r"C:\bots\b1\inbox\CV-Sabenza-Angular.pdf", "name": "CV-Sabenza-Angular.pdf", "size": 188416},
            "status": "done", "createdAt": "2026-09-27T14:00:00.000Z"
        }))
    }

    fn row(id: &str, data: Value) -> Message {
        message(json!({
            "id": id, "conversationId": "b1", "author": "b1", "kind": "handoff",
            "body": "Tailor the CV", "data": data, "status": "done", "createdAt": "2026-09-27T14:00:00.000Z"
        }))
    }

    #[test]
    fn says_when_the_exchange_was_asked_and_how_it_ended() {
        assert_eq!(
            exchange_meta(&exchange(ExchangeStatus::Waiting, None, None), &now()),
            "Today 9:31 AM · Waiting for a reply"
        );
        assert_eq!(
            exchange_meta(
                &exchange(
                    ExchangeStatus::Replied,
                    Some("Done."),
                    Some("2026-09-27T09:33:00.000Z")
                ),
                &now()
            ),
            "Today 9:31 AM · Replied after 2 minutes"
        );
        assert_eq!(
            exchange_meta(
                &exchange(
                    ExchangeStatus::Failed,
                    Some("it was stopped"),
                    Some("2026-09-27T09:32:00.000Z")
                ),
                &now()
            ),
            "Today 9:31 AM · No reply: it was stopped"
        );
    }

    #[test]
    fn says_how_long_the_answer_took_in_words_rounding_halves_up_like_javascript() {
        let took = |replied_at: &str| {
            exchange_meta(
                &exchange(ExchangeStatus::Replied, Some("Done."), Some(replied_at)),
                &now(),
            )
        };
        assert_eq!(
            took("2026-09-27T09:31:29.000Z"),
            "Today 9:31 AM · Replied after less than a minute"
        );
        assert_eq!(
            took("2026-09-27T09:31:30.000Z"),
            "Today 9:31 AM · Replied after 1 minute"
        );
        assert_eq!(
            took("2026-09-27T11:00:00.000Z"),
            "Today 9:31 AM · Replied after 1 hour"
        );
        assert_eq!(
            took("2026-09-27T11:01:00.000Z"),
            "Today 9:31 AM · Replied after 2 hours"
        );
    }

    #[test]
    fn says_where_the_answer_and_its_files_went() {
        let replied = exchange(ExchangeStatus::Replied, Some("Done."), None);
        assert_eq!(
            exchange_caption(&replied, &[], &bots()),
            Some("Sent back to Pnet apply.".to_owned())
        );
        assert_eq!(
            exchange_caption(&replied, &[file_message()], &bots()),
            Some("Sent back to Pnet apply. The file is in Pnet apply's inbox.".to_owned())
        );
        assert_eq!(
            exchange_caption(
                &exchange(ExchangeStatus::Waiting, None, None),
                &[file_message()],
                &bots()
            ),
            None
        );
    }

    #[test]
    fn reads_an_exchanges_status_from_a_chats_handoff_row_which_the_daemon_updates_when_its_answered()
     {
        let messages = [
            file_message(),
            row("h1", json!({"roomMessageId": "r1"})),
            row(
                "h2",
                json!({"exchangeId": "x0", "fromBotId": "b1", "toBotId": "b2", "messages": 2, "status": "replied"}),
            ),
            row(
                "h3",
                json!({"exchangeId": "x1", "fromBotId": "b1", "toBotId": "b2", "messages": 1, "status": "waiting"}),
            ),
            // Doesn't match the schema, so it's not read.
            row("h4", json!({"exchangeId": "x2", "status": "done"})),
        ];
        assert_eq!(
            handoff_status(&messages, "x1"),
            Some(ExchangeStatus::Waiting)
        );
        assert_eq!(
            handoff_status(&messages, "x0"),
            Some(ExchangeStatus::Replied)
        );
        assert_eq!(handoff_status(&messages, "x2"), None);
        assert_eq!(handoff_status(&messages, "x9"), None);
    }
}
