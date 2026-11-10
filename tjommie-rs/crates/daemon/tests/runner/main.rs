//! `runner.test.ts`, split by its `describe` blocks. Events reach the runner through each session's channel, so a test
//! settles the runtime after emitting where TypeScript's listener ran synchronously. The "account-bound runner" cases
//! drive T3.4's `AccountRuntime` and are ported with it.

#[path = "../common/mod.rs"]
mod common;

mod approvals;
mod deletion;
mod delivery;
mod hooks;
mod launch;
mod queueing;
mod settlement;
mod upgrade;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use common::{TempRoot, insert_account, test_db};
use daemon::bots::{BotStatus, StoredBot, bot_state_of, create_bot, get_stored_bot};
use daemon::chat::{get_message, history};
use daemon::providers::{Approval as ProviderApproval, TurnResult as ProviderTurn};
use daemon::providers::{
    ApprovalAnswer, Limit, LimitState, ProviderError, ProviderEvent, ProviderSession, StartOptions,
    TokenUsage, Usage,
};
use daemon::queue::{NewWork, WorkItem, WorkSource, WorkStatus, get_work_item};
use daemon::runner::{
    Broadcast, ExecutionPhase, Runner, RunnerDeps, RunnerHooks, SessionFactory,
    TaskExecutionContext, TurnOutcome, TurnResult, activity_body, copy_into_inbox_at,
};
use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use protocol::model::{
    ActivityData, ActivityItem, ActivityStatus, BotState, Message, MessageData, message_data,
};
use protocol::{ErrorCode, Event};
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

// ---------------------------------------------------------------------------------------------------------------------
// Gates: promises a test resolves or rejects later.

pub type Pending<T> = Shared<BoxFuture<'static, T>>;

/// Resolves a [`gate`]. Dropping it leaves the gate pending forever, like a promise nobody settles.
pub struct Opener<T>(Mutex<Option<oneshot::Sender<T>>>);

impl<T> Opener<T> {
    pub fn open(&self, value: T) {
        if let Some(sender) = self.0.lock().unwrap().take() {
            let _ = sender.send(value);
        }
    }
}

pub fn gate<T: Clone + Send + Sync + 'static>() -> (Pending<T>, Opener<T>) {
    let (sender, receiver) = oneshot::channel();
    let pending = async move {
        match receiver.await {
            Ok(value) => value,
            Err(_) => std::future::pending().await,
        }
    };
    (pending.boxed().shared(), Opener(Mutex::new(Some(sender))))
}

pub fn ready<T: Clone + Send + Sync + 'static>(value: T) -> Pending<T> {
    futures_util::future::ready(value).boxed().shared()
}

// ---------------------------------------------------------------------------------------------------------------------
// FakeSession

pub struct FakeState {
    pub start: Pending<Result<(), String>>,
    pub send: Pending<Result<(), String>>,
    pub interrupt: Pending<Result<(), String>>,
    pub exit: Pending<()>,
    pub steer_pending: Option<Pending<bool>>,
    pub sent: Vec<String>,
    pub steered: Vec<String>,
    pub answers: Vec<(String, ApprovalAnswer)>,
    pub interrupts: u32,
    pub closed: bool,
    pub close_calls: u32,
    pub steer_result: bool,
    pub steer_error: Option<String>,
    pub interrupt_error: Option<String>,
    pub hide_pid_when_closed: bool,
    subscribers: Vec<mpsc::UnboundedSender<ProviderEvent>>,
}

pub struct FakeSession(Arc<Mutex<FakeState>>);

impl FakeSession {
    pub fn new() -> Arc<Self> {
        Arc::new(FakeSession(Arc::new(Mutex::new(FakeState {
            start: ready(Ok(())),
            send: ready(Ok(())),
            interrupt: ready(Ok(())),
            exit: ready(()),
            steer_pending: None,
            sent: Vec::new(),
            steered: Vec::new(),
            answers: Vec::new(),
            interrupts: 0,
            closed: false,
            close_calls: 0,
            steer_result: true,
            steer_error: None,
            interrupt_error: None,
            hide_pid_when_closed: false,
            subscribers: Vec::new(),
        }))))
    }

    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set(&self, change: impl FnOnce(&mut FakeState)) {
        change(&mut self.state());
    }

    pub fn sent(&self) -> Vec<String> {
        self.state().sent.clone()
    }

    pub fn steered(&self) -> Vec<String> {
        self.state().steered.clone()
    }

    pub fn answers(&self) -> Vec<(String, ApprovalAnswer)> {
        self.state().answers.clone()
    }

    pub fn interrupts(&self) -> u32 {
        self.state().interrupts
    }

    pub fn closed(&self) -> bool {
        self.state().closed
    }

