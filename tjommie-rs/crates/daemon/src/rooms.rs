//! `rooms.ts`: rooms and their members, plus the `rooms.*` and `room.history` handlers from `daemon.ts`. The `room.post`
//! handler belongs to the team module, and `exchanges.get` is not here.

use std::collections::HashSet;
use std::sync::Arc;

use protocol::model::{Room, RoomInput, RoomPatch, TEAM_ROOM_ID};
use protocol::wire::{Empty, Id, method};
use protocol::{ErrorCode, Event};
use rusqlite::{Connection, OptionalExtension, params};

use crate::bots::{StoredBot, get_stored_bot, list_stored_bots};
use crate::chat::history_page;
use crate::context::Context;
use crate::ids::{new_id, now_iso};
use crate::server::{Handlers, RpcError};

/// Every room. Everyone comes first, then the rest by creation.
pub fn list_rooms(db: &Connection) -> Result<Vec<Room>, RpcError> {
    let ids = db
        .prepare("SELECT id FROM conversations WHERE kind = 'room' ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END, created_at, rowid")?
        .query_map([TEAM_ROOM_ID], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| get_room(db, id)).collect()
}

/// One room with its members. `NotFound` when no room has this id.
pub fn get_room(db: &Connection, id: &str) -> Result<Room, RpcError> {
    let (name, created_at) = db
        .query_row(
            "SELECT id, name, created_at FROM conversations WHERE id = ?1 AND kind = 'room'",
            [id],
            |row| Ok((row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?)),
        )
        .optional()?
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such room"))?;
    Ok(Room {
        id: id.to_owned(),
        name,
        bot_ids: room_members(db, id)?
            .into_iter()
            .map(|bot| bot.id)
            .collect(),
        everyone: id == TEAM_ROOM_ID,
        created_at,
    })
}

