//! The fixture the team tests share: a database in a temp folder, a [`Team`] on a fake host, and the recorders the
//! TypeScript tests keep in `delivered`, `events` and `current`.
//!
//! The fake host stands in for the runner. `deliver` records the work, or fails when a test asks it to; `stop_item`
//! records; `current_work` reads a map the test fills in; a bot's state is `Idle` unless the test sets one.
//! [`DbAttachments`] is a small `SharedAttachments` over the real `attachments` and `message_attachments` tables, so the
//! team's file cards link real rows without the real `Attachments` (which locks its own database handle).
#![allow(dead_code)] // each test binary uses part of this

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use daemon::attachments::attachment_content_type;
use daemon::bots::{StoredBot, create_bot, update_bot};
use daemon::ids::{new_id, now_iso};
use daemon::mcp::McpTool;
use daemon::queue::WorkItem;
use daemon::server::RpcError;
use daemon::team::{HostAccess, SharedAttachments, Team, TeamDeps, TeamHost};
use protocol::Event;
use protocol::attachments::AttachmentInfo;
use protocol::model::{Bot, BotInput, BotPatch, BotState};
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub type Recorded<T> = Arc<Mutex<Vec<T>>>;

pub fn recorded<T>(list: &Recorded<T>) -> MutexGuard<'_, Vec<T>> {
    list.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The runner's side of the fixture: what the TypeScript tests pass as `deliver`, `stopItem`, `currentWork` and
/// `stateOf`.
pub struct FakeHost {
    pub db: Connection,
    pub delivered: Recorded<WorkItem>,
    pub stopped: Recorded<(String, String)>,
    /// `currentWork`: the work a bot is running now, by bot id.
    pub current: Arc<Mutex<HashMap<String, WorkItem>>>,
    /// A bot's state by bot id. A bot not listed is idle.
    pub states: Arc<Mutex<HashMap<String, BotState>>>,
    /// `effectiveFolder`, by bot id.
    pub effective: Arc<Mutex<HashMap<String, String>>>,
    /// While `Some`, `deliver` fails with this message, as a throwing `deliver` does in TypeScript.
    pub deliver_fails: Arc<Mutex<Option<String>>>,
}

impl TeamHost for FakeHost {
    fn db(&self) -> &Connection {
        &self.db
    }

    fn deliver(&mut self, work: &WorkItem) -> Result<(), String> {
        if let Some(message) = self
            .deliver_fails
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(message);
        }
        recorded(&self.delivered).push(work.clone());
        Ok(())
    }

    fn stop_item(&mut self, item_id: &str, notice: &str) {
        recorded(&self.stopped).push((item_id.to_owned(), notice.to_owned()));
    }

    fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(bot_id)
            .cloned()
    }

    fn to_bot(&mut self, bot: StoredBot) -> Bot {
        let state = self
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&bot.id)
            .copied()
            .unwrap_or(BotState::Idle);
        bot.into_bot(state, None)
    }

    fn effective_folder(&self, bot_id: &str) -> Result<Option<String>, RpcError> {
        Ok(self
            .effective
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(bot_id)
            .cloned())
    }
}

/// The way the MCP tools reach the fixture's host.
pub struct FakeAccess(pub Arc<Mutex<FakeHost>>);

impl HostAccess for FakeAccess {
    fn with_host(&self, work: &mut dyn FnMut(&mut dyn TeamHost)) {
        let mut host = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        work(&mut *host);
    }
}

/// Fields drop in order: the database closes (with `host`) before `root` removes its folder, which Windows requires.
pub struct Fixture {
    pub host: Arc<Mutex<FakeHost>>,
    pub team: Arc<Team>,
    /// `events`: every broadcast, in order.
    pub events: Recorded<Event>,
    /// Errors the team logged (`console.error` in TypeScript): context and error text.
    pub logs: Recorded<(String, String)>,
    pub delivered: Recorded<WorkItem>,
    pub stopped: Recorded<(String, String)>,
    pub current: Arc<Mutex<HashMap<String, WorkItem>>>,
    pub states: Arc<Mutex<HashMap<String, BotState>>>,
    pub effective: Arc<Mutex<HashMap<String, String>>>,
    pub deliver_fails: Arc<Mutex<Option<String>>>,
    /// Present when the fixture was made with attachments.
    pub attachments: Option<Arc<DbAttachments>>,
    pub root: TempDir,
}