    pub fn close_calls(&self) -> u32 {
        self.state().close_calls
    }

    pub fn emit(&self, event: ProviderEvent) {
        self.state()
            .subscribers
            .retain(|sender| sender.send(event.clone()).is_ok());
    }
}

impl ProviderSession for FakeSession {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let start = self.state().start.clone();
        Box::pin(async move { start.await.map_err(ProviderError::Failed) })
    }

    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut state = self.state();
        state.sent.push(text.to_owned());
        let send = state.send.clone();
        Box::pin(async move { send.await.map_err(ProviderError::Failed) })
    }

    fn steer(&self, text: &str) -> BoxFuture<'static, Result<bool, ProviderError>> {
        let mut state = self.state();
        state.steered.push(text.to_owned());
        if let Some(error) = state.steer_error.clone() {
            return Box::pin(async move { Err(ProviderError::Failed(error)) });
        }
        if let Some(pending) = state.steer_pending.clone() {
            return Box::pin(async move { Ok(pending.await) });
        }
        let result = state.steer_result;
        Box::pin(async move { Ok(result) })
    }

    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>> {
        let mut state = self.state();
        state.interrupts += 1;
        let pending = state.interrupt.clone();
        let shared = self.0.clone();
        Box::pin(async move {
            pending.await.map_err(ProviderError::Failed)?;
            match shared.lock().unwrap().interrupt_error.clone() {
                Some(error) => Err(ProviderError::Failed(error)),
                None => Ok(false),
            }
        })
    }

    fn answer(&self, approval_id: &str, answer: ApprovalAnswer) {
        self.state().answers.push((approval_id.to_owned(), answer));
    }

    fn close(&self) {
        let mut state = self.state();
        state.close_calls += 1;
        state.closed = true;
    }

    fn wait_for_exit(&self) -> BoxFuture<'static, ()> {
        let exit = self.state().exit.clone();
        Box::pin(exit)
    }

    fn pid(&self) -> Option<u32> {
        let state = self.state();
        if state.closed && state.hide_pid_when_closed {
            None
        } else {
            Some(4321)
        }
    }

    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.state().subscribers.push(sender);
        receiver
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Provider events

pub fn turn_done(ok: bool, error: Option<&str>) -> ProviderEvent {
    ProviderEvent::TurnDone(ProviderTurn {
        ok,
        error: error.map(Into::into),
    })
}

pub fn exit(error: Option<&str>) -> ProviderEvent {
    ProviderEvent::Exit {
        error: error.map(Into::into),
        completed_turn: None,
    }
}

pub fn exit_completed(ok: bool) -> ProviderEvent {
    ProviderEvent::Exit {
        error: None,
        completed_turn: Some(ProviderTurn { ok, error: None }),
    }
}

pub fn text(delta: &str) -> ProviderEvent {
    ProviderEvent::Text {
        delta: delta.into(),
    }
}

pub fn tool(id: &str, name: &str, summary: &str) -> ProviderEvent {
    ProviderEvent::Tool {
        id: id.into(),
        name: name.into(),
        summary: summary.into(),
    }
}

pub fn tool_done(id: &str, ok: bool) -> ProviderEvent {
    ProviderEvent::ToolDone { id: id.into(), ok }
}

pub fn approval(id: &str, summary: &str, rule: Option<&str>) -> ProviderEvent {
    ProviderEvent::Approval(ProviderApproval {
        id: id.into(),
        tool: "Write".into(),
        summary: summary.into(),
        detail: "{}".into(),
        rule: rule.map(Into::into),
    })
}

pub fn limit_reached(resets_at: Option<&str>) -> ProviderEvent {
    ProviderEvent::Limit(Limit {
        state: LimitState::Reached,
        used_percent: Some(100.0),
        resets_at: resets_at.map(Into::into),
        limit_id: None,
        windows: None,
    })
}

pub const FAR_RESET: &str = "2099-09-25T10:00:00.000Z";

pub fn session_id(id: &str) -> ProviderEvent {
    ProviderEvent::Session {
        cli_session_id: id.into(),
    }
}

