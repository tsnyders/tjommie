//! `rooms.test.ts`: the four cases, in order, with the same data and assertions.

use daemon::bots::{StoredBot, create_bot, delete_bot};
use daemon::chat::{NewMessage, add_message};
use daemon::queue::{NewWork, WorkSource, enqueue};
use daemon::rooms::{create_room, delete_room, get_room, list_rooms, room_members, update_room};
use protocol::ErrorCode;
use protocol::model::{
    BotInput, MessageKind, MessageStatus, Room, RoomInput, RoomPatch, TEAM_ROOM_ID,
};
use regex::Regex;
use rusqlite::Connection;
use serde_json::json;
use tempfile::TempDir;

/// `beforeEach`. `db` comes first, so it closes before its folder is removed. `_root` keeps the folder alive.
struct Fixture {
    db: Connection,
    alice: StoredBot,
    bob: StoredBot,
    _root: TempDir,
}

fn fixture() -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap();
    let db = daemon::db::open(&root.path().join("rooms.db")).unwrap();
    let bots_dir = root.path().join("bots");
    let alice = create_bot(&db, &bots_dir, &bot_input("Alice")).unwrap();
    let bob = create_bot(&db, &bots_dir, &bot_input("Bob")).unwrap();
    Fixture {
        db,
        alice,
        bob,
        _root: root,
    }
}

fn bot_input(name: &str) -> BotInput {
    serde_json::from_value(json!({
        "name": name,
        "instructions": "",
        "color": "blue",
        "shape": "circle",
        "provider": "claude",
        "model": "sonnet",
    }))
    .unwrap()
}

fn input<S: AsRef<str>>(name: &str, bot_ids: &[S]) -> RoomInput {
    RoomInput {
        name: name.to_owned(),
        bot_ids: bot_ids.iter().map(|id| id.as_ref().to_owned()).collect(),
    }
}

fn bot_ids_of(bots: Vec<StoredBot>) -> Vec<String> {
    bots.into_iter().map(|bot| bot.id).collect()
}

fn room_ids_of(rooms: Vec<Room>) -> Vec<String> {
    rooms.into_iter().map(|room| room.id).collect()
}