/// The bots in a room. Everyone holds every bot.
pub fn room_members(db: &Connection, room_id: &str) -> Result<Vec<StoredBot>, RpcError> {
    if room_id == TEAM_ROOM_ID {
        return list_stored_bots(db);
    }
    let ids = db
        .prepare("SELECT b.id FROM bots b JOIN room_members m ON m.bot_id = b.id WHERE m.room_id = ?1 ORDER BY b.created_at, b.rowid")?
        .query_map([room_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| get_stored_bot(db, id)).collect()
}

/// Makes the room and its members in one transaction, then reads it back.
pub fn create_room(db: &Connection, input: &RoomInput) -> Result<Room, RpcError> {
    assert_name_free(db, &input.name, None)?;
    let id = new_id();
    let transaction = db.unchecked_transaction()?;
    transaction.execute(
        "INSERT INTO conversations (id, kind, bot_id, name, created_at) VALUES (?1, 'room', NULL, ?2, ?3)",
        params![id, input.name, now_iso()],
    )?;
    set_members(&transaction, &id, &input.bot_ids)?;
    transaction.commit()?;
    get_room(db, &id)
}

/// Changes only the patched fields. Everyone can't be changed.
pub fn update_room(db: &Connection, id: &str, patch: &RoomPatch) -> Result<Room, RpcError> {
    editable(db, id)?;
    if let Some(name) = &patch.name {
        assert_name_free(db, name, Some(id))?;
    }
    let transaction = db.unchecked_transaction()?;
    if let Some(name) = &patch.name {
        transaction.execute(
            "UPDATE conversations SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
    }
    if let Some(bot_ids) = &patch.bot_ids {
        set_members(&transaction, id, bot_ids)?;
    }
    transaction.commit()?;
    get_room(db, id)
}

/// Deletes the room and, through its foreign keys, its messages. Refused while a project is active in the room or a bot
/// is still answering there.
pub fn delete_room(db: &Connection, id: &str) -> Result<(), RpcError> {
    editable(db, id)?;
    let project_active = db
        .query_row(
            "SELECT 1 FROM projects WHERE state='active' AND json_extract(data,'$.roomId')=?1 LIMIT 1",
            [id],
            |_| Ok(()),
        )
        .optional()?;
    if project_active.is_some() {
        return Err(RpcError::new(
            ErrorCode::Busy,
            "Archive or unlink the project before deleting its team room.",
        ));
    }
    let answering = db
        .query_row(
            "SELECT 1 FROM work_items WHERE conversation_id = ?1 AND status IN ('queued', 'running') LIMIT 1",
            [id],
            |_| Ok(()),
        )
        .optional()?;
    if answering.is_some() {
        return Err(RpcError::new(
            ErrorCode::Busy,
            "This room has a bot still answering",
        ));
    }
    db.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
    Ok(())
}

/// Everyone can't be changed. Any other room must exist.
fn editable(db: &Connection, id: &str) -> Result<(), RpcError> {
    if id == TEAM_ROOM_ID {
        return Err(RpcError::new(
            ErrorCode::BadState,
            "Everyone cannot be changed",
        ));
    }
    get_room(db, id).map(|_| ())
}

/// SQLite's `lower()` folds ASCII only, as the TypeScript does.
fn assert_name_free(db: &Connection, name: &str, except_id: Option<&str>) -> Result<(), RpcError> {
    let taken = db
        .query_row(
            "SELECT id FROM conversations WHERE kind = 'room' AND lower(name) = lower(?1) AND id IS NOT ?2",
            params![name, except_id],
            |_| Ok(()),
        )
        .optional()?;
    match taken {
        Some(()) => Err(RpcError::new(
            ErrorCode::NameTaken,
            format!("There's already a room called {name}"),
        )),
        None => Ok(()),
    }
}

/// Each bot once, in the order given. Every bot must exist before any membership row changes.
fn set_members(db: &Connection, room_id: &str, bot_ids: &[String]) -> Result<(), RpcError> {
    let mut seen = HashSet::new();
    let unique: Vec<&str> = bot_ids
        .iter()
        .map(String::as_str)
        .filter(|bot_id| seen.insert(*bot_id))
        .collect();
    for &bot_id in &unique {
        let known = db
            .query_row("SELECT 1 FROM bots WHERE id = ?1", [bot_id], |_| Ok(()))
            .optional()?;
        if known.is_none() {
            return Err(RpcError::new(
                ErrorCode::BadParams,
                "Room member: no such bot",
            ));
        }
    }
    db.execute("DELETE FROM room_members WHERE room_id = ?1", [room_id])?;
    for bot_id in unique {
        db.execute(
            "INSERT INTO room_members (room_id, bot_id) VALUES (?1, ?2)",
            [room_id, bot_id],
        )?;
    }
    Ok(())
}

/// The `rooms.*` methods and `room.history`. `cleanup_shared` is `cleanupSharedFiles()`: it runs after a room is deleted,
/// outside the database lock.
pub fn register(
    handlers: Handlers,
    context: &Context,
    cleanup_shared: Arc<dyn Fn() + Send + Sync>,
) -> Handlers {
    let (history, list, create, update, delete) = (
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
    );
    handlers
        .on::<method::RoomHistory, _>(move |params, _| {
            // daemon.ts:568: the room must exist before its page is read.
            let page = history.with_db(|db| {
                get_room(db, &params.room_id)?;
                history_page(db, &params.room_id, params.before.as_deref(), params.limit)
            });
            async move { page }
        })
        .on::<method::RoomsList, _>(move |_, _| {
            let rooms = list.with_db(list_rooms);
            async move { rooms }
        })
        .on::<method::RoomsCreate, _>(move |input, _| {
            let room = create.with_db(|db| create_room(db, &input));
            if let Ok(room) = &room {
                create.events.send(Event::RoomUpdated(room.clone()));
            }
            async move { room }
        })
        .on::<method::RoomsUpdate, _>(move |params, _| {
            let room = update.with_db(|db| update_room(db, &params.id, &params.patch));
            if let Ok(room) = &room {
                update.events.send(Event::RoomUpdated(room.clone()));
            }
            async move { room }
        })
        .on::<method::RoomsDelete, _>(move |Id { id }, _| {
            let deleted = delete.with_db(|db| delete_room(db, &id)).map(|()| {
                cleanup_shared();
                delete
                    .events
                    .send(Event::RoomDeleted(Id { id: id.clone() }));
                Empty {}
            });
            async move { deleted }
        })
}
