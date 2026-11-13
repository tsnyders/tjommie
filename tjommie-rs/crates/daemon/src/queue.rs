//! `queue.ts`: each bot's work, in the order its turns run.

use protocol::ErrorCode;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::bots::{from_text, text};
use crate::ids::{new_id, now_iso};
use crate::server::RpcError;

/// Where a piece of work came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkSource {
    You,
    Room,
    BotRequest,
    BotReply,
    Routine,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Queued,
    Running,
    Done,
    Failed,
    Interrupted,
}

impl WorkSource {
    /// Your messages and replies to a bot's own requests go first; requests from other bots and routine runs wait.
    fn priority(self) -> i64 {
        match self {
            WorkSource::You | WorkSource::Room | WorkSource::BotReply => 0,
            WorkSource::BotRequest | WorkSource::Routine | WorkSource::Task => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItem {
    pub id: String,
    pub bot_id: String,
    pub source: WorkSource,
    pub text: String,
    pub message_id: Option<String>,
    /// Where the turn's output goes: the bot's chat, "team" or an exchange.
    pub conversation_id: String,
    pub chain_id: Option<String>,
    pub exchange_id: Option<String>,
    pub routine_run_id: Option<String>,
    pub status: WorkStatus,
    pub created_at: String,
    pub task_attempt_id: Option<String>,
    pub task_revision: Option<i64>,
    pub execution_context_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWork {
    pub bot_id: String,
    pub source: WorkSource,
    pub text: String,
    pub message_id: Option<String>,
    /// The bot's own chat when unset.
    pub conversation_id: Option<String>,
    pub chain_id: Option<String>,
    pub exchange_id: Option<String>,
    pub routine_run_id: Option<String>,
    pub task_attempt_id: Option<String>,
    pub task_revision: Option<i64>,
    pub execution_context_key: Option<String>,
}

impl NewWork {
    pub fn new(bot_id: &str, source: WorkSource, text: &str) -> Self {
        Self {
            bot_id: bot_id.into(),
            source,
            text: text.into(),
            message_id: None,
            conversation_id: None,
            chain_id: None,
            exchange_id: None,
            routine_run_id: None,
            task_attempt_id: None,
            task_revision: None,
            execution_context_key: None,
        }
    }

    /// `workContext`: the item's task context, for work it causes.
    pub fn with_context_of(self, item: &WorkItem) -> Self {
        Self {
            task_attempt_id: item.task_attempt_id.clone(),
            task_revision: item.task_revision,
            execution_context_key: item.execution_context_key.clone(),
            ..self
        }
    }
}

/// A thrown `Error` in `queue.ts`: the client sees it as `internal`.
fn internal(message: String) -> RpcError {
    RpcError::new(ErrorCode::Internal, message)
}

const COLUMNS: &str = "id, bot_id, source, text, message_id, conversation_id, chain_id, exchange_id, routine_run_id, status, created_at, task_attempt_id, task_revision, execution_context_key";

/// A row and its `conversation_id`, which must not be null.
type WorkRow = (WorkItem, Option<String>);

fn read_row(row: &Row) -> rusqlite::Result<WorkRow> {
    let item = WorkItem {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        source: from_text(row, 2)?,
        text: row.get(3)?,
        message_id: row.get(4)?,
        conversation_id: String::new(),
        chain_id: row.get(6)?,
        exchange_id: row.get(7)?,
        routine_run_id: row.get(8)?,
        status: from_text(row, 9)?,
        created_at: row.get(10)?,
        task_attempt_id: row.get(11)?,
        task_revision: row.get(12)?,
        execution_context_key: row.get(13)?,
    };
    Ok((item, row.get(5)?))
}

fn to_item((item, conversation_id): WorkRow) -> Result<WorkItem, RpcError> {
    let Some(conversation_id) = conversation_id else {
        return Err(internal(format!(
            "Work item {} has a null conversation_id",
            item.id
        )));
    };
    Ok(WorkItem {
        conversation_id,
        ..item
    })
}

/// The first work item `filter` (the SQL after `FROM work_items`) finds.
fn one(
    db: &Connection,
    filter: &str,
    params: impl rusqlite::Params,
) -> Result<Option<WorkItem>, RpcError> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM work_items {filter}"),
        params,
        read_row,
    )
    .optional()?
    .map(to_item)
    .transpose()
}

pub fn enqueue(db: &Connection, work: &NewWork) -> Result<WorkItem, RpcError> {
    let has_context = work.task_attempt_id.is_some()
        || work.task_revision.is_some()
        || work.execution_context_key.is_some();
    let complete = work
        .task_attempt_id
        .as_deref()
        .is_some_and(|id| !id.is_empty())
        && work
            .execution_context_key
            .as_deref()
            .is_some_and(|key| !key.is_empty())
        && work.task_revision.is_some_and(|revision| revision >= 1);
    if has_context && !complete {
        return Err(internal(
            "A task work item needs its complete immutable execution context.".into(),
        ));
    }
    if work.source == WorkSource::Task && work.task_attempt_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(internal("A task work item needs an attempt.".into()));
    }
    let id = new_id();
    db.execute(
        "INSERT INTO work_items (id, bot_id, source, text, message_id, conversation_id, priority, chain_id, exchange_id, routine_run_id, task_attempt_id, task_revision, execution_context_key, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'queued', ?14)",
        params![
            id,
            work.bot_id,
            text(&work.source),
            work.text,
            work.message_id,
            work.conversation_id.as_ref().unwrap_or(&work.bot_id),
            work.source.priority(),
            work.chain_id,
            work.exchange_id,
            work.routine_run_id,
            work.task_attempt_id,
            work.task_revision,
            work.execution_context_key,
            now_iso(),
        ],
    )?;
    get_work_item(db, &id)
}

pub fn get_work_item(db: &Connection, id: &str) -> Result<WorkItem, RpcError> {
    one(db, "WHERE id = ?1", [id])?.ok_or_else(|| internal(format!("No work item {id}")))
}

/// Highest priority first, then oldest. rowid breaks ties within one millisecond.
pub fn next_queued(db: &Connection, bot_id: &str) -> Result<Option<WorkItem>, RpcError> {
    one(
        db,
        "WHERE bot_id = ?1 AND status = 'queued' ORDER BY priority, created_at, rowid LIMIT 1",
        [bot_id],
    )
}

/// Work that may join a turn already writing to `conversation_id`: your messages there, and replies to the bot's own
/// requests, from the same task context.
pub fn next_steerable(
    db: &Connection,
    bot_id: &str,
    conversation_id: &str,
    execution_context_key: Option<&str>,
) -> Result<Option<WorkItem>, RpcError> {
    one(
        db,
        "WHERE bot_id = ?1 AND status = 'queued' AND source IN ('you', 'bot_reply') AND conversation_id = ?2 AND execution_context_key IS ?3
         ORDER BY priority, created_at, rowid LIMIT 1",
        params![bot_id, conversation_id, execution_context_key],
    )
}

pub fn set_status(db: &Connection, id: &str, status: WorkStatus) -> Result<(), RpcError> {
    match status {
        WorkStatus::Queued => db.execute("UPDATE work_items SET status = 'queued', started_at = NULL, ended_at = NULL WHERE id = ?1", [id])?,
        WorkStatus::Running => db.execute("UPDATE work_items SET status = 'running', started_at = ?1 WHERE id = ?2", [&now_iso(), id])?,
        _ => db.execute("UPDATE work_items SET status = ?1, ended_at = ?2 WHERE id = ?3", [&text(&status), &now_iso(), id])?,
    };
    Ok(())
}

pub fn last_item_for_message(
    db: &Connection,
    message_id: &str,
    bot_id: Option<&str>,
) -> Result<Option<WorkItem>, RpcError> {
    match bot_id {
        None => one(
            db,
            "WHERE message_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [message_id],
        ),
        Some(bot_id) => one(
            db,
            "WHERE message_id = ?1 AND bot_id = ?2 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [message_id, bot_id],
        ),
    }
}

/// After a restart nothing is running any more: mark what was running as interrupted and return it.
pub fn recover_after_restart(db: &Connection) -> Result<Vec<WorkItem>, RpcError> {
    let rows = db
        .prepare(&format!(
            "SELECT {COLUMNS} FROM work_items WHERE status = 'running' ORDER BY created_at, rowid"
        ))?
        .query_map([], read_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    db.execute(
        "UPDATE work_items SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'",
        [now_iso()],
    )?;
    rows.into_iter()
        .map(|row| {
            to_item(row).map(|item| WorkItem {
                status: WorkStatus::Interrupted,
                ..item
            })
        })
        .collect()
}

/// A chain is one message from you, one room post or one routine run, plus everything it causes.
pub fn new_chain(db: &Connection) -> Result<String, RpcError> {
    let id = new_id();
    db.execute(
        "INSERT INTO chains (id, requests_made, created_at) VALUES (?1, 0, ?2)",
        [&id, &now_iso()],
    )?;
    Ok(id)
}

/// Counts one bot-to-bot request against the chain. False once the chain has used `limit` requests.
pub fn use_chain_request(db: &Connection, chain_id: &str, limit: i64) -> Result<bool, RpcError> {
    let changed = db.execute(
        "UPDATE chains SET requests_made = requests_made + 1 WHERE id = ?1 AND requests_made < ?2",
        params![chain_id, limit],
    )?;
    Ok(changed == 1)
}
