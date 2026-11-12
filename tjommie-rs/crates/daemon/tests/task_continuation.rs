#![cfg(any())]
//! Port of `packages/daemon/src/task-continuation.test.ts`: the real runner (Core's `runner.rs`) driven by synthetic
//! provider sessions, with the task service and the team service, over the `project_support` fixture.
//!
//! The TypeScript file has 6 `it` blocks, 3 of them `it.each`; expanded, that is 12 cases. All 12 are ported, in order.
//!
//! Compiled out until lane A's `team.rs` and `team/tools.rs` land, and until the `TODO(M5-integration)` assumptions are
//! checked against them, against the merged runner (Core's `runner.rs` and `providers/mod.rs`) and against
//! `common::no_cli_setup`. Then delete the `cfg(any())` line.
//!
//! Timing. The TypeScript runs a provider's events synchronously. Here they reach the runner through a channel, so an
//! assertion made right after an event waits (`eventually`) or lets the runtime run (`settle`). `vi.spyOn(console,
//! "error")` is the runner's `Log`, captured into the harness's `logs`.

mod common;
mod project_support;

use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use daemon::bots::{StoredBot, delete_bot, get_stored_bot, update_bot};
use daemon::cli::BoxFuture;
// TODO(M5-integration): `daemon::mcp::McpTool` is lane A's (mcp.rs:32); it is not in this branch yet.
use daemon::mcp::McpTool;
// TODO(M5-integration): the runner and its provider-session seam are Core's (rs-core runner.rs, providers/mod.rs),
// read-only from this lane. `ProviderSession` here is Core's: `wait_for_exit` returns `Result<(), ProviderError>`.
// Lane B's providers/mod.rs has `BoxFuture<'static, ()>` instead; the merge must pick Core's.
use daemon::providers::{
    ApprovalAnswer, ProviderError, ProviderEvent, ProviderSession, StartOptions,
    TurnResult as ProviderTurn,
};
use daemon::queue::{WorkItem, WorkSource, WorkStatus, get_work_item};
use daemon::runner::{
    Broadcast, Core, ExecutionContextHook, ItemHook, Log, Runner, RunnerDeps, RunnerHooks,
    SessionClosedHook, SessionFactory, TurnEndedHook, TurnResult,
};
use daemon::tasks::{Tasks, TasksOptions};
// TODO(M5-integration): lane A's `daemon::team` is in progress (team.rs). Assumed: `Team::new(TeamDeps)` (team.rs:378),
// `TeamDeps` (team.rs:154), `HostAccess` (team.rs:112), `with_host` (team.rs:124), `get_exchange` (team.rs:253).
use daemon::team::{HostAccess, Team, TeamDeps, get_exchange, with_host};
use futures_util::FutureExt;
use futures_util::future::{Shared, ready};
use project_support::{Fixture, eventually};
use protocol::Event;
use protocol::model::{BotPatch, ExchangeStatus};
use protocol::projects::{
    AttemptState, Project, ProjectPatch, ProjectTask, TaskAttempt, TaskInput, TaskPatch, TaskState,
};
use rusqlite::OptionalExtension;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

/// Fails the test instead of hanging the run; the TypeScript tests have 30 s timeouts.
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .expect("timed out after 30 s")
}

/// The TypeScript `setImmediate`: lets the runtime run what is ready.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

/// A promise the test settles: TypeScript's `new Promise` with its `resolve` or `reject`. It stays pending if its
/// sender is dropped, like a promise nobody settles.
fn gate<T: Clone + Send + Sync + 'static>()
-> (Shared<BoxFuture<'static, T>>, oneshot::Sender<T>) {
    let (sender, receiver) = oneshot::channel::<T>();
    let pending: BoxFuture<'static, T> = Box::pin(async move {
        match receiver.await {
            Ok(value) => value,
            Err(_) => std::future::pending().await,
        }
    });
    (pending.shared(), sender)
}

/// What the test and the runner share with one synthetic session, behind one lock.
struct SyntheticState {
    listeners: Vec<mpsc::UnboundedSender<ProviderEvent>>,
    sent: Vec<String>,
    steered: Vec<String>,
    can_steer: bool,
    /// TypeScript `steerResult`: when set, `steer` answers with it.
    steer_result: Option<Shared<BoxFuture<'static, Result<bool, ProviderError>>>>,
    steer_answer: Option<oneshot::Sender<Result<bool, ProviderError>>>,
    /// TypeScript `exit`: resolved unless held.
    exit: Shared<BoxFuture<'static, Result<(), ProviderError>>>,
    exit_release: Option<oneshot::Sender<Result<(), ProviderError>>>,
}

/// TypeScript `SyntheticSession`: a provider session the test drives by hand.
struct SyntheticSession(Mutex<SyntheticState>);

