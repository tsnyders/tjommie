//! `team.ts`: bots working together. The tools on Tjommie's MCP server (in [`tools`]), exchanges between bots, and rooms
//! (Everyone and the others), plus the `room.post` and `exchanges.get` handlers from `daemon.ts`.
//!
//! **Locks.** `Team` has no database of its own. The runner calls its hooks ([`Team::turn_ended`], [`Team::bot_deleting`],
//! …) inside the runner's locks, with a [`Core`], so every method takes a [`TeamHost`]: the database the caller already
//! holds, plus the few runner operations `Team` needs. The MCP tools, which run on a bare tokio task, reach the same
//! host through a [`HostAccess`] (the runner's `with`). A method never calls `deliver` or another runner operation while
//! it holds a borrow of the database, and events go out only after the transaction that caused them commits.
//!
//! **Shared files.** `Attachments` locks the database itself, which the host already holds, so `Team` uses the
//! [`SharedAttachments`] trait, whose methods take the connection.

mod files;
mod tools;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::SystemTime;

use protocol::ErrorCode;
use protocol::Event;
use protocol::attachments::{AttachmentCompanion, AttachmentCompanions, AttachmentInfo};
use protocol::mentions::find_mentions;
use protocol::model::{
    Approval, ApprovalDecision, ApprovalKind, ApprovalStatus, Bot, BotState, Exchange,
    ExchangeFile, ExchangeHandoffData, ExchangePostData, ExchangeStatus, FileData, Message,
    MessageKind, MessageStatus, NoticeData, RoomHandoffData, RoutineApprovalData, RoutineInput,
    TEAM_ROOM_ID,
};
use protocol::wire::{ExchangeView, Id, method};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::approvals::{get_approval, resolve_approval};
use crate::bots::{StoredBot, from_text, get_stored_bot, list_stored_bots, text};
use crate::chat::{
    MessagePatch, NewMessage, add_message, get_message, history, history_page, update_message,
};
use crate::context::Context;
use crate::ids::{new_id, now_iso};
use crate::providers::js;
use crate::queue::{NewWork, WorkItem, WorkSource, enqueue, new_chain, set_status};
use crate::rooms::{get_room, room_members};
use crate::routines::{create_routine, now_ms};
use crate::runner::{Broadcast, Core, Log, Runner, TurnOutcome, TurnResult, copy_into_inbox};
use crate::server::{Handlers, RpcError};

pub use files::file_inside;
use files::{first_line, snapshot_folder, sweep_stale};
pub use tools::{CompanionArg, RoutineProposal};

/// Room messages given to a bot that is mentioned in a room.
const ROOM_CONTEXT: u32 = 20;

/// A message a bot reads, as an error. TypeScript's `throw new Error(message)`; only the text matters.
fn plain(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::Internal, message)
}

/// What `Team` needs from the runner, and the database the caller holds.
pub trait TeamHost {
    fn db(&self) -> &Connection;
    /// `deliver`: hands work already in the queue to the runner.
    fn deliver(&mut self, work: &WorkItem) -> Result<(), String>;
    /// `stopItem`: stops one item, saying why. Not waited for; a failure is logged.
    fn stop_item(&mut self, item_id: &str, notice: &str);
    fn current_work(&self, bot_id: &str) -> Option<WorkItem>;
    /// `botView`: the wire bot, with its state.
    fn to_bot(&mut self, bot: StoredBot) -> Bot;
    /// `effectiveFolder`: the folder the bot works in now (a task's worktree), if the host knows one.
    fn effective_folder(&self, bot_id: &str) -> Result<Option<String>, RpcError>;
}

impl TeamHost for Core<'_> {
    fn db(&self) -> &Connection {
        Core::db(self)
    }

    fn deliver(&mut self, work: &WorkItem) -> Result<(), String> {
        Core::deliver(self, work.clone())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn stop_item(&mut self, item_id: &str, notice: &str) {
        let stopping = Core::stop_item(self, item_id, notice);
        tokio::spawn(async move {
            if let Err(error) = stopping.await {
                eprintln!("Tjommie: could not stop a deleted bot's request: {error}");
            }
        });
    }

    fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        Core::current_work(self, bot_id)
    }

    fn to_bot(&mut self, bot: StoredBot) -> Bot {
        Core::to_bot(self, bot)
    }

    fn effective_folder(&self, bot_id: &str) -> Result<Option<String>, RpcError> {
        Core::effective_folder(self, bot_id).map(Some)
    }
}

/// A way to a [`TeamHost`] from outside a runner hook: the MCP tools, which run on their own task.
pub trait HostAccess: Send + Sync {
    /// Runs `work` with the host, the runner's locks held as they are inside a hook.
    fn with_host(&self, work: &mut dyn FnMut(&mut dyn TeamHost));
}

impl HostAccess for Runner {
    fn with_host(&self, work: &mut dyn FnMut(&mut dyn TeamHost)) {
        self.with(|core| work(core));
    }
}

/// Runs `work` on the host `access` provides, and gives back what it returned.
pub fn with_host<T>(access: &dyn HostAccess, work: impl FnOnce(&mut dyn TeamHost) -> T) -> T {
    let mut work = Some(work);
    let mut out = None;
    access.with_host(&mut |host| {
        if let Some(work) = work.take() {
            out = Some(work(host));
        }
    });
    out.expect("the host ran the work")
}

/// The part of `Attachments` that `Team` uses, with the caller's database passed in: the real `Attachments` locks its
/// own, which the host already holds.
pub trait SharedAttachments: Send + Sync {
    fn register_snapshot(
        &self,
        db: &Connection,
        path: &Path,
        name: &str,
    ) -> Result<AttachmentInfo, RpcError>;
    fn link(
        &self,
        db: &Connection,
        message_id: &str,
        index: u64,
        attachment_id: &str,
    ) -> Result<(), RpcError>;
    fn cleanup(&self, db: &Connection) -> Result<(), RpcError>;
}

