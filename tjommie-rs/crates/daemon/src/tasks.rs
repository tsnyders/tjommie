//! `tasks.ts`: tasks, their attempts and their evaluations.
//!
//! Locking. The database is one `Mutex<Connection>`, and the runner locks it itself while it runs a turn and its hooks.
//! So:
//! - Methods that take a `&Connection` expect the caller to hold the lock (handlers, and the runner's hooks, which get
//!   the connection from `Core`). They never call the runner.
//! - Methods without a connection (`start`, `resume`, `stop`, `evaluate`, `confirmed_exited`, `shutdown`) lock the
//!   database in short sections and never hold it across an `.await` or a call into the runner, which would deadlock.
//!   Git and other slow work runs with the lock released.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use protocol::model::Autonomy;
use protocol::projects::{
    AttemptState, EvaluationKind, EvaluationState, ProjectRole, ProjectTask, TaskAttempt,
    TaskCleanupPreview, TaskDetail, TaskDiff, TaskEvaluation, TaskInput, TaskIntegration,
    TaskPatch, TaskPhase, TaskState,
};
use protocol::{ErrorCode, Event};
use rusqlite::{Connection, params};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::bots::{StoredBot, get_stored_bot};
use crate::cli::BoxFuture;
use crate::ids::{new_id, now_iso};
use crate::project_checks::{CheckError, CheckRuntime};
use crate::project_review::{
    ReviewContext, ReviewInstructions, ReviewRuntime, frozen_review_context, review_coverage,
    review_snapshot,
};
use crate::projects::{
    Projects, data_list, data_of, exists, slice_units, to_json, transaction, wire_text,
};
use crate::queue::{NewWork, WorkItem, WorkSource, WorkStatus, enqueue, get_work_item, set_status};
use crate::runner::{ExecutionPhase, TaskExecutionContext, TurnOutcome, TurnResult};
use crate::server::RpcError;
use crate::task_execution::execution_key;
use crate::worktrees::{ManagedWorktree, Repository};

/// `TaskRunner` (`tasks.ts:15`): what tasks need from the runner. Core's `Runner` implements it below. The methods
/// lock the database themselves, so call them with the database lock released.
pub trait TaskRunner: Send + Sync {
    /// `deliver(item)`: hands a queued item to the runner.
    fn deliver(&self, item: WorkItem) -> Result<WorkItem, RpcError>;
    fn current_work(&self, bot_id: &str) -> Option<WorkItem>;
    /// `stopItem(id, notice)`.
    fn stop_item(&self, item_id: &str, notice: &str) -> BoxFuture<'static, Result<(), String>>;
    /// `retired(botId)`: resolves once the bot's last process is confirmed gone.
    fn retired(&self, bot_id: &str) -> BoxFuture<'static, Result<(), String>>;
}

/// Core's runner is the real `TaskRunner`.
impl TaskRunner for crate::runner::Runner {
    fn deliver(&self, item: WorkItem) -> Result<WorkItem, RpcError> {
        crate::runner::Runner::deliver(self, item)
    }

    fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        crate::runner::Runner::current_work(self, bot_id)
    }

    fn stop_item(&self, item_id: &str, notice: &str) -> BoxFuture<'static, Result<(), String>> {
        let stopping = crate::runner::Runner::stop_item(self, item_id, notice);
        Box::pin(async move { stopping.await.map_err(|error| error.message().to_owned()) })
    }

    fn retired(&self, bot_id: &str) -> BoxFuture<'static, Result<(), String>> {
        let retiring = crate::runner::Runner::retired(self, bot_id);
        Box::pin(async move { retiring.await.map_err(|error| error.message().to_owned()) })
    }
}

/// `new Tasks(projects, runner, { review, checks })`, plus the host check runtime a Full-auto tester falls back to.
#[derive(Default, Clone)]
pub struct TasksOptions {
    pub review: Option<Arc<dyn ReviewRuntime>>,
    pub checks: Option<Arc<dyn CheckRuntime>>,
    /// `new HostCheckRuntime()`: used when `checks` is unset and the tester's autonomy is `full`.
    pub host_checks: Option<Arc<dyn CheckRuntime>>,
}

#[derive(Default)]
struct State {
    operations: HashMap<String, CancellationToken>,
    closing: bool,
    termination_unknown: bool,
}

/// What `settle` reads in one database pass: the attempt, and its repository and managed worktree (or why they
/// could not be read, which settles the attempt as blocked instead of failing the whole turn).
type SettlePlan = (TaskAttempt, Result<(Repository, ManagedWorktree), RpcError>);

pub struct Tasks {
    pub projects: Arc<Projects>,
    runner: Arc<dyn TaskRunner>,
    options: TasksOptions,
    handle: Handle,
    state: Mutex<State>,
    /// How many `evaluate` calls are running; `shutdown` waits for zero.
    settling: watch::Sender<usize>,
}

struct Settling<'a>(&'a Tasks);

impl<'a> Settling<'a> {
    fn new(tasks: &'a Tasks) -> Self {
        tasks.settling.send_modify(|count| *count += 1);
        Self(tasks)
    }
}

impl Drop for Settling<'_> {
    fn drop(&mut self) {
        self.0.settling.send_modify(|count| *count -= 1);
    }
}

pub fn active_attempt(attempt: &TaskAttempt) -> bool {
    matches!(
        attempt.state,
        AttemptState::Provisioning | AttemptState::Queued | AttemptState::Running
    )
}

pub(crate) fn bad_state(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadState, message)
}

pub(crate) fn internal(message: impl ToString) -> RpcError {
    RpcError::new(ErrorCode::Internal, message.to_string())
}

/// `evaluationData`: a stored evaluation, with `reviewCoverage` present (`null` when it was missing).
fn normalise(mut evaluation: TaskEvaluation) -> TaskEvaluation {
    if evaluation.review_coverage.is_none() {
        evaluation.review_coverage = Some(None);
    }
    evaluation
}

pub(crate) fn failed(error: RpcError) -> CheckError {
    CheckError::Failed(error.message)
}

/// Runs blocking work (git, the database) off the async workers.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| internal("The task service stopped unexpectedly."))
}

fn execution_phase(phase: TaskPhase) -> ExecutionPhase {
    match phase {
        TaskPhase::Develop => ExecutionPhase::Develop,
        TaskPhase::Review => ExecutionPhase::Review,
        TaskPhase::Test => ExecutionPhase::Test,
    }
}

/// What `evaluate` needs once its first section has run.
struct EvaluationRun {
    attempt: TaskAttempt,
    task: ProjectTask,
    project: protocol::projects::Project,
    bot: StoredBot,
    checks: Option<Arc<dyn CheckRuntime>>,
    controller: CancellationToken,
    key: String,
}

enum Begin {
    /// The evaluation was `unavailable`; it is saved and finished.
    Done(Box<TaskEvaluation>),
    Run(Box<(EvaluationRun, TaskEvaluation)>),
}

impl Tasks {
    /// Must be called inside a Tokio runtime: settling and integration work run on it.
    pub fn new(
        projects: Arc<Projects>,
        runner: Arc<dyn TaskRunner>,
        options: TasksOptions,
    ) -> Arc<Self> {
        Arc::new(Self {
            projects,
            runner,
            options,
            handle: Handle::current(),
            state: Mutex::default(),
            settling: watch::channel(0).0,
        })
    }

    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn closing(&self) -> bool {
        self.state().closing
    }