impl SyntheticSession {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(SyntheticState {
            listeners: Vec::new(),
            sent: Vec::new(),
            steered: Vec::new(),
            can_steer: false,
            steer_result: None,
            steer_answer: None,
            exit: ready(Ok(())).boxed().shared(),
            exit_release: None,
        })))
    }

    fn state(&self) -> MutexGuard<'_, SyntheticState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sent(&self) -> Vec<String> {
        self.state().sent.clone()
    }

    fn steered(&self) -> Vec<String> {
        self.state().steered.clone()
    }

    /// TypeScript `canSteer = true`.
    fn allow_steer(&self) {
        self.state().can_steer = true;
    }

    fn emit(&self, event: ProviderEvent) {
        self.state()
            .listeners
            .retain(|listener| listener.send(event.clone()).is_ok());
    }

    /// TypeScript `finish(text)`: the text, its end, then a successful `turn_done`.
    fn finish(&self, text: &str) {
        self.emit(ProviderEvent::Text { delta: text.to_owned() });
        self.emit(ProviderEvent::TextEnd);
        self.emit(ProviderEvent::TurnDone(ProviderTurn { ok: true, error: None }));
    }

    /// TypeScript `holdExit()`.
    fn hold_exit(&self) {
        let (exit, release) = gate();
        let mut state = self.state();
        state.exit = exit;
        state.exit_release = Some(release);
    }

    /// TypeScript `releaseExit()`.
    fn release_exit(&self) {
        let release = self.state().exit_release.take();
        if let Some(release) = release {
            let _ = release.send(Ok(()));
        }
    }

    /// TypeScript `steerResult = new Promise(...)`: `steer` answers with it until `answer_steer`.
    fn hold_steer(&self) {
        let (result, answer) = gate();
        let mut state = self.state();
        state.steer_result = Some(result);
        state.steer_answer = Some(answer);
    }

    /// TypeScript `answer(...)` or `reject(...)` for the held steer.
    fn answer_steer(&self, value: Result<bool, ProviderError>) {
        let answer = self.state().steer_answer.take();
        if let Some(answer) = answer {
            let _ = answer.send(value);
        }
    }

    /// TypeScript `releaseSteer()`: answers a still-held steer with `false`.
    fn release_steer(&self) {
        self.answer_steer(Ok(false));
    }
}

impl ProviderSession for SyntheticSession {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async { Ok::<(), ProviderError>(()) })
    }

    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        self.state().sent.push(text.to_owned());
        Box::pin(async { Ok::<(), ProviderError>(()) })
    }

    /// TypeScript `steer`: records the text when it can be steered, or when a steer result is held.
    fn steer(&self, text: &str) -> BoxFuture<'static, Result<bool, ProviderError>> {
        let mut state = self.state();
        if state.can_steer || state.steer_result.is_some() {
            state.steered.push(text.to_owned());
        }
        if let Some(result) = state.steer_result.clone() {
            return Box::pin(result);
        }
        let can_steer = state.can_steer;
        Box::pin(async move { Ok::<bool, ProviderError>(can_steer) })
    }

    /// TypeScript `interrupt()` ends the turn itself with `turn_done(ok: false)`.
    // TODO(M5-integration): assumed `Ok(true)` means "handled" (Core runner.rs:1965-1968, 2372-2386): a `false` would make
    // the runner call `stop` again, which the TypeScript does not do.
    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>> {
        self.emit(ProviderEvent::TurnDone(ProviderTurn { ok: false, error: None }));
        Box::pin(async { Ok::<bool, ProviderError>(true) })
    }

    fn answer(&self, _approval_id: &str, _answer: ApprovalAnswer) {}

    fn close(&self) {}

    fn wait_for_exit(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let exit = self.state().exit.clone();
        Box::pin(exit)
    }

    fn pid(&self) -> Option<u32> {
        None
    }

    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.state().listeners.push(sender);
        receiver
    }
}

/// One session the runner started, as TypeScript's `sessions` entries record it.
#[derive(Clone)]
struct SessionEntry {
    bot_id: String,
    options: StartOptions,
    session: Arc<SyntheticSession>,
}

type Sessions = Arc<Mutex<Vec<SessionEntry>>>;
type Logs = Arc<Mutex<Vec<(String, String)>>>;
/// The task service needs the runner, so the runner's hooks reach it weakly until `fixture` sets it.
type TasksCell = Arc<OnceLock<Weak<Tasks>>>;

fn tasks_of(cell: &TasksCell) -> Arc<Tasks> {
    cell.get()
        .and_then(Weak::upgrade)
        .expect("the task service is wired before the runner starts")
}

/// `hooks.executionContext`: the task service's `executionContext`.
fn execution_context_hook(tasks: TasksCell) -> ExecutionContextHook {
    Arc::new(move |core: &mut Core<'_>, bot: &StoredBot, item: &WorkItem| {
        tasks_of(&tasks)
            .execution_context(core.db(), bot, item)
            .map_err(|error| error.message)
    })
}

