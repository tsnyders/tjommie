//! The harness for the M4 tests that drive a real `Runner` with a fake provider session and M4's hooks.
//!
//! It is built the way `daemon.ts` composes its pieces: M4 first, then the runner with hooks that call M4's methods, then
//! `m4.attach(runner)`. The runner's own tests keep their fake and setup private, so what these tests need is copied here
//! (`FakeSession`, the no-CLI host, the usage reader), not imported.
//!
//! Waiting: the runner works on tokio tasks, so an event a test emits is handled later. [`wait_until`] polls in real time
//! (5 s at most). An item's status and its hook effects are written inside one runner lock, so once a poll sees a
//! terminal status, the turn's effects are in the database too.
//!
//! Locks: the runner locks the database while it runs a hook. [`Harness::conn`] is for one query, never held across a
//! runner call or an `.await`.

#![allow(dead_code)] // Each test binary uses part of this.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use daemon::account_quota::{UsageFetch, UsageResponse};
use daemon::bots::{StoredBot, create_bot, update_bot};
use daemon::chat::history;
use daemon::cli::{CliChild, CliCommand, CliHost, LaunchOptions};
use daemon::context::Db;
use daemon::m4::{M4, M4Deps};
use daemon::providers::{
    ApprovalAnswer, ProviderError, ProviderEvent, ProviderSession, StartOptions,
    TurnResult as ProviderTurn,
};
use daemon::queue::{WorkItem, WorkStatus, get_work_item};
use daemon::routines::TurnEnded;
use daemon::runner::{Broadcast, Log, Runner, RunnerDeps, RunnerHooks, SessionFactory, TurnResult};
use daemon::setup::Setup;
use futures_util::future::BoxFuture;
use protocol::Event;
use protocol::model::{BotInput, BotPatch, Message};
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub type Recorded<T> = Arc<Mutex<Vec<T>>>;

pub fn recorded<T>(list: &Recorded<T>) -> MutexGuard<'_, Vec<T>> {
    list.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A provider session the runner drives. It records what the runner sends and counts interrupts. The test speaks as the
/// provider with [`FakeSession::emit`] or [`FakeSession::finish`].
pub struct FakeSession {
    state: Mutex<SessionState>,
}

#[derive(Default)]
struct SessionState {
    sent: Vec<String>,
    steered: Vec<String>,
    interrupts: u32,
    subscribers: Vec<mpsc::UnboundedSender<ProviderEvent>>,
}

impl FakeSession {
    pub fn new() -> Arc<Self> {
        Arc::new(FakeSession {
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn sent(&self) -> Vec<String> {
        self.state().sent.clone()
    }

    pub fn steered(&self) -> Vec<String> {
        self.state().steered.clone()
    }

    pub fn interrupts(&self) -> u32 {
        self.state().interrupts
    }

    /// Hands one event to the runner, as the CLI's output would.
    pub fn emit(&self, event: ProviderEvent) {
        self.state()
            .subscribers
            .retain(|sender| sender.send(event.clone()).is_ok());
    }

    /// TypeScript's `finish(text)`: a text delta, the end of the text, then a turn that finished ok.
    pub fn finish(&self, text: &str) {
        self.emit(ProviderEvent::Text {
            delta: text.to_owned(),
        });
        self.emit(ProviderEvent::TextEnd);
        self.emit(turn_done(true, None));
    }
}

impl ProviderSession for FakeSession {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async { Ok::<(), ProviderError>(()) })
    }

    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        self.state().sent.push(text.to_owned());
        Box::pin(async { Ok::<(), ProviderError>(()) })
    }

    fn steer(&self, text: &str) -> BoxFuture<'static, Result<bool, ProviderError>> {
        self.state().steered.push(text.to_owned());
        Box::pin(async { Ok::<bool, ProviderError>(true) })
    }

    /// Counts the Stop. `Ok(false)`: this fake does not handle it itself, as Core's fake doesn't.
    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>> {
        self.state().interrupts += 1;
        Box::pin(async { Ok::<bool, ProviderError>(false) })
    }

    fn answer(&self, _approval_id: &str, _answer: ApprovalAnswer) {}

    fn close(&self) {}

    fn wait_for_exit(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async { Ok::<(), ProviderError>(()) })
    }

    /// No process, so nothing for `kill_tree` to stop.
    fn pid(&self) -> Option<u32> {
        None
    }

    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.state().subscribers.push(sender);
        receiver
    }
}

/// A turn that ended: `ok`, or failed with `error`.
pub fn turn_done(ok: bool, error: Option<&str>) -> ProviderEvent {
    ProviderEvent::TurnDone(ProviderTurn {
        ok,
        error: error.map(str::to_owned),
    })
}

/// `waitFor` in real time: polls `check` every 10 ms for up to 5 s, then fails the test with `what`.
pub async fn wait_until(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A CLI host that finds no provider CLI, so nothing reaches a real Claude or Codex.
struct NoCli;

impl CliHost for NoCli {
    fn launch<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        Box::pin(async { Err(io::Error::from(io::ErrorKind::NotFound)) })
    }
}