/// The real `Attachments`, through its `*_in` variants that use the connection the host already holds.
impl SharedAttachments for crate::attachments::Attachments {
    fn register_snapshot(
        &self,
        db: &Connection,
        path: &Path,
        name: &str,
    ) -> Result<AttachmentInfo, RpcError> {
        self.register_snapshot_in(db, path, name)
    }

    fn link(
        &self,
        db: &Connection,
        message_id: &str,
        index: u64,
        attachment_id: &str,
    ) -> Result<(), RpcError> {
        self.link_in(db, message_id, index, attachment_id)
    }

    fn cleanup(&self, db: &Connection) -> Result<(), RpcError> {
        self.cleanup_in(db)
    }
}

pub struct TeamDeps {
    /// Sends an event to every client. Called after the change commits, outside any borrow of the database.
    pub broadcast: Broadcast,
    pub log: Log,
    /// The protected `shared-files` folder, which the daemon makes and protects before serving tools.
    pub shared_files: PathBuf,
    pub attachments: Option<Arc<dyn SharedAttachments>>,
}

#[derive(Default)]
struct State {
    /// Work item id -> files the bot shared during that turn.
    shared: HashMap<String, Vec<PathBuf>>,
    /// Autonomous CLI work shares one chain until `session_closed`.
    session_chains: HashMap<String, String>,
    /// Events of the transaction in progress, sent when it commits.
    pending: Option<Vec<Event>>,
    /// Serial numbers of the deletion handles this team issued.
    issued: HashSet<u64>,
    deleted_bots: HashSet<String>,
    serial: u64,
}

/// Bots working together: the tools on Tjommie's MCP server, exchanges between bots, and rooms.
pub struct Team {
    deps: TeamDeps,
    /// The shared-file links. Given at construction, or once later with [`Team::attach_attachments`] when the real
    /// `Attachments` is made after the runner (which needs the team's hooks first).
    attachments: OnceLock<Arc<dyn SharedAttachments>>,
    state: Mutex<State>,
}

/// An exchange as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeRow {
    pub id: String,
    pub from_bot_id: String,
    pub to_bot_id: String,
    pub request: String,
    pub reply: Option<String>,
    pub status: ExchangeStatus,
    pub chain_id: String,
    pub reply_to: String,
    pub created_at: String,
    pub replied_at: Option<String>,
    pub task_attempt_id: Option<String>,
    pub task_revision: Option<i64>,
    pub execution_context_key: Option<String>,
}

const EXCHANGE_COLUMNS: &str = "id, from_bot_id, to_bot_id, request, reply, status, chain_id, reply_to, created_at, replied_at, task_attempt_id, task_revision, execution_context_key";

fn read_exchange(row: &Row<'_>) -> rusqlite::Result<ExchangeRow> {
    Ok(ExchangeRow {
        id: row.get(0)?,
        from_bot_id: row.get(1)?,
        to_bot_id: row.get(2)?,
        request: row.get(3)?,
        reply: row.get(4)?,
        status: from_text(row, 5)?,
        chain_id: row.get(6)?,
        reply_to: row.get(7)?,
        created_at: row.get(8)?,
        replied_at: row.get(9)?,
        task_attempt_id: row.get(10)?,
        task_revision: row.get(11)?,
        execution_context_key: row.get(12)?,
    })
}

fn to_exchange(row: &ExchangeRow) -> Exchange {
    Exchange {
        id: row.id.clone(),
        from_bot_id: row.from_bot_id.clone(),
        to_bot_id: row.to_bot_id.clone(),
        request: row.request.clone(),
        reply: row.reply.clone(),
        status: row.status,
        created_at: row.created_at.clone(),
        replied_at: row.replied_at.clone(),
    }
}

/// An open exchange of a bot that is being deleted, with what is needed to settle it once the bot is gone.
#[derive(Debug, Clone)]
pub struct CapturedExchange {
    pub row: ExchangeRow,
    pub asker: Option<StoredBot>,
    pub reply_to: String,
    pub from_name: String,
    pub to_name: String,
}

/// What [`Team::bot_deleting`] captured before the bot's rows go. It survives cascaded rows and repeated use.
#[derive(Debug, Clone)]
pub struct BotDeletionHandle {
    serial: u64,
    pub bot: StoredBot,
    pub exchanges: Vec<CapturedExchange>,
    pub snapshot_folders: Vec<PathBuf>,
}

pub fn get_exchange(db: &Connection, id: &str) -> Result<Exchange, RpcError> {
    read_row(db, id)?
        .map(|row| to_exchange(&row))
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such exchange"))
}

fn read_row(db: &Connection, id: &str) -> Result<Option<ExchangeRow>, RpcError> {
    Ok(db
        .query_row(
            &format!("SELECT {EXCHANGE_COLUMNS} FROM exchanges WHERE id = ?1"),
            [id],
            read_exchange,
        )
        .optional()?)
}

fn request_prompt(from: &str, request: &str) -> String {
    format!(
        "[Message from {from}, another bot. Your final message in this turn goes back to {from} as your reply.]\n{request}"
    )
}

fn reply_prompt(to: &str, request: &str, reply: &str, files: &[String], first: bool) -> String {
    let mut lines = vec![
        format!(
            "[{to} {} on your request \"{}\":]",
            if first { "replied" } else { "followed up" },
            first_line(request, 120)
        ),
        reply.to_owned(),
    ];
    if !files.is_empty() {
        lines.push(format!(
            "[Files {to} shared, now in your inbox folder: {}]",
            files.join(", ")
        ));
    }
    lines.join("\n")
}

fn failed_prompt(to: &str, request: &str, reason: &str) -> String {
    format!(
        "[{to} couldn't finish your request \"{}\": {reason}. Tell the person if it matters.]",
        first_line(request, 120)
    )
}

fn room_prompt(name: &str, context: &[String], text: &str) -> String {
    let mut lines = vec![format!(
        "[Team room \"{name}\": the person mentioned you. Your reply is posted in the room for everyone."
    )];
    if !context.is_empty() {
        lines.push("Recent room messages, oldest first:".to_owned());
        lines.extend(context.iter().cloned());
    }
    lines.push("]".to_owned());
    lines.push(format!("The person wrote: {text}"));
    lines.join("\n")
}

