//! `chat.test.ts`

mod common;

use common::{TempRoot, new_bot, test_db};
use daemon::chat::{
    MessagePatch, NewMessage, add_message, build_recap, get_message, history, history_page,
    update_message,
};
use protocol::ErrorCode;
use protocol::model::{Message, MessageKind, MessageStatus};
use protocol::wire::MessagePage;
use rusqlite::Connection;
use serde_json::{Map, Value, json};

struct Fixture {
    db: Connection,
    bot_id: String,
    root: TempRoot,
}

fn setup() -> Fixture {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    let bot_id = new_bot(&db, &root.0.join("bots"), "Writer", "codex").id;
    Fixture { db, bot_id, root }
}

fn text(conversation_id: &str, body: &str) -> NewMessage {
    NewMessage {
        conversation_id: conversation_id.into(),
        author: "you".into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status: MessageStatus::Done,
    }
}

fn object(value: Value) -> Map<String, Value> {
    let Value::Object(map) = value else {
        panic!("not an object")
    };
    map
}

#[test]
fn pages_through_history_newest_page_first_each_page_oldest_first() {
    let f = setup();
    let ids: Vec<String> = ["one", "two", "three"]
        .map(|body| add_message(&f.db, &text(&f.bot_id, body)).unwrap().id)
        .into();
    let bodies = |messages: Vec<Message>| {
        messages
            .into_iter()
            .map(|message| message.body)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        bodies(history(&f.db, &f.bot_id, None, 2).unwrap()),
        ["two", "three"]
    );
    assert_eq!(
        bodies(history(&f.db, &f.bot_id, Some(&ids[1]), 50).unwrap()),
        ["one"]
    );
}

#[test]
fn rejects_a_history_cursor_from_another_conversation() {
    let f = setup();
    let other = new_bot(&f.db, &f.root.0.join("bots"), "Reader", "codex");
    add_message(&f.db, &text(&f.bot_id, "one")).unwrap();
    let cursor = add_message(&f.db, &text(&other.id, "other")).unwrap();
    let error = history(&f.db, &f.bot_id, Some(&cursor.id), 20).unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (ErrorCode::NotFound, "No such message")
    );
}

#[test]
fn reports_more_history_only_when_an_earlier_message_remains() {
    let f = setup();
    let page = |before: Option<&str>, limit| history_page(&f.db, &f.bot_id, before, limit).unwrap();
    assert_eq!(
        page(None, 2),
        MessagePage {
            messages: vec![],
            has_more: false
        }
    );
    let messages: Vec<Message> = ["one", "two", "three", "four"]
        .map(|body| add_message(&f.db, &text(&f.bot_id, body)).unwrap())
        .into();
    let newest = page(None, 2);
    assert_eq!(
        newest,
        MessagePage {
            messages: messages[2..].to_vec(),
            has_more: true
        }
    );
    assert_eq!(
        page(Some(&newest.messages[0].id), 2),
        MessagePage {
            messages: messages[..2].to_vec(),
            has_more: false
        }
    );
    assert_eq!(
        page(Some(&messages[0].id), 2),
        MessagePage {
            messages: vec![],
            has_more: false
        }
    );
    assert_eq!(
        page(None, 10),
        MessagePage {
            messages,
            has_more: false
        }
    );
}

#[test]
fn rejects_a_history_page_cursor_from_another_conversation() {
    let f = setup();
    let other = new_bot(&f.db, &f.root.0.join("bots"), "Reader", "codex");
    let cursor = add_message(&f.db, &text(&other.id, "other")).unwrap();
    assert_eq!(
        history_page(&f.db, &f.bot_id, Some(&cursor.id), 2)
            .unwrap_err()
            .message,
        "No such message"
    );
}

#[test]
fn updates_body_status_and_data_and_keeps_data_when_an_update_doesnt_mention_it() {
    let f = setup();
    let files = object(json!({ "files": ["a.pdf"] }));
    let new = NewMessage {
        author: f.bot_id.clone(),
        body: String::new(),
        data: Some(files.clone()),
        status: MessageStatus::Streaming,
        ..text(&f.bot_id, "")
    };
    let message = add_message(&f.db, &new).unwrap();
    let updated = update_message(
        &f.db,
        &message.id,
        &MessagePatch {
            body: Some("Done.".into()),
            status: Some(MessageStatus::Done),
            data: None,
        },
    )
    .unwrap();
    assert_eq!(
        (updated.body.as_str(), updated.status, updated.data),
        ("Done.", MessageStatus::Done, Some(files))
    );
    assert_eq!(
        update_message(
            &f.db,
            &message.id,
            &MessagePatch {
                data: Some(None),
                ..MessagePatch::default()
            }
        )
        .unwrap()
        .data,
        None
    );
    let error = get_message(&f.db, "missing").unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (ErrorCode::NotFound, "No such message")
    );
}

#[test]
fn builds_a_recap_from_text_messages_only_oldest_first() {
    let message = |author: &str, kind, body: &str| Message {
        id: body.into(),
        conversation_id: "c".into(),
        author: author.into(),
        kind,
        body: body.into(),
        data: None,
        status: MessageStatus::Done,
        created_at: String::new(),
    };
    let recap = build_recap(
        &[
            message("you", MessageKind::Text, "Remember the word ZEBRA."),
            message("bot-1", MessageKind::Activity, "1 step: Read notes.md"),
            message("bot-1", MessageKind::Text, "Noted."),
        ],
        "Writer",
    );
    assert!(
        recap.contains("Person: Remember the word ZEBRA.\nWriter: Noted."),
        "{recap}"
    );
    assert!(!recap.contains("1 step"));
    assert_eq!(build_recap(&[], "Writer"), "");
}
