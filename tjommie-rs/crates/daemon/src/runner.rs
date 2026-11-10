//! `runner.ts`: runs each bot's queued work through its CLI session, one turn at a time, and steers your follow-ups into
//! the turn that is running.
//!
//! **Locks.** The runner's state has its own mutex and is only touched with the database locked first: database, then
//! state, the order [`BotView::to_bot`] callers already hold. Hooks run inside both locks and get a [`Core`], so a hook
//! calls the runner through that `Core`; calling a [`Runner`] method from a hook would deadlock.
//!
//! **Await points.** A TypeScript async method runs synchronously up to its first `await`, and callers rely on that: a
//! session exists as soon as `submit` returns, and steering claims a queued item inside `deliver`. The ports keep it:
//! each does its synchronous part inside the lock, and only the rest runs as a task (see [`Turn`]).
//!
//! **Steering after the first send (on purpose, not TypeScript's way).** When a turn's first message reaches the CLI,
//! queued follow-ups are steered in. If reading the queue (`next_steerable`) or claiming an item (`set_status`) fails
//! then, the error is logged and the turn carries on with its item unclaimed, so it runs as a later turn. In TypeScript
//! that throw rejected `#startTurn` and failed a turn that was already running fine. Keep this.
//!
//! **T3.4 seams.** Accounts come in through [`Accounts`] (`AccountRuntime`). `accountFor`, `accountEnv`, `priceUsage` and
//! `recordUsage` are small stand-ins at the end of this file, each marked for T3.4's version at merge.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use protocol::accounts::{AccountKind, UsageChanged};
use protocol::model::{
    ActivityData, ActivityItem, ActivityStatus, Approval, ApprovalDecision, ApprovalKind,
    ApprovalStatus, Bot, BotState, ErrorData, Message, MessageData, MessageKind, MessageStatus,
    Provider, SETUP_SIGN_IN_PREFIX, ToolApprovalData, UserMessageData, message_data,
};
use protocol::wire::{Empty, MessageDelta, Notify, method};
use protocol::{ErrorCode, Event};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use crate::approvals::{
    NewApproval, allow_rules, create_approval, expire_pending_approvals, resolve_approval,
};
use crate::bots::{
    BotStatus, StoredBot, bot_prompt, bot_state_of, get_stored_bot, list_stored_bots,
    set_limited_until, text,
};
use crate::chat::{
    MessagePatch, NewMessage, add_message, build_recap, get_message, history, update_message,
};
use crate::cli::{BoxFuture as CliFuture, CliChild, CliCommand, CliHost, LaunchOptions};
use crate::context::{BotView, Context, Db};
use crate::ids::{new_id, now_iso};
use crate::providers::claude::ClaudeSession;
use crate::providers::codex::CodexSession;
use crate::providers::{
    ApiKey, Approval as ProviderApproval, ApprovalAnswer, Limit, LimitState, McpServer,
    ProviderError, ProviderEvent, ProviderSession, StartOptions, TurnResult as ProviderTurn, Usage,
    js,
};
use crate::queue::{
    NewWork, WorkItem, WorkSource, WorkStatus, enqueue, get_work_item, last_item_for_message,
    new_chain, next_queued, next_steerable, recover_after_restart, set_status,
};
use crate::server::{Handlers, RpcError};

const IDLE_SHUTDOWN: Duration = Duration::from_secs(10 * 60);
const RECAP_MESSAGES: u32 = 20;
const UNKNOWN_RESET_MS: i64 = 60 * 60_000;
const MAX_TIMER_MS: i64 = (1 << 31) - 1;
const RESTARTED: &str = "Tjommie restarted while this was running.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Done,
    Failed,
    Interrupted,
}

impl TurnOutcome {
    fn work_status(self) -> WorkStatus {
        match self {
            TurnOutcome::Done => WorkStatus::Done,
            TurnOutcome::Failed => WorkStatus::Failed,
            TurnOutcome::Interrupted => WorkStatus::Interrupted,
        }
    }

    fn message_status(self) -> MessageStatus {
        match self {
            TurnOutcome::Done => MessageStatus::Done,
            TurnOutcome::Failed => MessageStatus::Failed,
            TurnOutcome::Interrupted => MessageStatus::Interrupted,
        }
    }
}

/// How a turn ended. The last finished text is the answer passed back to a bot that requested this work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnResult {
    pub outcome: TurnOutcome,
    pub text: Option<String>,
    pub text_message_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPhase {
    Discussion,
    Develop,
    Review,
    Test,
}

/// `TaskExecutionContext` from `task-execution.ts` (M5): where and how a task attempt's turn runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskExecutionContext {
    pub key: String,
    pub cwd: String,
    pub attempt_id: String,
    pub revision: i64,
    pub phase: ExecutionPhase,
    pub instructions: String,
}

/// `AccountRunContext` from `account-runtime.ts`: the account one run is billed to.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRunContext {
    pub run_id: String,
    pub bot_id: String,
    pub provider: Provider,
    pub pinned_account_id: String,
    pub effective_account_id: String,
    pub kind: AccountKind,
    pub account_revision: u64,
    pub using_fallback: bool,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountSelection {
    Ready(AccountRunContext),
    NotReady {
        reason: String,
        account_id: String,
        /// `Some` when the account is limited (TypeScript's `"limitedUntil" in selection`), with its reset when known.
        limited_until: Option<Option<String>>,
    },
}

/// `PreparedAccount`: what a run's CLI gets from its account.
#[derive(Debug, Clone)]
pub struct PreparedAccount {
    pub env: BTreeMap<String, String>,
    pub api_key: Option<ApiKey>,
    pub codex_sandbox_ready: bool,
    pub max_budget_usd: Option<f64>,
}

/// The part of T3.4's `AccountRuntime` the runner uses. Implementations get the database the runner already holds and
/// must not call back into the runner synchronously (TypeScript's `onChanged` -> `setupChanged` has to be deferred).
pub trait Accounts: Send + Sync {
    fn inspect(&self, db: &Connection, bot: &StoredBot) -> AccountSelection;
    fn prepare(
        &self,
        context: &AccountRunContext,
    ) -> BoxFuture<'static, Result<PreparedAccount, String>>;
    fn release(&self, context: &AccountRunContext);
    fn assert_current(&self, context: &AccountRunContext) -> Result<(), String>;
    fn complete_turn(&self, context: &AccountRunContext, successful: bool) -> bool;
    fn observe_limit(&self, context: &AccountRunContext, limit: &Limit);
    fn usage_changed(&self, account_id: &str);
}

/// Sends an event to every client. An error is logged; TypeScript's tests throw from it to check that.
///
/// It runs inside both runner locks (database, then state). Its fan-out (the server, the remote transport, push) must
/// never take the database lock or call the runner: queue the event and return.
pub type Broadcast = Arc<dyn Fn(Event) -> Result<(), String> + Send + Sync>;
/// `SessionFactory`: makes the CLI session for a bot. An error fails the turn with its message.
pub type SessionFactory =
    Arc<dyn Fn(&StoredBot, StartOptions) -> Result<Arc<dyn ProviderSession>, String> + Send + Sync>;
/// `console.error(context, error)`.
pub type Log = Arc<dyn Fn(&str, &str) + Send + Sync>;
/// `resolveCli(provider)`: the installed CLI, if any.
pub type ResolveCli = Arc<dyn Fn(Provider) -> Option<CliCommand> + Send + Sync>;

/// `cliSessionFactory`: a Claude or Codex session whose CLI launches through `host`.
pub fn cli_session_factory(host: Arc<dyn CliHost>, resolve: ResolveCli) -> SessionFactory {
    Arc::new(move |bot: &StoredBot, options: StartOptions| {
        let claude = bot.provider == Provider::Claude;
        let Some(cmd) = resolve(bot.provider) else {
            return Err(format!(
                "{} is not installed",
                if claude { "Claude Code" } else { "Codex" }
            ));
        };
        Ok(if claude {
            Arc::new(ClaudeSession::new(host.clone(), cmd, options)) as Arc<dyn ProviderSession>
        } else {
            Arc::new(CodexSession::new(host.clone(), cmd, options))
        })
    })
}

/// Lane B's `win::boundary::WorkerBoundary`, which is itself a [`CliHost`], plus its gate. While Windows worker
/// isolation is on, bot CLIs launch through it under the isolated worker accounts; a launch whose stop it can't
/// confirm ends with `CliExit::Failed`, which the providers report as an exit with an error, a crashed turn.
pub trait WorkerIsolation: CliHost {
    /// `boundary()?.required`.
    fn required(&self) -> bool;
}

/// `launchCli`'s choice (`cli.ts:153-154`): through the worker boundary when isolation requires it, otherwise a
/// direct launch. Codex's app-server goes the same way: `start_codex_server` launches through this host.
pub struct BoundaryHost {
    pub direct: Arc<dyn CliHost>,
    pub boundary: Option<Arc<dyn WorkerIsolation>>,
}

impl CliHost for BoundaryHost {
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> CliFuture<'a, io::Result<CliChild>> {
        match &self.boundary {
            Some(boundary) if boundary.required() => boundary.launch(cmd, args, options),
            _ => self.direct.launch(cmd, args, options),
        }
    }
}