/// `hooks.turnStarted`: the task service's `turnStarted`.
fn turn_started_hook(tasks: TasksCell) -> ItemHook {
    Arc::new(move |core: &mut Core<'_>, item: &WorkItem| {
        tasks_of(&tasks)
            .turn_started(core.db(), item)
            .map_err(|error| error.message)
    })
}

/// `hooks.turnRequeued`: the task service's `turnRequeued`.
fn turn_requeued_hook(tasks: TasksCell) -> ItemHook {
    Arc::new(move |core: &mut Core<'_>, item: &WorkItem| {
        tasks_of(&tasks)
            .turn_requeued(core.db(), item)
            .map_err(|error| error.message)
    })
}

/// `hooks.turnEnded`: `tasks.turnEnded(item, result)`, then `team.turnEnded(item, result)`.
fn turn_ended_hook(tasks: TasksCell, team: Arc<Team>) -> TurnEndedHook {
    Arc::new(move |core: &mut Core<'_>, item: &WorkItem, result: &TurnResult| -> Result<(), String> {
        tasks_of(&tasks)
            .turn_ended(core.db(), item, result)
            .map_err(|error| error.message)?;
        // TODO(M5-integration): assumed `Team::turn_ended(&self, host: &mut dyn TeamHost, item, result)` returning
        // `Result<(), RpcError>` (lane A team.rs:740), with the runner's `Core` as the host.
        team.turn_ended(core, item, result)
            .map_err(|error| error.message)
    })
}

/// `hooks.sessionClosed`: `team.sessionClosed(id)`.
fn session_closed_hook(team: Arc<Team>) -> SessionClosedHook {
    Arc::new(move |_core: &mut Core<'_>, bot_id: &str| {
        // TODO(M5-integration): assumed `Team::session_closed(&self, bot_id: &str)` (lane A team.rs:398).
        team.session_closed(bot_id);
        Ok::<(), String>(())
    })
}

/// The TypeScript `fixture(options)`: `projectFixture` with the runner, the task service and the team wired as the
/// TypeScript test wires them, and the task and its first attempt.
struct Harness {
    f: Fixture,
    project: Project,
    developer: StoredBot,
    helper: StoredBot,
    runner: Runner,
    tasks: Arc<Tasks>,
    team: Arc<Team>,
    tools: Vec<McpTool>,
    sessions: Sessions,
    logs: Logs,
    task: ProjectTask,
    attempt: TaskAttempt,
}