    /// The database, for a caller that runs several steps in one lock section.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.projects.lock()
    }

    pub fn emit(&self, event: Event) {
        self.projects.emit(event);
    }

    // -- storage ---------------------------------------------------------------------------------------------------

    pub fn task(&self, db: &Connection, id: &str) -> Result<ProjectTask, RpcError> {
        data_of(db, "SELECT data FROM project_tasks WHERE id = ?1", [id])
    }

    pub fn attempt(&self, db: &Connection, id: &str) -> Result<TaskAttempt, RpcError> {
        data_of(db, "SELECT data FROM task_attempts WHERE id = ?1", [id])
    }

    pub fn list(&self, db: &Connection, project_id: &str) -> Result<Vec<ProjectTask>, RpcError> {
        self.projects.get(db, project_id)?;
        data_list(
            db,
            "SELECT data FROM project_tasks WHERE project_id = ?1 ORDER BY rowid",
            [project_id],
        )
    }

    /// `detail`: the task with every attempt, evaluation and integration, after refreshing its current attempt.
    pub fn detail(&self, db: &Connection, id: &str) -> Result<TaskDetail, RpcError> {
        let mut task = self.task(db, id)?;
        if let Some(attempt_id) = task.current_attempt_id.clone() {
            self.refresh(db, &attempt_id)?;
            task = self.task(db, id)?;
        }
        let attempts = data_list(
            db,
            "SELECT data FROM task_attempts WHERE task_id = ?1 ORDER BY ordinal",
            [id],
        )?;
        let evaluations = data_list::<TaskEvaluation>(
            db,
            "SELECT e.data FROM task_evaluations e JOIN task_attempts a ON a.id = e.attempt_id WHERE a.task_id = ?1 ORDER BY e.rowid",
            [id],
        )?
        .into_iter()
        .map(normalise)
        .collect();
        let integrations = data_list::<TaskIntegration>(
            db,
            "SELECT i.data FROM task_integrations i JOIN task_attempts a ON a.id = i.attempt_id WHERE a.task_id = ?1 ORDER BY i.rowid",
            [id],
        )?;
        Ok(TaskDetail {
            task,
            attempts,
            evaluations,
            integrations,
        })
    }

    pub fn save_task(&self, db: &Connection, task: &ProjectTask) -> Result<(), RpcError> {
        db.execute(
            "UPDATE project_tasks SET state = ?1, revision = ?2, data = ?3 WHERE id = ?4",
            params![
                wire_text(&task.state),
                task.revision as i64,
                to_json(task)?,
                task.id
            ],
        )?;
        self.emit(Event::TaskUpdated(task.clone()));
        Ok(())
    }

    pub fn save_attempt(&self, db: &Connection, attempt: &TaskAttempt) -> Result<(), RpcError> {
        db.execute(
            "UPDATE task_attempts SET state = ?1, data = ?2 WHERE id = ?3",
            params![wire_text(&attempt.state), to_json(attempt)?, attempt.id],
        )?;
        self.emit(Event::TaskAttemptUpdated(attempt.clone()));
        Ok(())
    }

    pub fn save_evaluation(
        &self,
        db: &Connection,
        evaluation: &TaskEvaluation,
    ) -> Result<(), RpcError> {
        let evaluation = normalise(evaluation.clone());
        db.execute(
            "INSERT INTO task_evaluations VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(id) DO UPDATE SET state = excluded.state, data = excluded.data",
            params![
                evaluation.id,
                evaluation.attempt_id,
                evaluation.revision as i64,
                evaluation.source_sha,
                wire_text(&evaluation.kind),
                wire_text(&evaluation.state),
                to_json(&evaluation)?
            ],
        )?;
        self.emit(Event::TaskEvaluationUpdated(evaluation));
        Ok(())
    }

    /// `#assignments`: each named bot is a project member in the matching role.
    fn assignments(&self, db: &Connection, task: &TaskInput) -> Result<(), RpcError> {
        let project = self.projects.active(db, &task.project_id)?;
        let named = [
            (Some(&task.assignee_id), ProjectRole::Developer),
            (task.reviewer_id.as_ref(), ProjectRole::Reviewer),
            (task.tester_id.as_ref(), ProjectRole::Tester),
        ];
        for (id, role) in named {
            let Some(id) = id.filter(|id| !id.is_empty()) else {
                continue;
            };
            get_stored_bot(db, id)?;
            let member = project
                .input
                .members
                .iter()
                .any(|member| &member.bot_id == id && member.roles.contains(&role));
            if !member {
                return Err(RpcError::new(
                    ErrorCode::BadParams,
                    format!("Assign a project {} to this task.", wire_text(&role)),
                ));
            }
        }
        Ok(())
    }

    pub fn create(&self, db: &Connection, input: TaskInput) -> Result<ProjectTask, RpcError> {
        self.assignments(db, &input)?;
        let timestamp = now_iso();
        let task = ProjectTask {
            input,
            id: new_id(),
            state: TaskState::Todo,
            current_attempt_id: None,
            revision: 1,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        db.execute(
            "INSERT INTO project_tasks VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                task.id,
                task.input.project_id,
                wire_text(&task.state),
                task.revision as i64,
                to_json(&task)?
            ],
        )?;
        self.emit(Event::TaskUpdated(task.clone()));
        Ok(task)
    }

    pub fn expected(
        &self,
        db: &Connection,
        id: &str,
        revision: u64,
    ) -> Result<ProjectTask, RpcError> {
        let task = self.task(db, id)?;
        if task.revision != revision {
            return Err(bad_state("This task changed. Refresh before continuing."));
        }
        Ok(task)
    }

    pub fn update(
        &self,
        db: &Connection,
        id: &str,
        revision: u64,
        patch: TaskPatch,
    ) -> Result<ProjectTask, RpcError> {
        let task = self.expected(db, id, revision)?;
        if let Some(attempt_id) = &task.current_attempt_id
            && active_attempt(&self.attempt(db, attempt_id)?)
        {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "Stop the current attempt before editing its task.",
            ));
        }
        let mut input = task.input.clone();
        if let Some(title) = patch.title {
            input.title = title;
        }
        if let Some(instructions) = patch.instructions {
            input.instructions = instructions;
        }
        if let Some(acceptance) = patch.acceptance {
            input.acceptance = acceptance;
        }
        if let Some(assignee_id) = patch.assignee_id {
            input.assignee_id = assignee_id;
        }
        if let Some(reviewer_id) = patch.reviewer_id {
            input.reviewer_id = reviewer_id;
        }
        if let Some(tester_id) = patch.tester_id {
            input.tester_id = tester_id;
        }
        let next = ProjectTask {
            input,
            state: TaskState::Todo,
            current_attempt_id: None,
            revision: task.revision + 1,
            updated_at: now_iso(),
            ..task
        };
        self.assignments(db, &next.input)?;
        self.save_task(db, &next)?;
        Ok(next)
    }

    // -- attempts, leases, requests ---------------------------------------------------------------------------------

    pub fn work(&self, attempt: &TaskAttempt) -> Result<ManagedWorktree, RpcError> {
        let work = self.projects.worktrees.describe(
            &attempt.id,
            &attempt.base_sha,
            Some(&attempt.branch),
        )?;
        if work.path != attempt.worktree_path {
            return Err(bad_state("The persisted task worktree path changed."));
        }
        Ok(work)
    }

    pub fn repository(
        &self,
        db: &Connection,
        attempt: &TaskAttempt,
    ) -> Result<Repository, RpcError> {
        let task = self.task(db, &attempt.task_id)?;
        let project = self.projects.get(db, &task.input.project_id)?;
        self.projects.repository(db, &project.input.repository_id)
    }

    pub fn bot_busy(&self, db: &Connection, bot_id: &str) -> Result<bool, RpcError> {
        exists(
            db,
            "SELECT 1 FROM project_bot_leases WHERE bot_id = ?1",
            [bot_id],
        )
    }

    pub fn lease(
        &self,
        db: &Connection,
        bot_id: &str,
        owner_id: &str,
        kind: &str,
    ) -> Result<(), RpcError> {
        self.projects.assert_bot_idle(db, bot_id)?;
        if self.bot_busy(db, bot_id)? {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "This bot already owns an active task or check.",
            ));
        }
        db.execute(
            "INSERT INTO project_bot_leases VALUES (?1, ?2, ?3)",
            params![bot_id, owner_id, kind],
        )?;
        Ok(())
    }

    pub fn release(&self, db: &Connection, bot_id: &str, owner_id: &str) -> Result<(), RpcError> {
        db.execute(
            "DELETE FROM project_bot_leases WHERE bot_id = ?1 AND owner_id = ?2",
            params![bot_id, owner_id],
        )?;
        Ok(())
    }

    /// `#request`: the result of an earlier request with this id, if any.
    fn request(
        &self,
        db: &Connection,
        id: &str,
        operation: &str,
        task_id: &str,
    ) -> Result<Option<String>, RpcError> {
        let row: Option<(String, String, String)> =
            rusqlite::OptionalExtension::optional(db.query_row(
                "SELECT operation, task_id, result_id FROM task_requests WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ))?;
        match row {
            None => Ok(None),
            Some((used, task, result)) if used == operation && task == task_id => Ok(Some(result)),
            Some(_) => Err(RpcError::new(
                ErrorCode::BadParams,
                "This request ID was already used for another action.",
            )),
        }
    }

    /// `#context`: the instructions and identity of a developer attempt's next turn.
    fn context(
        &self,
        db: &Connection,
        attempt: &TaskAttempt,
    ) -> Result<TaskExecutionContext, RpcError> {
        let task = self.task(db, &attempt.task_id)?;
        let project = self.projects.active(db, &task.input.project_id)?;
        let member = project
            .input
            .members
            .iter()
            .find(|candidate| candidate.bot_id == attempt.bot_id)
            .filter(|member| member.roles.contains(&ProjectRole::Developer))
            .ok_or_else(|| bad_state("The task's developer assignment changed."))?;
        let recent = data_list::<TaskEvaluation>(
            db,
            "SELECT data FROM task_evaluations WHERE attempt_id = ?1 ORDER BY rowid DESC LIMIT 2",
            [&attempt.id],
        )?;
        let mut lines: Vec<String> = Vec::new();
        for evaluation in recent.into_iter().map(normalise) {
            lines.push(evaluation.reason.clone().unwrap_or_default());
            for finding in &evaluation.findings {
                lines.push(format!(
                    "{} {}:{}: {}",
                    wire_text(&finding.severity),
                    finding.path,
                    finding
                        .line
                        .map(|line| line.to_string())
                        .unwrap_or_default(),
                    finding.message
                ));
            }
            for check in evaluation
                .checks
                .iter()
                .filter(|check| check.exit_code != Some(0))
            {
                lines.push(format!(
                    "{}: {}",
                    check.command,
                    slice_units(&check.output, 4000)
                ));
            }
        }
        let feedback = slice_units(
            &lines
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            12_000,
        );
        let mut parts = vec![
            format!(
                "Project: {}. Task: {}.",
                project.input.name, task.input.title
            ),
            task.input.instructions.clone(),
        ];
        parts.extend(
            task.input
                .acceptance
                .iter()
                .map(|item| format!("Acceptance: {item}")),
        );
        parts.push(member.instructions.clone());
        if !feedback.is_empty() {
            parts.push(format!("Previous independent feedback:\n{feedback}"));
        }
        parts.push("Work only in the assigned managed worktree. Commit completed task changes on its assigned branch with the existing Git identity. Do not switch or change the target branch, push, rebase, stash or remove worktrees. Report checks accurately; use share_file for deliverables.".into());
        let instructions = parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let phase = execution_phase(attempt.phase);
        let key = execution_key(
            &attempt.bot_id,
            &attempt.id,
            attempt.revision,
            phase,
            &attempt.worktree_path,
            &instructions,
        )?;
        Ok(TaskExecutionContext {
            key,
            cwd: attempt.worktree_path.clone(),
            attempt_id: attempt.id.clone(),
            revision: attempt.revision as i64,
            phase,
            instructions,
        })
    }

    /// `#matchesActiveWork`: the item is the current develop turn of the task's current attempt.
    fn matches_active_work(
        &self,
        db: &Connection,
        attempt: &TaskAttempt,
        item: &WorkItem,
    ) -> Result<bool, RpcError> {
        let task = self.task(db, &attempt.task_id)?;
        Ok(
            task.current_attempt_id.as_deref() == Some(attempt.id.as_str())
                && self.projects.get(db, &task.input.project_id)?.state
                    == protocol::projects::ProjectState::Active
                && matches!(attempt.state, AttemptState::Queued | AttemptState::Running)
                && attempt.bot_id == item.bot_id
                && Some(attempt.revision as i64) == item.task_revision
                && item.execution_context_key.as_deref() == Some(attempt.context_key.as_str())
                && exists(
                    db,
                    "SELECT 1 FROM project_bot_leases WHERE bot_id = ?1 AND owner_id = ?2 AND kind = 'develop'",
                    params![item.bot_id, attempt.id],
                )?,
        )
    }

    /// `#pendingReplies`: the developer is waiting on another bot's reply for this attempt.
    fn pending_replies(&self, db: &Connection, attempt: &TaskAttempt) -> Result<bool, RpcError> {
        let revision = attempt.revision as i64;
        Ok(exists(
            db,
            "SELECT 1 FROM exchanges WHERE from_bot_id = ?1 AND task_attempt_id = ?2 AND task_revision = ?3 AND execution_context_key = ?4 AND status = 'waiting' LIMIT 1",
            params![attempt.bot_id, attempt.id, revision, attempt.context_key],
        )? || exists(
            db,
            "SELECT 1 FROM work_items WHERE bot_id = ?1 AND task_attempt_id = ?2 AND task_revision = ?3 AND execution_context_key = ?4 AND source = 'bot_reply' AND status IN ('queued', 'running') LIMIT 1",
            params![attempt.bot_id, attempt.id, revision, attempt.context_key],
        )?)
    }

    /// `executionContext`: the runner's `execution_context` hook. `None` for a bot outside any project.
    pub fn execution_context(
        &self,
        db: &Connection,
        bot: &StoredBot,
        item: &WorkItem,
    ) -> Result<Option<TaskExecutionContext>, RpcError> {
        let Some(attempt_id) = &item.task_attempt_id else {
            let binding: Option<String> = rusqlite::OptionalExtension::optional(db.query_row(
                "SELECT project_id FROM project_members WHERE bot_id = ?1 AND active = 1",
                [&bot.id],
                |row| row.get(0),
            ))?;
            let Some(project_id) = binding else {
                return Ok(None);
            };
            let project = self.projects.active(db, &project_id)?;
            let member_instructions = project
                .input
                .members
                .iter()
                .find(|candidate| candidate.bot_id == bot.id)
                .map(|member| member.instructions.clone())
                .unwrap_or_default();
            let attempt_id = format!("discussion-{}", project.id);
            let instructions = format!(
                "Project team: {}. This is a discussion turn in your home folder. Propose task drafts and assignments in the room; coding in the repository begins only after the person starts a task. Do not use the user's repository checkout as your working folder.\n{}",
                project.input.name, member_instructions
            );
            let key = execution_key(
                &bot.id,
                &attempt_id,
                project.revision,
                ExecutionPhase::Discussion,
                &bot.home_folder,
                &instructions,
            )?;
            return Ok(Some(TaskExecutionContext {
                key,
                cwd: bot.home_folder.clone(),
                attempt_id,
                revision: project.revision as i64,
                phase: ExecutionPhase::Discussion,
                instructions,
            }));
        };
        let attempt = self.attempt(db, attempt_id)?;
        if attempt.bot_id != bot.id || !self.matches_active_work(db, &attempt, item)? {
            return Err(bad_state("This task attempt is no longer active."));
        }
        let repo = self.repository(db, &attempt)?;
        self.projects
            .worktrees
            .assert_owned(&repo, &self.work(&attempt)?)?;
        let context = self.context(db, &attempt)?;
        if Some(&context.key) != item.execution_context_key.as_ref()
            || context.key != attempt.context_key
        {
            return Err(bad_state("The task context changed."));
        }
        Ok(Some(context))
    }

    /// The database half of `#queue`: the work item and the `queued` attempt, in one transaction.
    fn enqueue_attempt(
        &self,
        db: &Connection,
        attempt: &TaskAttempt,
    ) -> Result<WorkItem, RpcError> {
        let context = self.context(db, attempt)?;
        let task = self.task(db, &attempt.task_id)?;
        let project = self.projects.active(db, &task.input.project_id)?;
        transaction(db, || {
            let queued = enqueue(
                db,
                &NewWork {
                    conversation_id: Some(
                        project
                            .input
                            .room_id
                            .clone()
                            .unwrap_or_else(|| attempt.bot_id.clone()),
                    ),
                    task_attempt_id: Some(attempt.id.clone()),
                    task_revision: Some(attempt.revision as i64),
                    execution_context_key: Some(context.key.clone()),
                    ..NewWork::new(&attempt.bot_id, WorkSource::Task, &context.instructions)
                },
            )?;
            self.save_attempt(
                db,
                &TaskAttempt {
                    state: AttemptState::Queued,
                    context_key: context.key.clone(),
                    work_item_id: Some(queued.id.clone()),
                    updated_at: now_iso(),
                    ..attempt.clone()
                },
            )?;
            Ok(queued)
        })
    }

    /// `#queue`: queues the attempt's turn and hands it to the runner (with the lock released).
    fn queue(&self, attempt: &TaskAttempt) -> Result<TaskAttempt, RpcError> {
        let item = {
            let db = self.lock();
            self.enqueue_attempt(&db, attempt)?
        };
        self.runner.deliver(item)?;
        self.attempt(&self.lock(), &attempt.id)
    }

    // -- lifecycle ---------------------------------------------------------------------------------------------------

    pub fn start(
        &self,
        id: &str,
        revision: u64,
        request_id: &str,
    ) -> Result<TaskAttempt, RpcError> {
        if self.closing() {
            return Err(bad_state("The task service is closing."));
        }
        let (repo, work, attempt) = {
            let db = self.lock();
            if let Some(repeated) = self.request(&db, request_id, "start", id)? {
                return self.attempt(&db, &repeated);
            }
            let task = self.expected(&db, id, revision)?;
            let project = self.projects.active(&db, &task.input.project_id)?;
            self.assignments(&db, &task.input)?;
            if let Some(current) = &task.current_attempt_id
                && active_attempt(&self.attempt(&db, current)?)
            {
                return Err(RpcError::new(
                    ErrorCode::Busy,
                    "This task already has an active attempt.",
                ));
            }
            let repo = self
                .projects
                .repository(&db, &project.input.repository_id)?;
            let base_sha = self
                .projects
                .worktrees
                .resolve_ref(&repo, &project.input.base_ref)?;
            let attempt_id = new_id();
            let work = self
                .projects
                .worktrees
                .describe(&attempt_id, &base_sha, None)?;
            let timestamp = now_iso();
            let ordinal: i64 = db.query_row(
                "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM task_attempts WHERE task_id = ?1",
                [id],
                |row| row.get(0),
            )?;
            let attempt = TaskAttempt {
                id: attempt_id,
                task_id: id.to_owned(),
                ordinal: ordinal as u64,
                base_sha: base_sha.clone(),
                head_sha: Some(base_sha),
                branch: work.branch.clone(),
                worktree_path: work.path.clone(),
                phase: TaskPhase::Develop,
                state: AttemptState::Provisioning,
                revision: 1,
                bot_id: task.input.assignee_id.clone(),
                work_item_id: None,
                context_key: String::new(),
                reason: None,
                created_at: timestamp.clone(),
                updated_at: timestamp.clone(),
            };
            transaction(&db, || {
                self.lease(&db, &attempt.bot_id, &attempt.id, "develop")?;
                db.execute(
                    "INSERT INTO task_attempts VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        attempt.id,
                        id,
                        ordinal,
                        wire_text(&attempt.state),
                        attempt.bot_id,
                        attempt.worktree_path,
                        attempt.branch,
                        to_json(&attempt)?
                    ],
                )?;
                db.execute(
                    "INSERT INTO task_requests VALUES (?1, ?2, ?3, ?4)",
                    params![request_id, "start", id, attempt.id],
                )?;
                self.save_task(
                    &db,
                    &ProjectTask {
                        current_attempt_id: Some(attempt.id.clone()),
                        state: TaskState::Running,
                        revision: task.revision + 1,
                        updated_at: timestamp.clone(),
                        ..task.clone()
                    },
                )
            })?;
            (repo, work, attempt)
        };
        // The worktree is materialised with the lock released: it can take seconds.
        let queued = self
            .projects
            .worktrees
            .create(&repo, &work, false)
            .and_then(|()| self.queue(&attempt));
        match queued {
            Ok(attempt) => Ok(attempt),
            Err(_) => {
                let db = self.lock();
                self.block(
                    &db,
                    &attempt,
                    "Worktree setup did not finish. Retained work must be reconciled before retrying.",
                    AttemptState::Blocked,
                )?;
                self.attempt(&db, &attempt.id)
            }
        }
    }

    pub fn resume(
        &self,
        id: &str,
        revision: u64,
        request_id: &str,
    ) -> Result<TaskAttempt, RpcError> {
        if self.closing() {
            return Err(bad_state("The task service is closing."));
        }
        let next = {
            let db = self.lock();
            if let Some(repeated) = self.request(&db, request_id, "resume", id)? {
                return self.attempt(&db, &repeated);
            }
            let task = self.expected(&db, id, revision)?;
            self.projects.active(&db, &task.input.project_id)?;
            let Some(current) = task.current_attempt_id.clone() else {
                return Err(bad_state("Start a new task attempt first."));
            };
            let old = self.attempt(&db, &current)?;
            if active_attempt(&old) {
                return Err(RpcError::new(
                    ErrorCode::Busy,
                    "This attempt is still active.",
                ));
            }
            let repo = self.repository(&db, &old)?;
            self.projects
                .worktrees
                .assert_owned(&repo, &self.work(&old)?)?;
            let next = TaskAttempt {
                state: AttemptState::Queued,
                revision: old.revision + 1,
                reason: None,
                updated_at: now_iso(),
                ..old.clone()
            };
            transaction(&db, || {
                self.lease(&db, &old.bot_id, &old.id, "develop")?;
                self.invalidate(&db, &old.id)?;
                self.save_task(
                    &db,
                    &ProjectTask {
                        state: TaskState::Running,
                        revision: task.revision + 1,
                        updated_at: now_iso(),
                        ..task.clone()
                    },
                )?;
                db.execute(
                    "INSERT INTO task_requests VALUES (?1, ?2, ?3, ?4)",
                    params![request_id, "resume", id, old.id],
                )?;
                Ok(())
            })?;
            next
        };
        match self.queue(&next) {
            Ok(attempt) => Ok(attempt),
            Err(_) => {
                let db = self.lock();
                self.block(
                    &db,
                    &next,
                    "This task could not resume. Its files were retained.",
                    AttemptState::Blocked,
                )?;
                self.attempt(&db, &next.id)
            }
        }
    }

    /// The runner's `turn_started` hook.
    pub fn turn_started(&self, db: &Connection, item: &WorkItem) -> Result<(), RpcError> {
        let Some(attempt_id) = &item.task_attempt_id else {
            return Ok(());
        };
        let attempt = self.attempt(db, attempt_id)?;
        if self.matches_active_work(db, &attempt, item)?
            && (attempt.work_item_id.as_deref() == Some(item.id.as_str())
                || item.source == WorkSource::BotReply)
        {
            self.save_attempt(
                db,
                &TaskAttempt {
                    work_item_id: Some(item.id.clone()),
                    state: AttemptState::Running,
                    reason: None,
                    updated_at: now_iso(),
                    ..attempt
                },
            )?;
        }
        Ok(())
    }

    /// The runner's `turn_requeued` hook.
    pub fn turn_requeued(&self, db: &Connection, item: &WorkItem) -> Result<(), RpcError> {
        let Some(attempt_id) = &item.task_attempt_id else {
            return Ok(());
        };
        let attempt = self.attempt(db, attempt_id)?;
        if self.matches_active_work(db, &attempt, item)?
            && attempt.work_item_id.as_deref() == Some(item.id.as_str())
        {
            self.save_attempt(
                db,
                &TaskAttempt {
                    state: AttemptState::Queued,
                    reason: Some("Waiting for the provider/account to become ready.".into()),
                    updated_at: now_iso(),
                    ..attempt
                },
            )?;
        }
        Ok(())
    }

    /// The runner's `turn_ended` hook. The attempt settles on the runtime once the bot's process is confirmed gone.
    pub fn turn_ended(
        self: &Arc<Self>,
        db: &Connection,
        item: &WorkItem,
        result: &TurnResult,
    ) -> Result<(), RpcError> {
        if self.closing() {
            return Ok(());
        }
        let Some(attempt_id) = &item.task_attempt_id else {
            return Ok(());
        };
        let current = self.attempt(db, attempt_id)?;
        if !self.matches_active_work(db, &current, item)?
            || current.work_item_id.as_deref() != Some(item.id.as_str())
        {
            return Ok(());
        }
        let (tasks, item, outcome) = (self.clone(), item.clone(), result.outcome);
        self.handle.spawn(async move {
            if tasks.settle(&item, outcome).await.is_err() {
                tasks.settle_unconfirmed(&item);
            }
        });
        Ok(())
    }

    /// The `settle` closure of `turnEnded`.
    async fn settle(
        self: &Arc<Self>,
        item: &WorkItem,
        outcome: TurnOutcome,
    ) -> Result<(), RpcError> {
        let attempt_id = item
            .task_attempt_id
            .clone()
            .ok_or_else(|| internal("A task turn needs its attempt."))?;
        self.runner.retired(&item.bot_id).await.map_err(internal)?;
        if self.closing() {
            return Ok(());
        }
        let this = self.clone();
        let (item_a, attempt_id_a) = (item.clone(), attempt_id.clone());
        let prepared = blocking(move || -> Result<Option<SettlePlan>, RpcError> {
            let db = this.lock();
            let attempt = this.attempt(&db, &attempt_id_a)?;
            if !this.matches_active_work(&db, &attempt, &item_a)?
                || attempt.work_item_id.as_deref() != Some(item_a.id.as_str())
            {
                return Ok(None);
            }
            if outcome != TurnOutcome::Done {
                let interrupted = outcome == TurnOutcome::Interrupted;
                this.block(
                    &db,
                    &attempt,
                    if interrupted {
                        "Stopped. Resume explicitly to continue."
                    } else {
                        "The developer turn failed. Inspect the retained work."
                    },
                    if interrupted {
                        AttemptState::Interrupted
                    } else {
                        AttemptState::Failed
                    },
                )?;
                return Ok(None);
            }
            if this.pending_replies(&db, &attempt)? {
                this.save_attempt(
                    &db,
                    &TaskAttempt {
                        state: AttemptState::Running,
                        reason: Some("Waiting for requested bot replies.".into()),
                        updated_at: now_iso(),
                        ..attempt
                    },
                )?;
                return Ok(None);
            }
            let located = this
                .repository(&db, &attempt)
                .and_then(|repo| this.work(&attempt).map(|work| (repo, work)));
            Ok(Some((attempt, located)))
        })
        .await??;
        let Some((attempt, located)) = prepared else {
            return Ok(());
        };
        // Git runs with the lock released.
        let worktrees = self.projects.worktrees.clone();
        let inspected = blocking(move || -> Result<(String, bool), RpcError> {
            let (repo, work) = located?;
            let head = worktrees.head(&repo, &work)?;
            let dirty = worktrees.dirty(&repo, &work, Some(&head), false)?;
            Ok((head, dirty))
        })
        .await?;
        let this = self.clone();
        let item_c = item.clone();
        blocking(move || -> Result<(), RpcError> {
            let db = this.lock();
            let current = this.attempt(&db, &attempt.id)?;
            if !this.matches_active_work(&db, &current, &item_c)?
                || current.work_item_id.as_deref() != Some(item_c.id.as_str())
            {
                return Ok(());
            }
            match inspected {
                Err(_) => this.block(
                    &db,
                    &attempt,
                    "The managed worktree changed or is missing. Retained work needs reconciliation.",
                    AttemptState::Blocked,
                ),
                Ok((head, true)) => this.block(
                    &db,
                    &TaskAttempt { head_sha: Some(head), ..attempt },
                    "Commit the retained task changes before review.",
                    AttemptState::Blocked,
                ),
                Ok((head, false)) => {
                    this.save_attempt(
                        &db,
                        &TaskAttempt {
                            head_sha: Some(head),
                            state: AttemptState::Completed,
                            updated_at: now_iso(),
                            ..attempt.clone()
                        },
                    )?;
                    this.release(&db, &attempt.bot_id, &attempt.id)?;
                    let task = this.task(&db, &attempt.task_id)?;
                    this.save_task(
                        &db,
                        &ProjectTask {
                            state: TaskState::Review,
                            revision: task.revision + 1,
                            updated_at: now_iso(),
                            ..task
                        },
                    )
                }
            }
        })
        .await?
    }

    /// The `.catch` of `turnEnded`: the process exit could not be confirmed, so the lease stays.
    fn settle_unconfirmed(&self, item: &WorkItem) {
        if self.closing() {
            return;
        }
        let Some(attempt_id) = &item.task_attempt_id else {
            return;
        };
        let db = self.lock();
        let run = || -> Result<(), RpcError> {
            let attempt = self.attempt(&db, attempt_id)?;
            if !self.matches_active_work(&db, &attempt, item)?
                || attempt.work_item_id.as_deref() != Some(item.id.as_str())
            {
                return Ok(());
            }
            self.save_attempt(
                &db,
                &TaskAttempt {
                    state: AttemptState::Blocked,
                    reason: Some("Process exit is unconfirmed; the bot lease is retained.".into()),
                    updated_at: now_iso(),
                    ..attempt.clone()
                },
            )?;
            let task = self.task(&db, &attempt.task_id)?;
            self.save_task(
                &db,
                &ProjectTask {
                    state: TaskState::Blocked,
                    revision: task.revision + 1,
                    updated_at: now_iso(),
                    ..task
                },
            )
        };
        let _ = run();
    }

    /// `block`: the attempt ends in `state` with `message`, its lease goes, and its task is blocked.
    pub fn block(
        &self,
        db: &Connection,
        attempt: &TaskAttempt,
        message: &str,
        state: AttemptState,
    ) -> Result<(), RpcError> {
        self.save_attempt(
            db,
            &TaskAttempt {
                state,
                reason: Some(message.to_owned()),
                updated_at: now_iso(),
                ..attempt.clone()
            },
        )?;
        self.release(db, &attempt.bot_id, &attempt.id)?;
        let task = self.task(db, &attempt.task_id)?;
        self.save_task(
            db,
            &ProjectTask {
                state: TaskState::Blocked,
                revision: task.revision + 1,
                updated_at: now_iso(),
                ..task
            },
        )
    }

    pub async fn stop(
        self: &Arc<Self>,
        id: &str,
        revision: u64,
        cancel: bool,
    ) -> Result<ProjectTask, RpcError> {
        let (task, attempt) = {
            let db = self.lock();
            let task = self.expected(&db, id, revision)?;
            let Some(current) = task.current_attempt_id.clone() else {
                let next = ProjectTask {
                    state: if cancel {
                        TaskState::Cancelled
                    } else {
                        TaskState::Blocked
                    },
                    revision: task.revision + 1,
                    updated_at: now_iso(),
                    ..task
                };
                self.save_task(&db, &next)?;
                return Ok(next);
            };
            let attempt = self.attempt(&db, &current)?;
            (task, attempt)
        };
        {
            let prefix = format!("{}:", attempt.id);
            for (key, controller) in &self.state().operations {
                if key.starts_with(&prefix) {
                    controller.cancel();
                }
            }
        }
        if let Some(work_id) = &attempt.work_item_id {
            let item = get_work_item(&self.lock(), work_id)?;
            match item.status {
                WorkStatus::Queued => set_status(&self.lock(), &item.id, WorkStatus::Interrupted)?,
                WorkStatus::Running => {
                    self.runner
                        .stop_item(&item.id, "Task stopped; its worktree is retained.")
                        .await
                        .map_err(internal)?;
                    self.runner
                        .retired(&attempt.bot_id)
                        .await
                        .map_err(internal)?;
                }
                _ => {}
            }
        }
        let db = self.lock();
        let current_attempt = self.attempt(&db, &attempt.id)?;
        self.save_attempt(
            &db,
            &TaskAttempt {
                state: if cancel {
                    AttemptState::Cancelled
                } else {
                    AttemptState::Interrupted
                },
                reason: Some(
                    if cancel {
                        "Cancelled; work retained."
                    } else {
                        "Stopped; resume explicitly."
                    }
                    .into(),
                ),
                updated_at: now_iso(),
                ..current_attempt
            },
        )?;
        self.release(&db, &attempt.bot_id, &attempt.id)?;
        let current = self.task(&db, &task.id)?;
        let next = ProjectTask {
            state: if cancel {
                TaskState::Cancelled
            } else {
                TaskState::Blocked
            },
            revision: current.revision + 1,
            updated_at: now_iso(),
            ..current
        };
        self.save_task(&db, &next)?;
        Ok(next)
    }

    /// `invalidate`: every evaluation of the attempt that isn't stale already becomes stale.
    pub fn invalidate(&self, db: &Connection, attempt_id: &str) -> Result<(), RpcError> {
        let rows = data_list::<TaskEvaluation>(
            db,
            "SELECT data FROM task_evaluations WHERE attempt_id = ?1 AND state <> 'stale'",
            [attempt_id],
        )?;
        for evaluation in rows.into_iter().map(normalise) {
            self.save_evaluation(
                db,
                &TaskEvaluation {
                    state: EvaluationState::Stale,
                    reason: Some("The task source or execution revision changed.".into()),
                    updated_at: now_iso(),
                    ..evaluation
                },
            )?;
        }
        Ok(())
    }

    /// `refresh`: re-reads the worktree; changed source blocks the attempt and invalidates its evaluations.
    pub fn refresh(&self, db: &Connection, id: &str) -> Result<TaskAttempt, RpcError> {
        let attempt = self.attempt(db, id)?;
        if active_attempt(&attempt)
            || matches!(
                attempt.state,
                AttemptState::Cancelled | AttemptState::Cleaned
            )
        {
            return Ok(attempt);
        }
        const CHANGED: &str = "Source changed; commit and rerun review and checks.";
        const MISSING: &str = "Managed worktree is missing or changed.";
        let inspect = || -> Result<(), RpcError> {
            let repo = self.repository(db, &attempt)?;
            let work = self.work(&attempt)?;
            let head = self.projects.worktrees.head(&repo, &work)?;
            let dirty = self
                .projects
                .worktrees
                .dirty(&repo, &work, Some(&head), false)?;
            if Some(&head) != attempt.head_sha.as_ref()
                || dirty && attempt.reason.as_deref() != Some(CHANGED)
            {
                self.invalidate(db, id)?;
                self.block(
                    db,
                    &TaskAttempt {
                        head_sha: Some(head),
                        revision: attempt.revision + 1,
                        ..attempt.clone()
                    },
                    CHANGED,
                    AttemptState::Blocked,
                )?;
            }
            Ok(())
        };
        if inspect().is_err() && attempt.reason.as_deref() != Some(MISSING) {
            self.block(db, &attempt, MISSING, AttemptState::Blocked)?;
        }
        self.attempt(db, id)
    }

    /// `exact`: the attempt, if its current completed commit is exactly `sha` at `revision`.
    pub fn exact(
        &self,
        db: &Connection,
        id: &str,
        revision: u64,
        sha: &str,
    ) -> Result<TaskAttempt, RpcError> {
        let attempt = self.refresh(db, id)?;
        let task = self.task(db, &attempt.task_id)?;
        self.projects.active(db, &task.input.project_id)?;
        if task.current_attempt_id.as_deref() != Some(id)
            || attempt.revision != revision
            || attempt.head_sha.as_deref() != Some(sha)
            || attempt.state != AttemptState::Completed
        {
            return Err(bad_state(
                "The exact task commit is changed, dirty or still running.",
            ));
        }
        Ok(attempt)
    }

    pub fn diff(&self, db: &Connection, id: &str) -> Result<TaskDiff, RpcError> {
        let attempt = self.refresh(db, id)?;
        let repo = self.repository(db, &attempt)?;
        let work = self.work(&attempt)?;
        let sha = self.projects.worktrees.head(&repo, &work)?;
        let diff = self
            .projects
            .worktrees
            .diff(&repo, &attempt.base_sha, &sha)?;
        Ok(TaskDiff {
            attempt_id: id.to_owned(),
            revision: attempt.revision,
            source_sha: sha.clone(),
            base_sha: attempt.base_sha.clone(),
            text: diff.text,
            truncated: diff.truncated,
            dirty: self
                .projects
                .worktrees
                .dirty(&repo, &work, Some(&sha), false)?,
        })
    }

    pub fn cleanup_preview(
        &self,
        db: &Connection,
        id: &str,
    ) -> Result<TaskCleanupPreview, RpcError> {
        let attempt = self.attempt(db, id)?;
        let blocked = active_attempt(&attempt)
            || self.bot_busy(db, &attempt.bot_id)?
            || exists(
                db,
                "SELECT 1 FROM task_evaluations WHERE attempt_id = ?1 AND state = 'running'",
                [id],
            )?
            || exists(
                db,
                "SELECT 1 FROM task_integrations WHERE attempt_id = ?1 AND state IN ('preparing', 'checking', 'ready')",
                [id],
            )?;
        if blocked {
            return Ok(TaskCleanupPreview {
                attempt_id: id.to_owned(),
                worktree_path: attempt.worktree_path,
                head_sha: attempt.head_sha,
                files: Vec::new(),
                truncated: false,
                can_remove: false,
                reason: "Stop active work and confirm its process exit before cleanup.".into(),
            });
        }
        let inspect = || -> Result<TaskCleanupPreview, RpcError> {
            let repo = self.repository(db, &attempt)?;
            let work = self.work(&attempt)?;
            let head = self.projects.worktrees.head(&repo, &work)?;
            let files = self.projects.worktrees.changed_files(&repo, &work, &head)?;
            Ok(TaskCleanupPreview {
                attempt_id: id.to_owned(),
                worktree_path: work.path,
                head_sha: Some(head),
                truncated: files.len() > 200,
                can_remove: files.is_empty(),
                reason: if files.is_empty() {
                    "This clean worktree can be removed. Its branch and task history remain."
                } else {
                    "Modified and untracked files are retained. Save them before removing this worktree."
                }
                .into(),
                files: files.into_iter().take(200).collect(),
            })
        };
        Ok(inspect().unwrap_or_else(|_| TaskCleanupPreview {
            attempt_id: id.to_owned(),
            worktree_path: attempt.worktree_path.clone(),
            head_sha: None,
            files: Vec::new(),
            truncated: false,
            can_remove: false,
            reason: "The worktree is missing or its ownership cannot be verified. Nothing will be removed.".into(),
        }))
    }

    pub fn cleanup(
        &self,
        db: &Connection,
        id: &str,
        expected_head_sha: &str,
    ) -> Result<TaskAttempt, RpcError> {
        let preview = self.cleanup_preview(db, id)?;
        let attempt = self.attempt(db, id)?;
        if !preview.can_remove || preview.head_sha.as_deref() != Some(expected_head_sha) {
            return Err(bad_state(
                "The cleanup preview is stale or contains retained work.",
            ));
        }
        self.projects.worktrees.remove(
            &self.repository(db, &attempt)?,
            &self.work(&attempt)?,
            expected_head_sha,
        )?;
        let cleaned = TaskAttempt {
            state: AttemptState::Cleaned,
            reason: Some(
                "Worktree removed explicitly. The branch and history are retained.".into(),
            ),
            updated_at: now_iso(),
            ..attempt
        };
        self.save_attempt(db, &cleaned)?;
        Ok(cleaned)
    }

    /// `accepted`: the attempt's exact commit has a passing review (with full coverage) and passing checks, both at
    /// the project's current revision.
    pub fn accepted(&self, db: &Connection, attempt: &TaskAttempt) -> Result<bool, RpcError> {
        let task = self.task(db, &attempt.task_id)?;
        let project = self.projects.get(db, &task.input.project_id)?;
        let Some(head) = attempt.head_sha.as_deref() else {
            return Ok(false);
        };
        if task.input.reviewer_id.is_none()
            || task.input.tester_id.is_none()
            || project.input.checks.is_empty()
        {
            return Ok(false);
        }
        for kind in [EvaluationKind::Review, EvaluationKind::Test] {
            let rows = data_list::<TaskEvaluation>(
                db,
                "SELECT data FROM task_evaluations WHERE attempt_id = ?1 AND revision = ?2 AND source_sha = ?3 AND kind = ?4 ORDER BY rowid DESC LIMIT 1",
                params![attempt.id, attempt.revision as i64, head, wire_text(&kind)],
            )?;
            let Some(evaluation) = rows.into_iter().next().map(normalise) else {
                return Ok(false);
            };
            if evaluation.state != EvaluationState::Passed
                || evaluation.project_revision != project.revision
            {
                return Ok(false);
            }
            if kind == EvaluationKind::Review {
                let covered = evaluation
                    .review_coverage
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|coverage| coverage.source_sha == head);
                if !covered {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// `recover`: after a restart, nothing that was running is still running.
    pub fn recover(&self, db: &Connection) -> Result<(), RpcError> {
        let attempts = data_list::<TaskAttempt>(
            db,
            "SELECT data FROM task_attempts WHERE state IN ('provisioning', 'queued', 'running')",
            [],
        )?;
        for attempt in attempts {
            // A queued item still waiting for the runner keeps its attempt when its context still matches; any
            // mismatch is persisted below without touching the worktree.
            if attempt.state == AttemptState::Queued
                && let Some(work_id) = &attempt.work_item_id
                && let Ok(item) = get_work_item(db, work_id)
                && item.status == WorkStatus::Queued
                && get_stored_bot(db, &attempt.bot_id)
                    .and_then(|bot| self.execution_context(db, &bot, &item))
                    .is_ok()
            {
                continue;
            }
            let reason = if std::path::Path::new(&attempt.worktree_path).exists() {
                "Daemon restarted. Confirm the old process has exited before resuming retained work."
            } else {
                "Managed worktree is missing or changed."
            };
            self.save_attempt(
                db,
                &TaskAttempt {
                    state: AttemptState::Interrupted,
                    reason: Some(reason.into()),
                    updated_at: now_iso(),
                    ..attempt.clone()
                },
            )?;
            let task = self.task(db, &attempt.task_id)?;
            self.save_task(
                db,
                &ProjectTask {
                    state: TaskState::Blocked,
                    revision: task.revision + 1,
                    updated_at: now_iso(),
                    ..task
                },
            )?;
        }
        let running = data_list::<TaskEvaluation>(
            db,
            "SELECT data FROM task_evaluations WHERE state = 'running'",
            [],
        )?;
        for evaluation in running.into_iter().map(normalise) {
            // A persisted PID alone is unsafe to kill after a restart; the lease stays until the host proves exit.
            self.save_evaluation(
                db,
                &TaskEvaluation {
                    state: EvaluationState::Unavailable,
                    reason: Some("Daemon restarted during evaluation; rerun explicitly.".into()),
                    updated_at: now_iso(),
                    ..evaluation
                },
            )?;
        }
        Ok(())
    }

    /// `confirmedExited`: the host proved the process is gone, so the lease goes. Call with the lock released.
    pub fn confirmed_exited(&self, bot_id: &str, owner_id: &str) -> Result<(), RpcError> {
        if self.runner.current_work(bot_id).is_some() {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "This bot still has a live turn.",
            ));
        }
        self.release(&self.lock(), bot_id, owner_id)
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        {
            let mut state = self.state();
            state.closing = true;
            for controller in state.operations.values() {
                controller.cancel();
            }
        }
        let mut idle = self.settling.subscribe();
        let _ = idle.wait_for(|count| *count == 0).await;
        if self.state().termination_unknown {
            return Err("An owned check process could not be confirmed stopped; retain its task state for reconciliation.".into());
        }
        Ok(())
    }

    // -- evaluation --------------------------------------------------------------------------------------------------

    /// `evaluate`: a review or the project checks over the attempt's exact commit.
    pub async fn evaluate(
        self: &Arc<Self>,
        id: &str,
        revision: u64,
        sha: &str,
        kind: EvaluationKind,
    ) -> Result<TaskEvaluation, RpcError> {
        if self.closing() {
            return Err(bad_state("The task service is closing."));
        }
        let _settling = Settling::new(self);
        let this = self.clone();
        let (id, sha) = (id.to_owned(), sha.to_owned());
        let begin = blocking(move || this.begin_evaluation(&id, revision, &sha, kind)).await??;
        let (run, mut evaluation) = match begin {
            Begin::Done(evaluation) => return Ok(*evaluation),
            Begin::Run(parts) => *parts,
        };
        let mut termination_unknown = false;
        let outcome = self.run_evaluation(&run, &mut evaluation, revision).await;
        if let Err(error) = outcome {
            termination_unknown = matches!(error, CheckError::Termination(_));
            if termination_unknown {
                self.state().termination_unknown = true;
            }
            let review = evaluation.kind == EvaluationKind::Review;
            evaluation = TaskEvaluation {
                review_coverage: Some(None),
                state: if review { EvaluationState::Unavailable } else { EvaluationState::Failed },
                reason: Some(
                    if termination_unknown {
                        "Evaluation process termination is unconfirmed; its bot lease is retained for host reconciliation."
                    } else if review {
                        "review_isolation_unavailable or incomplete snapshot coverage"
                    } else {
                        "The configured check did not complete. Inspect retained check work."
                    }
                    .into(),
                ),
                ..evaluation
            };
        }
        self.state().operations.remove(&run.key);
        let this = self.clone();
        let (attempt_id, bot_id) = (run.attempt.id.clone(), run.bot.id.clone());
        let finished = blocking(move || -> Result<TaskEvaluation, RpcError> {
            let db = this.lock();
            if !termination_unknown {
                this.release(&db, &bot_id, &evaluation.id)?;
            }
            evaluation.updated_at = now_iso();
            this.save_evaluation(&db, &evaluation)?;
            let current = this.task(&db, &run.task.id)?;
            if current.current_attempt_id.as_deref() == Some(attempt_id.as_str())
                && current.state != TaskState::Cancelled
                && this.attempt(&db, &attempt_id)?.revision == revision
            {
                let next_state = match evaluation.state {
                    EvaluationState::Findings | EvaluationState::Failed => Some(TaskState::Blocked),
                    EvaluationState::Passed if this.accepted(&db, &run.attempt)? => {
                        Some(TaskState::Done)
                    }
                    _ => None,
                };
                if let Some(state) = next_state {
                    this.save_task(
                        &db,
                        &ProjectTask {
                            state,
                            revision: current.revision + 1,
                            updated_at: now_iso(),
                            ..current
                        },
                    )?;
                }
            }
            Ok(evaluation)
        })
        .await??;
        Ok(finished)
    }

    /// The first section of `#evaluate`, with the lock held: validation, the lease and the `running` record.
    fn begin_evaluation(
        &self,
        id: &str,
        revision: u64,
        sha: &str,
        kind: EvaluationKind,
    ) -> Result<Begin, RpcError> {
        let db = self.lock();
        let attempt = self.exact(&db, id, revision, sha)?;
        let task = self.task(&db, &attempt.task_id)?;
        let project = self.projects.active(&db, &task.input.project_id)?;
        let bot_id = match kind {
            EvaluationKind::Review => task.input.reviewer_id.clone(),
            EvaluationKind::Test => task.input.tester_id.clone(),
        };
        let timestamp = now_iso();
        let mut evaluation = TaskEvaluation {
            id: new_id(),
            attempt_id: id.to_owned(),
            revision,
            source_sha: sha.to_owned(),
            project_revision: project.revision,
            kind,
            bot_id: bot_id.clone(),
            state: EvaluationState::Running,
            worktree_path: None,
            review_coverage: Some(None),
            reason: None,
            findings: Vec::new(),
            checks: Vec::new(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        let unavailable =
            |reason: &str, evaluation: &mut TaskEvaluation| -> Result<Begin, RpcError> {
                evaluation.state = EvaluationState::Unavailable;
                evaluation.reason = Some(reason.to_owned());
                self.save_evaluation(&db, evaluation)?;
                Ok(Begin::Done(Box::new(evaluation.clone())))
            };
        let role = if kind == EvaluationKind::Review {
            "reviewer"
        } else {
            "tester"
        };
        let Some(bot_id) = bot_id else {
            return unavailable(&format!("Assign a {role} first."), &mut evaluation);
        };
        if kind == EvaluationKind::Review && self.options.review.is_none() {
            return unavailable("review_isolation_unavailable", &mut evaluation);
        }
        if kind == EvaluationKind::Test && project.input.checks.is_empty() {
            return unavailable(
                "Configure and accept project checks first.",
                &mut evaluation,
            );
        }
        let bot = get_stored_bot(&db, &bot_id)?;
        let checks = self.options.checks.clone().or_else(|| {
            (bot.autonomy == Autonomy::Full)
                .then(|| self.options.host_checks.clone())
                .flatten()
        });
        if kind == EvaluationKind::Test && checks.is_none() {
            return unavailable(
                "check_isolation_unavailable: configured host checks require Full auto or an approved isolated check runtime.",
                &mut evaluation,
            );
        }
        let controller = CancellationToken::new();
        let key = format!("{id}:{}", evaluation.id);
        transaction(&db, || {
            self.lease(&db, &bot_id, &evaluation.id, &wire_text(&kind))?;
            self.save_evaluation(&db, &evaluation)
        })?;
        self.state()
            .operations
            .insert(key.clone(), controller.clone());
        Ok(Begin::Run(Box::new((
            EvaluationRun {
                attempt,
                task,
                project,
                bot,
                checks,
                controller,
                key,
            },
            evaluation,
        ))))
    }

    /// The `try` body of `#evaluate`. It mutates `evaluation` as it goes, so a failure keeps what was gathered.
    async fn run_evaluation(
        self: &Arc<Self>,
        run: &EvaluationRun,
        evaluation: &mut TaskEvaluation,
        revision: u64,
    ) -> Result<(), CheckError> {
        let this = self.clone();
        let attempt = run.attempt.clone();
        let repo = blocking(move || this.repository(&this.lock(), &attempt))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        let sha = evaluation.source_sha.clone();
        let worktrees = self.projects.worktrees.clone();
        if evaluation.kind == EvaluationKind::Review {
            let (repo_s, base, sha_s) = (repo.clone(), run.attempt.base_sha.clone(), sha.clone());
            let snapshot = blocking(move || review_snapshot(&worktrees, &repo_s, &base, &sha_s))
                .await
                .map_err(failed)?
                .map_err(CheckError::Failed)?;
            let role = run
                .project
                .input
                .members
                .iter()
                .find(|member| {
                    member.bot_id == run.bot.id && member.roles.contains(&ProjectRole::Reviewer)
                })
                .map(|member| member.instructions.clone())
                .unwrap_or_default();
            let context = frozen_review_context(ReviewContext {
                bot_id: run.bot.id.clone(),
                project_id: run.project.id.clone(),
                task_id: run.task.id.clone(),
                attempt_id: run.attempt.id.clone(),
                evaluation_id: evaluation.id.clone(),
                revision,
                project_revision: run.project.revision,
                task_revision: run.task.revision,
                instructions: ReviewInstructions {
                    reviewer: run.bot.instructions.clone(),
                    role,
                    title: run.task.input.title.clone(),
                    task: run.task.input.instructions.clone(),
                    acceptance: run.task.input.acceptance.clone(),
                },
            })
            .map_err(CheckError::Failed)?;
            let reviewer = self
                .options
                .review
                .clone()
                .ok_or_else(|| CheckError::Failed("review_isolation_unavailable".into()))?;
            let result = reviewer
                .review(&snapshot, &run.controller, &context)
                .await?;
            evaluation.state = result.state;
            evaluation.reason = result.reason;
            evaluation.findings = result.findings;
            evaluation.review_coverage = Some(result.coverage);
            if matches!(
                evaluation.state,
                EvaluationState::Passed | EvaluationState::Findings
            ) {
                let expected = review_coverage(&snapshot).map_err(CheckError::Failed)?;
                if evaluation.review_coverage.clone().flatten() != Some(expected) {
                    return Err(CheckError::Failed("review_coverage_incomplete".into()));
                }
            }
            if evaluation.state == EvaluationState::Passed && !evaluation.findings.is_empty() {
                evaluation.state = EvaluationState::Findings;
            }
            if !matches!(
                evaluation.state,
                EvaluationState::Passed
                    | EvaluationState::Findings
                    | EvaluationState::Failed
                    | EvaluationState::Unavailable
            ) {
                return Err(CheckError::Failed("Invalid review result.".into()));
            }
            let this = self.clone();
            let (task_id, project_id, bot_id) = (
                run.task.id.clone(),
                run.project.id.clone(),
                run.bot.id.clone(),
            );
            let (task_revision, project_revision, instructions) = (
                run.task.revision,
                run.project.revision,
                run.bot.instructions.clone(),
            );
            let changed = blocking(move || -> Result<bool, RpcError> {
                let db = this.lock();
                Ok(this.task(&db, &task_id)?.revision != task_revision
                    || this.projects.get(&db, &project_id)?.revision != project_revision
                    || get_stored_bot(&db, &bot_id)?.instructions != instructions)
            })
            .await
            .map_err(failed)?
            .map_err(failed)?;
            if changed {
                evaluation.state = EvaluationState::Stale;
                evaluation.reason = Some("The review criteria changed during evaluation.".into());
            }
        } else {
            let work = worktrees
                .describe(&format!("test-{}", evaluation.id), &sha, None)
                .map_err(failed)?;
            let (created, repo_c, work_c) = (worktrees.clone(), repo.clone(), work.clone());
            blocking(move || created.create(&repo_c, &work_c, false))
                .await
                .map_err(failed)?
                .map_err(failed)?;
            evaluation.worktree_path = Some(work.path.clone());
            let checks = run
                .checks
                .clone()
                .ok_or_else(|| CheckError::Failed("check_isolation_unavailable".into()))?;
            for check in &run.project.input.checks {
                let result = checks.run(check, &work.path, &run.controller).await?;
                let stop = result.exit_code != Some(0);
                evaluation.checks.push(result);
                if stop {
                    break;
                }
            }
            let passed = evaluation.checks.len() == run.project.input.checks.len()
                && evaluation
                    .checks
                    .iter()
                    .all(|result| result.exit_code == Some(0));
            evaluation.state = if passed {
                EvaluationState::Passed
            } else {
                EvaluationState::Failed
            };
            let (repo_d, work_d, sha_d) = (repo.clone(), work.clone(), sha.clone());
            let dirty = blocking(move || worktrees.dirty(&repo_d, &work_d, Some(&sha_d), true))
                .await
                .map_err(failed)?
                .map_err(failed)?;
            if dirty {
                evaluation.state = EvaluationState::Failed;
                evaluation.reason = Some(
                    "Checks changed the isolated source. Preserve and inspect the check worktree."
                        .into(),
                );
            }
        }
        let this = self.clone();
        let attempt_id = run.attempt.id.clone();
        let current = blocking(move || this.refresh(&this.lock(), &attempt_id))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        if run.controller.is_cancelled()
            || current.revision != revision
            || current.head_sha.as_deref() != Some(sha.as_str())
        {
            evaluation.state = EvaluationState::Stale;
            evaluation.reason =
                Some("The attempt changed or was stopped during evaluation.".into());
        }
        Ok(())
    }
}