#[test]
fn creates_lists_updates_and_deletes_custom_rooms_while_everyone_contains_all_bots() {
    let f = fixture();
    let db = &f.db;
    let (alice, bob) = (&f.alice, &f.bob);
    let first = create_room(db, &input("Planning", &[&alice.id, &bob.id])).unwrap();
    let second = create_room(db, &input("Review", &[&bob.id])).unwrap();
    assert_eq!(first.name, "Planning");
    assert_eq!(first.bot_ids, vec![alice.id.clone(), bob.id.clone()]);
    assert!(!first.everyone);
    let created_at = first.created_at.clone().unwrap();
    assert!(
        Regex::new(r"^\d{4}-\d\d-\d\dT")
            .unwrap()
            .is_match(&created_at),
        "{created_at}"
    );
    assert_eq!(
        room_ids_of(list_rooms(db).unwrap()),
        vec![TEAM_ROOM_ID.to_owned(), first.id.clone(), second.id.clone()]
    );
    assert_eq!(
        get_room(db, TEAM_ROOM_ID).unwrap(),
        Room {
            id: TEAM_ROOM_ID.to_owned(),
            name: "Everyone".to_owned(),
            bot_ids: vec![alice.id.clone(), bob.id.clone()],
            everyone: true,
            created_at: None,
        }
    );
    assert_eq!(
        bot_ids_of(room_members(db, &first.id).unwrap()),
        vec![alice.id.clone(), bob.id.clone()]
    );
    let renamed = update_room(
        db,
        &first.id,
        &RoomPatch {
            name: Some("Plans".to_owned()),
            bot_ids: Some(vec![bob.id.clone(), bob.id.clone()]),
        },
    )
    .unwrap();
    assert_eq!(renamed.name, "Plans");
    assert_eq!(renamed.bot_ids, vec![bob.id.clone()]);
    assert_eq!(
        bot_ids_of(room_members(db, &first.id).unwrap()),
        vec![bob.id.clone()]
    );
    add_message(
        db,
        &NewMessage {
            conversation_id: first.id.clone(),
            author: "you".to_owned(),
            kind: MessageKind::Text,
            body: "Stored note".to_owned(),
            data: None,
            status: MessageStatus::Done,
        },
    )
    .unwrap();
    delete_room(db, &first.id).unwrap();
    let stored: i64 = db
        .query_row(
            "SELECT COUNT(*) AS n FROM messages WHERE conversation_id = ?1",
            [&first.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, 0);
    assert!(
        get_room(db, &first.id)
            .unwrap_err()
            .message
            .contains("No such room")
    );
    assert_eq!(
        room_ids_of(list_rooms(db).unwrap()),
        vec![TEAM_ROOM_ID.to_owned(), second.id.clone()]
    );
}

#[test]
fn rejects_unknown_members_duplicate_names_including_everyone_and_edits_to_everyone() {
    let f = fixture();
    let db = &f.db;
    let (alice, bob) = (&f.alice, &f.bob);
    assert_eq!(
        create_room(db, &input("Missing", &["missing"]))
            .unwrap_err()
            .code,
        ErrorCode::BadParams
    );
    assert_eq!(
        create_room(db, &input("Empty id", &[""]))
            .unwrap_err()
            .code,
        ErrorCode::BadParams
    );
    let room = create_room(db, &input("Planning", &[&alice.id])).unwrap();
    assert_eq!(
        create_room(db, &input("Duplicates", &[&alice.id, &alice.id]))
            .unwrap()
            .bot_ids,
        vec![alice.id.clone()]
    );
    assert_eq!(
        create_room(db, &input("pLaNnInG", &[&bob.id]))
            .unwrap_err()
            .code,
        ErrorCode::NameTaken
    );
    assert_eq!(
        create_room(db, &input("everyone", &[&bob.id]))
            .unwrap_err()
            .code,
        ErrorCode::NameTaken
    );
    assert_eq!(
        update_room(
            db,
            &room.id,
            &RoomPatch {
                bot_ids: Some(vec!["missing".to_owned()]),
                ..Default::default()
            },
        )
        .unwrap_err()
        .code,
        ErrorCode::BadParams
    );
    assert_eq!(
        update_room(
            db,
            TEAM_ROOM_ID,
            &RoomPatch {
                name: Some("Other".to_owned()),
                ..Default::default()
            },
        )
        .unwrap_err()
        .code,
        ErrorCode::BadState
    );
    assert_eq!(
        delete_room(db, TEAM_ROOM_ID).unwrap_err().code,
        ErrorCode::BadState
    );
}

#[test]
fn refuses_deletion_while_a_room_turn_is_queued() {
    let f = fixture();
    let db = &f.db;
    let room = create_room(db, &input("Planning", &[&f.alice.id])).unwrap();
    let item = enqueue(
        db,
        &NewWork {
            conversation_id: Some(room.id.clone()),
            ..NewWork::new(&f.alice.id, WorkSource::Room, "Work")
        },
    )
    .unwrap();
    assert_eq!(
        delete_room(db, &room.id).unwrap_err().code,
        ErrorCode::Busy
    );
    assert_eq!(get_room(db, &room.id).unwrap().name, "Planning");
    db.execute(
        "UPDATE work_items SET status = 'running' WHERE id = ?1",
        [&item.id],
    )
    .unwrap();
    assert_eq!(
        delete_room(db, &room.id).unwrap_err().code,
        ErrorCode::Busy
    );
    db.execute(
        "UPDATE work_items SET status = 'done' WHERE id = ?1",
        [&item.id],
    )
    .unwrap();
    delete_room(db, &room.id).unwrap();
}

#[test]
fn cascades_a_deleted_bot_out_of_room_membership() {
    let f = fixture();
    let db = &f.db;
    let room = create_room(db, &input("Planning", &[&f.alice.id, &f.bob.id])).unwrap();
    delete_bot(db, &f.alice.id).unwrap();
    assert_eq!(
        get_room(db, &room.id).unwrap().bot_ids,
        vec![f.bob.id.clone()]
    );
    assert_eq!(
        get_room(db, TEAM_ROOM_ID).unwrap().bot_ids,
        vec![f.bob.id.clone()]
    );
}
