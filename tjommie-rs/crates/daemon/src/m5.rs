//! M5 (projects and tasks) wiring: `daemon.ts:167` (projects), `:243-290` (the runner's task hooks, the review runtime,
//! tasks and integrations), `:384` (the handlers), `:636-637` (recovery) and the shutdown order of `:290`.
//!
//! The runner is built before the tasks service, and the service needs the runner, so the runner's hooks reach it
//! through a [`LateTasks`] cell that [`install`] fills.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use protocol::ErrorCode;
use rusqlite::Connection;

use crate::account_runtime::AccountRuntime;
use crate::bots::StoredBot;
use crate::cli::CliHost;
use crate::context::Db;
use crate::project_checks::HostCheckRuntime;
use crate::project_handlers::{ProjectServices, register};
use crate::project_review_account::review_account_resolver;
use crate::project_review_runtime::{ProjectReviewOptions, create_project_review_runtime};
use crate::projects::{AssertAvailable, Projects, SqlRooms};
use crate::queue::WorkItem;
use crate::runner::{Core, ItemHook, Runner, RunnerHooks, TurnResult};
use crate::server::{Broadcaster, Handlers, RpcError};
use crate::task_integration::TaskIntegrations;
use crate::tasks::{Tasks, TasksOptions};
use crate::win::boundary::WorkerBoundary;
use crate::worktrees::Worktrees;

/// The tasks service, set once [`install`] has built it. Until then the runner's task hooks do nothing, which is right:
/// no work can run before the daemon finishes starting.
#[derive(Clone, Default)]
pub struct LateTasks(Arc<OnceLock<Arc<Tasks>>>);

impl LateTasks {
    /// `base` with the task hooks put in front of it: `executionContext`, and the task part of `turnStarted`,
    /// `turnRequeued` and `turnEnded` (`daemon.ts:244`, `:258-266`, `:293-302`). `base`'s hooks (the scheduler's, the
    /// team's, the turn notice) run after the task's, as in TypeScript.
    pub fn hooks(&self, base: RunnerHooks) -> RunnerHooks {
        let cell = self.0.clone();
        let execution_context = Arc::new(
            move |core: &mut Core<'_>, bot: &StoredBot, item: &WorkItem| match cell.get() {
                Some(tasks) => tasks
                    .execution_context(core.db(), bot, item)
                    .map_err(|error| error.message),
                None => Ok(None),
            },
        );
        let ended = self.0.clone();
        let next_ended = base.turn_ended.clone();
        RunnerHooks {
            execution_context: Some(execution_context),
            turn_started: Some(item_hook(self, base.turn_started.clone(), Tasks::turn_started)),
            turn_requeued: Some(item_hook(self, base.turn_requeued.clone(), Tasks::turn_requeued)),
            // The `finally`: whatever follows (the team's hook and the notice) runs even if the task's hook failed,
            // and the task's own error is the one reported.
            turn_ended: Some(Arc::new(
                move |core: &mut Core<'_>, item: &WorkItem, result: &TurnResult| {
                    let tasks_result = match ended.get() {
                        Some(tasks) => tasks
                            .turn_ended(core.db(), item, result)
                            .map_err(|error| error.message),
                        None => Ok(()),
                    };
                    let rest = match &next_ended {
                        Some(next) => next(core, item, result),
                        None => Ok(()),
                    };
                    tasks_result.and(rest)
                },
            )),
            ..base
        }
    }

    fn get(&self) -> Option<&Arc<Tasks>> {
        self.0.get()
    }
}

/// The task part of an item hook, then `next`. A task error stops the chain, as a throw does in TypeScript.
fn item_hook(
    late: &LateTasks,
    next: Option<ItemHook>,
    run: fn(&Tasks, &Connection, &WorkItem) -> Result<(), RpcError>,
) -> ItemHook {
    let late = late.clone();
    Arc::new(move |core: &mut Core<'_>, item: &WorkItem| {
        if let Some(tasks) = late.get() {
            run(tasks, core.db(), item).map_err(|error| error.message)?;
        }
        match &next {
            Some(next) => next(core, item),
            None => Ok(()),
        }
    })
}