pub fn fixture() -> Fixture {
    fixture_with(false)
}

/// `beforeEach`: a temp root (with a space and an accented letter, like a Windows profile) holding `shared-files`, a
/// database `t.db` and a `bots` folder. With `attachments`, the team links file cards through [`DbAttachments`].
pub fn fixture_with(attachments: bool) -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap();
    fs::create_dir(root.path().join("shared-files")).unwrap();
    let db = daemon::db::open(&root.path().join("t.db")).unwrap();
    let events: Recorded<Event> = Arc::default();
    let logs: Recorded<(String, String)> = Arc::default();
    let delivered: Recorded<WorkItem> = Arc::default();
    let stopped: Recorded<(String, String)> = Arc::default();
    let current: Arc<Mutex<HashMap<String, WorkItem>>> = Arc::default();
    let states: Arc<Mutex<HashMap<String, BotState>>> = Arc::default();
    let effective: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let deliver_fails: Arc<Mutex<Option<String>>> = Arc::default();
    let shared = attachments.then(|| {
        Arc::new(DbAttachments {
            root: root.path().join("shared-files").join("attachments"),
            host_id: "host".to_owned(),
            linked: Arc::default(),
        })
    });
    let (sink, log_sink) = (events.clone(), logs.clone());
    let team = Arc::new(Team::new(TeamDeps {
        broadcast: Arc::new(move |event| {
            recorded(&sink).push(event);
            Ok(())
        }),
        log: Arc::new(move |context, error| {
            recorded(&log_sink).push((context.to_owned(), error.to_owned()));
        }),
        shared_files: root.path().join("shared-files"),
        attachments: shared
            .clone()
            .map(|shared| shared as Arc<dyn SharedAttachments>),
    }));
    let host = Arc::new(Mutex::new(FakeHost {
        db,
        delivered: delivered.clone(),
        stopped: stopped.clone(),
        current: current.clone(),
        states: states.clone(),
        effective: effective.clone(),
        deliver_fails: deliver_fails.clone(),
    }));
    Fixture {
        root,
        host,
        team,
        events,
        logs,
        delivered,
        stopped,
        current,
        states,
        effective,
        deliver_fails,
        attachments: shared,
    }
}

impl Fixture {
    /// The fake host, locked. Never hold it across an `.await` of a tool, which locks it too.
    pub fn host(&self) -> MutexGuard<'_, FakeHost> {
        self.host.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `work` on the database.
    pub fn db<T>(&self, work: impl FnOnce(&Connection) -> T) -> T {
        work(&self.host().db)
    }

    /// Runs `work` with the team and its host: `fx.team_call(|team, host| team.post_to_room(host, "team", "hi"))`.
    pub fn team_call<T>(&self, work: impl FnOnce(&Team, &mut dyn TeamHost) -> T) -> T {
        let mut host = self.host();
        work(&self.team, &mut *host)
    }

    pub fn bots_dir(&self) -> PathBuf {
        self.root.path().join("bots")
    }

    pub fn shared_files(&self) -> PathBuf {
        self.root.path().join("shared-files")
    }

    /// `createBot` with the TypeScript tests' usual fields. `can_message` is bot ids.
    pub fn bot(
        &self,
        name: &str,
        color: &str,
        shape: &str,
        instructions: &str,
        provider: &str,
        model: &str,
    ) -> StoredBot {
        let input: BotInput = serde_json::from_value(json!({
            "name": name, "color": color, "shape": shape, "instructions": instructions,
            "provider": provider, "model": model,
        }))
        .unwrap();
        create_bot(&self.host().db, &self.bots_dir(), &input).unwrap()
    }