/// `fixture(options)`. With `helper_outside_project`, the helper is removed from the project first.
fn fixture(helper_outside_project: bool) -> Harness {
    // `projectFixture()` builds its own task service over a fake runner. This test replaces it, as the TypeScript does.
    let f = project_support::fixture(TasksOptions::default());
    let developer = f.bots[0].clone();
    let helper = f.bots[1].clone();
    let project = if helper_outside_project {
        let members = f
            .project
            .input
            .members
            .iter()
            .filter(|member| member.bot_id != helper.id)
            .cloned()
            .collect::<Vec<_>>();
        let db = f.db.lock().unwrap();
        f.projects
            .update(
                &db,
                &f.project.id,
                f.project.revision,
                ProjectPatch { members: Some(members), ..ProjectPatch::default() },
            )
            .unwrap()
    } else {
        f.project.clone()
    };
    update_bot(
        &f.db.lock().unwrap(),
        &developer.id,
        &BotPatch { can_message: Some(vec![helper.id.clone()]), ..BotPatch::default() },
    )
    .unwrap();

    let sessions: Sessions = Arc::default();
    let logs: Logs = Arc::default();
    let log: Log = {
        let logs = logs.clone();
        Arc::new(move |context: &str, error: &str| {
            logs.lock().unwrap().push((context.to_owned(), error.to_owned()));
        })
    };
    let broadcast: Broadcast = Arc::new(|_event: Event| -> Result<(), String> { Ok(()) });
    // The TypeScript derives the shared folder from the database's location (team.ts:711-716); this test's database
    // sits in the fixture root, so the folder is `<root>/shared-files`.
    let shared_files = f.root.0.join("shared-files");
    fs::create_dir_all(&shared_files).unwrap();
    // TODO(M5-integration): assumed lane A's `TeamDeps` fields (team.rs:154). The TypeScript passes the runner's
    // closures; here the runner's hooks pass their `Core` as the host instead.
    let team = Arc::new(Team::new(TeamDeps {
        broadcast: broadcast.clone(),
        log: log.clone(),
        shared_files,
        attachments: None,
    }));

    let tasks_cell: TasksCell = Arc::default();
    let create_session: SessionFactory = {
        let sessions = sessions.clone();
        Arc::new(
            move |bot: &StoredBot, options: StartOptions| -> Result<Arc<dyn ProviderSession>, String> {
                let session = SyntheticSession::new();
                sessions.lock().unwrap().push(SessionEntry {
                    bot_id: bot.id.clone(),
                    options,
                    session: session.clone(),
                });
                Ok(session as Arc<dyn ProviderSession>)
            },
        )
    };
    let hooks = RunnerHooks {
        execution_context: Some(execution_context_hook(tasks_cell.clone())),
        turn_started: Some(turn_started_hook(tasks_cell.clone())),
        turn_requeued: Some(turn_requeued_hook(tasks_cell.clone())),
        turn_ended: Some(turn_ended_hook(tasks_cell.clone(), team.clone())),
        session_closed: Some(session_closed_hook(team.clone())),
        ..RunnerHooks::default()
    };
    // `new Runner({ db, broadcast, isSignedIn: () => true, createSession, hooks })`: the other deps are the defaults.
    // TODO(M5-integration): assumed `common::no_cli_setup()` (Core's tests/common/mod.rs:589); lane B's common lacks it.
    let runner = Runner::new(RunnerDeps {
        hooks,
        log,
        ..RunnerDeps::new(f.db.clone(), broadcast, create_session, common::no_cli_setup())
    });

    // `new Tasks(h.projects, {deliver, currentWork, stopItem, retired})`: over the real runner, which is a TaskRunner.
    let tasks = Tasks::new(f.projects.clone(), Arc::new(runner.clone()), TasksOptions::default());
    let _ = tasks_cell.set(Arc::downgrade(&tasks));
    runner.start().unwrap();
    // TODO(M5-integration): assumed `Team::tools(&self, host: Arc<dyn HostAccess>) -> Vec<McpTool>` (the TypeScript
    // `team.tools()`, team.ts:241). Lane A's tools.rs is not written yet.
    let tools = team.tools(Arc::new(runner.clone()) as Arc<dyn HostAccess>);

    let task = tasks
        .create(
            &f.db.lock().unwrap(),
            TaskInput {
                project_id: project.id.clone(),
                title: "Use helper feedback".into(),
                instructions: "Ask the other developer, then finish in this task worktree.".into(),
                acceptance: vec!["Use the helper reply".into()],
                assignee_id: developer.id.clone(),
                reviewer_id: Some(f.bots[2].id.clone()),
                tester_id: Some(f.bots[3].id.clone()),
            },
        )
        .unwrap();
    let attempt = tasks.start(&task.id, task.revision, "synthetic-start").unwrap();
    Harness {
        f,
        project,
        developer,
        helper,
        runner,
        tasks,
        team,
        tools,
        sessions,
        logs,
        task,
        attempt,
    }
}

impl Harness {
    /// TypeScript `own()`: the developer's sessions, in the order they were started.
    fn own(&self) -> Vec<SessionEntry> {
        self.sessions_of(&self.developer.id)
    }