/// Usage reads without a network.
struct NoFetch;

impl UsageFetch for NoFetch {
    fn get<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(&'static str, String)],
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<UsageResponse, String>> {
        Box::pin(async { Err("no network in tests".to_owned()) })
    }
}

/// The MCP address the hooks give every session. The fake session ignores it.
const MCP_URL: &str = "http://127.0.0.1:0/mcp";

/// A real runner, M4 and their shared database. `root` is last so the database closes before the folder is removed.
pub struct Harness {
    pub runner: Runner,
    pub m4: Arc<M4>,
    pub db: Db,
    /// Each bot's latest session, as the TypeScript tests' `sessions` map.
    sessions: Arc<Mutex<HashMap<String, Arc<FakeSession>>>>,
    /// `turnStarted`, in order. Steered work never starts a turn, so it isn't here.
    pub started: Recorded<WorkItem>,
    /// `turnEnded`, in order.
    pub ended: Recorded<WorkItem>,
    pub events: Recorded<Event>,
    pub logs: Recorded<String>,
    invalid: Recorded<String>,
    pub root: tempfile::TempDir,
}

/// `new Runner(...)` plus M4, as `daemon.ts` builds them. `signed_in: false` holds every bot's queue, so nothing starts.
pub fn harness(signed_in: bool) -> Harness {
    let root = tempfile::Builder::new()
        .prefix("tjommie m4 ñ-")
        .tempdir()
        .unwrap();
    let shared_files = root.path().join("shared-files");
    fs::create_dir(&shared_files).unwrap();
    let connection = daemon::db::open(&root.path().join("t.db")).unwrap();
    let setup = Setup::new(Arc::new(NoCli), Arc::new(NoFetch));
    setup
        .ensure_accounts(&connection, &root.path().join("accounts"), root.path())
        .unwrap();
    let db: Db = Arc::new(Mutex::new(connection));

    let events: Recorded<Event> = Arc::default();
    let logs: Recorded<String> = Arc::default();
    let invalid: Recorded<String> = Arc::default();
    let started: Recorded<WorkItem> = Arc::default();
    let ended: Recorded<WorkItem> = Arc::default();
    let sessions: Arc<Mutex<HashMap<String, Arc<FakeSession>>>> = Arc::default();

    let broadcast: Broadcast = {
        let (events, invalid) = (events.clone(), invalid.clone());
        Arc::new(move |event: Event| {
            let value = serde_json::to_value(&event).unwrap();
            let name = value["event"].as_str().unwrap().to_owned();
            if let Err(error) = protocol::validate(&name, &value["data"]) {
                recorded(&invalid).push(format!("{name}: {error}"));
            }
            recorded(&events).push(event);
            Ok(())
        })
    };
    let log: Log = {
        let logs = logs.clone();
        Arc::new(move |context: &str, error: &str| {
            recorded(&logs).push(format!("{context}: {error}"))
        })
    };
    let create_session: SessionFactory = {
        let sessions = sessions.clone();
        Arc::new(move |bot: &StoredBot, _options: StartOptions| {
            let session = FakeSession::new();
            sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(bot.id.clone(), session.clone());
            Ok(session as Arc<dyn ProviderSession>)
        })
    };

    // M4 is made first: its hooks are part of the runner, and it gets the runner afterwards.
    let turn_ended: TurnEnded = Arc::new(|_: &WorkItem, _: &TurnResult| {});
    let m4 = Arc::new(M4::new(M4Deps {
        db: db.clone(),
        broadcast: broadcast.clone(),
        log: log.clone(),
        shared_files,
        attachments: None,
        cleanup_shared: Arc::new(|| {}),
        turn_ended,
    }));

    // Each hook keeps its own handle on M4, as the closures in `daemon.ts` do.
    let (mcp_m4, closed_m4, start_m4) = (m4.clone(), m4.clone(), m4.clone());
    let (requeue_m4, end_m4, retry_m4) = (m4.clone(), m4.clone(), m4.clone());
    let (started_log, ended_log) = (started.clone(), ended.clone());
    let hooks = RunnerHooks {
        mcp_servers: Some(Arc::new(move |_core, bot: &StoredBot, _item: &WorkItem| {
            Ok(vec![mcp_m4.mcp_server(&bot.id, MCP_URL)])
        })),
        session_closed: Some(Arc::new(move |_core, bot_id: &str| {
            closed_m4.session_closed(bot_id);
            Ok(())
        })),
        turn_started: Some(Arc::new(move |core, item: &WorkItem| {
            recorded(&started_log).push(item.clone());
            start_m4.turn_started(core, item)
        })),
        turn_requeued: Some(Arc::new(move |_core, item: &WorkItem| {
            requeue_m4.turn_requeued(item);
            Ok(())
        })),
        turn_ended: Some(Arc::new(move |core, item: &WorkItem, result: &TurnResult| {
            recorded(&ended_log).push(item.clone());
            end_m4.turn_ended(core, item, result)
        })),
        retry_routine: Some(Arc::new(move |core, message: &Message| {
            retry_m4.retry_routine(core, message)
        })),
        ..RunnerHooks::default()
    };
    let runner = Runner::new(RunnerDeps {
        is_signed_in: Arc::new(move |_| signed_in),
        hooks,
        log,
        ..RunnerDeps::new(db.clone(), broadcast, create_session, setup)
    });
    m4.attach(runner.clone());

    Harness {
        runner,
        m4,
        db,
        sessions,
        started,
        ended,
        events,
        logs,
        invalid,
        root,
    }
}

impl Harness {
    /// The database, locked. For one query only: never hold it across a runner call or an `.await`.
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `createBot` from the TypeScript input.
    pub fn create(&self, input: Value) -> StoredBot {
        let input: BotInput = serde_json::from_value(input).unwrap();
        create_bot(&self.conn(), &self.root.path().join("bots"), &input).unwrap()
    }

