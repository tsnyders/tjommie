//! `chat.ts`: messages in a conversation, in order.

use protocol::ErrorCode;
use protocol::model::{Message, MessageKind, MessageStatus};
use protocol::wire::MessagePage;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Map, Value};

use crate::bots::{from_text, text};
use crate::ids::{new_id, now_iso};
use crate::providers::js;
use crate::server::RpcError;

#[derive(Debug, Clone, PartialEq)]
pub struct NewMessage {
    pub conversation_id: String,
    pub author: String,
    pub kind: MessageKind,
    pub body: String,
    pub data: Option<Map<String, Value>>,
    pub status: MessageStatus,
}

/// Only what is set changes. `data: Some(None)` clears the data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessagePatch {
    pub body: Option<String>,
    pub data: Option<Option<Map<String, Value>>>,
    pub status: Option<MessageStatus>,
}

const COLUMNS: &str = "id, conversation_id, author, kind, body, data, status, created_at";

fn not_found() -> RpcError {
    RpcError::new(ErrorCode::NotFound, "No such message")
}

fn read_message(row: &Row) -> rusqlite::Result<Message> {
    let data: Option<String> = row.get(5)?;
    let data = data
        .map(|data| serde_json::from_str(&data))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(Message {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        author: row.get(2)?,
        kind: from_text(row, 3)?,
        body: row.get(4)?,
        data,
        status: from_text(row, 6)?,
        created_at: row.get(7)?,
    })
}

fn stored_data(data: Option<&Map<String, Value>>) -> Option<String> {
    data.map(|data| Value::Object(data.clone()).to_string())
}

pub fn add_message(db: &Connection, message: &NewMessage) -> Result<Message, RpcError> {
    let id = new_id();
    let seq: i64 = db.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE conversation_id = ?1",
        [&message.conversation_id],
        |row| row.get(0),
    )?;
    db.execute(
        "INSERT INTO messages (id, conversation_id, author, kind, body, data, status, created_at, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            message.conversation_id,
            message.author,
            text(&message.kind),
            message.body,
            stored_data(message.data.as_ref()),
            text(&message.status),
            now_iso(),
            seq,
        ],
    )?;
    get_message(db, &id)
}

pub fn get_message(db: &Connection, id: &str) -> Result<Message, RpcError> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM messages WHERE id = ?1"),
        [id],
        read_message,
    )
    .optional()?
    .ok_or_else(not_found)
}

pub fn update_message(
    db: &Connection,
    id: &str,
    patch: &MessagePatch,
) -> Result<Message, RpcError> {
    let current = get_message(db, id)?;
    let data = patch.data.as_ref().unwrap_or(&current.data);
    db.execute(
        "UPDATE messages SET body = ?1, data = ?2, status = ?3 WHERE id = ?4",
        params![
            patch.body.as_ref().unwrap_or(&current.body),
            stored_data(data.as_ref()),
            text(&patch.status.unwrap_or(current.status)),
            id,
        ],
    )?;
    get_message(db, id)
}

/// The newest `limit` messages before the message `before`, oldest first.
pub fn history(
    db: &Connection,
    conversation_id: &str,
    before: Option<&str>,
    limit: u32,
) -> Result<Vec<Message>, RpcError> {
    let cursor = match before {
        None => i64::MAX,
        Some(before) => db
            .query_row(
                "SELECT seq FROM messages WHERE id = ?1 AND conversation_id = ?2",
                [before, conversation_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(not_found)?,
    };
    let mut messages = db
        .prepare(&format!("SELECT {COLUMNS} FROM messages WHERE conversation_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3"))?
        .query_map(params![conversation_id, cursor, limit], read_message)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    messages.reverse();
    Ok(messages)
}

/// Reads one extra message so a full final page still reports `has_more: false`.
pub fn history_page(
    db: &Connection,
    conversation_id: &str,
    before: Option<&str>,
    limit: u32,
) -> Result<MessagePage, RpcError> {
    let mut messages = history(db, conversation_id, before, limit.saturating_add(1))?;
    let has_more = messages.len() > limit as usize;
    if has_more {
        messages.remove(0);
    }
    Ok(MessagePage { messages, has_more })
}

/// Opening text for a fresh CLI session, so the bot picks up an ongoing chat where it left off.
pub fn build_recap(messages: &[Message], bot_name: &str) -> String {
    let lines: Vec<String> = messages
        .iter()
        .filter(|message| message.kind == MessageKind::Text && !js::trim(&message.body).is_empty())
        .map(|message| {
            let speaker = if message.author == "you" {
                "Person"
            } else {
                bot_name
            };
            format!("{speaker}: {}", js::slice(js::trim(&message.body), 2000))
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let header = "[Tjommie: this is a new session of an ongoing chat. Your notes are in notes.md in your home folder. Recent messages, oldest first:";
    [header.to_owned()]
        .into_iter()
        .chain(lines)
        .chain(["]".to_owned()])
        .collect::<Vec<_>>()
        .join("\n")
}