    fn sessions_of(&self, bot_id: &str) -> Vec<SessionEntry> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.bot_id == bot_id)
            .cloned()
            .collect()
    }

    /// TypeScript `sessions.find(entry => entry.botId === id)`.
    fn first_session_of(&self, bot_id: &str) -> Option<SessionEntry> {
        self.sessions_of(bot_id).into_iter().next()
    }

    /// `vi.waitFor(() => expect(own()[index]?.session.sent).toHaveLength(1))` for the bot's session at `index`.
    async fn sent_one(&self, bot_id: &str, index: usize) -> SessionEntry {
        eventually(&format!("{bot_id}'s session {index} to get its message"), || {
            self.sessions_of(bot_id)
                .get(index)
                .filter(|entry| entry.session.sent().len() == 1)
                .cloned()
        })
        .await
    }

    /// TypeScript `asking()`: the developer asks the helper for feedback through `message_bot`, and the helper's
    /// session gets the request. Returns the helper's session.
    async fn asking(&self) -> Arc<SyntheticSession> {
        self.sent_one(&self.developer.id, 0).await;
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name == "message_bot")
            .expect("the message_bot tool");
        // TODO(M5-integration): assumed `McpTool::call(&self, bot_id: &str, arguments: &Value) -> ToolFuture`, which
        // parses the arguments with the tool's schema and then runs it (TypeScript `tool.input.parse` then `tool.run`).
        // Lane A's mcp.rs keeps `prepare` private, so there is no public way to run a tool yet.
        tool.call(
            &self.developer.id,
            &json!({ "bot": self.helper.name, "message": "Give me synthetic feedback." }),
        )
        .await
        .unwrap();
        self.sent_one(&self.helper.id, 0).await.session
    }

    /// TypeScript `reply()`: the developer's latest bot reply.
    fn reply_opt(&self) -> Option<WorkItem> {
        let db = self.f.db.lock().unwrap();
        let id: Option<String> = db
            .query_row(
                "SELECT id FROM work_items WHERE bot_id=?1 AND source='bot_reply' ORDER BY rowid DESC LIMIT 1",
                [&self.developer.id],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        id.map(|id| get_work_item(&db, &id).unwrap())
    }

    fn reply(&self) -> WorkItem {
        self.reply_opt().expect("the developer has a bot reply")
    }

    /// `vi.waitFor(() => expect(reply().status).toBe(status))`.
    async fn reply_reaches(&self, status: WorkStatus) {
        eventually(&format!("the reply to be {status:?}"), || {
            self.reply_opt().filter(|reply| reply.status == status).map(|_| ())
        })
        .await;
    }

    fn stored_attempt(&self, id: &str) -> TaskAttempt {
        self.tasks.attempt(&self.f.db.lock().unwrap(), id).unwrap()
    }

    fn stored_task(&self, id: &str) -> ProjectTask {
        self.tasks.task(&self.f.db.lock().unwrap(), id).unwrap()
    }

    /// `vi.waitFor(() => expect(tasks.attempt(attempt.id).state).toBe(state))`.
    async fn attempt_reaches(&self, state: AttemptState) {
        eventually(&format!("the attempt to be {state:?}"), || {
            (self.stored_attempt(&self.attempt.id).state == state).then_some(())
        })
        .await;
    }

    /// `vi.waitFor(() => expect(tasks.attempt(attempt.id).reason).toBe(reason))`.
    async fn attempt_reason_is(&self, reason: &str) {
        eventually(&format!("the attempt reason to be {reason:?}"), || {
            (self.stored_attempt(&self.attempt.id).reason.as_deref() == Some(reason)).then_some(())
        })
        .await;
    }

    fn bot_busy(&self, bot_id: &str) -> bool {
        self.tasks.bot_busy(&self.f.db.lock().unwrap(), bot_id).unwrap()
    }

    fn bot_in_use(&self, bot_id: &str) -> bool {
        self.f.projects.bot_in_use(&self.f.db.lock().unwrap(), bot_id).unwrap()
    }

    fn git(&self, path: &str, args: &[&str]) -> String {
        self.f.git(Path::new(path), args)
    }

    /// The TypeScript `afterEach` for this fixture: release what is held, shut the services down, let the runtime
    /// settle.
    async fn close(self) {
        let entries = self.sessions.lock().unwrap().clone();
        for entry in &entries {
            entry.session.release_exit();
            entry.session.release_steer();
        }
        within(self.tasks.shutdown()).await.unwrap();
        within(self.runner.shutdown()).await.unwrap();
        settle().await;
    }
}

#[tokio::test]
async fn retains_the_lease_after_an_asking_turn_and_completes_after_the_helper_continuation() {
    let h = fixture(false);
    let owner_before = h.f.owner();
    let helper = h.asking().await;
    h.own()[0].session.finish("The helper will answer later.");
    within(h.runner.retired(&h.developer.id)).await.unwrap();
    h.attempt_reaches(AttemptState::Running).await;
    assert!(h.bot_busy(&h.developer.id));

    helper.finish("Synthetic feedback: change the task file.");
    let second = h.sent_one(&h.developer.id, 1).await;
    assert!(second.session.sent()[0].contains("Synthetic feedback: change the task file."));
    assert_eq!(second.options.cwd, h.attempt.worktree_path);
    assert_eq!(h.runner.effective_folder(&h.developer.id).unwrap(), h.attempt.worktree_path);
    let reply = h.reply();
    let now = h.stored_attempt(&h.attempt.id);
    assert_eq!(now.work_item_id.as_deref(), Some(reply.id.as_str()));
    assert_eq!(now.state, AttemptState::Running);
    assert_eq!(now.revision, h.attempt.revision);

    let worktree = h.attempt.worktree_path.clone();
    fs::write(Path::new(&worktree).join("src").join("a.txt"), "Used synthetic feedback\n").unwrap();
    h.git(&worktree, &["add", "--", "src/a.txt"]);
    h.git(&worktree, &["commit", "-m", "Synthetic continuation result"]);
    second.session.finish("Committed the task after reading the helper reply.");
    h.attempt_reaches(AttemptState::Completed).await;
    assert_eq!(h.stored_task(&h.task.id).state, TaskState::Review);
    assert!(!h.bot_busy(&h.developer.id));
    let head = h.git(&worktree, &["rev-parse", "HEAD"]);
    assert_eq!(h.stored_attempt(&h.attempt.id).head_sha.as_deref(), Some(head.as_str()));
    assert_eq!(h.f.owner(), owner_before);
    h.close().await;
}