    /// A Claude bot on Sonnet, with the TypeScript tests' usual fields.
    pub fn bot(&self, name: &str) -> StoredBot {
        self.create(json!({
            "name": name, "color": "blue", "shape": "circle", "instructions": "",
            "provider": "claude", "model": "sonnet",
        }))
    }

    /// `updateBot(db, id, { canMessage })`.
    pub fn set_can_message(&self, bot_id: &str, can_message: &[&str]) -> StoredBot {
        let patch: BotPatch = serde_json::from_value(json!({ "canMessage": can_message })).unwrap();
        update_bot(&self.conn(), bot_id, &patch).unwrap()
    }

    pub fn work(&self, id: &str) -> WorkItem {
        get_work_item(&self.conn(), id).unwrap()
    }

    pub fn status(&self, id: &str) -> WorkStatus {
        self.work(id).status
    }

    /// Waits until a work item reaches `status`.
    pub async fn await_status(&self, id: &str, status: WorkStatus) {
        wait_until(&format!("work {id} to be {status:?}"), || {
            self.status(id) == status
        })
        .await;
    }

    /// Every work row, oldest first. The TypeScript `delivered` list is this, plus the submits (which it never recorded).
    pub fn all_work(&self) -> Vec<WorkItem> {
        let ids: Vec<String> = {
            let db = self.conn();
            let mut statement = db.prepare("SELECT id FROM work_items ORDER BY rowid").unwrap();
            let rows: Vec<String> = statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows
        };
        ids.iter().map(|id| self.work(id)).collect()
    }

    /// The newest work row: TypeScript's `delivered.at(-1)`, since each team delivery is written before it is woken.
    pub fn latest_work(&self) -> WorkItem {
        let id: String = self
            .conn()
            .query_row(
                "SELECT id FROM work_items ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        self.work(&id)
    }

    /// The oldest work row for a message: the work a room post queued first.
    pub fn first_work_for(&self, message_id: &str) -> WorkItem {
        self.all_work()
            .into_iter()
            .find(|item| item.message_id.as_deref() == Some(message_id))
            .expect("the message queued work")
    }

    pub fn count_work_for(&self, message_id: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM work_items WHERE message_id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// The last `limit` messages of a conversation, oldest first.
    pub fn chat(&self, conversation_id: &str, limit: u32) -> Vec<Message> {
        history(&self.conn(), conversation_id, None, limit).unwrap()
    }

    /// The bot's session. Panics when it has none.
    pub fn session(&self, bot_id: &str) -> Arc<FakeSession> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(bot_id)
            .cloned()
            .unwrap_or_else(|| panic!("{bot_id} has no session"))
    }

    /// How many bots have had a session made. Zero for a bot the runner never started.
    pub fn sessions_made(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn sent_count(&self, bot_id: &str) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(bot_id)
            .map_or(0, |session| session.sent().len())
    }

    /// TypeScript's `sent(bot, count)`: waits until the bot's session has been sent `count` messages, then returns it.
    pub async fn sent(&self, bot_id: &str, count: usize) -> Arc<FakeSession> {
        wait_until(&format!("{bot_id} to be sent {count} message(s)"), || {
            self.sent_count(bot_id) >= count
        })
        .await;
        self.session(bot_id)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        // The TypeScript `afterEach` stops the runner. M4 and the runner hold each other (M4 keeps the runner, the hooks
        // keep M4), so the database can outlive the test and the folder removal is best effort on Windows.
        // `shutdown` does its work in the call and returns only the wait for the CLI processes to exit, which a sync drop
        // can't await.
        drop(self.runner.shutdown());
        let invalid = recorded(&self.invalid).clone();
        assert!(
            invalid.is_empty(),
            "the runner sent events the protocol rejects: {invalid:?}"
        );
    }
}

/// A message's data as a JSON object, or `null` when it has none.
pub fn data_of(message: &Message) -> Value {
    message.data.clone().map_or(Value::Null, Value::Object)
}