const DAY_LABELS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

pub(crate) fn describe_days(days: &[u8]) -> String {
    let mut sorted = days.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let key: String = sorted.iter().map(|day| day.to_string()).collect();
    match key.as_str() {
        "0123456" => "every day".to_owned(),
        "12345" => "weekdays".to_owned(),
        "06" => "weekends".to_owned(),
        _ => sorted
            .iter()
            .map(|&day| DAY_LABELS.get(usize::from(day)).copied().unwrap_or("?"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn state_word(state: BotState) -> &'static str {
    match state {
        BotState::Idle => "idle",
        BotState::Working => "working",
        BotState::Waiting => "waiting for the person",
        BotState::Paused => "paused",
        BotState::Limited => "at its usage limit",
        BotState::NeedsSetup => "not set up yet",
    }
}

fn to_map<T: Serialize>(value: &T) -> Map<String, Value> {
    match serde_json::to_value(value).expect("message data serializes") {
        Value::Object(map) => map,
        _ => unreachable!("message data is an object"),
    }
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The typed form of a message's data, when it carries files.
enum Typed {
    File(FileData),
    Post(ExchangePostData),
    Other,
}

/// Each file's attachment id and its companions, in message order.
fn file_slots(typed: &mut Typed) -> Vec<(Option<String>, &mut Option<AttachmentCompanions>)> {
    match typed {
        Typed::File(file) => vec![(file.attachment_id.clone(), &mut file.companions)],
        Typed::Post(post) => post
            .files
            .iter_mut()
            .flatten()
            .map(|file| (file.attachment_id.clone(), &mut file.companions))
            .collect(),
        Typed::Other => Vec::new(),
    }
}

impl Team {
    pub fn new(deps: TeamDeps) -> Self {
        let attachments = OnceLock::new();
        if let Some(given) = &deps.attachments {
            let _ = attachments.set(given.clone());
        }
        Team {
            deps,
            attachments,
            state: Mutex::default(),
        }
    }

    /// Gives the team its shared-file links, once. A second call is ignored.
    pub fn attach_attachments(&self, attachments: Arc<dyn SharedAttachments>) {
        let _ = self.attachments.set(attachments);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn log(&self, context: &str, error: &str) {
        (self.deps.log)(context, error);
    }

    /// Call once at daemon startup, before starting work. Old folders survived a crash.
    pub fn start(&self) {
        sweep_stale(&self.deps.shared_files, SystemTime::now());
    }

    pub fn session_closed(&self, bot_id: &str) {
        self.state().session_chains.remove(bot_id);
    }

    // ---- events and transactions ----

    fn send(&self, event: Event) {
        if let Err(error) = (self.deps.broadcast)(event) {
            self.log("Tjommie: a Team broadcast failed", &error);
        }
    }

    /// Sends the event now, or when the transaction in progress commits.
    fn emit(&self, event: Event) {
        let now = {
            let mut state = self.state();
            match state.pending.as_mut() {
                Some(pending) => {
                    pending.push(event);
                    None
                }
                None => Some(event),
            }
        };
        if let Some(event) = now {
            self.send(event);
        }
    }

    /// One transaction. A nested call joins the outer one. Events from the work go out only after the commit, and not at
    /// all if it fails.
    fn atomic<T>(
        &self,
        host: &mut dyn TeamHost,
        work: impl FnOnce(&mut dyn TeamHost) -> Result<T, RpcError>,
    ) -> Result<T, RpcError> {
        self.run_atomic(host, work, None)
    }

    /// [`Team::atomic`] with something to do once the transaction has committed, before its events go out.
    fn atomic_then<T>(
        &self,
        host: &mut dyn TeamHost,
        work: impl FnOnce(&mut dyn TeamHost) -> Result<T, RpcError>,
        after_commit: &mut dyn FnMut(),
    ) -> Result<T, RpcError> {
        self.run_atomic(host, work, Some(after_commit))
    }

    fn run_atomic<T>(
        &self,
        host: &mut dyn TeamHost,
        work: impl FnOnce(&mut dyn TeamHost) -> Result<T, RpcError>,
        after_commit: Option<&mut dyn FnMut()>,
    ) -> Result<T, RpcError> {
        {
            let mut state = self.state();
            if state.pending.is_some() {
                drop(state);
                if after_commit.is_some() {
                    return Err(plain(
                        "An after-commit action cannot run inside another Team transaction.",
                    ));
                }
                return work(host);
            }
            state.pending = Some(Vec::new());
        }
        // A panic in the work must not leave later transactions joining one that never ends.
        struct Reset<'a>(&'a Team);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.state().pending = None;
            }
        }
        let _reset = Reset(self);
        let result = (|| -> Result<T, RpcError> {
            host.db().execute_batch("BEGIN")?;
            // A panic in the work would leave the `BEGIN` open, and every later transaction on this connection would fail
            // with "cannot start a transaction within a transaction": roll it back, then let the panic go on.
            let worked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(host)));
            match worked {
                Err(panic) => {
                    let _ = host.db().execute_batch("ROLLBACK");
                    std::panic::resume_unwind(panic)
                }
                Ok(Ok(value)) => match host.db().execute_batch("COMMIT") {
                    Ok(()) => Ok(value),
                    Err(error) => {
                        let _ = host.db().execute_batch("ROLLBACK");
                        Err(error.into())
                    }
                },
                Ok(Err(error)) => {
                    let _ = host.db().execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })();
        let events = self.state().pending.take().unwrap_or_default();
        let value = result?;
        if let Some(after_commit) = after_commit {
            after_commit();
        }
        for event in events {
            self.send(event);
        }
        Ok(value)
    }

    /// `#wake`: hands queued work to the runner. If that fails the item is marked failed and the error goes on.
    fn wake(&self, host: &mut dyn TeamHost, item: &WorkItem) -> Result<(), RpcError> {
        if let Err(error) = host.deliver(item) {
            set_status(host.db(), &item.id, crate::queue::WorkStatus::Failed)?;
            return Err(plain(error));
        }
        Ok(())
    }

    // ---- reading ----

    fn exchange(&self, db: &Connection, id: &str) -> Result<Option<ExchangeRow>, RpcError> {
        read_row(db, id)
    }

    fn bot_or_null(&self, db: &Connection, id: &str) -> Result<Option<StoredBot>, RpcError> {
        match get_stored_bot(db, id) {
            Ok(bot) => Ok(Some(bot)),
            Err(error) if error.code == ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn name(&self, db: &Connection, id: &str) -> Result<String, RpcError> {
        Ok(self
            .bot_or_null(db, id)?
            .map_or_else(|| "A deleted bot".to_owned(), |bot| bot.name))
    }

    /// `#roomsFor`: Everyone, and each other room that holds both bots.
    fn rooms_for(
        &self,
        db: &Connection,
        from_id: &str,
        to_id: &str,
    ) -> Result<Vec<String>, RpcError> {
        let mut rooms = vec![TEAM_ROOM_ID.to_owned()];
        let others = db
            .prepare("SELECT room_id FROM room_members WHERE room_id <> ?1 AND bot_id IN (?2, ?3) GROUP BY room_id HAVING COUNT(*) = 2")?
            .query_map(params![TEAM_ROOM_ID, from_id, to_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rooms.extend(others);
        Ok(rooms)
    }

    /// Room messages as lines of text for a bot's prompt.
    fn describe(&self, db: &Connection, messages: &[Message]) -> Result<Vec<String>, RpcError> {
        let mut lines = Vec::new();
        for message in messages {
            let body = js::trim(&message.body);
            if !matches!(message.kind, MessageKind::Text | MessageKind::Notice) || body.is_empty() {
                continue;
            }
            let name = |id: &str| -> Result<String, RpcError> {
                Ok(match id {
                    "you" => "Person".to_owned(),
                    "system" => "Tjommie".to_owned(),
                    other => self.name(db, other)?,
                })
            };
            let to = message
                .data
                .as_ref()
                .and_then(|data| data.get("toBotId"))
                .and_then(Value::as_str);
            let to = match to {
                Some(to) => format!(" to {}", name(to)?),
                None => String::new(),
            };
            lines.push(format!(
                "{}{to}: {}",
                name(&message.author)?,
                js::slice(body, 2000)
            ));
        }
        Ok(lines)
    }

    // ---- messages ----

    /// `#add`: a message in a conversation, with its attachments linked, and its event. File cards and exchange posts
    /// are checked, and their companions' indexes follow their files'.
    fn add(&self, db: &Connection, message: NewMessage) -> Result<Message, RpcError> {
        let bad = |error: serde_json::Error| plain(error.to_string());
        let mut typed = match message.kind {
            MessageKind::File => {
                let data = message.data.clone().map_or(Value::Null, Value::Object);
                Typed::File(serde_json::from_value(data).map_err(bad)?)
            }
            MessageKind::Text => match message
                .data
                .clone()
                .map(Value::Object)
                .and_then(|data| serde_json::from_value::<ExchangePostData>(data).ok())
            {
                Some(post) => Typed::Post(post),
                None => Typed::Other,
            },
            _ => Typed::Other,
        };
        let mut slots = file_slots(&mut typed);
        let mut next_index = slots.len() as u64;
        for (_, companions) in &mut slots {
            if let Some(list) = companions.take() {
                let remapped: Vec<AttachmentCompanion> = list
                    .as_slice()
                    .iter()
                    .map(|companion| {
                        let remapped = AttachmentCompanion {
                            name: companion.name.clone(),
                            attachment_id: companion.attachment_id.clone(),
                            attachment_index: next_index,
                        };
                        next_index += 1;
                        remapped
                    })
                    .collect();
                **companions = Some(
                    AttachmentCompanions::new(remapped)
                        .map_err(|error| plain(error.to_string()))?,
                );
            }
        }
        let links: Vec<(Option<String>, Vec<AttachmentCompanion>)> = slots
            .iter()
            .map(|(id, companions)| {
                let list = match &**companions {
                    Some(list) => list.as_slice().to_vec(),
                    None => Vec::new(),
                };
                (id.clone(), list)
            })
            .collect();
        drop(slots);
        let data = match &typed {
            Typed::File(file) => Some(to_map(file)),
            Typed::Post(post) => Some(to_map(post)),
            Typed::Other => message.data.clone(),
        };
        let stored = add_message(db, &NewMessage { data, ..message })?;
        if let Some(attachments) = self.attachments.get() {
            for (index, (attachment_id, companions)) in links.iter().enumerate() {
                if let Some(id) = attachment_id {
                    attachments.link(db, &stored.id, index as u64, id)?;
                }
                for companion in companions {
                    attachments.link(
                        db,
                        &stored.id,
                        companion.attachment_index,
                        &companion.attachment_id,
                    )?;
                }
            }
        }
        self.emit(Event::MessageAdded(stored.clone()));
        Ok(stored)
    }

    fn update_data(
        &self,
        db: &Connection,
        message_id: &str,
        data: Map<String, Value>,
    ) -> Result<Message, RpcError> {
        let updated = update_message(
            db,
            message_id,
            &MessagePatch {
                data: Some(Some(data)),
                ..MessagePatch::default()
            },
        )?;
        self.emit(Event::MessageUpdated(updated.clone()));
        Ok(updated)
    }

    // ---- rooms ----

    /// A post from the person in a room. Every bot of the room it mentions answers in the room.
    pub fn post_to_room(
        &self,
        host: &mut dyn TeamHost,
        room_id: &str,
        text: &str,
    ) -> Result<Message, RpcError> {
        let (room, context, mentioned) = {
            let db = host.db();
            let room = get_room(db, room_id)?;
            let context = self.describe(db, &history(db, room_id, None, ROOM_CONTEXT)?)?;
            let bots = list_stored_bots(db)?;
            let members = room_members(db, room_id)?;
            let pairs = |bots: &[StoredBot]| -> Vec<(String, String)> {
                bots.iter()
                    .map(|bot| (bot.id.clone(), bot.name.clone()))
                    .collect()
            };
            let (all, inside) = (pairs(&bots), pairs(&members));
            let all: Vec<(&str, &str)> = all
                .iter()
                .map(|(id, name)| (id.as_str(), name.as_str()))
                .collect();
            let inside: Vec<(&str, &str)> = inside
                .iter()
                .map(|(id, name)| (id.as_str(), name.as_str()))
                .collect();
            (room, context, find_mentions(text, &all, &inside))
        };
        let mut queued: Vec<WorkItem> = Vec::new();
        let post = self.atomic(host, |host| {
            let db = host.db();
            let message = self.add(
                db,
                NewMessage {
                    conversation_id: room_id.to_owned(),
                    author: "you".to_owned(),
                    kind: MessageKind::Text,
                    body: text.to_owned(),
                    data: None,
                    status: MessageStatus::Done,
                },
            )?;
            if !mentioned.is_empty() {
                let chain_id = new_chain(db)?;
                for bot_id in &mentioned {
                    queued.push(enqueue(
                        db,
                        &NewWork {
                            message_id: Some(message.id.clone()),
                            conversation_id: Some(room_id.to_owned()),
                            chain_id: Some(chain_id.clone()),
                            ..NewWork::new(
                                bot_id,
                                WorkSource::Room,
                                &room_prompt(&room.name, &context, text),
                            )
                        },
                    )?);
                }
            }
            Ok(message)
        })?;
        for item in &queued {
            self.wake(host, item)?;
        }
        Ok(post)
    }

    // ---- turns and exchanges ----

    /// The runner calls this after each turn it started.
    pub fn turn_ended(
        &self,
        host: &mut dyn TeamHost,
        item: &WorkItem,
        result: &TurnResult,
    ) -> Result<(), RpcError> {
        let files = {
            let mut state = self.state();
            state.session_chains.remove(&item.bot_id);
            state.shared.remove(&item.id).unwrap_or_default()
        };
        let exchange = self.exchange(host.db(), &item.conversation_id)?;
        if let Some(row) = exchange.filter(|row| row.to_bot_id == item.bot_id) {
            self.answer(host, &row, result, files)?;
        }
        let room: Option<String> = host
            .db()
            .query_row(
                "SELECT name FROM conversations WHERE id = ?1 AND kind = 'room'",
                [&item.conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        if let (Some(name), TurnOutcome::Done, Some(text_message_id)) =
            (room, result.outcome, &result.text_message_id)
        {
            self.add(
                host.db(),
                NewMessage {
                    conversation_id: item.bot_id.clone(),
                    author: item.bot_id.clone(),
                    kind: MessageKind::Handoff,
                    body: format!("Answered in {name}"),
                    data: Some(to_map(&RoomHandoffData {
                        room_message_id: text_message_id.clone(),
                        room_id: Some(item.conversation_id.clone()),
                    })),
                    status: MessageStatus::Done,
                },
            )?;
        }
        Ok(())
    }

    /// What the bot that was asked said at the end of a turn in the exchange, or why it couldn't answer.
    fn answer(
        &self,
        host: &mut dyn TeamHost,
        row: &ExchangeRow,
        result: &TurnResult,
        files: Vec<PathBuf>,
    ) -> Result<(), RpcError> {
        // A failed request may have been retried.
        let first = row.status != ExchangeStatus::Replied;
        let (to, from) = {
            let db = host.db();
            (
                self.bot_or_null(db, &row.to_bot_id)?,
                self.bot_or_null(db, &row.from_bot_id)?,
            )
        };
        let Some(to) = to else {
            return Ok(());
        };
        // Deletion already settled this exchange. Late output must not revive it.
        if from.is_none() && row.status == ExchangeStatus::Failed {
            if !files.is_empty() {
                self.save_files_for_person(host, &to, &files)?;
            }
            return Ok(());
        }
        if result.outcome != TurnOutcome::Done {
            let reason = if result.outcome == TurnOutcome::Interrupted {
                "it was stopped".to_owned()
            } else {
                result
                    .error
                    .clone()
                    .unwrap_or_else(|| "something went wrong".to_owned())
            };
            if first {
                self.atomic(host, |host| self.fail(host.db(), row, &reason, None))?;
            }
            if !files.is_empty()
                && let Err(error) = self.save_files_for_person(host, &to, &files)
            {
                self.log(
                    "Tjommie: could not save files from a failed exchange turn",
                    &error.to_string(),
                );
            }
            return self.tell(host, row, &failed_prompt(&to.name, &row.request, &reason));
        }
        let reply = result
            .text
            .clone()
            .unwrap_or_else(|| "(No written reply.)".to_owned());
        let recipient = from.clone().unwrap_or_else(|| to.clone());
        let mut inbox: Vec<String> = Vec::new();
        let copied: std::io::Result<()> = files.iter().try_for_each(|path| {
            inbox.push(copy_into_inbox(Path::new(&recipient.home_folder), path)?);
            Ok(())
        });
        if copied.is_err() {
            remove_inbox_copies(&recipient.home_folder, &inbox);
            let reason = "the shared files could not be copied into your inbox";
            if first {
                self.atomic(host, |host| self.fail(host.db(), row, reason, None))?;
            }
            return self.tell(host, row, &failed_prompt(&to.name, &row.request, reason));
        }
        let saved = self.atomic(host, |host| -> Result<Option<WorkItem>, RpcError> {
            let db = host.db();
            if first {
                db.execute(
                    "UPDATE exchanges SET status = 'replied', reply = ?1, replied_at = ?2 WHERE id = ?3",
                    params![reply, now_iso(), row.id],
                )?;
            }
            let mut shared = Vec::new();
            for (index, path) in files.iter().enumerate() {
                shared.push(self.exchange_file(db, path, &recipient.home_folder, &inbox[index])?);
            }
            for room_id in self.rooms_for(db, &row.from_bot_id, &to.id)? {
                self.add(
                    db,
                    NewMessage {
                        conversation_id: room_id,
                        author: to.id.clone(),
                        kind: MessageKind::Text,
                        body: reply.clone(),
                        data: Some(to_map(&ExchangePostData {
                            exchange_id: row.id.clone(),
                            to_bot_id: row.from_bot_id.clone(),
                            files: Some(shared.clone()),
                        })),
                        status: MessageStatus::Done,
                    },
                )?;
            }
            self.refresh_rows(db, &row.id)?;
            self.queue_reply(db, row, &reply_prompt(&to.name, &row.request, &reply, &inbox, first))
        });
        let queued = match saved {
            Ok(queued) => queued,
            Err(error) => {
                remove_inbox_copies(&recipient.home_folder, &inbox);
                if first {
                    self.atomic(host, |host| {
                        self.fail(host.db(), row, "the reply could not be saved", None)
                    })?;
                }
                return Err(error);
            }
        };
        if let Some(queued) = queued
            && let Err(error) = self.wake(host, &queued)
        {
            if first {
                self.atomic(host, |host| {
                    self.fail(host.db(), row, "the reply could not be delivered", None)
                })?;
            }
            return Err(error);
        }
        self.release_snapshots(&files);
        Ok(())
    }

    /// The reply's file entry for the recipient: its inbox copy, with the attachment the card had.
    fn exchange_file(
        &self,
        db: &Connection,
        snapshot: &Path,
        home_folder: &str,
        inbox_name: &str,
    ) -> Result<ExchangeFile, RpcError> {
        let durable = Path::new(home_folder).join("inbox").join(inbox_name);
        let card: Option<String> = db
            .query_row(
                "SELECT data FROM messages WHERE kind = 'file' AND json_extract(data, '$.path') = ?1 LIMIT 1",
                [snapshot.to_string_lossy().into_owned()],
                |row| row.get(0),
            )
            .optional()?;
        let metadata = card.and_then(|data| serde_json::from_str::<FileData>(&data).ok());
        self.point_file_cards(db, snapshot, &durable)?;
        let mut file = ExchangeFile {
            name: basename(snapshot),
            path: durable.to_string_lossy().into_owned(),
            attachment_id: None,
            content_type: None,
            size: None,
            companions: None,
        };
        if let Some(metadata) = metadata
            && metadata.attachment_id.is_some()
        {
            file.attachment_id = metadata.attachment_id;
            file.content_type = metadata.content_type;
            file.size = Some(metadata.size);
            file.companions = metadata.companions;
        }
        Ok(file)
    }

    /// Ends an exchange that won't get an answer, in its handoff rows and in the room.
    fn fail(
        &self,
        db: &Connection,
        row: &ExchangeRow,
        reason: &str,
        names: Option<(String, String)>,
    ) -> Result<(), RpcError> {
        db.execute(
            "UPDATE exchanges SET status = 'failed', reply = ?1, replied_at = ?2 WHERE id = ?3",
            params![reason, now_iso(), row.id],
        )?;
        self.refresh_rows(db, &row.id)?;
        let (from_name, to_name) = match names {
            Some(names) => names,
            None => (
                self.name(db, &row.from_bot_id)?,
                self.name(db, &row.to_bot_id)?,
            ),
        };
        for room_id in self.rooms_for(db, &row.from_bot_id, &row.to_bot_id)? {
            self.add(
                db,
                NewMessage {
                    conversation_id: room_id,
                    author: "system".to_owned(),
                    kind: MessageKind::Notice,
                    body: format!("{to_name} couldn't answer {from_name}: {reason}."),
                    data: Some(to_map(&NoticeData {
                        exchange_id: row.id.clone(),
                    })),
                    status: MessageStatus::Done,
                },
            )?;
        }
        Ok(())
    }

    /// Sends a reply or a failure to the bot that asked, into the conversation its asking turn was writing to.
    fn tell(&self, host: &mut dyn TeamHost, row: &ExchangeRow, text: &str) -> Result<(), RpcError> {
        let queued = self.queue_reply(host.db(), row, text)?;
        if let Some(item) = queued {
            self.wake(host, &item)?;
        }
        Ok(())
    }

    fn queue_reply(
        &self,
        db: &Connection,
        row: &ExchangeRow,
        text: &str,
    ) -> Result<Option<WorkItem>, RpcError> {
        if self.bot_or_null(db, &row.from_bot_id)?.is_none() {
            return Ok(None);
        }
        let known = db
            .query_row(
                "SELECT 1 FROM conversations WHERE id = ?1",
                [&row.reply_to],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let reply_to = if known {
            &row.reply_to
        } else {
            &row.from_bot_id
        };
        Ok(Some(enqueue(
            db,
            &NewWork {
                conversation_id: Some(reply_to.clone()),
                chain_id: Some(row.chain_id.clone()),
                exchange_id: Some(row.id.clone()),
                task_attempt_id: row.task_attempt_id.clone(),
                task_revision: row.task_revision,
                execution_context_key: row.execution_context_key.clone(),
                ..NewWork::new(&row.from_bot_id, WorkSource::BotReply, text)
            },
        )?))
    }

    /// Brings the handoff rows in both chats up to date: message count and status.
    fn refresh_rows(&self, db: &Connection, exchange_id: &str) -> Result<(), RpcError> {
        let row = self
            .exchange(db, exchange_id)?
            .ok_or_else(|| plain(format!("No exchange {exchange_id}")))?;
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1 AND kind = 'text'",
            [exchange_id],
            |row| row.get(0),
        )?;
        let ids = db
            .prepare("SELECT id FROM messages WHERE kind = 'handoff' AND json_extract(data, '$.exchangeId') = ?1")?
            .query_map([exchange_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            let message = get_message(db, &id)?;
            let current = message.data.clone().map_or(Value::Null, Value::Object);
            let mut handoff: ExchangeHandoffData =
                serde_json::from_value(current).map_err(|error| plain(error.to_string()))?;
            handoff.messages = u64::try_from(count).unwrap_or(0);
            handoff.status = row.status;
            self.update_data(db, &id, to_map(&handoff))?;
        }
        Ok(())
    }

    // ---- shared file snapshots ----

    fn snapshot_folder_of(&self, file: &Path) -> Option<PathBuf> {
        snapshot_folder(&self.deps.shared_files, file)
    }

    fn remove_snapshot_folder(&self, folder: &Path) {
        // A locked orphan is retried by the next startup sweep.
        if let Err(error) = fs::remove_dir_all(folder)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            self.log("Tjommie: shared file cleanup failed", &error.to_string());
        }
    }

    fn release_snapshots(&self, files: &[PathBuf]) {
        for file in files {
            if let Some(folder) = self.snapshot_folder_of(file) {
                self.remove_snapshot_folder(&folder);
            }
        }
    }

    /// Points every file card that shows `snapshot` at the durable copy.
    fn point_file_cards(
        &self,
        db: &Connection,
        snapshot: &Path,
        durable: &Path,
    ) -> Result<(), RpcError> {
        let ids = db
            .prepare(
                "SELECT id FROM messages WHERE kind = 'file' AND json_extract(data, '$.path') = ?1",
            )?
            .query_map([snapshot.to_string_lossy().into_owned()], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            let message = get_message(db, &id)?;
            let current = message.data.clone().map_or(Value::Null, Value::Object);
            let mut file: FileData =
                serde_json::from_value(current).map_err(|error| plain(error.to_string()))?;
            file.path = durable.to_string_lossy().into_owned();
            self.update_data(db, &id, to_map(&file))?;
        }
        Ok(())
    }

    /// Keeps the files of a turn that couldn't be answered in the bot's own inbox, so the person still has them.
    fn save_files_for_person(
        &self,
        host: &mut dyn TeamHost,
        bot: &StoredBot,
        files: &[PathBuf],
    ) -> Result<(), RpcError> {
        let mut copies: Vec<PathBuf> = Vec::new();
        let saved = (|| -> Result<(), RpcError> {
            for file in files {
                let name = copy_into_inbox(Path::new(&bot.home_folder), file)?;
                copies.push(Path::new(&bot.home_folder).join("inbox").join(name));
            }
            self.atomic(host, |host| {
                for (index, file) in files.iter().enumerate() {
                    self.point_file_cards(host.db(), file, &copies[index])?;
                }
                Ok(())
            })
        })();
        if let Err(error) = saved {
            for copy in &copies {
                let _ = fs::remove_file(copy);
            }
            return Err(error);
        }
        self.release_snapshots(files);
        Ok(())
    }

    // ---- two-phase bot deletion ----

    /// Capture after the session closes and before attempting the fallible folder-first delete. This does not change the
    /// queue, exchanges, snapshots or published state.
    pub fn bot_deleting(
        &self,
        host: &mut dyn TeamHost,
        bot_id: &str,
    ) -> Result<BotDeletionHandle, RpcError> {
        let db = host.db();
        let bot = get_stored_bot(db, bot_id)?;
        let open = db
            .prepare(&format!("SELECT {EXCHANGE_COLUMNS} FROM exchanges WHERE status = 'waiting' AND (from_bot_id = ?1 OR to_bot_id = ?1)"))?
            .query_map([bot_id], read_exchange)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut folders: Vec<PathBuf> = Vec::new();
        let staging = &self.deps.shared_files;
        if let Ok(entries) = fs::read_dir(staging) {
            let prefix = format!("share-{bot_id}-");
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(&prefix) && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    folders.push(staging.join(name));
                }
            }
        }
        // Also capture folders made by the earlier version, before names included the owner.
        let cards = db
            .prepare("SELECT json_extract(data, '$.path') FROM messages WHERE kind = 'file' AND author = ?1")?
            .query_map([bot_id], |row| row.get::<_, Option<String>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for path in cards.into_iter().flatten() {
            if let Some(folder) = self.snapshot_folder_of(Path::new(&path))
                && !folders.contains(&folder)
            {
                folders.push(folder);
            }
        }
        let mut exchanges = Vec::new();
        for row in open {
            exchanges.push(CapturedExchange {
                asker: self.bot_or_null(db, &row.from_bot_id)?,
                reply_to: row.reply_to.clone(),
                from_name: self.name(db, &row.from_bot_id)?,
                to_name: self.name(db, &row.to_bot_id)?,
                row,
            });
        }
        let serial = {
            let mut state = self.state();
            state.serial += 1;
            let serial = state.serial;
            state.issued.insert(serial);
            serial
        };
        Ok(BotDeletionHandle {
            serial,
            bot,
            exchanges,
            snapshot_folders: folders,
        })
    }

    /// Call only after `delete_bot` succeeds. The handle survives cascaded rows and repeated calls.
    pub fn bot_deleted(
        &self,
        host: &mut dyn TeamHost,
        handle: &BotDeletionHandle,
    ) -> Result<(), RpcError> {
        if !self.state().issued.contains(&handle.serial) {
            return Err(RpcError::new(
                ErrorCode::BadParams,
                "Unknown bot deletion handle",
            ));
        }
        let bot_id = handle.bot.id.clone();
        if self.state().deleted_bots.contains(&bot_id) {
            return Ok(());
        }
        if self.bot_or_null(host.db(), &bot_id)?.is_some() {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "The bot must be deleted before finishing its exchanges",
            ));
        }
        let queued: RefCell<Vec<WorkItem>> = RefCell::new(Vec::new());
        let stopping: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let moved: RefCell<Vec<(PathBuf, PathBuf)>> = RefCell::new(Vec::new());
        let was = format!("{} was deleted", handle.bot.name);
        let outcome = self.atomic_then(
            host,
            |host| {
                let db = host.db();
                for captured in &handle.exchanges {
                    let row = &captured.row;
                    let existing = self.exchange(db, &row.id)?;
                    if existing.as_ref().is_some_and(|row| row.status != ExchangeStatus::Waiting) {
                        continue;
                    }
                    if existing.is_none() {
                        db.execute(
                            "INSERT INTO exchanges (id, from_bot_id, to_bot_id, request, reply, status, chain_id, reply_to, created_at, replied_at, task_attempt_id, task_revision, execution_context_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                            params![
                                row.id,
                                row.from_bot_id,
                                row.to_bot_id,
                                row.request,
                                row.reply,
                                text(&row.status),
                                row.chain_id,
                                row.reply_to,
                                row.created_at,
                                row.replied_at,
                                row.task_attempt_id,
                                row.task_revision,
                                row.execution_context_key,
                            ],
                        )?;
                    }
                    let running = db
                        .prepare("SELECT id FROM work_items WHERE exchange_id = ?1 AND source = 'bot_request' AND status = 'running'")?
                        .query_map([&row.id], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    stopping.borrow_mut().extend(running);
                    db.execute(
                        "UPDATE work_items SET status = 'failed', ended_at = ?1 WHERE exchange_id = ?2 AND source = 'bot_request' AND status = 'queued'",
                        params![now_iso(), row.id],
                    )?;
                    self.fail(
                        db,
                        row,
                        &was,
                        Some((captured.from_name.clone(), captured.to_name.clone())),
                    )?;
                    if row.to_bot_id == bot_id
                        && let Some(asker) = &captured.asker
                        && self.bot_or_null(db, &asker.id)?.is_some()
                        && let Some(reply) = self.queue_reply(
                            db,
                            row,
                            &failed_prompt(&handle.bot.name, &row.request, &was),
                        )?
                    {
                        queued.borrow_mut().push(reply);
                    }
                }
                // Renames are reversible if the transaction cannot commit. Delete the moved directories only after
                // commit, before publishing any events.
                for from in &handle.snapshot_folders {
                    if !from.exists() {
                        continue;
                    }
                    let to = self
                        .deps
                        .shared_files
                        .join(format!("cleanup-{bot_id}-{}", new_id()));
                    fs::rename(from, &to)?;
                    moved.borrow_mut().push((from.clone(), to));
                }
                Ok(())
            },
            &mut || {
                {
                    let mut state = self.state();
                    state.deleted_bots.insert(bot_id.clone());
                    state.session_chains.remove(&bot_id);
                    let folders = &handle.snapshot_folders;
                    state.shared.retain(|_, files| {
                        files.retain(|file| !file.parent().is_some_and(|dir| folders.iter().any(|folder| folder == dir)));
                        !files.is_empty()
                    });
                }
                for (_, to) in moved.borrow().iter() {
                    self.remove_snapshot_folder(to);
                }
            },
        );
        if let Err(error) = outcome {
            for (from, to) in moved.borrow().iter().rev() {
                fs::rename(to, from)?;
            }
            return Err(error);
        }
        for item_id in stopping.into_inner() {
            host.stop_item(&item_id, &format!("{}.", was));
        }
        for item in &queued.into_inner() {
            self.wake(host, item)?;
        }
        Ok(())
    }

    // ---- routine proposals ----

    /// The person answered a routine proposal: "Add routine" (once or always) or "Dismiss" (deny).
    pub fn answer_routine_proposal(
        &self,
        host: &mut dyn TeamHost,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<Approval, RpcError> {
        self.atomic(host, |host| {
            let (approval, bot) = {
                let db = host.db();
                let pending = get_approval(db, approval_id)?;
                if pending.kind != ApprovalKind::Routine {
                    return Err(RpcError::new(
                        ErrorCode::BadParams,
                        "This is not a routine proposal",
                    ));
                }
                let approval = resolve_approval(db, approval_id, decision)?;
                if approval.status == ApprovalStatus::Allowed {
                    let payload = approval.payload.clone().map_or(Value::Null, Value::Object);
                    let input: RoutineInput = serde_json::from_value(payload)
                        .map_err(|error| plain(error.to_string()))?;
                    if input.bot_id != approval.bot_id {
                        return Err(RpcError::new(
                            ErrorCode::BadParams,
                            "The routine belongs to a different bot",
                        ));
                    }
                    self.emit(Event::RoutineUpdated(create_routine(db, &input, now_ms())?));
                }
                let message = get_message(db, &approval.message_id)?;
                let current = message.data.clone().map_or(Value::Null, Value::Object);
                // A malformed saved card must not prevent the person from answering its approval.
                let data = match serde_json::from_value::<RoutineApprovalData>(current) {
                    Ok(mut data) => {
                        data.status = approval.status;
                        to_map(&data)
                    }
                    Err(_) => {
                        let mut data = message.data.clone().unwrap_or_default();
                        data.insert("status".to_owned(), Value::String(text(&approval.status)));
                        data
                    }
                };
                self.update_data(db, &message.id, data)?;
                self.emit(Event::ApprovalResolved(approval.clone()));
                (approval.clone(), get_stored_bot(db, &approval.bot_id)?)
            };
            let view = host.to_bot(bot);
            self.emit(Event::BotUpdated(view));
            Ok(approval)
        })
    }
}

/// Removes inbox copies made for a delivery that failed.
fn remove_inbox_copies(home_folder: &str, names: &[String]) {
    for name in names {
        let _ = fs::remove_file(Path::new(home_folder).join("inbox").join(name));
    }
}

/// `room.post` and `exchanges.get` (`daemon.ts:569`, `:587`).
pub fn register(
    handlers: Handlers,
    context: &Context,
    team: Arc<Team>,
    access: Arc<dyn HostAccess>,
) -> Handlers {
    let exchange = context.clone();
    handlers
        .on::<method::RoomPost, _>(move |params, _| {
            let post = with_host(&*access, |host| {
                team.post_to_room(host, &params.room_id, &params.text)
            });
            async move { post }
        })
        .on::<method::ExchangesGet, _>(move |Id { id }, _| {
            let view = exchange.with_db(|db| {
                let exchange = get_exchange(db, &id)?;
                let page = history_page(db, &id, None, 200)?;
                Ok(ExchangeView {
                    messages: page.messages,
                    has_more: page.has_more,
                    exchange,
                })
            });
            async move { view }
        })
}