#[tokio::test]
async fn settles_the_reply_item_when_its_answer_arrives_before_the_original_process_retires() {
    let h = fixture(false);
    let helper = h.asking().await;
    let first = h.own()[0].session.clone();
    first.hold_exit();
    first.finish("Waiting for helper.");
    helper.finish("Immediate synthetic feedback.");
    h.reply_reaches(WorkStatus::Queued).await;
    assert!(h.bot_busy(&h.developer.id));

    first.release_exit();
    let second = h.sent_one(&h.developer.id, 1).await;
    assert!(second.session.sent()[0].contains("Immediate synthetic feedback."));
    let reply = h.reply();
    let now = h.stored_attempt(&h.attempt.id);
    assert_eq!(now.work_item_id.as_deref(), Some(reply.id.as_str()));
    assert_eq!(now.state, AttemptState::Running);

    second.session.finish("Used the reply.");
    h.attempt_reaches(AttemptState::Completed).await;
    assert_eq!(h.stored_task(&h.task.id).state, TaskState::Review);
    assert!(!h.bot_busy(&h.developer.id));
    h.close().await;
}

/// `it.each(["retained", "cascaded"])`: the helper's exchange is kept, or the database cascades it away with the bot.
async fn continues_in_the_task_worktree_after_deleting_an_external_helper(cascade: bool) {
    let h = fixture(true);
    let owner_before = h.f.owner();
    h.asking().await;
    assert!(!h.bot_in_use(&h.helper.id));
    assert!(!h.bot_busy(&h.helper.id));
    assert_eq!(h.first_session_of(&h.helper.id).unwrap().options.cwd, h.helper.home_folder);
    let request = h.runner.current_work(&h.helper.id).expect("the helper works on the request");
    let exchange_id = request.exchange_id.clone().expect("the request belongs to an exchange");
    h.own()[0].session.finish("The external helper will answer later.");
    h.attempt_reason_is("Waiting for requested bot replies.").await;
    assert!(h.bot_busy(&h.developer.id));

    within(h.runner.close_for_deletion(&h.helper.id).unwrap()).await.unwrap();
    // TODO(M5-integration): assumed lane A's `Team::bot_deleting(&self, host, bot_id) -> Result<BotDeletionHandle, _>`
    // (team.rs:1130), run through `with_host` because the test is outside a runner hook (TypeScript team.ts:345, test
    // line 123).
    let handle = with_host(&h.runner, |host| h.team.bot_deleting(host, &h.helper.id)).unwrap();
    if cascade {
        h.f.db
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER cascade_exchanges AFTER DELETE ON bots BEGIN DELETE FROM exchanges WHERE from_bot_id = OLD.id OR to_bot_id = OLD.id; END",
            )
            .unwrap();
    }
    delete_bot(&h.f.db.lock().unwrap(), &h.helper.id).unwrap();
    assert!(get_stored_bot(&h.f.db.lock().unwrap(), &h.helper.id).is_err());
    if cascade {
        // TODO(M5-integration): assumed `get_exchange(&Connection, &str)` (team.rs:253, TypeScript team.ts:109).
        let error = get_exchange(&h.f.db.lock().unwrap(), &exchange_id).unwrap_err();
        assert_eq!(error.message, "No such exchange");
    }
    // TODO(M5-integration): assumed `Team::bot_deleted(&self, host, &BotDeletionHandle) -> Result<(), RpcError>`
    // (team.rs:1190, TypeScript team.ts:379, test line 128).
    with_host(&h.runner, |host| h.team.bot_deleted(host, &handle)).unwrap();
    h.runner.forget(&h.helper.id);

    let second = h.sent_one(&h.developer.id, 1).await;
    let reply = h.reply();
    let deleted = format!("{} was deleted", h.helper.name);
    assert!(second.session.sent()[0].contains(&deleted));
    assert_eq!(second.options.cwd, h.attempt.worktree_path);
    assert_eq!(h.runner.effective_folder(&h.developer.id).unwrap(), h.attempt.worktree_path);
    assert_eq!(reply.source, WorkSource::BotReply);
    assert_eq!(reply.status, WorkStatus::Running);
    assert_eq!(reply.exchange_id.as_deref(), Some(exchange_id.as_str()));
    assert_eq!(reply.task_attempt_id.as_deref(), Some(h.attempt.id.as_str()));
    assert_eq!(reply.task_revision, Some(h.attempt.revision as i64));
    assert_eq!(reply.execution_context_key.as_deref(), Some(h.attempt.context_key.as_str()));
    let now = h.stored_attempt(&h.attempt.id);
    assert_eq!(now.state, AttemptState::Running);
    assert_eq!(now.work_item_id.as_deref(), Some(reply.id.as_str()));
    assert_eq!(now.revision, h.attempt.revision);
    assert_eq!(now.context_key, h.attempt.context_key);
    assert!(h.bot_busy(&h.developer.id));
    let exchange = get_exchange(&h.f.db.lock().unwrap(), &exchange_id).unwrap();
    assert_eq!(exchange.status, ExchangeStatus::Failed);
    assert_eq!(exchange.reply.as_deref(), Some(deleted.as_str()));
    let stored: (Option<String>, Option<i64>, Option<String>) = h
        .f
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT task_attempt_id, task_revision, execution_context_key FROM exchanges WHERE id = ?1",
            [&exchange_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        stored,
        (
            Some(h.attempt.id.clone()),
            Some(h.attempt.revision as i64),
            Some(h.attempt.context_key.clone()),
        )
    );

    second.session.finish("Handled the deleted helper and finished the task.");
    h.attempt_reaches(AttemptState::Completed).await;
    assert_eq!(h.stored_attempt(&h.attempt.id).work_item_id.as_deref(), Some(reply.id.as_str()));
    h.reply_reaches(WorkStatus::Done).await;
    assert_eq!(h.stored_task(&h.task.id).state, TaskState::Review);
    assert!(!h.bot_busy(&h.developer.id));
    let worktree = h.attempt.worktree_path.clone();
    let head = h.git(&worktree, &["rev-parse", "HEAD"]);
    assert_eq!(h.stored_attempt(&h.attempt.id).head_sha.as_deref(), Some(head.as_str()));
    assert_eq!(h.f.owner(), owner_before);
    h.close().await;
}