    /// `updateBot(db, id, { canMessage })`.
    pub fn set_can_message(&self, bot_id: &str, can_message: &[&str]) -> StoredBot {
        let patch: BotPatch = serde_json::from_value(json!({ "canMessage": can_message })).unwrap();
        update_bot(&self.host().db, bot_id, &patch).unwrap()
    }

    /// The MCP tools of the team, on this fixture's host.
    pub fn tools(&self) -> Vec<McpTool> {
        self.team.tools(Arc::new(FakeAccess(self.host.clone())))
    }

    /// Runs a tool as `bot_id` (`tool.run(botId, args)`): `Ok` is the text the bot reads, `Err` the message of a failed
    /// call. Arguments are checked first, as the MCP endpoint does.
    pub fn run_tool(&self, name: &str, bot_id: &str, arguments: Value) -> Result<String, String> {
        let tools = self.tools();
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        let call = tool.call(bot_id, &arguments);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(call)
    }

    /// Every event so far as `{ event, data }` JSON, which is how it reaches a client.
    pub fn event_json(&self) -> Vec<Value> {
        recorded(&self.events)
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect()
    }

    /// The data of every event with this wire name (`"message.added"`), in order.
    pub fn events_named(&self, name: &str) -> Vec<Value> {
        self.event_json()
            .into_iter()
            .filter(|event| event["event"] == name)
            .map(|event| event["data"].clone())
            .collect()
    }

    /// The wire names of every event so far, in order.
    pub fn event_names(&self) -> Vec<String> {
        self.event_json()
            .iter()
            .map(|event| event["event"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    pub fn clear_events(&self) {
        recorded(&self.events).clear();
    }

    /// `count(*)` of a query with no parameters.
    pub fn count(&self, sql: &str) -> i64 {
        self.db(|db| db.query_row(sql, [], |row| row.get(0)).unwrap())
    }
}

/// `SharedAttachments` over the real tables: copies a snapshot into `shared-files\attachments`, records its row, links
/// message rows, and removes attachments no message links. What `Attachments` does, without its own database lock.
pub struct DbAttachments {
    pub root: PathBuf,
    pub host_id: String,
    /// Every `link` call: message id, index, attachment id.
    pub linked: Recorded<(String, u64, String)>,
}

impl SharedAttachments for DbAttachments {
    fn register_snapshot(
        &self,
        db: &Connection,
        path: &Path,
        name: &str,
    ) -> Result<AttachmentInfo, RpcError> {
        let bytes = fs::read(path)?;
        let id = new_id();
        let storage = format!("{id}.bin");
        fs::create_dir_all(&self.root)?;
        fs::write(self.root.join(&storage), &bytes)?;
        let content_type = attachment_content_type(name, &bytes[..bytes.len().min(4096)]);
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        db.execute(
            "INSERT INTO attachments (id, host_id, storage_name, original_name, byte_length, content_type, sha256, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![id, self.host_id, storage, name, bytes.len() as i64, content_type, sha256, now_iso()],
        )?;
        Ok(AttachmentInfo {
            id,
            host_id: self.host_id.clone(),
            name: name.to_owned(),
            size: bytes.len() as u64,
            content_type: content_type.to_owned(),
            sha256,
        })
    }

    fn link(
        &self,
        db: &Connection,
        message_id: &str,
        index: u64,
        attachment_id: &str,
    ) -> Result<(), RpcError> {
        db.execute(
            "INSERT INTO message_attachments (message_id, attachment_index, attachment_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![message_id, index as i64, attachment_id],
        )?;
        recorded(&self.linked).push((message_id.to_owned(), index, attachment_id.to_owned()));
        Ok(())
    }

    fn cleanup(&self, db: &Connection) -> Result<(), RpcError> {
        let orphans: Vec<(String, String)> = db
            .prepare("SELECT id, storage_name FROM attachments WHERE NOT EXISTS (SELECT 1 FROM message_attachments m WHERE m.attachment_id = attachments.id)")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (id, storage) in orphans {
            let _ = fs::remove_file(self.root.join(storage));
            db.execute("DELETE FROM attachments WHERE id = ?1", [id])?;
        }
        Ok(())
    }
}