pub fn usage(cost_usd: f64, total_cost_usd: f64) -> ProviderEvent {
    ProviderEvent::Usage(Usage {
        model: "haiku".into(),
        tokens: TokenUsage {
            input: 1.0,
            cached_input: 0.0,
            output: 1.0,
            reasoning: 0.0,
        },
        cost_usd: Some(cost_usd),
        total_cost_usd: Some(total_cost_usd),
        event_id: None,
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// Waiting

/// Lets every task that is ready run until it waits on something, like `setImmediate` after a microtask flush.
pub async fn settle() {
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
}

pub async fn wait_for(check: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the condition"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Harness

/// Sees each event after it is recorded; an `Err` is what the runner gets back from broadcasting.
pub type OnBroadcast = Arc<dyn Fn(&Event) -> Result<(), String> + Send + Sync>;

pub type Factory = Arc<dyn Fn(&StartOptions) -> Result<Arc<FakeSession>, String> + Send + Sync>;

pub struct Setup {
    pub factory: Factory,
    pub signed_in: bool,
    pub hooks: RunnerHooks,
    pub on_broadcast: Option<OnBroadcast>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            factory: Arc::new(|_| Ok(FakeSession::new())),
            signed_in: true,
            hooks: RunnerHooks::default(),
            on_broadcast: None,
        }
    }
}

/// Always hands out the same session.
pub fn single(session: &Arc<FakeSession>) -> Factory {
    let session = session.clone();
    Arc::new(move |_| Ok(session.clone()))
}

/// Hands out `sessions` in order, then fails like TypeScript's "Unexpected extra session".
pub fn sequence(sessions: Vec<Arc<FakeSession>>) -> Factory {
    let next = AtomicU32::new(0);
    Arc::new(move |_| {
        let index = next.fetch_add(1, Ordering::SeqCst) as usize;
        sessions
            .get(index)
            .cloned()
            .ok_or_else(|| "Unexpected extra session".to_owned())
    })
}

/// Records each session's start options, then hands out a fresh session.
pub fn capturing(captured: &Arc<Mutex<Vec<StartOptions>>>) -> Factory {
    let captured = captured.clone();
    Arc::new(move |options| {
        captured.lock().unwrap().push(options.clone());
        Ok(FakeSession::new())
    })
}

pub type Recorded<T> = Arc<Mutex<Vec<T>>>;

pub fn recorded<T>(list: &Recorded<T>) -> MutexGuard<'_, Vec<T>> {
    list.lock().unwrap_or_else(PoisonError::into_inner)
}

static BOT_COUNT: AtomicU32 = AtomicU32::new(0);

pub struct Harness {
    pub root: TempRoot,
    pub db: daemon::context::Db,
    pub runner: Runner,
    pub events: Recorded<Event>,
    pub sessions: Recorded<Arc<FakeSession>>,
    pub logs: Recorded<String>,
    pub kills: Recorded<Option<u32>>,
    invalid: Recorded<String>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert_eq!(
                *recorded(&self.invalid),
                Vec::<String>::new(),
                "the runner sent events the protocol rejects"
            );
        }
    }
}

pub fn harness() -> Harness {
    harness_with(Setup::default())
}

pub fn harness_with(setup: Setup) -> Harness {
    harness_in(TempRoot::new(), setup)
}

/// `ensureAccounts`: the default Claude and Codex subscription accounts.
fn ensure_accounts(db: &Connection, root: &std::path::Path) {
    for (id, provider) in [("claude-main", "claude"), ("codex-main", "codex")] {
        let folder = root.join("accounts").join(id);
        fs::create_dir_all(&folder).unwrap();
        insert_account(db, id, provider, &folder);
    }
}

pub fn harness_in(root: TempRoot, setup: Setup) -> Harness {
    let connection = test_db(&root.0);
    ensure_accounts(&connection, &root.0);
    let db = Arc::new(Mutex::new(connection));
    let events: Recorded<Event> = Arc::default();
    let sessions: Recorded<Arc<FakeSession>> = Arc::default();
    let logs: Recorded<String> = Arc::default();
    let kills: Recorded<Option<u32>> = Arc::default();
    let invalid: Recorded<String> = Arc::default();
    let broadcast = recorder(&events, &invalid, setup.on_broadcast);
    let factory = setup.factory;
    let made = sessions.clone();
    let create_session: SessionFactory =
        Arc::new(move |_bot: &StoredBot, options: StartOptions| {
            let session = factory(&options)?;
            made.lock().unwrap().push(session.clone());
            Ok(session as Arc<dyn ProviderSession>)
        });
    let signed_in = setup.signed_in;
    let deps = RunnerDeps {
        is_signed_in: Arc::new(move |_| signed_in),
        hooks: setup.hooks,
        ..base_deps(&db, broadcast, create_session, &logs, &kills)
    };
    let runner = Runner::new(deps);
    Harness {
        root,
        db,
        runner,
        events,
        sessions,
        logs,
        kills,
        invalid,
    }
}

fn recorder(
    events: &Recorded<Event>,
    invalid: &Recorded<String>,
    on_broadcast: Option<OnBroadcast>,
) -> Broadcast {
    let events = events.clone();
    let invalid = invalid.clone();
    Arc::new(move |event: Event| {
        let value = serde_json::to_value(&event).unwrap();
        let name = value["event"].as_str().unwrap().to_owned();
        if let Err(error) = protocol::validate(&name, &value["data"]) {
            invalid.lock().unwrap().push(format!("{name}: {error}"));
        }
        events.lock().unwrap().push(event.clone());
        on_broadcast.as_ref().map_or(Ok(()), |hook| hook(&event))
    })
}

