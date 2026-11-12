//! `turn-notices.ts`: the phone notice a finished turn earns, once its whole chain is done.

use protocol::wire::Notify;
use rusqlite::{Connection, OptionalExtension};

use crate::bots::get_stored_bot;
use crate::queue::{WorkItem, WorkSource, get_work_item};
use crate::server::RpcError;

pub use crate::runner::{TurnOutcome, TurnResult};

pub fn turn_notice(
    db: &Connection,
    item: &WorkItem,
    result: &TurnResult,
) -> Result<Option<Notify>, RpcError> {
    if matches!(item.source, WorkSource::Routine | WorkSource::BotRequest)
        || result.outcome == TurnOutcome::Interrupted
    {
        return Ok(None);
    }
    // A chain can be assigned during the turn, so a done turn reads it again.
    let chain_id = match (result.outcome, &item.chain_id) {
        (TurnOutcome::Done, Some(chain_id)) => Some(chain_id.clone()),
        (TurnOutcome::Done, None) => get_work_item(db, &item.id)?.chain_id,
        _ => None,
    };
    if let Some(chain_id) = chain_id {
        let busy: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_items WHERE chain_id = ?1 AND id <> ?2 AND status IN ('queued', 'running'))
                 OR EXISTS(SELECT 1 FROM work_items WHERE chain_id = ?1 AND routine_run_id IS NOT NULL)
                 OR EXISTS(SELECT 1 FROM exchanges WHERE chain_id = ?1 AND status = 'waiting')",
            [&chain_id, &item.id],
            |row| row.get(0),
        )?;
        if busy {
            return Ok(None);
        }
    }
    let bot = get_stored_bot(db, &item.bot_id)?;
    let room: Option<String> = db
        .query_row(
            "SELECT name FROM conversations WHERE id = ?1 AND kind = 'room'",
            [&item.conversation_id],
            |row| row.get(0),
        )
        .optional()?;
    let room_id = room.as_ref().map(|_| item.conversation_id.clone());
    Ok(Some(match result.outcome {
        TurnOutcome::Failed => Notify::Failed {
            title: format!("{} ran into a problem", bot.name),
            body: result
                .error
                .clone()
                .unwrap_or_else(|| "Something went wrong.".into()),
            bot_id: bot.id,
            room_id,
        },
        _ => Notify::Done {
            title: room.map_or_else(
                || format!("{} is done", bot.name),
                |room| format!("Done in {room}"),
            ),
            body: result
                .text
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(140)
                .collect(),
            bot_id: bot.id,
            room_id,
        },
    }))
}