pub struct M5Deps {
    pub db: Db,
    /// The data root: `workspaces` (managed worktrees) and `data/reviewer` (review receipts) live under it.
    pub root: PathBuf,
    pub events: Broadcaster,
    /// `switchingBots`, shared with the runner's `is_held`: a bot whose provider is changing can't join a project or
    /// lease a task.
    pub switching: Arc<Mutex<HashSet<String>>>,
    /// `deletionRequests`, shared with the bot handlers: a bot being deleted can't join a project either.
    pub deleting: Arc<Mutex<HashSet<String>>>,
    pub runner: Runner,
    pub accounts: AccountRuntime,
    /// How checks and the review CLI launch when no worker is required.
    pub host: Arc<dyn CliHost>,
    /// The installed review worker (`installedReviewWorker`); the runtime uses it only when it is required.
    pub review_worker: Option<Arc<WorkerBoundary>>,
    /// The cell the runner's hooks read; it must be the one `LateTasks::hooks` was called on.
    pub late: LateTasks,
}

/// What shutdown and the bot lifecycle still need after [`install`].
pub struct M5 {
    pub projects: Arc<Projects>,
    pub tasks: Arc<Tasks>,
    pub integrations: Arc<TaskIntegrations>,
}

/// `daemon.ts:167`: `switchingBots.has(botId) || deletionRequests.has(botId)` is "busy", for the projects to refuse a bot
/// whose settings or lifecycle is changing.
pub fn bot_available(
    switching: Arc<Mutex<HashSet<String>>>,
    deleting: Arc<Mutex<HashSet<String>>>,
) -> AssertAvailable {
    Arc::new(move |bot_id| {
        let changing = |set: &Mutex<HashSet<String>>| {
            set.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(bot_id)
        };
        if changing(&switching) || changing(&deleting) {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "This bot's settings or lifecycle is changing. Try again shortly.",
            ));
        }
        Ok(())
    })
}

/// Builds the project services, binds the runner's task hooks and registers the 22 `projects.*` and `tasks.*` methods.
/// Call it inside the runtime (settling and integration work run on it), after the runner exists.
pub fn install(handlers: &mut Handlers, deps: M5Deps) -> Result<M5, RpcError> {
    let events = deps.events.clone();
    let (switching, deleting) = (deps.switching.clone(), deps.deleting.clone());
    let projects = Arc::new(
        Projects::new(
            deps.db.clone(),
            Arc::new(Worktrees::new(&deps.root.join("workspaces"))?),
            Arc::new(SqlRooms),
        )
        .with_emit(Arc::new(move |event| events.send(event)))
        .with_assert_available(bot_available(switching, deleting)),
    );
    let review = create_project_review_runtime(ProjectReviewOptions {
        root: deps.root.join("data").join("reviewer"),
        resolve: review_account_resolver(deps.db.clone(), deps.accounts, deps.events),
        host: deps.host.clone(),
        worker: deps.review_worker,
        cli: None,
        machine_policy_hash: None,
    })?;
    let tasks = Tasks::new(
        projects.clone(),
        Arc::new(deps.runner),
        TasksOptions {
            review: Some(Arc::new(review)),
            checks: None,
            // `new HostCheckRuntime()`: a Full-auto tester's checks launch the way every CLI does.
            host_checks: Some(Arc::new(HostCheckRuntime::new(deps.host))),
        },
    );
    deps.late
        .0
        .set(tasks.clone())
        .map_err(|_| RpcError::new(ErrorCode::Internal, "The task service was already installed."))?;
    let integrations = TaskIntegrations::new(tasks.clone(), None, None);
    *handlers = register(
        std::mem::take(handlers),
        &ProjectServices {
            projects: projects.clone(),
            tasks: tasks.clone(),
            integrations: integrations.clone(),
        },
    );
    Ok(M5 {
        projects,
        tasks,
        integrations,
    })
}

impl M5 {
    /// `tasks.recover(); integrations.recover()` (`daemon.ts:636-637`): what a restart cut off is settled before any
    /// request is served. After `runner.recover()`.
    pub fn recover(&self, db: &Connection) -> Result<(), RpcError> {
        self.tasks.recover(db)?;
        self.integrations.recover(db)
    }

    /// `integrations.shutdown()` then `tasks.shutdown()` (`daemon.ts:290`), both before the runner stops.
    pub async fn shutdown(&self) {
        for stopped in [self.integrations.shutdown().await, self.tasks.shutdown().await] {
            if let Err(error) = stopped {
                eprintln!("{error}");
            }
        }
    }
}