fn base_deps(
    db: &daemon::context::Db,
    broadcast: Broadcast,
    create_session: SessionFactory,
    logs: &Recorded<String>,
    kills: &Recorded<Option<u32>>,
) -> RunnerDeps {
    let logs = logs.clone();
    let kills = kills.clone();
    RunnerDeps {
        log: Arc::new(move |context: &str, error: &str| {
            logs.lock().unwrap().push(format!("{context}: {error}"))
        }),
        kill_tree: Arc::new(move |pid| kills.lock().unwrap().push(pid)),
        ..RunnerDeps::new(db.clone(), broadcast, create_session)
    }
}

impl Harness {
    /// Deps for another runner on this harness's database, sharing its logs, kills and event list.
    pub fn deps(&self, create_session: SessionFactory) -> RunnerDeps {
        let broadcast = recorder(&self.events, &self.invalid, None);
        base_deps(&self.db, broadcast, create_session, &self.logs, &self.kills)
    }

    /// Never hold this across a runner call: the runner locks the database too.
    pub fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn path(&self, name: &str) -> String {
        self.root.0.join(name).display().to_string()
    }

    /// `newBot`: a Claude haiku bot named `Bot <n>`, with `overrides` on top.
    pub fn bot(&self, overrides: Value) -> StoredBot {
        let n = BOT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        let mut input = json!({ "name": format!("Bot {n}"), "color": "blue", "shape": "circle", "instructions": "Be terse.", "provider": "claude", "model": "haiku" });
        input
            .as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        create_bot(
            &self.db(),
            &self.root.0.join("bots"),
            &serde_json::from_value(input).unwrap(),
        )
        .unwrap()
    }

    pub fn session(&self, index: usize) -> Arc<FakeSession> {
        recorded(&self.sessions)[index].clone()
    }

    pub fn session_count(&self) -> usize {
        recorded(&self.sessions).len()
    }

    /// How many messages session `index` was sent; 0 before it exists.
    pub fn sent(&self, index: usize) -> usize {
        recorded(&self.sessions)
            .get(index)
            .map_or(0, |session| session.sent().len())
    }

    pub fn events(&self) -> Vec<Event> {
        recorded(&self.events).clone()
    }

    pub fn event_count(&self) -> usize {
        recorded(&self.events).len()
    }

    pub fn logged(&self, needle: &str) -> bool {
        recorded(&self.logs)
            .iter()
            .any(|line| line.contains(needle))
    }

    pub fn no_logs(&self) -> bool {
        recorded(&self.logs).is_empty()
    }

    pub fn work(&self, id: &str) -> WorkItem {
        get_work_item(&self.db(), id).unwrap()
    }

    pub fn status(&self, id: &str) -> WorkStatus {
        self.work(id).status
    }

    pub fn message(&self, id: &str) -> Message {
        get_message(&self.db(), id).unwrap()
    }

    pub fn history(&self, conversation_id: &str) -> Vec<Message> {
        history(&self.db(), conversation_id, None, 20).unwrap()
    }

    pub fn last(&self, conversation_id: &str) -> Message {
        history(&self.db(), conversation_id, None, 1)
            .unwrap()
            .remove(0)
    }