pub type ItemHook = Arc<dyn Fn(&mut Core<'_>, &WorkItem) -> Result<(), String> + Send + Sync>;
pub type McpServersHook = Arc<
    dyn Fn(&mut Core<'_>, &StoredBot, &WorkItem) -> Result<Vec<McpServer>, String> + Send + Sync,
>;
pub type ExecutionContextHook = Arc<
    dyn Fn(&mut Core<'_>, &StoredBot, &WorkItem) -> Result<Option<TaskExecutionContext>, String>
        + Send
        + Sync,
>;
pub type SessionClosedHook = Arc<dyn Fn(&mut Core<'_>, &str) -> Result<(), String> + Send + Sync>;
pub type TurnEndedHook =
    Arc<dyn Fn(&mut Core<'_>, &WorkItem, &TurnResult) -> Result<(), String> + Send + Sync>;
pub type RetryRoutineHook =
    Arc<dyn Fn(&mut Core<'_>, &Message) -> Result<Message, RpcError> + Send + Sync>;
pub type SetupIssue = Arc<dyn Fn(&StoredBot) -> Option<String> + Send + Sync>;
pub type CloseBrowser = Arc<dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync>;

/// `RunnerHooks`. Each runs inside the runner's locks with a [`Core`]; an `Err` is logged like a thrown hook.
#[derive(Clone, Default)]
pub struct RunnerHooks {
    pub mcp_servers: Option<McpServersHook>,
    pub execution_context: Option<ExecutionContextHook>,
    pub session_closed: Option<SessionClosedHook>,
    pub turn_started: Option<ItemHook>,
    pub turn_ended: Option<TurnEndedHook>,
    /// A limit or failed resume restarts the item; clear its timer until `turn_started` re-arms it.
    pub turn_requeued: Option<ItemHook>,
    pub retry_routine: Option<RetryRoutineHook>,
}

/// The `Runner` constructor's `deps`. `cli.ts`'s `killTree` and `browser.ts`'s `closeBrowser` come in here too.
#[derive(Clone)]
pub struct RunnerDeps {
    pub db: Db,
    pub broadcast: Broadcast,
    pub is_signed_in: Arc<dyn Fn(Provider) -> bool + Send + Sync>,
    pub setup_issue: SetupIssue,
    pub is_held: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub codex_sandbox_ready: Arc<dyn Fn() -> bool + Send + Sync>,
    pub create_session: SessionFactory,
    pub accounts: Option<Arc<dyn Accounts>>,
    pub hooks: RunnerHooks,
    /// `killTree(pid)`: stops a CLI's whole process tree without waiting.
    pub kill_tree: Arc<dyn Fn(Option<u32>) + Send + Sync>,
    /// `closeBrowser(botId)`: resolves once the bot's browser tools are closed.
    pub close_browser: CloseBrowser,
    pub log: Log,
}

impl RunnerDeps {
    /// The TypeScript defaults: signed out of nothing, no setup issue, nothing held, Codex's sandbox not ready.
    pub fn new(db: Db, broadcast: Broadcast, create_session: SessionFactory) -> Self {
        RunnerDeps {
            db,
            broadcast,
            is_signed_in: Arc::new(|_| true),
            setup_issue: Arc::new(|_| None),
            is_held: Arc::new(|_| false),
            codex_sandbox_ready: Arc::new(|| false),
            create_session,
            accounts: None,
            hooks: RunnerHooks::default(),
            kill_tree: Arc::new(|_| {}),
            close_browser: Arc::new(|_| futures_util::future::ready(()).boxed()),
            log: Arc::new(|context, error| eprintln!("{context}: {error}")),
        }
    }
}

/// New work to queue, or work already queued with its exchange that only needs waking (`NewWork | WorkItem`).
pub enum Delivery {
    New(NewWork),
    Queued(WorkItem),
}

impl From<NewWork> for Delivery {
    fn from(work: NewWork) -> Self {
        Delivery::New(work)
    }
}

impl From<WorkItem> for Delivery {
    fn from(item: WorkItem) -> Self {
        Delivery::Queued(item)
    }
}

pub fn activity_body(items: &[ActivityItem]) -> String {
    let steps = if items.len() == 1 { "step" } else { "steps" };
    let latest = items.last().map_or("", |item| item.summary.as_str());
    js::slice(&format!("{} {steps}: {latest}", items.len()), 160).to_owned()
}

/// Copies a file the person attached into the bot's inbox, never overwriting an earlier one.
pub fn copy_into_inbox(home_folder: &Path, file: &Path) -> io::Result<String> {
    copy_into_inbox_at(home_folder, file, jiff::Timestamp::now().as_millisecond())
}

/// [`copy_into_inbox`] with `Date.now()` given.
pub fn copy_into_inbox_at(home_folder: &Path, file: &Path, now_ms: i64) -> io::Result<String> {
    let inbox = home_folder.join("inbox");
    fs::create_dir_all(&inbox)?;
    let original = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut name = original.clone();
    if inbox.join(&name).exists() {
        let mut collision = 0;
        loop {
            name = if collision == 0 {
                format!("{now_ms}-{original}")
            } else {
                format!("{now_ms}-{collision}-{original}")
            };
            collision += 1;
            if !inbox.join(&name).exists() {
                break;
            }
        }
    }
    fs::copy(file, inbox.join(&name))?;
    Ok(name)
}

fn retry_data(item: &WorkItem) -> Option<ErrorData> {
    let retry_message_id = item.message_id.clone()?;
    let retry_bot_id = (item.source == WorkSource::Room).then(|| item.bot_id.clone());
    Some(ErrorData {
        retry_message_id,
        retry_bot_id,
    })
}

fn object<T: Serialize>(value: &T) -> Map<String, Value> {
    match serde_json::to_value(value).expect("message data serializes") {
        Value::Object(map) => map,
        _ => unreachable!("message data is an object"),
    }
}

fn answer(decision: ApprovalDecision) -> ApprovalAnswer {
    match decision {
        ApprovalDecision::Once => ApprovalAnswer::Once,
        ApprovalDecision::Always => ApprovalAnswer::Always,
        ApprovalDecision::Deny => ApprovalAnswer::Deny,
    }
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn iso_from_ms(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|time| time.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|_| now_iso())
}

/// `toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) on toLocaleDateString([], { weekday: "long" })`
/// in the local zone. ponytail: en-US wording only; Node followed the PC's locale.
fn reset_time_text(until: &str) -> String {
    match until.parse::<jiff::Timestamp>() {
        Ok(time) => time
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%-I:%M %p on %A")
            .to_string(),
        Err(_) => "Invalid Date on Invalid Date".to_owned(),
    }
}

/// `standaloneExecutionKey` from `task-execution.ts`: the same SHA-256 TypeScript stored, so saved sessions resume.
fn standalone_execution_key(bot: &StoredBot) -> Result<String, String> {
    let folder = bot.project_folder.as_deref().unwrap_or(&bot.home_folder);
    let real = fs::canonicalize(folder)
        .map_err(|error| error.to_string())?
        .display()
        .to_string();
    // realpathSync.native returns `C:\...`, not the `\\?\C:\...` form canonicalize gives.
    let real = match real.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => real.strip_prefix(r"\\?\").unwrap_or(&real).to_owned(),
    };
    let json = serde_json::to_string(&(real, bot.autonomy, &bot.model, &bot.instructions))
        .expect("strings serialize");
    Ok(Sha256::digest(json)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

// ---------------------------------------------------------------------------------------------------------------------
// State

type LiveId = u64;
/// A promise others wait on: resolves once its sender sends or drops.
type Waiter = Shared<BoxFuture<'static, ()>>;
type StopPending = Shared<BoxFuture<'static, Result<bool, ProviderError>>>;
type ShutdownWait = Shared<BoxFuture<'static, Result<(), String>>>;

fn waiter() -> (oneshot::Sender<()>, Waiter) {
    let (sender, receiver) = oneshot::channel::<()>();
    (sender, receiver.map(drop).boxed().shared())
}

/// One CLI session. The serial stands for TypeScript's object identity.
#[derive(Clone)]
struct SessionRef {
    serial: u64,
    session: Arc<dyn ProviderSession>,
}

#[derive(Clone)]
struct Text {
    id: String,
    body: String,
}

#[derive(Clone)]
struct Activity {
    id: String,
    items: Vec<ActivityItem>,
}

/// What the runner knows about a bot that has (or recently had) a CLI session.
#[derive(Clone, Default)]
struct Live {
    session: Option<SessionRef>,
    context: Option<AccountRunContext>,
    execution: Option<TaskExecutionContext>,
    execution_key: String,
    retiring: Option<Waiter>,
    /// Started without resuming.
    fresh: bool,
    /// The CLI reported its session id since it started.
    saw_session: bool,
    /// A turn is running.
    busy: bool,
    /// The current turn's first message has reached the CLI, so more can be steered in.
    sent: bool,
    steering: bool,
    steering_done: Option<(u64, Waiter)>,
    steering_item: Option<WorkItem>,
    /// None while the CLI runs a turn it started by itself. The `Arc` stands for TypeScript's object identity.
    item: Option<Arc<WorkItem>>,
    text: Option<Text>,
    last_text: Option<Text>,
    activity: Option<Activity>,
    stop_requested: bool,
    stop_notice: Option<String>,
    stop_pending: Option<StopPending>,
    limit_hit: bool,
    last_resets_at: Option<String>,
    reload: bool,
    /// Tjommie approval id -> the provider's own request id, in the order they arrived.
    approvals: Vec<(String, String)>,
    idle_timer: Option<(u64, AbortHandle)>,
    limit_timer: Option<(u64, AbortHandle)>,
}

struct Deleting {
    serial: u64,
    live: LiveId,
    items: Vec<WorkItem>,
    pid: Option<u32>,
    process_exited: bool,
    closed: Waiter,
    settled: bool,
    restore_when_closed: bool,
}

#[derive(Default)]
struct State {
    /// Every `Live` by id. ponytail: never removed (detached ones are a few hundred bytes per deleted or restored bot),
    /// so a late callback finds the object it captured, as TypeScript's closures do.
    lives: HashMap<LiveId, Live>,
    /// `#live`, in insertion order like a `Map`.
    live: Vec<(String, LiveId)>,
    deleting: HashMap<String, Deleting>,
    recovered: bool,
    ready: bool,
    shutdown: Option<ShutdownWait>,
    events_closed: bool,
    launches: HashMap<u64, Waiter>,
    task_waiters: HashMap<String, Vec<oneshot::Sender<()>>>,
    serial: u64,
}

struct Inner {
    deps: RunnerDeps,
    state: Mutex<State>,
    handle: Handle,
}

/// `Runner`. Cheap to clone; every clone is the same runner.
#[derive(Clone)]
pub struct Runner(Arc<Inner>);

/// The runner with its database and state locked: what its methods and hooks work on.
pub struct Core<'a> {
    runner: &'a Runner,
    db: &'a Connection,
    st: &'a mut State,
}

impl Runner {
    /// Must be called inside a Tokio runtime: the runner's tasks run on it.
    pub fn new(deps: RunnerDeps) -> Self {
        let state = State {
            ready: true,
            ..State::default()
        };
        Runner(Arc::new(Inner {
            deps,
            state: Mutex::new(state),
            handle: Handle::current(),
        }))
    }

    pub(crate) fn with<T>(&self, work: impl FnOnce(&mut Core<'_>) -> T) -> T {
        let db = self
            .0
            .deps
            .db
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        work(&mut Core {
            runner: self,
            db: &db,
            st: &mut state,
        })
    }

    /// Settles interrupted work before listening. Hooks may queue replies, but cannot start a CLI yet.
    pub fn recover(&self) -> Result<(), RpcError> {
        self.with(|core| core.recover())
    }

    /// Starts pumping, after the daemon knows the listening address its MCP server uses.
    pub fn start(&self) -> Result<(), RpcError> {
        self.with(|core| core.start())
    }

    pub fn to_bot(&self, bot: StoredBot) -> Bot {
        self.with(|core| core.to_bot(bot))
    }

    pub fn process_id(&self, bot_id: &str) -> Option<u32> {
        self.with(|core| core.process_id(bot_id))
    }

    /// None when idle or in a turn the CLI started by itself.
    pub fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        self.with(|core| core.current_work(bot_id))
    }

    pub fn effective_folder(&self, bot_id: &str) -> Result<String, RpcError> {
        self.with(|core| core.effective_folder(bot_id))
    }

    pub fn retired(&self, bot_id: &str) -> BoxFuture<'static, ()> {
        self.with(|core| core.retired(bot_id))
    }

    pub fn submit(&self, bot_id: &str, text: &str, files: &[String]) -> Result<Message, RpcError> {
        self.with(|core| core.submit(bot_id, text, files))
    }

    pub fn deliver(&self, work: impl Into<Delivery>) -> Result<WorkItem, RpcError> {
        self.with(|core| core.deliver(work))
    }

    pub fn retry(&self, message_id: &str, bot_id: Option<&str>) -> Result<Message, RpcError> {
        self.with(|core| core.retry(message_id, bot_id))
    }

    pub fn stop(&self, bot_id: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        self.with(|core| core.stop(bot_id))
    }

    pub fn stop_item(
        &self,
        item_id: &str,
        notice: &str,
    ) -> BoxFuture<'static, Result<(), ProviderError>> {
        self.with(|core| core.stop_item(item_id, notice))
    }

    pub fn answer_approval(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<Approval, RpcError> {
        self.with(|core| core.answer_approval(approval_id, decision))
    }

    pub fn bot_changed(&self, bot_id: &str, provider_changed: bool) -> Result<(), RpcError> {
        self.with(|core| core.bot_changed(bot_id, provider_changed))
    }

    pub fn close_for_deletion(&self, bot_id: &str) -> Result<BoxFuture<'static, ()>, RpcError> {
        self.with(|core| core.close_for_deletion(bot_id))
    }

    pub fn forget(&self, bot_id: &str) {
        self.with(|core| core.forget(bot_id));
    }

    pub fn cancel_deletion(&self, bot_id: &str) {
        self.with(|core| core.cancel_deletion(bot_id));
    }

    pub fn pump(&self, bot_id: &str) {
        self.with(|core| core.pump_logged(bot_id));
    }

    pub fn pump_all(&self) {
        self.with(|core| core.pump_all());
    }

    pub fn setup_changed(&self) {
        self.with(|core| core.setup_changed());
    }

    pub fn shutdown(&self) -> ShutdownWait {
        self.with(|core| core.shutdown())
    }
}

impl BotView for Runner {
    fn to_bot(&self, db: &Connection, bot: StoredBot) -> Bot {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        Core {
            runner: self,
            db,
            st: &mut state,
        }
        .to_bot(bot)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The turn launch: `#startTurn`, a step at a time.

#[derive(Clone)]
struct Account {
    id: String,
    folder: String,
    kind: AccountKind,
}

/// One `#startTurn` call in progress. Each step runs inside the lock and ends at the next `await`.
struct Turn {
    bot: StoredBot,
    item: Arc<WorkItem>,
    /// `#launchTurn`'s `live`.
    launch_live: LiveId,
    /// `#startTurn`'s own `live`.
    live: LiveId,
    execution: Option<TaskExecutionContext>,
    context_key: String,
    text: String,
    account: Option<Account>,
    created: Option<SessionRef>,
    resume_id: Option<String>,
    session: Option<SessionRef>,
}

enum Phase {
    Retired,
    Unretired,
    Prepared,
    Started,
    Sent,
    StopChecked,
    Stopped,
    Steered,
}

enum Awaited {
    Done,
    Prepared(Result<PreparedAccount, String>),
    Result(Result<(), ProviderError>),
    Stop(Result<bool, ProviderError>),
}

enum Step {
    Ready(Result<(), String>),
    Await(Phase, BoxFuture<'static, Awaited>),
}

/// One `#steerQueued` call in progress.
struct Steering {
    bot_id: String,
    live: LiveId,
    serial: u64,
    done: oneshot::Sender<()>,
}

impl Core<'_> {
    pub fn db(&self) -> &Connection {
        self.db
    }

    fn deps(&self) -> &RunnerDeps {
        &self.runner.0.deps
    }

    fn log(&self, context: &str, error: &str) {
        (self.deps().log)(context, error);
    }

    fn next_serial(&mut self) -> u64 {
        self.st.serial += 1;
        self.st.serial
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) -> AbortHandle {
        self.runner.0.handle.spawn(task).abort_handle()
    }

    fn live_id(&self, bot_id: &str) -> Option<LiveId> {
        self.st
            .live
            .iter()
            .find(|(bot, _)| bot == bot_id)
            .map(|(_, id)| *id)
    }

    fn live(&self, id: LiveId) -> &Live {
        self.st.lives.get(&id).expect("lives are never removed")
    }

    fn lv(&mut self, id: LiveId) -> &mut Live {
        self.st.lives.get_mut(&id).expect("lives are never removed")
    }

    fn live_for(&mut self, bot_id: &str) -> LiveId {
        if let Some(id) = self.live_id(bot_id) {
            return id;
        }
        let id = self.next_serial();
        self.st.lives.insert(id, Live::default());
        self.st.live.push((bot_id.to_owned(), id));
        id
    }

    fn dest(&self, bot_id: &str, live: LiveId) -> String {
        self.live(live)
            .item
            .as_ref()
            .map_or_else(|| bot_id.to_owned(), |item| item.conversation_id.clone())
    }

    fn same_item(item: &Option<Arc<WorkItem>>, id: &str) -> bool {
        item.as_ref().is_some_and(|item| item.id == id)
    }

    fn session_serial(&self, live: LiveId) -> Option<u64> {
        self.live(live)
            .session
            .as_ref()
            .map(|session| session.serial)
    }

    // --- hooks ---

    fn safely(&self, outcome: Result<(), String>) {
        if let Err(error) = outcome {
            self.log("Tjommie: a runner hook failed", &error);
        }
    }

    fn turn_started_hook(&mut self, item: &WorkItem) {
        if let Some(hook) = self.deps().hooks.turn_started.clone() {
            let outcome = hook(self, item);
            self.safely(outcome);
        }
    }

    fn turn_ended_hook(&mut self, item: &WorkItem, result: &TurnResult) {
        if let Some(hook) = self.deps().hooks.turn_ended.clone() {
            let outcome = hook(self, item, result);
            self.safely(outcome);
        }
    }

    fn turn_requeued_hook(&mut self, item: &WorkItem) {
        if let Some(hook) = self.deps().hooks.turn_requeued.clone() {
            let outcome = hook(self, item);
            self.safely(outcome);
        }
    }

    fn session_closed_hook(&mut self, bot_id: &str) {
        if let Some(hook) = self.deps().hooks.session_closed.clone() {
            let outcome = hook(self, bot_id);
            self.safely(outcome);
        }
    }

    /// Starting without the requested servers would hide a broken tool configuration, so an error fails the turn.
    fn mcp_servers(&mut self, bot: &StoredBot, item: &WorkItem) -> Result<Vec<McpServer>, String> {
        let Some(hook) = self.deps().hooks.mcp_servers.clone() else {
            return Ok(Vec::new());
        };
        hook(self, bot, item).inspect_err(|error| self.log("Tjommie: a runner hook failed", error))
    }

    // --- events out ---

    fn send(&self, event: Event) {
        if let Err(error) = (self.deps().broadcast)(event) {
            self.log("Tjommie: broadcasting an event failed", &error);
        }
    }

    fn emit_message(&self, message: Message, added: bool) {
        self.send(if added {
            Event::MessageAdded(message)
        } else {
            Event::MessageUpdated(message)
        });
    }

    fn emit_bot(&mut self, bot_id: &str) {
        let bot = match get_stored_bot(self.db, bot_id) {
            Ok(bot) => bot,
            Err(error) => {
                if error.code != ErrorCode::NotFound {
                    self.log("Tjommie: loading a bot failed", &error.message);
                }
                return;
            }
        };
        let shown = self.to_bot(bot);
        if let Err(error) = (self.deps().broadcast)(Event::BotUpdated(shown)) {
            self.log("Tjommie: broadcasting a bot failed", &error);
        }
    }

    fn notice(
        &self,
        conversation_id: &str,
        kind: MessageKind,
        body: &str,
        data: Option<ErrorData>,
    ) -> Result<(), RpcError> {
        let message = NewMessage {
            conversation_id: conversation_id.to_owned(),
            author: "system".into(),
            kind,
            body: js::slice(body, 500).to_owned(),
            data: data.as_ref().map(object),
            status: MessageStatus::Done,
        };
        let message = add_message(self.db, &message)?;
        self.emit_message(message, true);
        Ok(())
    }

    fn set_chat_message_status(
        &self,
        item: &WorkItem,
        status: MessageStatus,
    ) -> Result<(), RpcError> {
        let Some(message_id) = &item.message_id else {
            return Ok(());
        };
        if item.conversation_id != item.bot_id {
            return Ok(());
        }
        let message = get_message(self.db, message_id)?;
        if message.conversation_id != item.bot_id
            || message.author != "you"
            || message.kind != MessageKind::Text
        {
            return Ok(());
        }
        let updated = update_message(
            self.db,
            &message.id,
            &MessagePatch {
                status: Some(status),
                ..MessagePatch::default()
            },
        )?;
        self.emit_message(updated, false);
        Ok(())
    }

    // --- public methods ---

    pub fn recover(&mut self) -> Result<(), RpcError> {
        if self.st.recovered {
            return Ok(());
        }
        self.st.ready = false;
        for item in recover_after_restart(self.db)? {
            self.set_chat_message_status(&item, MessageStatus::Interrupted)?;
            self.notice(
                &item.conversation_id,
                MessageKind::Error,
                RESTARTED,
                retry_data(&item),
            )?;
            let result = TurnResult {
                outcome: TurnOutcome::Failed,
                text: None,
                text_message_id: None,
                error: Some(RESTARTED.into()),
            };
            self.turn_ended_hook(&item, &result);
        }
        let streaming: Vec<String> = self
            .db
            .prepare(
                "SELECT id FROM messages WHERE status = 'streaming' ORDER BY created_at, rowid",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in streaming {
            let message = get_message(self.db, &id)?;
            let data = match message_data(&message) {
                Ok(MessageData::Activity(activity)) if message.kind == MessageKind::Activity => {
                    let items = activity
                        .items
                        .into_iter()
                        .map(|item| match item.status {
                            ActivityStatus::Running => ActivityItem {
                                status: ActivityStatus::Failed,
                                ..item
                            },
                            _ => item,
                        })
                        .collect();
                    Some(Some(object(&ActivityData { items })))
                }
                _ => None,
            };
            let patch = MessagePatch {
                data,
                status: Some(MessageStatus::Interrupted),
                body: None,
            };
            let updated = update_message(self.db, &id, &patch)?;
            self.emit_message(updated, false);
        }
        for approval in expire_pending_approvals(self.db)? {
            self.show_approval_status(&approval)?;
        }
        self.st.recovered = true;
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), RpcError> {
        self.recover()?;
        self.st.ready = true;
        for bot in list_stored_bots(self.db)? {
            if let Some(until) = &bot.limited_until {
                self.arm_limit_timer(&bot.id, until);
            }
            self.pump(&bot.id)?;
        }
        Ok(())
    }

    pub fn to_bot(&mut self, bot: StoredBot) -> Bot {
        let (has_approvals, busy, live_context) =
            match self.live_id(&bot.id).map(|id| self.live(id)) {
                Some(live) => (!live.approvals.is_empty(), live.busy, live.context.clone()),
                None => (false, false, None),
            };
        let deps = self.deps().clone();
        let selected = deps
            .accounts
            .as_ref()
            .map(|accounts| accounts.inspect(self.db, &bot));
        let signed_in = match &selected {
            Some(AccountSelection::Ready(_)) => true,
            Some(AccountSelection::NotReady { limited_until, .. }) => limited_until.is_some(),
            None => (deps.is_signed_in)(bot.provider),
        };
        let setup_reason = match &selected {
            Some(AccountSelection::Ready(_)) => (deps.setup_issue)(&bot),
            Some(AccountSelection::NotReady { reason, .. }) => Some(reason.clone()),
            None if signed_in => (deps.setup_issue)(&bot),
            None => Some(format!(
                "{SETUP_SIGN_IN_PREFIX} {}",
                if bot.provider == Provider::Codex {
                    "Codex"
                } else {
                    "Claude"
                }
            )),
        };
        let routine_waiting = self
            .db
            .query_row("SELECT 1 FROM approvals WHERE bot_id = ?1 AND kind = 'routine' AND status = 'pending' LIMIT 1", [&bot.id], |_| Ok(()))
            .optional()
            .is_ok_and(|row| row.is_some());
        let status = BotStatus {
            signed_in,
            setup_reason: setup_reason.clone(),
            paused: bot.paused,
            limited_until: bot.limited_until.clone(),
            waiting: has_approvals || routine_waiting,
            busy,
        };
        let state = bot_state_of(&status, &now_iso());
        let context = if busy {
            live_context
        } else {
            match &selected {
                Some(AccountSelection::Ready(context)) => Some(context.clone()),
                _ => None,
            }
        };
        let account_limited = matches!(
            &selected,
            Some(AccountSelection::NotReady {
                limited_until: Some(_),
                ..
            })
        );
        let state = if account_limited && !bot.paused {
            BotState::Limited
        } else {
            state
        };
        let pinned = bot.account_id.clone();
        let mut shown = bot.into_bot(state, setup_reason);
        if let Some(selected) = selected {
            shown.effective_account_id = Some(
                context
                    .as_ref()
                    .map(|context| context.effective_account_id.clone())
                    .or(pinned),
            );
            shown.using_fallback = Some(
                context
                    .as_ref()
                    .is_some_and(|context| context.using_fallback),
            );
            shown.limited_until = match selected {
                AccountSelection::NotReady {
                    limited_until: Some(until),
                    ..
                } => until,
                _ => None,
            };
        }
        shown
    }

    pub fn process_id(&self, bot_id: &str) -> Option<u32> {
        self.live_id(bot_id)
            .and_then(|id| self.live(id).session.as_ref())
            .and_then(|session| session.session.pid())
    }

    pub fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        let live = self.live(self.live_id(bot_id)?);
        if live.busy {
            live.item.as_deref().cloned()
        } else {
            None
        }
    }

    pub fn effective_folder(&self, bot_id: &str) -> Result<String, RpcError> {
        if let Some(live) = self.live_id(bot_id).map(|id| self.live(id))
            && live.busy
        {
            if let Some(item) = live
                .item
                .as_ref()
                .filter(|item| item.task_attempt_id.is_some())
            {
                let fits = live.execution.as_ref().filter(|execution| {
                    Some(&execution.key) == item.execution_context_key.as_ref()
                        && Some(&execution.attempt_id) == item.task_attempt_id.as_ref()
                });
                return fits.map(|execution| execution.cwd.clone()).ok_or_else(|| {
                    RpcError::new(
                        ErrorCode::BadState,
                        "The task's folder context is not ready.",
                    )
                });
            }
            if let Some(execution) = &live.execution {
                return Ok(execution.cwd.clone());
            }
        }
        let bot = get_stored_bot(self.db, bot_id)?;
        Ok(bot.project_folder.unwrap_or(bot.home_folder))
    }

    pub fn retired(&mut self, bot_id: &str) -> BoxFuture<'static, ()> {
        let Some(id) = self.live_id(bot_id) else {
            return futures_util::future::ready(()).boxed();
        };
        let live = self.live(id);
        if live.busy
            && live
                .item
                .as_ref()
                .is_some_and(|item| item.task_attempt_id.is_some())
        {
            let (sender, receiver) = oneshot::channel();
            self.st
                .task_waiters
                .entry(bot_id.to_owned())
                .or_default()
                .push(sender);
            return receiver.map(drop).boxed();
        }
        let waits = [
            live.retiring.clone(),
            live.steering_done.as_ref().map(|(_, done)| done.clone()),
        ];
        async move {
            for wait in waits.into_iter().flatten() {
                wait.await;
            }
        }
        .boxed()
    }

    pub fn submit(
        &mut self,
        bot_id: &str,
        text: &str,
        files: &[String],
    ) -> Result<Message, RpcError> {
        let bot = get_stored_bot(self.db, bot_id)?;
        for file in files {
            if !fs::metadata(file).is_ok_and(|metadata| metadata.is_file()) {
                return Err(RpcError::new(
                    ErrorCode::BadParams,
                    format!("Attachment is missing or is not a file: {file}"),
                ));
            }
        }
        let attached = files
            .iter()
            .map(|file| copy_into_inbox(Path::new(&bot.home_folder), Path::new(file)))
            .collect::<io::Result<Vec<_>>>()?;
        let data = (!attached.is_empty()).then(|| {
            object(&UserMessageData {
                files: attached.clone(),
            })
        });
        let message = NewMessage {
            conversation_id: bot_id.to_owned(),
            author: "you".into(),
            kind: MessageKind::Text,
            body: text.to_owned(),
            data,
            status: MessageStatus::Queued,
        };
        let message = add_message(self.db, &message)?;
        self.emit_message(message.clone(), true);
        let prompt = if attached.is_empty() {
            text.to_owned()
        } else {
            format!(
                "{text}\n\n[Attached files, now in your inbox folder: {}]",
                attached.join(", ")
            )
        };
        let chain_id = new_chain(self.db)?;
        let work = NewWork {
            message_id: Some(message.id.clone()),
            chain_id: Some(chain_id),
            ..NewWork::new(bot_id, WorkSource::You, &prompt)
        };
        self.deliver(work)?;
        get_message(self.db, &message.id)
    }

    /// Starts or steers new work, or wakes work already queued atomically with its exchange.
    pub fn deliver(&mut self, work: impl Into<Delivery>) -> Result<WorkItem, RpcError> {
        let item = match work.into() {
            Delivery::Queued(item) => get_work_item(self.db, &item.id)?,
            Delivery::New(work) => enqueue(self.db, &work)?,
        };
        if let Some(deleting) = self.st.deleting.get_mut(&item.bot_id) {
            if !deleting.items.iter().any(|captured| captured.id == item.id) {
                deleting.items.push(item.clone());
            }
            return Ok(item);
        }
        let live = self.live_for(&item.bot_id);
        let (busy, sent) = (self.live(live).busy, self.live(live).sent);
        if !busy {
            self.pump(&item.bot_id)?;
        } else if sent && let Some(rest) = self.steer_queued(&item.bot_id) {
            self.spawn(rest);
        }
        Ok(item)
    }

    pub fn retry(&mut self, message_id: &str, bot_id: Option<&str>) -> Result<Message, RpcError> {
        let mut previous =
            last_item_for_message(self.db, message_id, bot_id)?.ok_or_else(|| {
                RpcError::new(
                    ErrorCode::NotFound,
                    "There's nothing to retry for this message",
                )
            })?;
        let retryable =
            |item: &WorkItem| matches!(item.status, WorkStatus::Failed | WorkStatus::Interrupted);
        if previous.source == WorkSource::Room && bot_id.is_none() {
            let bots: Vec<String> = self
                .db
                .prepare("SELECT DISTINCT bot_id FROM work_items WHERE message_id = ?1")?
                .query_map([message_id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut failed = Vec::new();
            for bot in &bots {
                if let Some(item) =
                    last_item_for_message(self.db, message_id, Some(bot))?.filter(retryable)
                {
                    failed.push(item);
                }
            }
            if failed.len() > 1 {
                return Err(RpcError::new(
                    ErrorCode::BadParams,
                    "Choose the bot whose room turn you want to retry",
                ));
            }
            if let Some(only) = failed.pop() {
                previous = only;
            }
        }
        if !retryable(&previous) {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "Only failed or interrupted work can be retried",
            ));
        }
        if previous.source == WorkSource::Routine {
            let Some(hook) = self.deps().hooks.retry_routine.clone() else {
                return Err(RpcError::new(
                    ErrorCode::BadState,
                    "Routine retry is unavailable",
                ));
            };
            let message = get_message(self.db, message_id)?;
            return hook(self, &message);
        }
        if previous.task_attempt_id.is_some() {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "Resume this attempt from its project task.",
            ));
        }
        get_message(self.db, message_id)?;
        self.set_chat_message_status(&previous, MessageStatus::Queued)?;
        let chain_id = new_chain(self.db)?;
        self.deliver(NewWork {
            message_id: Some(message_id.to_owned()),
            conversation_id: Some(previous.conversation_id.clone()),
            exchange_id: previous.exchange_id.clone(),
            chain_id: Some(chain_id),
            ..NewWork::new(&previous.bot_id, previous.source, &previous.text)
        })?;
        get_message(self.db, message_id)
    }

    pub fn stop(&mut self, bot_id: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        let Some(id) = self.live_id(bot_id) else {
            return futures_util::future::ready(Ok(())).boxed();
        };
        if !self.live(id).busy {
            if self.deps().accounts.is_some() && self.live(id).session.is_some() {
                self.close_session(bot_id, id);
                let retiring = self.live(id).retiring.clone();
                return async move {
                    if let Some(retiring) = retiring {
                        retiring.await;
                    }
                    Ok(())
                }
                .boxed();
            }
            return futures_util::future::ready(Ok(())).boxed();
        }
        let live = self.lv(id);
        let session = live.session.clone();
        let item = live.item.clone();
        live.stop_requested = true;
        let pending: StopPending = match &session {
            Some(session) => session.session.interrupt().shared(),
            None => futures_util::future::ready(Ok(false)).boxed().shared(),
        };
        self.lv(id).stop_pending = Some(pending.clone());
        self.spawn(pending.clone().map(drop));
        let runner = self.runner.clone();
        let bot_id = bot_id.to_owned();
        let serial = session.map(|session| session.serial);
        async move {
            let result = pending.await;
            if result.is_err() {
                runner.with(|core| {
                    if core.live_id(&bot_id) != Some(id) || core.session_serial(id) != serial {
                        return;
                    }
                    let live = core.lv(id);
                    let same = match (&live.item, &item) {
                        (Some(now), Some(then)) => Arc::ptr_eq(now, then),
                        (None, None) => true,
                        _ => false,
                    };
                    if same && live.busy {
                        live.stop_requested = false;
                        live.stop_notice = None;
                    }
                });
            }
            result.map(drop)
        }
        .boxed()
    }

    /// A stale routine timer must never interrupt a later item on the same bot.
    pub fn stop_item(
        &mut self,
        item_id: &str,
        notice: &str,
    ) -> BoxFuture<'static, Result<(), ProviderError>> {
        let found = self.st.live.iter().find(|(_, id)| {
            let live = &self.st.lives[id];
            live.busy && Self::same_item(&live.item, item_id)
        });
        let Some((bot_id, id)) = found.cloned() else {
            return futures_util::future::ready(Ok(())).boxed();
        };
        self.lv(id).stop_notice = Some(notice.to_owned());
        self.stop(&bot_id)
    }

    pub fn answer_approval(
        &mut self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<Approval, RpcError> {
        let pending: Option<String> = self
            .db
            .query_row(
                "SELECT bot_id FROM approvals WHERE id=?1",
                [approval_id],
                |row| row.get(0),
            )
            .optional()?;
        if decision != ApprovalDecision::Deny
            && pending.is_some_and(|bot| (self.deps().is_held)(&bot))
        {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "Resume the bot before allowing tools.",
            ));
        }
        let approval = resolve_approval(self.db, approval_id, decision)?;
        if let Some(id) = self.live_id(&approval.bot_id) {
            let live = self.lv(id);
            if let Some(index) = live.approvals.iter().position(|(id, _)| id == approval_id) {
                let (_, provider_id) = live.approvals.remove(index);
                if let Some(session) = &live.session {
                    session.session.answer(&provider_id, answer(decision));
                }
            }
        }
        self.show_approval_status(&approval)?;
        self.emit_bot(&approval.bot_id);
        Ok(approval)
    }

    /// Settings changed. The next turn restarts the CLI with the new settings. The conversation carries on unless the
    /// provider changed.
    pub fn bot_changed(&mut self, bot_id: &str, provider_changed: bool) -> Result<(), RpcError> {
        if provider_changed {
            self.db
                .execute("DELETE FROM sessions WHERE bot_id = ?1", [bot_id])?;
        }
        let Some(id) = self.live_id(bot_id) else {
            return Ok(());
        };
        if self.live(id).session.is_none() {
            return Ok(());
        }
        if self.live(id).busy {
            self.lv(id).reload = true;
        } else {
            self.close_session(bot_id, id);
        }
        Ok(())
    }

    /// Releases folder locks without changing persisted work; only explicit cancellation may resume it.
    pub fn close_for_deletion(&mut self, bot_id: &str) -> Result<BoxFuture<'static, ()>, RpcError> {
        if let Some(existing) = self.st.deleting.get(bot_id) {
            if existing.restore_when_closed {
                return Err(RpcError::new(
                    ErrorCode::Busy,
                    "The bot is still closing. Try again shortly.",
                ));
            }
            return Ok(existing.closed.clone().boxed());
        }
        let live = self.live_for(bot_id);
        let ids: Vec<String> = self
            .db
            .prepare("SELECT id FROM work_items WHERE bot_id = ?1 AND status IN ('queued', 'running') ORDER BY created_at, rowid")?
            .query_map([bot_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut items = ids
            .iter()
            .map(|id| get_work_item(self.db, id))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(steering) = self.live(live).steering_item.clone()
            && !items.iter().any(|item| item.id == steering.id)
        {
            items.push(steering);
        }
        let session = self.live(live).session.clone();
        let serial = self.next_serial();
        let (done, closed) = waiter();
        let pid = session.as_ref().and_then(|session| session.session.pid());
        let detached = Deleting {
            serial,
            live,
            items,
            pid,
            process_exited: false,
            closed: closed.clone(),
            settled: false,
            restore_when_closed: false,
        };
        self.st.deleting.insert(bot_id.to_owned(), detached);
        self.st.live.retain(|(bot, _)| bot != bot_id);
        self.flush_text(live, MessageStatus::Interrupted)?;
        self.close_activity(live, ActivityStatus::Failed)?;
        self.release_session(bot_id, live);
        let exited = session.map(|session| session.session.wait_for_exit());
        let browser = (self.deps().close_browser)(bot_id);
        let runner = self.runner.clone();
        let bot = bot_id.to_owned();
        self.spawn(async move {
            let process = async {
                if let Some(exited) = exited {
                    exited.await;
                }
                runner.with(|core| {
                    if let Some(deleting) = core
                        .st
                        .deleting
                        .get_mut(&bot)
                        .filter(|deleting| deleting.serial == serial)
                    {
                        deleting.process_exited = true;
                    }
                });
            };
            futures_util::join!(process, browser);
            runner.with(|core| {
                if let Some(deleting) = core
                    .st
                    .deleting
                    .get_mut(&bot)
                    .filter(|deleting| deleting.serial == serial)
                {
                    deleting.settled = true;
                }
            });
            let _ = done.send(());
        });
        Ok(closed.boxed())
    }

    /// Call only after deletion succeeds. Captured work survives the database cascade long enough for hooks.
    pub fn forget(&mut self, bot_id: &str) {
        if !self.st.deleting.contains_key(bot_id)
            && self.live_id(bot_id).is_some()
            && let Err(error) = self.close_for_deletion(bot_id)
        {
            self.log("Tjommie: closing a deleted bot failed", &error.message);
        }
        let Some(detached) = self.st.deleting.remove(bot_id) else {
            return;
        };
        let result = TurnResult {
            outcome: TurnOutcome::Interrupted,
            text: None,
            text_message_id: None,
            error: Some("The bot was deleted.".into()),
        };
        for item in &detached.items {
            self.turn_ended_hook(item, &result);
        }
    }

    pub fn cancel_deletion(&mut self, bot_id: &str) {
        if let Err(error) = self.try_cancel_deletion(bot_id) {
            self.log(
                "Tjommie: restoring a bot after deletion failed",
                &error.message,
            );
        }
    }

    fn try_cancel_deletion(&mut self, bot_id: &str) -> Result<(), RpcError> {
        let Some(detached) = self.st.deleting.get_mut(bot_id) else {
            return Ok(());
        };
        if !detached.settled {
            if !detached.restore_when_closed {
                detached.restore_when_closed = true;
                let (closed, serial) = (detached.closed.clone(), detached.serial);
                let runner = self.runner.clone();
                let bot = bot_id.to_owned();
                self.spawn(async move {
                    closed.await;
                    runner.with(|core| {
                        if core
                            .st
                            .deleting
                            .get(&bot)
                            .is_some_and(|deleting| deleting.serial == serial)
                        {
                            core.cancel_deletion(&bot);
                        }
                    });
                });
            }
            return Ok(());
        }
        let detached = self.st.deleting.remove(bot_id).expect("checked above");
        self.flush_text(detached.live, MessageStatus::Interrupted)?;
        self.close_activity(detached.live, ActivityStatus::Failed)?;
        // A new object, so pending callbacks from the closed session cannot change resumed work. Settled means the
        // process exited, so nothing is retiring and the old run's account lease is gone. `close_session`'s own exit
        // wait may not have run yet: on a multi-thread runtime nothing orders it before ours, where TypeScript's
        // promise reactions ran first-in, first-out.
        let old = self.live(detached.live).clone();
        let live = self.next_serial();
        let restored = Live {
            session: None,
            retiring: None,
            context: None,
            sent: false,
            steering: false,
            steering_done: None,
            steering_item: None,
            text: None,
            activity: None,
            last_text: None,
            stop_pending: None,
            idle_timer: None,
            limit_timer: None,
            ..old.clone()
        };
        self.st.lives.insert(live, restored);
        self.st.live.push((bot_id.to_owned(), live));
        if let Some(steering) = &old.steering_item {
            set_status(self.db, &steering.id, WorkStatus::Queued)?;
        }
        let bot = get_stored_bot(self.db, bot_id)?;
        // These cards refer to the closed provider session, so its replacement can't answer them.
        self.expire_approvals(live)?;
        if let Some(until) = &bot.limited_until {
            self.arm_limit_timer(bot_id, until);
        }
        let (busy, stop_requested, item) = (
            self.live(live).busy,
            self.live(live).stop_requested,
            self.live(live).item.clone(),
        );
        if busy && stop_requested {
            return self.finish_turn(bot_id, false, None);
        }
        if busy && let Some(item) = item {
            // Keep the same work and its routine timer after a failed folder removal.
            self.launch_turn(bot, item, live);
            return Ok(());
        }
        self.lv(live).busy = false;
        self.pump(bot_id)
    }

    fn pump_logged(&mut self, bot_id: &str) {
        if let Err(error) = self.pump(bot_id) {
            self.log("Tjommie: starting queued work failed", &error.message);
        }
    }

    pub fn pump(&mut self, bot_id: &str) -> Result<(), RpcError> {
        if !self.st.ready || self.st.deleting.contains_key(bot_id) || (self.deps().is_held)(bot_id)
        {
            return Ok(());
        }
        let live = self.live_for(bot_id);
        if self.live(live).busy || self.live(live).retiring.is_some() {
            return Ok(());
        }
        let bot = match get_stored_bot(self.db, bot_id) {
            Ok(bot) => bot,
            Err(error) => {
                if error.code != ErrorCode::NotFound {
                    self.log("Tjommie: loading a bot failed", &error.message);
                }
                return Ok(());
            }
        };
        // A routine proposal changes the displayed state, but does not block work.
        let deps = self.deps().clone();
        let selection = deps
            .accounts
            .as_ref()
            .map(|accounts| accounts.inspect(self.db, &bot));
        if let Some(AccountSelection::NotReady {
            limited_until: Some(Some(until)),
            ..
        }) = &selection
            && !until.is_empty()
        {
            self.arm_limit_timer(bot_id, until);
        }
        let unready = match &selection {
            Some(selection) => !matches!(selection, AccountSelection::Ready(_)),
            None => !(deps.is_signed_in)(bot.provider),
        };
        let limited = selection.is_none()
            && bot
                .limited_until
                .as_deref()
                .is_some_and(|until| until > now_iso().as_str());
        let blocked = !self.live(live).approvals.is_empty() || self.live(live).busy;
        if unready || (deps.setup_issue)(&bot).is_some() || bot.paused || limited || blocked {
            self.arm_idle(bot_id, live);
            self.emit_bot(bot_id);
            return Ok(());
        }
        let Some(item) = next_queued(self.db, bot_id)? else {
            self.arm_idle(bot_id, live);
            return Ok(());
        };
        if let Some(AccountSelection::Ready(selected)) = &selection {
            match &self.live(live).context {
                Some(current)
                    if current.effective_account_id != selected.effective_account_id
                        || current.account_revision != selected.account_revision =>
                {
                    self.close_session(bot_id, live);
                    return Ok(());
                }
                Some(_) => {}
                None => self.lv(live).context = Some(selected.clone()),
            }
        }
        let item = Arc::new(item);
        let state = self.lv(live);
        state.busy = true; // set before any await, so a second pump can't start another turn
        state.sent = false;
        state.item = Some(item.clone());
        state.stop_requested = false;
        state.stop_notice = None;
        state.stop_pending = None;
        state.limit_hit = false;
        state.last_text = None;
        if let Some((_, timer)) = state.idle_timer.take() {
            timer.abort();
        }
        set_status(self.db, &item.id, WorkStatus::Running)?;
        self.set_chat_message_status(&item, MessageStatus::Sent)?;
        self.emit_bot(bot_id);
        self.turn_started_hook(&item);
        self.launch_turn(bot, item, live);
        Ok(())
    }

    pub fn pump_all(&mut self) {
        match list_stored_bots(self.db) {
            Ok(bots) => bots.iter().for_each(|bot| self.pump_logged(&bot.id)),
            Err(error) => self.log("Tjommie: loading bots failed", &error.message),
        }
    }

    pub fn setup_changed(&mut self) {
        let bots = match list_stored_bots(self.db) {
            Ok(bots) => bots,
            Err(error) => return self.log("Tjommie: loading bots failed", &error.message),
        };
        let deps = self.deps().clone();
        for bot in &bots {
            if (deps.is_held)(&bot.id) {
                continue;
            }
            let live = self.live_id(&bot.id);
            let changed = match (
                &deps.accounts,
                live.and_then(|id| self.live(id).context.clone()),
            ) {
                (Some(accounts), Some(context)) => accounts.assert_current(&context).is_err(),
                _ => false,
            };
            if (deps.setup_issue)(bot).is_none() && !changed {
                continue;
            }
            let Some(live) = live else { continue };
            if self.live(live).session.is_none() || self.live(live).limit_hit {
                continue;
            }
            if self.live(live).busy {
                self.lv(live).reload = true;
                if let Some(item) = self.live(live).item.clone() {
                    let notice = if deps.accounts.is_some() {
                        "This account or its setup changed. Check Accounts before continuing."
                    } else {
                        "Codex's sandbox is no longer ready. Set it up again to continue."
                    };
                    let stopping = self.stop_item(&item.id, notice);
                    let log = deps.log.clone();
                    self.spawn(async move {
                        if let Err(error) = stopping.await {
                            log("Could not stop a bot after setup changed.", error.message());
                        }
                    });
                }
            } else {
                self.close_session(&bot.id, live);
            }
        }
        self.pump_all();
    }

    pub fn shutdown(&mut self) -> ShutdownWait {
        if let Some(shutdown) = &self.st.shutdown {
            return shutdown.clone();
        }
        self.st.ready = false;
        let lives = std::mem::take(&mut self.st.live);
        let mut pending: Vec<Waiter> = self.st.launches.values().cloned().collect();
        let deleting: Vec<Deleting> = self
            .st
            .deleting
            .drain()
            .map(|(_, deleting)| deleting)
            .collect();
        let kill_tree = self.deps().kill_tree.clone();
        for detached in &deleting {
            if self.live(detached.live).busy && !detached.process_exited {
                kill_tree(detached.pid);
            }
            pending.push(detached.closed.clone());
        }
        for (bot_id, live) in &lives {
            let session = self.live(*live).session.clone();
            if self.live(*live).busy {
                kill_tree(session.as_ref().and_then(|session| session.session.pid()));
            }
            self.release_session(bot_id, *live);
            pending.push(match (self.live(*live).retiring.clone(), session) {
                (Some(retiring), _) => retiring,
                (None, Some(session)) => session.session.wait_for_exit().shared(),
                (None, None) => futures_util::future::ready(()).boxed().shared(),
            });
        }
        // Production teardown awaits this bounded barrier before closing SQLite; late usage stays billable until then.
        if self.deps().accounts.is_none() {
            self.st.events_closed = true;
            let done = futures_util::future::ready(Ok(())).boxed().shared();
            self.st.shutdown = Some(done.clone());
            return done;
        }
        let runner = self.runner.clone();
        let waiting = async move {
            let all = futures_util::future::join_all(pending);
            let result = match tokio::time::timeout(Duration::from_secs(30), all).await {
                Ok(_) => Ok(()),
                Err(_) => Err("Timed out waiting for owned account processes to close.".to_owned()),
            };
            runner.with(|core| core.st.events_closed = true);
            result
        }
        .boxed()
        .shared();
        self.spawn(waiting.clone().map(drop));
        self.st.shutdown = Some(waiting.clone());
        waiting
    }

    // --- the turn ---

    fn launch_turn(&mut self, bot: StoredBot, item: Arc<WorkItem>, live: LiveId) {
        let launch = self.next_serial();
        let (done, launching) = waiter();
        self.st.launches.insert(launch, launching);
        let mut turn = Turn {
            bot,
            item,
            launch_live: live,
            live,
            execution: None,
            context_key: String::new(),
            text: String::new(),
            account: None,
            created: None,
            resume_id: None,
            session: None,
        };
        let first = self.turn_begin(&mut turn);
        let runner = self.runner.clone();
        self.spawn(async move {
            let mut step = first;
            let result = loop {
                match step {
                    Step::Ready(result) => break result,
                    Step::Await(phase, pending) => {
                        let awaited = pending.await;
                        step = runner.with(|core| core.turn_resume(&mut turn, phase, awaited));
                    }
                }
            };
            runner.with(|core| {
                if let Err(error) = result {
                    core.launch_failed(&turn, &error);
                }
                core.st.launches.remove(&launch);
            });
            let _ = done.send(());
        });
    }

    fn launch_failed(&mut self, turn: &Turn, error: &str) {
        let live = turn.launch_live;
        if self.live_id(&turn.bot.id) != Some(live)
            || !Self::same_item(&self.live(live).item, &turn.item.id)
        {
            return;
        }
        if self.live(live).session.is_none()
            && self.live(live).retiring.is_none()
            && let Some(context) = self.lv(live).context.take()
            && let Some(accounts) = self.deps().accounts.clone()
        {
            accounts.release(&context);
        }
        if let Err(failure) = self.finish_turn(&turn.bot.id, false, Some(error.to_owned())) {
            self.log("Tjommie: finishing a turn failed", &failure.message);
        }
    }

    fn same_turn(&self, turn: &Turn) -> bool {
        let live = self.live(turn.live);
        self.live_id(&turn.bot.id) == Some(turn.live)
            && live.busy
            && Self::same_item(&live.item, &turn.item.id)
    }

    /// `#finishTurn(bot, false, null)` and return: a Stop arrived while the turn was starting.
    fn finish_stopped(&mut self, bot_id: &str) -> Step {
        if let Err(error) = self.finish_turn(bot_id, false, None) {
            self.log("Tjommie: finishing a turn failed", &error.message);
        }
        Step::Ready(Ok(()))
    }

    fn turn_resume(&mut self, turn: &mut Turn, phase: Phase, awaited: Awaited) -> Step {
        match (phase, awaited) {
            (Phase::Retired, _) => {
                if self.same_turn(turn) {
                    self.turn_prepare(turn)
                } else {
                    Step::Ready(Ok(()))
                }
            }
            (Phase::Unretired, _) => self.turn_account(turn),
            (Phase::Prepared, Awaited::Prepared(prepared)) => match prepared {
                Ok(prepared) => self.turn_create(turn, Some(prepared)),
                Err(error) => Step::Ready(Err(error)),
            },
            (Phase::Started, Awaited::Result(result)) => self.turn_started(turn, result),
            (Phase::Sent, Awaited::Result(result)) => self.turn_sent(turn, result),
            (Phase::StopChecked, Awaited::Stop(result)) => match result {
                // stop() reports the rejection and clears Stop. The provider still owns this turn's outcome.
                Err(_) => Step::Ready(Ok(())),
                Ok(handled) => self.turn_stop_checked(turn, handled),
            },
            (Phase::Stopped, Awaited::Result(result)) => {
                Step::Ready(result.map_err(|error| error.message().to_owned()))
            }
            _ => Step::Ready(Ok(())),
        }
    }

    fn turn_begin(&mut self, turn: &mut Turn) -> Step {
        let live = self.live_for(&turn.bot.id);
        turn.live = live;
        if (self.deps().setup_issue)(&turn.bot).is_some() {
            return Step::Ready(Err("The bot needs setup before it can run.".into()));
        }
        if self.live(live).stop_requested {
            return self.finish_stopped(&turn.bot.id);
        }
        let execution = match self.deps().hooks.execution_context.clone() {
            Some(hook) => match hook(self, &turn.bot, &turn.item) {
                Ok(execution) => execution,
                Err(error) => return Step::Ready(Err(error)),
            },
            None => None,
        };
        if let Some(attempt) = &turn.item.task_attempt_id {
            let fits = execution.as_ref().is_some_and(|execution| {
                &execution.attempt_id == attempt
                    && Some(execution.revision) == turn.item.task_revision
                    && Some(&execution.key) == turn.item.execution_context_key.as_ref()
            });
            if !fits {
                return Step::Ready(Err("The task execution context is missing or stale.".into()));
            }
        }
        if execution.as_ref().is_some_and(|execution| {
            matches!(
                execution.phase,
                ExecutionPhase::Review | ExecutionPhase::Test
            )
        }) {
            return Step::Ready(Err(
                "review_isolation_unavailable: review and configured checks require their isolated execution service.".into(),
            ));
        }
        turn.context_key = match &execution {
            Some(execution) => execution.key.clone(),
            None => match standalone_execution_key(&turn.bot) {
                Ok(key) => key,
                Err(error) => return Step::Ready(Err(error)),
            },
        };
        turn.execution = execution;
        let old = self.live(live).session.clone();
        if let Some(old) = old
            && self.live(live).execution_key != turn.context_key
        {
            self.close_session(&turn.bot.id, live);
            let exited = match self.live(live).retiring.clone() {
                Some(retiring) => retiring.boxed(),
                None => old.session.wait_for_exit(),
            };
            return Step::Await(Phase::Retired, exited.map(|_| Awaited::Done).boxed());
        }
        self.turn_prepare(turn)
    }

    fn turn_prepare(&mut self, turn: &mut Turn) -> Step {
        let live = self.lv(turn.live);
        live.execution = turn.execution.clone();
        live.execution_key = turn.context_key.clone();
        turn.text = turn.item.text.clone();
        match (live.session.clone(), live.retiring.clone()) {
            (Some(session), _) => {
                turn.session = Some(session);
                self.turn_send(turn)
            }
            (None, Some(retiring)) => {
                Step::Await(Phase::Unretired, retiring.map(|_| Awaited::Done).boxed())
            }
            (None, None) => self.turn_account(turn),
        }
    }

    fn turn_account(&mut self, turn: &mut Turn) -> Step {
        if !self.same_turn(turn) {
            return Step::Ready(Ok(()));
        }
        let accounts = self.deps().accounts.clone();
        if let Some(accounts) = &accounts
            && self.live(turn.live).context.is_none()
        {
            match accounts.inspect(self.db, &turn.bot) {
                AccountSelection::Ready(context) => self.lv(turn.live).context = Some(context),
                AccountSelection::NotReady { reason, .. } => return Step::Ready(Err(reason)),
            }
        }
        let context = self.live(turn.live).context.clone();
        let account = match &context {
            Some(context) => account_by_id(self.db, &context.effective_account_id),
            None => account_for(self.db, turn.bot.provider),
        };
        match account {
            Ok(account) => turn.account = Some(account),
            Err(error) => return Step::Ready(Err(error.message)),
        }
        if let (Some(context), Some(accounts)) = (context, accounts) {
            return Step::Await(
                Phase::Prepared,
                accounts.prepare(&context).map(Awaited::Prepared).boxed(),
            );
        }
        self.turn_create(turn, None)
    }

    fn turn_create(&mut self, turn: &mut Turn, prepared: Option<PreparedAccount>) -> Step {
        let accounts = self.deps().accounts.clone();
        if !self.same_turn(turn) || self.live(turn.live).stop_requested {
            if let (Some(context), Some(accounts)) = (&self.live(turn.live).context, &accounts) {
                accounts.release(context);
            }
            if self.live_id(&turn.bot.id) == Some(turn.live) && self.live(turn.live).busy {
                return self.finish_stopped(&turn.bot.id);
            }
            return Step::Ready(Ok(()));
        }
        if let (Some(context), Some(accounts)) = (&self.live(turn.live).context, &accounts)
            && let Err(error) = accounts.assert_current(context)
        {
            return Step::Ready(Err(error));
        }
        let account = turn.account.clone().expect("the account step ran");
        let resume = match self.saved_resume(turn, &account) {
            Ok(resume) => resume,
            Err(error) => return Step::Ready(Err(error.message)),
        };
        let allow_rules = match allow_rules(self.db, &turn.bot.id) {
            Ok(rules) => rules,
            Err(error) => return Step::Ready(Err(error.message)),
        };
        let bot = &turn.bot;
        let execution = &turn.execution;
        let prompt_bot = match execution {
            Some(execution) => StoredBot {
                project_folder: (execution.phase != ExecutionPhase::Discussion)
                    .then(|| execution.cwd.clone()),
                ..bot.clone()
            },
            None => bot.clone(),
        };
        let instructions = [
            Some(bot_prompt(&prompt_bot)),
            execution
                .as_ref()
                .map(|execution| execution.instructions.clone()),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
        let resume_id = resume.as_ref().map(|(id, _)| id.clone());
        let mut options = StartOptions {
            cwd: execution
                .as_ref()
                .map(|execution| execution.cwd.clone())
                .or_else(|| bot.project_folder.clone())
                .unwrap_or_else(|| bot.home_folder.clone()),
            extra_dirs: if execution.is_some() || bot.project_folder.is_some() {
                vec![bot.home_folder.clone()]
            } else {
                Vec::new()
            },
            model: bot.model.clone(),
            instructions,
            autonomy: bot.autonomy,
            codex_sandbox_ready: prepared.as_ref().map_or_else(
                || (self.deps().codex_sandbox_ready)(),
                |prepared| prepared.codex_sandbox_ready,
            ),
            allow_rules,
            resume_id: resume_id.clone(),
            resume_cost_usd: resume.and_then(|(_, cost)| cost),
            env: prepared.as_ref().map_or_else(
                || account_env(bot.provider, &account),
                |prepared| prepared.env.clone(),
            ),
            api_key: prepared
                .as_ref()
                .and_then(|prepared| prepared.api_key.clone()),
            max_budget_usd: prepared
                .as_ref()
                .and_then(|prepared| prepared.max_budget_usd),
            mcp_servers: Vec::new(),
        };
        let (bot, item) = (turn.bot.clone(), turn.item.clone());
        let built = self.mcp_servers(&bot, &item).and_then(|servers| {
            options.mcp_servers = servers;
            (self.deps().create_session)(&bot, options)
        });
        let created = match built {
            Ok(created) => created,
            Err(error) => {
                // The MCP hook can issue a token even when no session was made.
                self.session_closed_hook(&bot.id);
                if let (Some(context), Some(accounts)) =
                    (self.lv(turn.live).context.take(), &accounts)
                {
                    accounts.release(&context);
                }
                return Step::Ready(Err(error));
            }
        };
        let identity = self
            .live(turn.live)
            .context
            .clone()
            .unwrap_or_else(|| AccountRunContext {
                run_id: new_id(),
                bot_id: bot.id.clone(),
                provider: bot.provider,
                pinned_account_id: account.id.clone(),
                effective_account_id: account.id.clone(),
                kind: account.kind,
                account_revision: 0,
                using_fallback: false,
                model: bot.model.clone(),
            });
        let session = SessionRef {
            serial: self.next_serial(),
            session: created.clone(),
        };
        let live = self.lv(turn.live);
        live.session = Some(session.clone());
        live.fresh = resume_id.is_none();
        live.saw_session = false;
        self.forward_events(&bot.id, &session, identity, turn.context_key.clone());
        turn.created = Some(session);
        turn.resume_id = resume_id;
        Step::Await(Phase::Started, created.start().map(Awaited::Result).boxed())
    }

    /// The saved CLI session to resume, with its last reported cost, when it belongs to this provider, account and
    /// folder context.
    fn saved_resume(
        &self,
        turn: &Turn,
        account: &Account,
    ) -> Result<Option<(String, Option<f64>)>, RpcError> {
        let provider = text(&turn.bot.provider);
        let latest: Option<(String, String)> = self
            .db
            .query_row(
                "SELECT provider, account_id FROM sessions WHERE bot_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT 1",
                [&turn.bot.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if latest.is_some_and(|(latest_provider, latest_account)| {
            latest_account != account.id || latest_provider != provider
        }) {
            return Ok(None);
        }
        let saved: Option<(String, Option<f64>, String)> = self
            .db
            .query_row(
                "SELECT cli_session_id, total_cost_usd, execution_context_key FROM sessions WHERE bot_id = ?1 AND provider = ?2 AND account_id = ?3",
                params![turn.bot.id, provider, account.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let api_key_claude = self
            .live(turn.live)
            .context
            .as_ref()
            .is_some_and(|context| context.kind == AccountKind::ApiKey)
            && turn.bot.provider == Provider::Claude;
        Ok(saved
            .filter(|(_, cost, key)| {
                (*key == turn.context_key || (turn.execution.is_none() && key.is_empty()))
                    && !(api_key_claude && cost.is_none())
            })
            .map(|(id, cost, _)| (id, cost)))
    }

    fn turn_started(&mut self, turn: &mut Turn, result: Result<(), ProviderError>) -> Step {
        let created = turn.created.clone().expect("the session was made");
        if self.session_serial(turn.live) != Some(created.serial) {
            created.session.close();
            return Step::Ready(Ok(()));
        }
        if let Err(error) = result {
            self.close_session(&turn.bot.id, turn.live);
            let live = self.live(turn.live);
            if !live.busy || !Self::same_item(&live.item, &turn.item.id) {
                return Step::Ready(Ok(())); // an exit already settled this turn
            }
            if live.stop_requested {
                return self.finish_stopped(&turn.bot.id);
            }
            if turn.resume_id.is_none() {
                return Step::Ready(Err(error.message().to_owned()));
            }
            // Couldn't resume: start fresh with a recap.
            let account = turn
                .account
                .as_ref()
                .map(|account| account.id.clone())
                .unwrap_or_default();
            let forgotten = self.db.execute(
                "DELETE FROM sessions WHERE bot_id = ?1 AND provider = ?2 AND account_id = ?3",
                params![turn.bot.id, text(&turn.bot.provider), account],
            );
            if let Err(error) = forgotten {
                return Step::Ready(Err(error.to_string()));
            }
            let item = turn.item.clone();
            self.turn_requeued_hook(&item);
            self.turn_started_hook(&item); // this retry starts directly, without pump
            return self.turn_begin(turn);
        }
        let live = self.live(turn.live);
        if !live.busy || !Self::same_item(&live.item, &turn.item.id) {
            return Step::Ready(Ok(()));
        }
        if live.stop_requested {
            return self.finish_stopped(&turn.bot.id);
        }
        if live.fresh && turn.execution.is_none() {
            let before = (turn.item.conversation_id == turn.bot.id)
                .then(|| turn.item.message_id.clone())
                .flatten();
            match history(self.db, &turn.bot.id, before.as_deref(), RECAP_MESSAGES) {
                Ok(messages) => {
                    let recap = build_recap(&messages, &turn.bot.name);
                    if !recap.is_empty() {
                        turn.text = format!("{recap}\n\n{}", turn.text);
                    }
                }
                Err(error) => return Step::Ready(Err(error.message)),
            }
        }
        turn.session = Some(created);
        self.turn_send(turn)
    }

    fn turn_send(&mut self, turn: &mut Turn) -> Step {
        let session = turn.session.clone().expect("a session to send to");
        Step::Await(
            Phase::Sent,
            session
                .session
                .send(&turn.text)
                .map(Awaited::Result)
                .boxed(),
        )
    }

    fn turn_sent(&mut self, turn: &mut Turn, result: Result<(), ProviderError>) -> Step {
        let session = turn.session.clone().expect("a session to send to");
        let current = self.session_serial(turn.live) == Some(session.serial);
        if let Err(error) = result {
            // This attempt exited and was replaced while send was pending.
            return Step::Ready(if current {
                Err(error.message().to_owned())
            } else {
                Ok(())
            });
        }
        let live = self.live(turn.live);
        if !live.busy || !Self::same_item(&live.item, &turn.item.id) || !current {
            return Step::Ready(Ok(()));
        }
        self.lv(turn.live).sent = true;
        if self.live(turn.live).stop_requested {
            return match self.live(turn.live).stop_pending.clone() {
                Some(pending) => {
                    Step::Await(Phase::StopChecked, pending.map(Awaited::Stop).boxed())
                }
                None => self.turn_stop_checked(turn, false),
            };
        }
        // Anything sent while the turn was starting goes in now, in order.
        match self.steer_queued(&turn.bot.id) {
            Some(rest) => Step::Await(Phase::Steered, rest.map(|_| Awaited::Done).boxed()),
            None => Step::Ready(Ok(())),
        }
    }

    fn turn_stop_checked(&mut self, turn: &mut Turn, handled: bool) -> Step {
        let session = turn.session.as_ref().map(|session| session.serial);
        let live = self.live(turn.live);
        let current = self.live_id(&turn.bot.id) == Some(turn.live)
            && live.busy
            && Self::same_item(&live.item, &turn.item.id)
            && self.session_serial(turn.live) == session;
        if current && live.stop_requested && !handled {
            return Step::Await(
                Phase::Stopped,
                self.stop(&turn.bot.id).map(Awaited::Result).boxed(),
            );
        }
        Step::Ready(Ok(()))
    }

    // --- steering ---

    /// Only your messages and bot replies in this conversation may steer; other sources wait.
    fn steer_queued(&mut self, bot_id: &str) -> Option<BoxFuture<'static, ()>> {
        let live = self.live_for(bot_id);
        if self.live(live).steering {
            return None;
        }
        let serial = self.next_serial();
        let (done, steering_done) = waiter();
        let state = self.lv(live);
        state.steering = true;
        state.steering_done = Some((serial, steering_done));
        self.steer_next(Steering {
            bot_id: bot_id.to_owned(),
            live,
            serial,
            done,
        })
    }

    fn steer_next(&mut self, steering: Steering) -> Option<BoxFuture<'static, ()>> {
        let live = self.live(steering.live);
        let key = live
            .item
            .as_ref()
            .and_then(|item| item.execution_context_key.clone());
        let dest = self.dest(&steering.bot_id, steering.live);
        let item = match next_steerable(self.db, &steering.bot_id, &dest, key.as_deref()) {
            Ok(item) => item,
            Err(error) => {
                self.log("Tjommie: queued steering failed", &error.message);
                self.steer_finish(steering, false);
                return None;
            }
        };
        let live = self.live(steering.live);
        let open = live.busy && live.sent && !live.stop_requested && !live.reload;
        let (Some(item), true, Some(session)) = (item, open, live.session.clone()) else {
            self.steer_finish(steering, true);
            return None;
        };
        // Claim it without reporting delivery before the provider acknowledges it.
        if let Err(error) = set_status(self.db, &item.id, WorkStatus::Running) {
            self.log("Tjommie: queued steering failed", &error.message);
            self.steer_finish(steering, false);
            return None;
        }
        self.lv(steering.live).steering_item = Some(item.clone());
        let pending = session.session.steer(&item.text);
        let runner = self.runner.clone();
        Some(
            async move {
                let result = pending.await;
                if let Some(rest) = runner.with(|core| core.steer_resume(steering, item, result)) {
                    rest.await;
                }
            }
            .boxed(),
        )
    }

    fn steer_resume(
        &mut self,
        steering: Steering,
        item: WorkItem,
        result: Result<bool, ProviderError>,
    ) -> Option<BoxFuture<'static, ()>> {
        let still_live = self.live_id(&steering.bot_id) == Some(steering.live);
        let steered = match result {
            Ok(steered) => steered,
            Err(_) if !still_live => {
                self.steer_finish(steering, false);
                return None;
            }
            // Expected inability to steer is `Ok(false)` in the provider contract; an error is a failure.
            Err(error) => {
                self.log("Tjommie: steering a work item failed", error.message());
                false
            }
        };
        if !still_live {
            self.steer_finish(steering, false);
            return None;
        }
        self.lv(steering.live).steering_item = None;
        let outcome = if steered {
            set_status(self.db, &item.id, WorkStatus::Done)
                .and_then(|()| self.set_chat_message_status(&item, MessageStatus::Sent))
        } else {
            set_status(self.db, &item.id, WorkStatus::Queued) // it runs as the next turn instead
        };
        match outcome {
            Ok(()) if steered => self.steer_next(steering),
            Ok(()) => {
                self.steer_finish(steering, true);
                None
            }
            Err(error) => {
                self.log("Tjommie: queued steering failed", &error.message);
                self.steer_finish(steering, false);
                None
            }
        }
    }

    /// The `finally` and what follows it. `carry_on` is false where TypeScript returned or threw out of the loop.
    fn steer_finish(&mut self, steering: Steering, carry_on: bool) {
        let live = self.lv(steering.live);
        live.steering = false;
        if live
            .steering_done
            .as_ref()
            .is_some_and(|(serial, _)| *serial == steering.serial)
        {
            live.steering_done = None;
        }
        let _ = steering.done.send(());
        if carry_on
            && self.live_id(&steering.bot_id) == Some(steering.live)
            && !self.live(steering.live).busy
        {
            self.pump_logged(&steering.bot_id);
        }
    }

    // --- provider events ---

    fn forward_events(
        &self,
        bot_id: &str,
        session: &SessionRef,
        identity: AccountRunContext,
        key: String,
    ) {
        let mut events = session.session.subscribe();
        let runner = self.runner.clone();
        let bot_id = bot_id.to_owned();
        let serial = session.serial;
        self.spawn(async move {
            while let Some(event) = events.recv().await {
                runner.with(|core| core.on_event(&bot_id, serial, &identity, &key, event));
            }
        });
    }

    fn on_event(
        &mut self,
        bot_id: &str,
        serial: u64,
        identity: &AccountRunContext,
        key: &str,
        event: ProviderEvent,
    ) {
        if self.st.events_closed {
            return;
        }
        // Late usage still counts, against the account the session ran on.
        if let ProviderEvent::Usage(usage) = &event {
            if let Err(error) = self.record_usage(bot_id, usage, identity, key) {
                self.log("Tjommie: recording usage failed", &error.message);
            }
            return;
        }
        let Some(live) = self.live_id(bot_id) else {
            return;
        };
        if self.session_serial(live) != Some(serial) {
            return; // a session we closed or replaced
        }
        let turn_event = matches!(
            event,
            ProviderEvent::Text { .. } | ProviderEvent::Tool { .. } | ProviderEvent::Approval(_)
        );
        if !self.live(live).busy && turn_event {
            // The CLI started a turn by itself, for example after a background task finished.
            let state = self.lv(live);
            state.busy = true;
            state.sent = true;
            state.item = None;
            self.emit_bot(bot_id);
        }
        let outcome = match event {
            ProviderEvent::Session { cli_session_id } => {
                self.lv(live).saw_session = true;
                self.save_session(bot_id, &cli_session_id, identity)
            }
            ProviderEvent::Text { delta } => self.append_text(bot_id, live, &delta),
            ProviderEvent::TextEnd => self.flush_text(live, MessageStatus::Done),
            ProviderEvent::Tool { id, name, summary } => self.add_tool(
                bot_id,
                live,
                ActivityItem {
                    id,
                    name,
                    summary,
                    status: ActivityStatus::Running,
                },
            ),
            ProviderEvent::ToolDone { id, ok } => self.tool_done(live, &id, ok),
            ProviderEvent::Approval(approval) => self.on_approval(bot_id, live, approval),
            ProviderEvent::Usage(_) => Ok(()),
            ProviderEvent::Limit(limit) => self.on_limit(bot_id, live, &limit),
            ProviderEvent::TurnDone(turn) => self.finish_turn(bot_id, turn.ok, turn.error),
            ProviderEvent::Exit {
                error,
                completed_turn,
            } => self.on_exit(bot_id, live, error, completed_turn),
        };
        if let Err(error) = outcome {
            self.log("Tjommie: handling a CLI event failed", &error.message);
        }
    }

    fn save_session(
        &self,
        bot_id: &str,
        cli_session_id: &str,
        identity: &AccountRunContext,
    ) -> Result<(), RpcError> {
        get_stored_bot(self.db, bot_id)?;
        let live = self.live_id(bot_id).map(|id| self.live(id));
        let key = live
            .map(|live| live.execution_key.clone())
            .unwrap_or_default();
        let keep_unkeyed_cost = live.is_some_and(|live| live.execution.is_none() && !live.fresh);
        self.db.execute(
            "INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at, execution_context_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(bot_id, provider, account_id) DO UPDATE SET
               total_cost_usd = CASE WHEN sessions.provider = excluded.provider AND sessions.account_id = excluded.account_id
                 AND (sessions.execution_context_key = excluded.execution_context_key OR sessions.execution_context_key = '' AND ?7)
                 AND sessions.cli_session_id = excluded.cli_session_id THEN sessions.total_cost_usd ELSE NULL END,
               provider = excluded.provider, account_id = excluded.account_id,
               cli_session_id = excluded.cli_session_id, updated_at = excluded.updated_at, execution_context_key = excluded.execution_context_key",
            params![bot_id, text(&identity.provider), identity.effective_account_id, cli_session_id, now_iso(), key, keep_unkeyed_cost],
        )?;
        Ok(())
    }

    fn append_text(&mut self, bot_id: &str, live: LiveId, delta: &str) -> Result<(), RpcError> {
        let conversation_id = self.dest(bot_id, live);
        if self.live(live).text.is_none() {
            if self.live(live).activity.is_some() {
                self.close_activity(live, ActivityStatus::Done)?;
            }
            let message = NewMessage {
                conversation_id: conversation_id.clone(),
                author: bot_id.to_owned(),
                kind: MessageKind::Text,
                body: String::new(),
                data: None,
                status: MessageStatus::Streaming,
            };
            let message = add_message(self.db, &message)?;
            self.lv(live).text = Some(Text {
                id: message.id.clone(),
                body: String::new(),
            });
            self.emit_message(message, true);
        }
        let text = self.lv(live).text.as_mut().expect("made above");
        text.body.push_str(delta);
        let id = text.id.clone();
        self.send(Event::MessageDelta(MessageDelta {
            id,
            conversation_id,
            delta: delta.to_owned(),
        }));
        Ok(())
    }

    fn flush_text(&mut self, live: LiveId, status: MessageStatus) -> Result<(), RpcError> {
        let Some(text) = self.lv(live).text.take() else {
            return Ok(());
        };
        let patch = MessagePatch {
            body: Some(text.body.clone()),
            status: Some(status),
            data: None,
        };
        let updated = update_message(self.db, &text.id, &patch)?;
        self.emit_message(updated, false);
        if status == MessageStatus::Done && !js::trim(&text.body).is_empty() {
            self.lv(live).last_text = Some(text);
        }
        Ok(())
    }

    fn add_tool(&mut self, bot_id: &str, live: LiveId, item: ActivityItem) -> Result<(), RpcError> {
        self.flush_text(live, MessageStatus::Done)?;
        if let Some(activity) = self.lv(live).activity.as_mut() {
            activity.items.push(item);
            return self.save_activity(live, MessageStatus::Streaming);
        }
        let items = vec![item];
        let message = NewMessage {
            conversation_id: self.dest(bot_id, live),
            author: bot_id.to_owned(),
            kind: MessageKind::Activity,
            body: activity_body(&items),
            data: Some(object(&ActivityData {
                items: items.clone(),
            })),
            status: MessageStatus::Streaming,
        };
        let message = add_message(self.db, &message)?;
        self.lv(live).activity = Some(Activity {
            id: message.id.clone(),
            items,
        });
        self.emit_message(message, true);
        Ok(())
    }

    fn tool_done(&mut self, live: LiveId, id: &str, ok: bool) -> Result<(), RpcError> {
        let Some(item) = self
            .lv(live)
            .activity
            .as_mut()
            .and_then(|activity| activity.items.iter_mut().find(|item| item.id == id))
        else {
            return Ok(());
        };
        item.status = if ok {
            ActivityStatus::Done
        } else {
            ActivityStatus::Failed
        };
        self.save_activity(live, MessageStatus::Streaming)
    }

    fn save_activity(&mut self, live: LiveId, status: MessageStatus) -> Result<(), RpcError> {
        let Some(activity) = self.live(live).activity.clone() else {
            return Ok(());
        };
        let patch = MessagePatch {
            body: Some(activity_body(&activity.items)),
            data: Some(Some(object(&ActivityData {
                items: activity.items,
            }))),
            status: Some(status),
        };
        let updated = update_message(self.db, &activity.id, &patch)?;
        self.emit_message(updated, false);
        Ok(())
    }

    fn close_activity(&mut self, live: LiveId, unfinished: ActivityStatus) -> Result<(), RpcError> {
        if let Some(activity) = self.lv(live).activity.as_mut() {
            for item in activity
                .items
                .iter_mut()
                .filter(|item| item.status == ActivityStatus::Running)
            {
                item.status = unfinished;
            }
        }
        self.save_activity(live, MessageStatus::Done)?;
        self.lv(live).activity = None;
        Ok(())
    }

    fn on_approval(
        &mut self,
        bot_id: &str,
        live: LiveId,
        request: ProviderApproval,
    ) -> Result<(), RpcError> {
        let session = self.live(live).session.clone();
        let isolated = self.live(live).execution.as_ref().is_some_and(|execution| {
            matches!(
                execution.phase,
                ExecutionPhase::Review | ExecutionPhase::Test
            )
        });
        if (self.deps().is_held)(bot_id) || isolated {
            if let Some(session) = &session {
                session.session.answer(&request.id, ApprovalAnswer::Deny);
            }
            return Ok(());
        }
        if let Some(rule) = request.rule.as_deref().filter(|rule| !rule.is_empty())
            && allow_rules(self.db, bot_id)?
                .iter()
                .any(|allowed| allowed == rule)
        {
            if let Some(session) = &session {
                session.session.answer(&request.id, ApprovalAnswer::Once); // "Always allow" was chosen for this before
            }
            return Ok(());
        }
        self.flush_text(live, MessageStatus::Done)?;
        let approval_id = new_id();
        let card = ToolApprovalData {
            approval_id: approval_id.clone(),
            tool: request.tool.clone(),
            detail: request.detail.clone(),
            status: ApprovalStatus::Pending,
            kind: None,
            rule: request.rule.clone(),
        };
        let message = NewMessage {
            conversation_id: bot_id.to_owned(),
            author: bot_id.to_owned(),
            kind: MessageKind::Approval,
            body: request.summary.clone(),
            data: Some(object(&card)),
            status: MessageStatus::Done,
        };
        let message = add_message(self.db, &message)?;
        let approval = create_approval(
            self.db,
            &NewApproval {
                id: approval_id.clone(),
                bot_id: bot_id.to_owned(),
                kind: ApprovalKind::Tool,
                tool: request.tool,
                summary: request.summary.clone(),
                detail: request.detail,
                rule: request.rule,
                payload: None,
                message_id: message.id.clone(),
            },
        )?;
        self.lv(live)
            .approvals
            .push((approval_id.clone(), request.id));
        self.emit_message(message, true);
        self.send(Event::ApprovalAdded(approval));
        let name = get_stored_bot(self.db, bot_id)?.name;
        self.send(Event::Notify(Notify::Approval {
            bot_id: bot_id.to_owned(),
            title: format!("{name} is waiting for you"),
            body: request.summary,
            approval_id: Some(approval_id),
        }));
        self.emit_bot(bot_id);
        Ok(())
    }

    /// Sets the card's status. A damaged card keeps its fields, so it can't interrupt turn cleanup.
    fn show_approval_status(&self, approval: &Approval) -> Result<(), RpcError> {
        let message = get_message(self.db, &approval.message_id)?;
        let mut data = message.data.unwrap_or_default();
        data.insert(
            "status".into(),
            serde_json::to_value(approval.status).expect("statuses serialize"),
        );
        let updated = update_message(
            self.db,
            &message.id,
            &MessagePatch {
                data: Some(Some(data)),
                ..MessagePatch::default()
            },
        )?;
        self.emit_message(updated, false);
        self.send(Event::ApprovalResolved(approval.clone()));
        Ok(())
    }

    fn expire_approvals(&mut self, live: LiveId) -> Result<(), RpcError> {
        for (approval_id, _) in std::mem::take(&mut self.lv(live).approvals) {
            match resolve_approval(self.db, &approval_id, ApprovalDecision::Deny) {
                Ok(approval) => self.show_approval_status(&approval)?,
                Err(error) if error.code == ErrorCode::AlreadyAnswered => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn record_usage(
        &mut self,
        bot_id: &str,
        usage: &Usage,
        identity: &AccountRunContext,
        key: &str,
    ) -> Result<(), RpcError> {
        let provider = identity.provider;
        let account_id = identity.effective_account_id.clone();
        let estimated = if usage.cost_usd.is_none() {
            price_usage(provider, &usage.model, usage)
        } else {
            None
        };
        let cost_source = match (usage.cost_usd, &estimated) {
            (Some(_), _) => "reported",
            (None, Some(_)) => "estimated",
            (None, None) => "unknown",
        };
        let row = UsageRow {
            id: usage
                .event_id
                .as_ref()
                .map(|event| format!("{}:{event}", identity.run_id)),
            bot_id,
            account_id: &account_id,
            provider,
            model: &usage.model,
            usage,
            cost_usd: usage.cost_usd.or(estimated.as_ref().map(|(cost, _)| *cost)),
            cost_source,
            pricing_revision: estimated.map(|(_, revision)| revision),
        };
        let transaction = self.db.unchecked_transaction()?;
        let inserted = record_usage_row(&transaction, &row)?;
        if inserted && let Some(total) = usage.total_cost_usd {
            transaction.execute(
                "UPDATE sessions SET total_cost_usd = ?1 WHERE bot_id = ?2 AND provider = ?3 AND account_id = ?4 AND execution_context_key = ?5",
                params![total, bot_id, text(&provider), account_id, key],
            )?;
        }
        transaction.commit()?;
        if inserted {
            if let Some(accounts) = &self.deps().accounts {
                accounts.usage_changed(&account_id);
            }
            self.send(Event::UsageChanged(UsageChanged {
                account_id,
                bot_id: bot_id.to_owned(),
            }));
        }
        Ok(())
    }

    fn on_limit(&mut self, bot_id: &str, live: LiveId, limit: &Limit) -> Result<(), RpcError> {
        let reached = limit.state == LimitState::Reached;
        if let (Some(accounts), Some(context)) = (
            self.deps().accounts.clone(),
            self.live(live).context.clone(),
        ) {
            // Mark the turn before telling the accounts runtime, which can report a setup change.
            if reached {
                let state = self.lv(live);
                state.limit_hit = true;
                state.reload = true;
            }
            accounts.observe_limit(&context, limit);
            if let (Some(resets_at), true) = (
                limit.resets_at.as_deref().filter(|at| !at.is_empty()),
                reached,
            ) {
                self.arm_limit_timer(bot_id, resets_at);
            }
            self.emit_bot(bot_id);
            return Ok(());
        }
        let state = self.lv(live);
        if let Some(resets_at) = limit.resets_at.clone().filter(|at| !at.is_empty()) {
            state.last_resets_at = Some(resets_at);
        }
        if !reached || state.limit_hit {
            return Ok(()); // "ok" and "near" feed the Usage page
        }
        state.limit_hit = true;
        let until = limit
            .resets_at
            .clone()
            .or_else(|| state.last_resets_at.clone())
            .unwrap_or_else(|| iso_from_ms(now_ms() + UNKNOWN_RESET_MS));
        set_limited_until(self.db, bot_id, Some(&until))?;
        let name = get_stored_bot(self.db, bot_id)?.name;
        let at = reset_time_text(&until);
        self.notice(
            bot_id,
            MessageKind::Notice,
            &format!("{name} hit its usage limit. It will pick up again at {at}."),
            None,
        )?;
        self.send(Event::Notify(Notify::Limit {
            bot_id: bot_id.to_owned(),
            title: format!("{name} hit its usage limit"),
            body: format!("It will pick up again at {at}."),
        }));
        self.arm_limit_timer(bot_id, &until);
        self.emit_bot(bot_id);
        Ok(())
    }

    fn arm_limit_timer(&mut self, bot_id: &str, until: &str) {
        let live = self.live_for(bot_id);
        if let Some((_, timer)) = self.lv(live).limit_timer.take() {
            timer.abort();
        }
        let delay = match until.parse::<jiff::Timestamp>() {
            Ok(until) => ((until.as_millisecond() - now_ms()).max(0) + 60_000).min(MAX_TIMER_MS),
            Err(_) => 0,
        };
        let serial = self.next_serial();
        let runner = self.runner.clone();
        let bot = bot_id.to_owned();
        let timer = self.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay.unsigned_abs())).await;
            runner.with(|core| {
                if core
                    .live(live)
                    .limit_timer
                    .as_ref()
                    .is_none_or(|(armed, _)| *armed != serial)
                {
                    return; // cleared after it fired
                }
                core.lv(live).limit_timer = None;
                if let Err(error) = set_limited_until(core.db, &bot, None) {
                    core.log("Tjommie: clearing a usage limit failed", &error.message);
                }
                core.emit_bot(&bot);
                core.pump_logged(&bot);
            });
        });
        self.lv(live).limit_timer = Some((serial, timer));
    }

    fn finish_turn(
        &mut self,
        bot_id: &str,
        ok: bool,
        error: Option<String>,
    ) -> Result<(), RpcError> {
        let Some(live) = self.live_id(bot_id) else {
            return Ok(());
        };
        if !self.live(live).busy {
            return Ok(());
        }
        let stop_requested = self.live(live).stop_requested;
        let outcome = if ok {
            TurnOutcome::Done
        } else if stop_requested {
            TurnOutcome::Interrupted
        } else {
            TurnOutcome::Failed
        };
        self.flush_text(live, outcome.message_status())?;
        if self.live(live).activity.is_some() {
            let unfinished = if outcome == TurnOutcome::Done {
                ActivityStatus::Done
            } else {
                ActivityStatus::Failed
            };
            self.close_activity(live, unfinished)?;
        }
        let item = self.live(live).item.clone();
        let conversation_id = self.dest(bot_id, live);
        let requeued = self.live(live).limit_hit && !ok && !stop_requested;
        if let Some(item) = &item {
            if requeued {
                set_status(self.db, &item.id, WorkStatus::Queued)?; // runs again once the limit resets
                self.set_chat_message_status(item, MessageStatus::Queued)?;
            } else {
                set_status(self.db, &item.id, outcome.work_status())?;
                if outcome == TurnOutcome::Interrupted {
                    let notice = self
                        .live(live)
                        .stop_notice
                        .clone()
                        .unwrap_or_else(|| "Stopped.".into());
                    self.notice(&conversation_id, MessageKind::Notice, &notice, None)?;
                }
                if outcome == TurnOutcome::Failed {
                    let body = error
                        .clone()
                        .unwrap_or_else(|| "Something went wrong.".into());
                    self.notice(
                        &conversation_id,
                        MessageKind::Error,
                        &body,
                        retry_data(item),
                    )?;
                }
            }
        }
        self.expire_approvals(live)?; // the turn that asked is over
        let last_text = self.live(live).last_text.clone();
        let result = TurnResult {
            outcome,
            text: last_text.as_ref().map(|text| text.body.clone()),
            text_message_id: last_text.map(|text| text.id),
            error,
        };
        let accounts = self.deps().accounts.clone();
        let context = self.live(live).context.clone();
        let limit_hit = self.live(live).limit_hit;
        let retried_limit = match (&context, &accounts) {
            (Some(context), Some(accounts)) => {
                accounts.complete_turn(context, ok && !stop_requested && !limit_hit)
            }
            _ => false,
        };
        let state = self.lv(live);
        state.busy = false;
        state.sent = false;
        state.item = None;
        state.stop_requested = false;
        state.stop_notice = None;
        state.stop_pending = None;
        state.limit_hit = false;
        state.last_text = None;
        let api_key = context
            .as_ref()
            .is_some_and(|context| context.kind == AccountKind::ApiKey);
        if state.reload
            || retried_limit
            || api_key
            || (stop_requested && accounts.is_some())
            || state.execution.is_some()
        {
            state.reload = false;
            self.close_session(bot_id, live);
        }
        if let Some(item) = &item {
            if requeued {
                self.turn_requeued_hook(item);
            } else {
                self.turn_ended_hook(item, &result);
            }
            if item.task_attempt_id.is_some() {
                let waits = [
                    self.live(live).retiring.clone(),
                    self.live(live)
                        .steering_done
                        .as_ref()
                        .map(|(_, done)| done.clone()),
                ];
                let runner = self.runner.clone();
                let bot = bot_id.to_owned();
                self.spawn(async move {
                    for wait in waits.into_iter().flatten() {
                        wait.await;
                    }
                    runner.with(|core| {
                        for waiter in core.st.task_waiters.remove(&bot).unwrap_or_default() {
                            let _ = waiter.send(());
                        }
                    });
                });
            }
        }
        self.emit_bot(bot_id);
        self.pump(bot_id)
    }

    fn on_exit(
        &mut self,
        bot_id: &str,
        live: LiveId,
        error: Option<String>,
        completed_turn: Option<ProviderTurn>,
    ) -> Result<(), RpcError> {
        let state = self.live(live);
        let resume_failed = !state.fresh && !state.saw_session;
        let (stop_requested, limit_hit) = (state.stop_requested, state.limit_hit);
        let context = state.context.clone();
        let accounts = self.deps().accounts.clone();
        if let (Some(context), Some(accounts)) = (&context, &accounts) {
            let successful = completed_turn.as_ref().is_some_and(|turn| turn.ok)
                && !stop_requested
                && !limit_hit;
            accounts.complete_turn(context, successful);
        }
        let state = self.lv(live);
        state.session = None;
        state.context = None;
        if let (Some(context), Some(accounts)) = (&context, &accounts) {
            accounts.release(context);
        }
        self.session_closed_hook(bot_id);
        if let Some((_, timer)) = self.lv(live).idle_timer.take() {
            timer.abort();
        }
        if !self.live(live).busy {
            return Ok(());
        }
        if resume_failed
            && !stop_requested
            && let Some(item) = self.live(live).item.clone()
        {
            // The CLI couldn't resume this session: start fresh with a recap instead.
            self.flush_text(live, MessageStatus::Interrupted)?;
            if self.live(live).activity.is_some() {
                self.close_activity(live, ActivityStatus::Failed)?;
            }
            self.lv(live).last_text = None;
            match &context {
                Some(context) => self.db.execute(
                    "DELETE FROM sessions WHERE bot_id = ?1 AND provider = ?2 AND account_id = ?3",
                    params![
                        bot_id,
                        text(&context.provider),
                        context.effective_account_id
                    ],
                )?,
                None => {
                    let provider = get_stored_bot(self.db, bot_id)?.provider;
                    self.db.execute(
                        "DELETE FROM sessions WHERE bot_id = ?1 AND provider = ?2",
                        params![bot_id, text(&provider)],
                    )?
                }
            };
            set_status(self.db, &item.id, WorkStatus::Queued)?;
            let state = self.lv(live);
            state.busy = false;
            state.sent = false;
            state.item = None;
            self.turn_requeued_hook(&item);
            return self.pump(bot_id);
        }
        if accounts.is_none() {
            self.lv(live).limit_hit = false; // legacy exits are terminal; account-limited work waits or retries
        }
        let (ok, error) = match completed_turn {
            Some(turn) => (turn.ok, turn.error),
            None => (
                false,
                error.or_else(|| {
                    (!stop_requested).then(|| "The CLI stopped unexpectedly.".to_owned())
                }),
            ),
        };
        self.finish_turn(bot_id, ok, error)
    }

    fn close_session(&mut self, bot_id: &str, live: LiveId) {
        if self.live(live).retiring.is_some() {
            return;
        }
        let session = self.lv(live).session.take(); // late usage is still attributed by the session's own identity
        let context = self.live(live).context.clone();
        let accounts = self.deps().accounts.clone();
        let Some(session) = session else {
            self.lv(live).context = None;
            if let (Some(context), Some(accounts)) = (&context, &accounts) {
                accounts.release(context);
            }
            return;
        };
        let runner = self.runner.clone();
        let bot = bot_id.to_owned();
        if let (Some(context), Some(accounts)) = (context, accounts) {
            // Keep the account lease and queued work until the owned process exits.
            let (done, retiring) = waiter();
            self.lv(live).retiring = Some(retiring);
            session.session.close();
            let exited = session.session.wait_for_exit();
            self.spawn(async move {
                exited.await;
                runner.with(|core| {
                    let state = core.lv(live);
                    if state
                        .context
                        .as_ref()
                        .is_some_and(|current| current.run_id == context.run_id)
                    {
                        state.context = None;
                    }
                    state.retiring = None;
                    accounts.release(&context);
                    let _ = done.send(());
                    if core.live_id(&bot) == Some(live) {
                        core.pump_logged(&bot);
                    }
                });
            });
            self.session_closed_hook(bot_id);
            return;
        }
        if self.live(live).execution.is_some() {
            let (done, retiring) = waiter();
            self.lv(live).retiring = Some(retiring);
            session.session.close();
            let exited = session.session.wait_for_exit();
            self.spawn(async move {
                exited.await;
                runner.with(|core| {
                    core.lv(live).retiring = None;
                    core.pump_logged(&bot);
                });
                let _ = done.send(());
            });
            self.session_closed_hook(bot_id);
            return;
        }
        session.session.close();
        self.session_closed_hook(bot_id);
    }

    fn release_session(&mut self, bot_id: &str, live: LiveId) {
        let state = self.lv(live);
        for (_, timer) in [state.idle_timer.take(), state.limit_timer.take()]
            .into_iter()
            .flatten()
        {
            timer.abort();
        }
        self.close_session(bot_id, live);
    }

    fn arm_idle(&mut self, bot_id: &str, live: LiveId) {
        if self.live(live).session.is_none() || self.live(live).idle_timer.is_some() {
            return;
        }
        let serial = self.next_serial();
        let runner = self.runner.clone();
        let bot = bot_id.to_owned();
        let timer = self.spawn(async move {
            tokio::time::sleep(IDLE_SHUTDOWN).await;
            runner.with(|core| {
                if core
                    .live(live)
                    .idle_timer
                    .as_ref()
                    .is_none_or(|(armed, _)| *armed != serial)
                {
                    return; // cleared after it fired
                }
                core.lv(live).idle_timer = None;
                if !core.live(live).busy {
                    core.close_session(&bot, live);
                }
            });
        });
        self.lv(live).idle_timer = Some((serial, timer));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The chat and approval methods (`daemon.ts`).

/// `chat.*` and `approvals.respond`. Runner calls happen outside the database lock the context hands out.
pub fn register(handlers: Handlers, context: &Context, runner: &Runner) -> Handlers {
    let (history, send, stop, retry, respond) = (
        context.clone(),
        runner.clone(),
        runner.clone(),
        runner.clone(),
        runner.clone(),
    );
    let approvals = context.clone();
    handlers
        .on::<method::ChatHistory, _>(move |params, _| {
            let page = history.with_db(|db| {
                get_stored_bot(db, &params.bot_id)?;
                crate::chat::history_page(
                    db,
                    &params.bot_id,
                    params.before.as_deref(),
                    params.limit,
                )
            });
            async move { page }
        })
        .on::<method::ChatSend, _>(move |params, _| {
            let message = send.submit(&params.bot_id, &params.text, &params.files);
            async move { message }
        })
        .on::<method::ChatStop, _>(move |params, _| {
            let stopping = stop.stop(&params.bot_id);
            async move {
                stopping
                    .await
                    .map_err(|error| RpcError::new(ErrorCode::Internal, error.message()))?;
                Ok(Empty {})
            }
        })
        .on::<method::ChatRetry, _>(move |params, _| {
            let message = retry.retry(&params.message_id, params.bot_id.as_deref());
            async move { message }
        })
        .on::<method::ApprovalsRespond, _>(move |params, _| {
            let answered = approvals
                .with_db(|db| crate::approvals::get_approval(db, &params.approval_id))
                .and_then(|approval| match approval.kind {
                    // M4: `team.answerRoutineProposal` adds the routine on Allow.
                    ApprovalKind::Routine => Err(RpcError::new(
                        ErrorCode::Unavailable,
                        "Routine proposals can't be answered yet.",
                    )),
                    ApprovalKind::Tool => {
                        respond.answer_approval(&params.approval_id, params.decision)
                    }
                })
                .inspect(|approval| respond.pump(&approval.bot_id));
            async move { answered }
        })
}

// ---------------------------------------------------------------------------------------------------------------------
// Stand-ins for T3.4 (lane C). Each is replaced by its port at merge.

/// `getAccount(db, id)`. T3.4: `accounts_store::get_account`.
fn account_by_id(db: &Connection, id: &str) -> Result<Account, RpcError> {
    db.query_row(
        "SELECT id, folder, kind FROM accounts WHERE id = ?1 AND deleted_at IS NULL",
        [id],
        read_account,
    )
    .optional()?
    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "Account not found"))
}

/// `accountFor(db, provider)`: the provider's default account. T3.4: `setup::account_for`, which also maps the folder
/// into the worker boundary's root (`providerAccountFolder`).
fn account_for(db: &Connection, provider: Provider) -> Result<Account, RpcError> {
    db.query_row(
        "SELECT id, folder, kind FROM accounts WHERE provider = ?1 AND deleted_at IS NULL ORDER BY is_default DESC, created_at, id LIMIT 1",
        [text(&provider)],
        read_account,
    )
    .optional()?
    .ok_or_else(|| RpcError::new(ErrorCode::NoAccount, format!("No {} account", text(&provider))))
}

fn read_account(row: &rusqlite::Row) -> rusqlite::Result<Account> {
    let kind: String = row.get(2)?;
    Ok(Account {
        id: row.get(0)?,
        folder: row.get(1)?,
        kind: if kind == "api_key" {
            AccountKind::ApiKey
        } else {
            AccountKind::Subscription
        },
    })
}

/// The default account's `accountEnv` and folder, for a CLI started outside a turn (`listModels`).
pub(crate) fn default_account_env(
    db: &Connection,
    provider: Provider,
) -> Result<(BTreeMap<String, String>, String), RpcError> {
    let account = account_for(db, provider)?;
    Ok((account_env(provider, &account), account.folder))
}

/// `accountEnv(account)`. T3.4: `setup::account_env`, which starts from `cliEnv`'s scrubbed environment.
fn account_env(provider: Provider, account: &Account) -> BTreeMap<String, String> {
    let name = if provider == Provider::Claude {
        "CLAUDE_CONFIG_DIR"
    } else {
        "CODEX_HOME"
    };
    BTreeMap::from([(name.to_owned(), account.folder.clone())])
}

/// `priceUsage`: an estimated cost and its pricing revision. T3.4 ports the rates; until then nothing is estimated.
fn price_usage(_provider: Provider, _model: &str, _usage: &Usage) -> Option<(f64, String)> {
    None
}

struct UsageRow<'a> {
    id: Option<String>,
    bot_id: &'a str,
    account_id: &'a str,
    provider: Provider,
    model: &'a str,
    usage: &'a Usage,
    cost_usd: Option<f64>,
    cost_source: &'static str,
    pricing_revision: Option<String>,
}

/// `recordUsage` from `usage.ts`; the caller owns the transaction. A repeated provider event id is harmless, a
/// conflicting one an error. T3.4: `usage::record_usage`.
fn record_usage_row(db: &Connection, row: &UsageRow) -> Result<bool, RpcError> {
    let bad = |message: &str| Err(RpcError::new(ErrorCode::BadParams, message));
    let safe =
        |count: f64| count.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&count);
    let tokens = &row.usage.tokens;
    if ![
        tokens.input,
        tokens.cached_input,
        tokens.output,
        tokens.reasoning,
    ]
    .into_iter()
    .all(safe)
    {
        return bad("Usage token counts must be nonnegative safe integers");
    }
    if !safe(tokens.input + tokens.cached_input + tokens.output) {
        return bad("Usage token total is too large");
    }
    if row
        .cost_usd
        .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
    {
        return bad("Usage cost must be a nonnegative finite amount");
    }
    if row.cost_usd.is_none() != (row.cost_source == "unknown") {
        return bad("Usage cost provenance must match its amount");
    }
    if js::trim(row.bot_id).is_empty()
        || js::trim(row.account_id).is_empty()
        || js::trim(row.model).is_empty()
        || row.id.as_deref() == Some("")
    {
        return bad("Usage identity is required");
    }
    let account: Option<(String, String)> = db
        .query_row(
            "SELECT provider, kind FROM accounts WHERE id = ?1",
            [row.account_id],
            |found| Ok((found.get(0)?, found.get(1)?)),
        )
        .optional()?;
    let Some((_, account_kind)) = account.filter(|(provider, kind)| {
        *provider == text(&row.provider) && (kind == "subscription" || kind == "api_key")
    }) else {
        return bad("Usage must identify its actual provider account");
    };
    let id = row.id.clone().unwrap_or_else(new_id);
    let counts = [
        tokens.input,
        tokens.cached_input,
        tokens.output,
        tokens.reasoning,
    ]
    .map(|count| count as i64);
    let wanted = (
        row.bot_id.to_owned(),
        row.account_id.to_owned(),
        text(&row.provider),
        row.model.to_owned(),
        counts,
        row.cost_usd,
        Some(row.cost_source.to_owned()),
        account_kind.clone(),
        row.pricing_revision.clone(),
    );
    let existing = db
        .query_row(
            "SELECT bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, account_kind, pricing_revision FROM usage WHERE id = ?1",
            [&id],
            |found| {
                Ok((
                    found.get::<_, String>(0)?,
                    found.get::<_, String>(1)?,
                    found.get::<_, String>(2)?,
                    found.get::<_, String>(3)?,
                    [found.get::<_, i64>(4)?, found.get(5)?, found.get(6)?, found.get(7)?],
                    found.get::<_, Option<f64>>(8)?,
                    found.get::<_, Option<String>>(9)?,
                    found.get::<_, String>(10)?,
                    found.get::<_, Option<String>>(11)?,
                ))
            },
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing != wanted {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "Conflicting usage event identity",
            ));
        }
        return Ok(false);
    }
    db.execute(
        "INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at, account_kind, pricing_revision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            id,
            row.bot_id,
            row.account_id,
            text(&row.provider),
            row.model,
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            row.cost_usd,
            row.cost_source,
            now_iso(),
            account_kind,
            row.pricing_revision,
        ],
    )?;
    Ok(true)
}