#[tokio::test]
async fn continues_after_deleting_an_external_helper_with_its_exchange_retained() {
    continues_in_the_task_worktree_after_deleting_an_external_helper(false).await;
}

#[tokio::test]
async fn continues_after_deleting_an_external_helper_with_its_exchange_cascaded() {
    continues_in_the_task_worktree_after_deleting_an_external_helper(true).await;
}

#[tokio::test]
async fn does_not_create_another_turn_when_a_helper_reply_was_steered_into_the_task_turn() {
    let h = fixture(false);
    let helper = h.asking().await;
    let first = h.own()[0].session.clone();
    first.allow_steer();
    helper.finish("Synthetic feedback during the original turn.");
    eventually("the reply to be steered into the turn", || {
        (first.steered().len() == 1).then_some(())
    })
    .await;
    h.reply_reaches(WorkStatus::Done).await;
    assert_eq!(h.stored_attempt(&h.attempt.id).work_item_id, h.attempt.work_item_id);

    first.finish("Used the steered helper reply.");
    h.attempt_reaches(AttemptState::Completed).await;
    assert_eq!(h.own().len(), 1);
    assert!(!h.bot_busy(&h.developer.id));
    h.close().await;
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SteerOutcome {
    Declined,
    Rejected,
    Accepted,
}

/// `it.each(["declined", "rejected", "accepted"])`: the original process retires while its steer is still pending.
async fn retains_the_task_while_a_delayed_steer_is(outcome: SteerOutcome) {
    let h = fixture(false);
    let helper = h.asking().await;
    let first = h.own()[0].session.clone();
    first.hold_steer();
    // The TypeScript spies on `console.error` from here on.
    h.logs.lock().unwrap().clear();
    first.hold_exit();
    helper.finish("Feedback whose steering acknowledgment is delayed.");
    eventually("the reply to be steered", || (first.steered().len() == 1).then_some(())).await;
    first.finish("The original turn ended before steering resolved.");
    first.release_exit();
    settle().await;
    assert_eq!(h.stored_attempt(&h.attempt.id).state, AttemptState::Running);
    assert!(h.bot_busy(&h.developer.id));

    match outcome {
        SteerOutcome::Declined => first.answer_steer(Ok(false)),
        SteerOutcome::Accepted => first.answer_steer(Ok(true)),
        SteerOutcome::Rejected => first.answer_steer(Err(ProviderError::Failed(
            "Synthetic delayed steering rejection".into(),
        ))),
    }
    if outcome == SteerOutcome::Accepted {
        h.attempt_reaches(AttemptState::Completed).await;
        assert_eq!(h.reply().status, WorkStatus::Done);
        assert_eq!(h.own().len(), 1);
        assert!(!h.bot_busy(&h.developer.id));
        assert!(h.logs.lock().unwrap().is_empty());
        h.close().await;
        return;
    }

    let second = h.sent_one(&h.developer.id, 1).await;
    assert_eq!(second.options.cwd, h.attempt.worktree_path);
    assert!(second.session.sent()[0].contains("Feedback whose steering acknowledgment is delayed."));
    let now = h.stored_attempt(&h.attempt.id);
    assert_eq!(now.state, AttemptState::Running);
    assert_eq!(now.work_item_id.as_deref(), Some(h.reply().id.as_str()));

    second.session.finish("Completed after the delayed helper reply.");
    h.attempt_reaches(AttemptState::Completed).await;
    assert!(!h.bot_busy(&h.developer.id));
    let logs = h.logs.lock().unwrap().clone();
    if outcome == SteerOutcome::Rejected {
        assert!(logs.iter().any(|(context, error)| {
            context.as_str() == "Tjommie: steering a work item failed"
                && error.as_str() == "Synthetic delayed steering rejection"
        }));
    } else {
        assert!(logs.is_empty());
    }
    h.close().await;
}

#[tokio::test]
async fn retains_the_task_while_a_delayed_steer_is_declined() {
    retains_the_task_while_a_delayed_steer_is(SteerOutcome::Declined).await;
}

#[tokio::test]
async fn retains_the_task_while_a_delayed_steer_is_rejected() {
    retains_the_task_while_a_delayed_steer_is(SteerOutcome::Rejected).await;
}

#[tokio::test]
async fn retains_the_task_while_a_delayed_steer_is_accepted() {
    retains_the_task_while_a_delayed_steer_is(SteerOutcome::Accepted).await;
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LateOperation {
    Cancel,
    Resume,
    Edit,
    Archive,
}

/// `it.each(["cancel", "resume", "edit", "archive"])`: a helper reply that arrives after the task was stopped,
/// resumed, edited or archived belongs to the old context. It fails and never reaches the home folder.
async fn fences_a_late_task_helper_reply_after_an_explicit_operation(operation: LateOperation) {
    let h = fixture(false);
    let helper = h.asking().await;
    h.own()[0].session.finish("The helper reply will arrive later.");
    h.attempt_reason_is("Waiting for requested bot replies.").await;
    let cancel = matches!(operation, LateOperation::Cancel | LateOperation::Archive);
    let stopped = h
        .tasks
        .stop(&h.task.id, h.stored_task(&h.task.id).revision, cancel)
        .await
        .unwrap();
    match operation {
        LateOperation::Cancel => {}
        LateOperation::Resume => {
            let resumed = h.tasks.resume(&h.task.id, stopped.revision, "explicit-resume").unwrap();
            assert_eq!(resumed.revision, h.attempt.revision + 1);
            h.sent_one(&h.developer.id, 1).await;
        }
        LateOperation::Edit => {
            let db = h.f.db.lock().unwrap();
            h.tasks
                .update(
                    &db,
                    &h.task.id,
                    stopped.revision,
                    TaskPatch { title: Some("Revised task".into()), ..TaskPatch::default() },
                )
                .unwrap();
        }
        LateOperation::Archive => {
            let db = h.f.db.lock().unwrap();
            h.f.projects.archive(&db, &h.project.id, h.project.revision).unwrap();
        }
    }

    helper.finish("This answer belongs to the old immutable task context.");
    if operation == LateOperation::Resume {
        h.reply_reaches(WorkStatus::Queued).await;
        h.own()[1].session.finish("Completed the explicitly resumed task.");
        h.attempt_reaches(AttemptState::Completed).await;
    }
    h.reply_reaches(WorkStatus::Failed).await;
    let own = h.own();
    assert_eq!(own.len(), if operation == LateOperation::Resume { 2 } else { 1 });
    assert!(own.iter().all(|entry| entry.options.cwd == h.attempt.worktree_path));
    let reply = h.reply();
    assert_eq!(reply.task_attempt_id.as_deref(), Some(h.attempt.id.as_str()));
    assert_eq!(reply.task_revision, Some(h.attempt.revision as i64));
    assert_eq!(reply.execution_context_key.as_deref(), Some(h.attempt.context_key.as_str()));
    let (attempt_state, task_state) = match operation {
        LateOperation::Resume => (AttemptState::Completed, TaskState::Review),
        LateOperation::Edit => (AttemptState::Interrupted, TaskState::Todo),
        LateOperation::Cancel | LateOperation::Archive => {
            (AttemptState::Cancelled, TaskState::Cancelled)
        }
    };
    assert_eq!(h.stored_attempt(&h.attempt.id).state, attempt_state);
    assert_eq!(h.stored_task(&h.task.id).state, task_state);
    h.close().await;
}

#[tokio::test]
async fn fences_a_late_task_helper_reply_after_explicit_cancel_without_using_the_home_folder() {
    fences_a_late_task_helper_reply_after_an_explicit_operation(LateOperation::Cancel).await;
}

#[tokio::test]
async fn fences_a_late_task_helper_reply_after_explicit_resume_without_using_the_home_folder() {
    fences_a_late_task_helper_reply_after_an_explicit_operation(LateOperation::Resume).await;
}

#[tokio::test]
async fn fences_a_late_task_helper_reply_after_explicit_edit_without_using_the_home_folder() {
    fences_a_late_task_helper_reply_after_an_explicit_operation(LateOperation::Edit).await;
}

#[tokio::test]
async fn fences_a_late_task_helper_reply_after_explicit_archive_without_using_the_home_folder() {
    fences_a_late_task_helper_reply_after_an_explicit_operation(LateOperation::Archive).await;
}