    /// `itemForMessage`: the newest work item for a message.
    pub fn item_for_message(&self, message_id: &str) -> WorkItem {
        let id: String = self
            .db()
            .query_row(
                "SELECT id FROM work_items WHERE message_id = ?1 ORDER BY rowid DESC LIMIT 1",
                [message_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| panic!("No work item for {message_id}"));
        self.work(&id)
    }

    pub fn state(&self, bot_id: &str) -> BotState {
        let bot = get_stored_bot(&self.db(), bot_id).unwrap();
        self.runner.to_bot(bot).state
    }

    pub fn stored(&self, bot_id: &str) -> StoredBot {
        get_stored_bot(&self.db(), bot_id).unwrap()
    }

    pub fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    /// The first column of every row `sql` returns.
    pub fn strings(&self, sql: &str) -> Vec<String> {
        let db = self.db();
        let mut statement = db.prepare(sql).unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    pub fn insert_session(&self, bot_id: &str, cli_session_id: &str) {
        self.db()
            .execute(
                "INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?1, 'claude', 'claude-main', ?2, '2026-09-25T10:00:00.000Z')",
                [bot_id, cli_session_id],
            )
            .unwrap();
    }

    pub fn saved_session(&self, bot_id: &str) -> Option<String> {
        use rusqlite::OptionalExtension;
        self.db()
            .query_row(
                "SELECT cli_session_id FROM sessions WHERE bot_id = ?1",
                [bot_id],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    }

    /// `activityFor`: the newest activity row in a conversation.
    pub fn activity_for(&self, conversation_id: &str) -> Message {
        self.history(conversation_id)
            .into_iter()
            .rfind(|message| message.kind == protocol::model::MessageKind::Activity)
            .unwrap_or_else(|| panic!("No activity row in {conversation_id}"))
    }

    /// `expectActivitySettlement`: the row's steps ended as `statuses`, announced by exactly one update since `since`.
    pub fn expect_activity_settlement(
        &self,
        message_id: &str,
        statuses: &[ActivityStatus],
        since: usize,
    ) {
        let saved = self.message(message_id);
        let Ok(MessageData::Activity(data)) = message_data(&saved) else {
            panic!("not an activity row: {saved:?}")
        };
        assert_eq!(
            data.items
                .iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            statuses
        );
        let updates: Vec<Message> = self.events()[since..]
            .iter()
            .filter_map(|event| match event {
                Event::MessageUpdated(message) if message.id == message_id => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(updates, [saved]);
    }

    pub fn added_approvals(&self) -> Vec<protocol::model::Approval> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::ApprovalAdded(approval) => Some(approval),
                _ => None,
            })
            .collect()
    }
}

/// Records `turnStarted`, `turnEnded` and `turnRequeued`, in order.
#[derive(Clone, Default)]
pub struct Lifecycle(Recorded<(&'static str, WorkItem, Option<TurnResult>)>);

impl Lifecycle {
    pub fn hooks(&self) -> RunnerHooks {
        let (started, ended, requeued) = (self.0.clone(), self.0.clone(), self.0.clone());
        RunnerHooks {
            turn_started: Some(Arc::new(move |_core, item: &WorkItem| {
                started
                    .lock()
                    .unwrap()
                    .push(("started", item.clone(), None));
                Ok(())
            })),
            turn_ended: Some(Arc::new(
                move |_core, item: &WorkItem, result: &TurnResult| {
                    ended
                        .lock()
                        .unwrap()
                        .push(("ended", item.clone(), Some(result.clone())));
                    Ok(())
                },
            )),
            turn_requeued: Some(Arc::new(move |_core, item: &WorkItem| {
                requeued
                    .lock()
                    .unwrap()
                    .push(("requeued", item.clone(), None));
                Ok(())
            })),
            ..RunnerHooks::default()
        }
    }

    fn of(&self, kind: &str) -> Vec<(WorkItem, Option<TurnResult>)> {
        recorded(&self.0)
            .iter()
            .filter(|entry| entry.0 == kind)
            .map(|entry| (entry.1.clone(), entry.2.clone()))
            .collect()
    }

    pub fn started(&self) -> Vec<String> {
        self.of("started")
            .into_iter()
            .map(|entry| entry.0.id)
            .collect()
    }

    pub fn requeued(&self) -> Vec<String> {
        self.of("requeued")
            .into_iter()
            .map(|entry| entry.0.id)
            .collect()
    }

    pub fn ended(&self) -> Vec<(WorkItem, TurnResult)> {
        self.of("ended")
            .into_iter()
            .map(|entry| (entry.0, entry.1.unwrap()))
            .collect()
    }

    pub fn ended_ids(&self) -> Vec<String> {
        self.ended().into_iter().map(|entry| entry.0.id).collect()
    }

    pub fn results(&self) -> Vec<TurnResult> {
        self.ended().into_iter().map(|entry| entry.1).collect()
    }

    pub fn all(&self) -> Vec<(&'static str, String)> {
        recorded(&self.0)
            .iter()
            .map(|entry| (entry.0, entry.1.id.clone()))
            .collect()
    }
}

pub fn work(bot_id: &str, source: WorkSource, text: &str) -> NewWork {
    NewWork::new(bot_id, source, text)
}

pub fn work_in(bot_id: &str, source: WorkSource, text: &str, conversation_id: &str) -> NewWork {
    NewWork {
        conversation_id: Some(conversation_id.into()),
        ..NewWork::new(bot_id, source, text)
    }
}

pub fn task_work(bot_id: &str, text: &str, attempt: &str, key: &str) -> NewWork {
    NewWork {
        task_attempt_id: Some(attempt.into()),
        task_revision: Some(1),
        execution_context_key: Some(key.into()),
        ..NewWork::new(bot_id, WorkSource::Task, text)
    }
}

/// An `executionContext` hook giving each task item a develop context in `cwd(item)`.
pub fn develop_context(
    cwd: impl Fn(&WorkItem) -> String + Send + Sync + 'static,
    instructions: &str,
) -> RunnerHooks {
    let instructions = instructions.to_owned();
    RunnerHooks {
        execution_context: Some(Arc::new(move |_core, _bot, item: &WorkItem| {
            Ok(item
                .task_attempt_id
                .as_ref()
                .map(|attempt| TaskExecutionContext {
                    key: item.execution_context_key.clone().unwrap(),
                    cwd: cwd(item),
                    attempt_id: attempt.clone(),
                    revision: item.task_revision.unwrap(),
                    phase: ExecutionPhase::Develop,
                    instructions: instructions.clone(),
                }))
        })),
        ..RunnerHooks::default()
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Top-level cases

#[tokio::test]
async fn uses_a_task_cwd_closes_its_session_at_completion_and_refuses_cross_attempt_cli_resume() {
    let captured: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let root = TempRoot::new();
    let base = root.0.clone();
    let hooks = develop_context(
        move |item| {
            base.join(item.task_attempt_id.as_ref().unwrap())
                .display()
                .to_string()
        },
        "Task-scoped instructions",
    );
    let h = harness_in(
        root,
        Setup {
            factory: capturing(&captured),
            hooks,
            ..Setup::default()
        },
    );
    let original = h.path("original-project");
    fs::create_dir(&original).unwrap();
    let bot = h.bot(json!({ "projectFolder": original }));
    fs::create_dir(h.path("attempt-a")).unwrap();
    fs::create_dir(h.path("attempt-b")).unwrap();
    h.runner
        .deliver(task_work(&bot.id, "Task A", "attempt-a", "context-a"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let first = recorded(&captured)[0].clone();
    assert_eq!(first.cwd, h.path("attempt-a"));
    assert_eq!(h.runner.effective_folder(&bot.id).unwrap(), first.cwd);
    assert!(first.instructions.contains(&first.cwd));
    assert!(!first.instructions.contains(&original));
    assert_eq!(h.stored(&bot.id).project_folder, Some(original));
    h.session(0).emit(session_id("saved-for-a"));
    h.session(0).emit(turn_done(true, None));
    settle().await;
    h.runner
        .deliver(task_work(&bot.id, "Task B", "attempt-b", "context-b"))
        .unwrap();
    wait_for(|| recorded(&captured).len() == 2).await;
    let second = recorded(&captured)[1].clone();
    assert!(h.session(0).closed());
    assert_eq!(second.cwd, h.path("attempt-b"));
    assert_eq!(second.resume_id, None);
}

#[tokio::test]
async fn keeps_ordinary_chat_out_of_an_active_task_and_blocks_review_roles_or_missing_task_context_before_provider_launch()
 {
    let root = TempRoot::new();
    let cwd = root.0.display().to_string();
    let h = harness_in(
        root,
        Setup {
            hooks: develop_context(move |_| cwd.clone(), "Task"),
            ..Setup::default()
        },
    );
    let bot = h.bot(json!({}));
    h.runner
        .deliver(task_work(&bot.id, "Task", "a", "a"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.runner.submit(&bot.id, "Standalone chat", &[]).unwrap();
    settle().await;
    assert_eq!(h.session(0).steered(), Vec::<String>::new());
    h.session(0).emit(turn_done(true, None));
    wait_for(|| h.session_count() == 2).await;

    let review_root = TempRoot::new();
    let review_cwd = review_root.0.display().to_string();
    let hooks = RunnerHooks {
        execution_context: Some(Arc::new(move |_core, _bot, item: &WorkItem| {
            Ok(Some(TaskExecutionContext {
                key: "review".into(),
                cwd: review_cwd.clone(),
                attempt_id: item.task_attempt_id.clone().unwrap(),
                revision: 1,
                phase: ExecutionPhase::Review,
                instructions: "Read only".into(),
            }))
        })),
        ..RunnerHooks::default()
    };
    let blocked = harness_in(
        review_root,
        Setup {
            hooks,
            ..Setup::default()
        },
    );
    let reviewer = blocked.bot(json!({}));
    let job = blocked
        .runner
        .deliver(task_work(&reviewer.id, "Review", "review", "review"))
        .unwrap();
    wait_for(|| blocked.status(&job.id) == WorkStatus::Failed).await;
    assert_eq!(blocked.session_count(), 0);

    let missing = harness();
    let other = missing.bot(json!({}));
    let absent = missing
        .runner
        .deliver(task_work(&other.id, "Missing", "missing", "missing"))
        .unwrap();
    wait_for(|| missing.status(&absent.id) == WorkStatus::Failed).await;
    assert_eq!(missing.session_count(), 0);
}

#[tokio::test]
async fn keeps_the_next_task_queued_until_the_previous_tasks_owned_process_has_exited() {
    let first = FakeSession::new();
    let (exit, exited) = gate();
    first.set(|state| state.exit = exit);
    let made = AtomicU32::new(0);
    let reuse = first.clone();
    let factory: Factory = Arc::new(move |_| {
        Ok(if made.fetch_add(1, Ordering::SeqCst) == 0 {
            reuse.clone()
        } else {
            FakeSession::new()
        })
    });
    let root = TempRoot::new();
    let cwd = root.0.display().to_string();
    let h = harness_in(
        root,
        Setup {
            factory,
            hooks: develop_context(move |_| cwd.clone(), "Task"),
            ..Setup::default()
        },
    );
    let bot = h.bot(json!({}));
    h.runner.deliver(task_work(&bot.id, "A", "a", "a")).unwrap();
    wait_for(|| !first.sent().is_empty()).await;
    let retired = Arc::new(AtomicBool::new(false));
    let barrier = {
        let retired = retired.clone();
        let waiting = h.runner.retired(&bot.id);
        tokio::spawn(async move {
            waiting.await;
            retired.store(true, Ordering::SeqCst);
        })
    };
    first.emit(turn_done(true, None));
    settle().await;
    h.runner.deliver(task_work(&bot.id, "B", "b", "b")).unwrap();
    settle().await;
    assert_eq!(h.session_count(), 1);
    assert!(!retired.load(Ordering::SeqCst));
    exited.open(());
    barrier.await.unwrap();
    wait_for(|| h.session_count() == 2).await;
    assert!(retired.load(Ordering::SeqCst));
}

#[tokio::test]
async fn honors_the_host_hold_gate_before_starting_queued_provider_work() {
    let h = harness();
    let bot = h.bot(json!({}));
    let session = FakeSession::new();
    let held = Arc::new(AtomicBool::new(true));
    let gate_held = held.clone();
    let made = h.sessions.clone();
    let factory = single(&session);
    let create_session: SessionFactory = Arc::new(move |_bot, options| {
        let session = factory(&options)?;
        made.lock().unwrap().push(session.clone());
        Ok(session as Arc<dyn ProviderSession>)
    });
    let runner = Runner::new(RunnerDeps {
        is_held: Arc::new(move |_| gate_held.load(Ordering::SeqCst)),
        ..h.deps(create_session)
    });
    let message = runner.submit(&bot.id, "Explicit queued work", &[]).unwrap();
    assert_eq!(session.sent(), Vec::<String>::new());
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Queued);
    held.store(false, Ordering::SeqCst);
    runner.pump_all();
    wait_for(|| !session.sent().is_empty()).await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Running);
}

#[test]
fn bot_state_of_ranks_states_needs_setup_paused_limited_waiting_working_idle() {
    let now = "2026-09-25T10:00:00.000Z";
    let base = || BotStatus {
        signed_in: true,
        setup_reason: None,
        paused: false,
        limited_until: None,
        waiting: false,
        busy: false,
    };
    assert_eq!(
        bot_state_of(
            &BotStatus {
                signed_in: false,
                paused: true,
                ..base()
            },
            now
        ),
        BotState::NeedsSetup
    );
    assert_eq!(
        bot_state_of(
            &BotStatus {
                setup_reason: Some("Set up Codex's sandbox".into()),
                busy: true,
                ..base()
            },
            now
        ),
        BotState::NeedsSetup
    );
    assert_eq!(
        bot_state_of(
            &BotStatus {
                paused: true,
                busy: true,
                ..base()
            },
            now
        ),
        BotState::Paused
    );
    assert_eq!(
        bot_state_of(
            &BotStatus {
                limited_until: Some("2026-09-25T11:00:00.000Z".into()),
                waiting: true,
                ..base()
            },
            now
        ),
        BotState::Limited
    );
    assert_eq!(
        bot_state_of(
            &BotStatus {
                limited_until: Some("2026-09-25T09:00:00.000Z".into()),
                waiting: true,
                busy: true,
                ..base()
            },
            now
        ),
        BotState::Waiting
    );
    assert_eq!(
        bot_state_of(
            &BotStatus {
                busy: true,
                ..base()
            },
            now
        ),
        BotState::Working
    );
    assert_eq!(bot_state_of(&base(), now), BotState::Idle);
}

fn setup_runner(h: &Harness, reason: &Arc<Mutex<Option<String>>>, factory: Factory) -> Runner {
    let reason = reason.clone();
    let made = h.sessions.clone();
    let create_session: SessionFactory = Arc::new(move |_bot, options| {
        let session = factory(&options)?;
        made.lock().unwrap().push(session.clone());
        Ok(session as Arc<dyn ProviderSession>)
    });
    Runner::new(RunnerDeps {
        setup_issue: Arc::new(move |bot: &StoredBot| {
            let codex_folder = bot.provider == protocol::model::Provider::Codex
                && bot.autonomy == protocol::model::Autonomy::Folder;
            if codex_folder {
                reason.lock().unwrap().clone()
            } else {
                None
            }
        }),
        ..h.deps(create_session)
    })
}

#[tokio::test]
async fn holds_a_codex_own_folder_bot_until_sandbox_readiness_clears_then_pumps_queued_work() {
    let h = harness();
    let reason = Arc::new(Mutex::new(Some("Set up Codex's sandbox".to_owned())));
    let runner = setup_runner(&h, &reason, Arc::new(|_| Ok(FakeSession::new())));
    let bot = h.bot(json!({ "provider": "codex", "model": "test-model", "autonomy": "folder" }));
    let shown = runner.to_bot(bot.clone());
    assert_eq!(
        (shown.state, shown.setup_reason.as_deref()),
        (BotState::NeedsSetup, Some("Set up Codex's sandbox"))
    );
    runner.submit(&bot.id, "Begin", &[]).unwrap();
    assert_eq!(h.session_count(), 0);
    *reason.lock().unwrap() = None;
    runner.pump_all();
    wait_for(|| h.session_count() == 1).await;
    assert_eq!(runner.to_bot(h.stored(&bot.id)).setup_reason, None);
}

#[tokio::test]
async fn closes_an_active_codex_session_when_its_sandbox_becomes_unready() {
    let h = harness();
    let reason = Arc::new(Mutex::new(None));
    let session = FakeSession::new();
    let runner = setup_runner(&h, &reason, single(&session));
    let bot = h.bot(json!({ "provider": "codex", "model": "test-model", "autonomy": "folder" }));
    runner.submit(&bot.id, "Begin", &[]).unwrap();
    wait_for(|| session.sent().len() == 1).await;
    *reason.lock().unwrap() = Some("Set up Codex's sandbox".to_owned());
    runner.setup_changed();
    assert_eq!(session.interrupts(), 1);
    session.emit(turn_done(false, None));
    settle().await;
    assert!(session.closed());
    let notice = "Codex's sandbox is no longer ready. Set it up again to continue.";
    assert!(
        h.history(&bot.id)
            .iter()
            .any(|item| item.kind == protocol::model::MessageKind::Notice && item.body == notice)
    );
    let shown = runner.to_bot(h.stored(&bot.id));
    assert_eq!(
        (shown.state, shown.setup_reason.as_deref()),
        (BotState::NeedsSetup, Some("Set up Codex's sandbox"))
    );
}

#[test]
fn activity_body_counts_steps_and_shows_the_latest_one() {
    let step = |summary: &str| ActivityItem {
        id: summary.into(),
        name: "Tool".into(),
        summary: summary.into(),
        status: ActivityStatus::Done,
    };
    assert_eq!(
        activity_body(&[step("Read notes.md")]),
        "1 step: Read notes.md"
    );
    assert_eq!(
        activity_body(&[step("Read notes.md"), step("Run npm test")]),
        "2 steps: Run npm test"
    );
    let _ = ActivityData { items: vec![] };
}

#[tokio::test]
async fn copy_into_inbox_rejects_missing_paths_and_directories_before_copying_attachments_or_saving_a_message()
 {
    let h = harness();
    let bot = h.bot(json!({}));
    let file = h.path("valid.txt");
    fs::write(&file, "valid").unwrap();
    for invalid in [h.path("missing.txt"), h.root.0.display().to_string()] {
        let error = h
            .runner
            .submit(&bot.id, "Files", &[file.clone(), invalid.clone()])
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::BadParams);
        assert!(error.message.contains(&invalid), "{}", error.message);
        assert_eq!(h.history(&bot.id), Vec::<Message>::new());
        assert_eq!(
            fs::read_dir(PathBuf::from(&bot.home_folder).join("inbox"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn copy_into_inbox_copies_three_same_name_attachments_without_overwriting_under_a_fixed_clock() {
    let root = TempRoot::new();
    let home = root.0.join("home");
    fs::create_dir(&home).unwrap();
    let file = root.0.join("CV final \u{f1}.pdf");
    let mut names = Vec::new();
    for body in ["one", "two", "three"] {
        fs::write(&file, body).unwrap();
        names.push(copy_into_inbox_at(&home, &file, 1_797_000_000_000).unwrap());
    }
    assert_eq!(names[0], "CV final \u{f1}.pdf");
    assert_eq!(names[1], "1797000000000-CV final \u{f1}.pdf");
    let third = regex::Regex::new(r"^1797000000000-\d+-CV final \x{f1}\.pdf$").unwrap();
    assert!(third.is_match(&names[2]), "{}", names[2]);
    assert_eq!(
        names.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    for (name, body) in names.iter().zip(["one", "two", "three"]) {
        assert_eq!(
            fs::read_to_string(home.join("inbox").join(name)).unwrap(),
            body
        );
    }
}
