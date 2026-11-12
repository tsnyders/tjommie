//! M4 collaboration in one place, like `m6.rs`: the team (rooms, exchanges, the five tools), the MCP endpoint, routines,
//! the scheduler and the notification handlers. This is the part of `daemon.ts` (`:243-302`, `:568-618`, `:648`) that wires
//! them together.
//!
//! **Order.** The runner needs M4's hooks when it is built, and M4's tools and wake-ups need the runner, so M4 is made
//! first ([`M4::new`]), the runner is built with the hook bodies below, and then [`M4::attach`] gives M4 the runner and
//! [`M4::install`] registers the handlers and the `/mcp` route. [`M4::start`] comes with the daemon's other starts.
//!
//! **Hooks.** The runner calls these with its locks held. The daemon's own `turnEnded` also runs the task hooks first and
//! the turn notice after, which belong to other tasks, so Core's hooks call these methods inside their own closures:
//!
//! | `RunnerHooks` field | call |
//! |---|---|
//! | `mcp_servers` | [`M4::mcp_server`] for the `tjommie` entry |
//! | `session_closed` | [`M4::session_closed`] |
//! | `turn_started` | [`M4::turn_started`] |
//! | `turn_requeued` | [`M4::turn_requeued`] |
//! | `turn_ended` | [`M4::turn_ended`] (after the task hook, before the notice) |
//! | `retry_routine` | [`M4::retry_routine`] |

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::FutureExt;
use protocol::ErrorCode;
use protocol::model::Message;

use crate::context::{Context, Db};
use crate::mcp::{McpTokens, McpTool, mcp_route};
use crate::providers::McpServer;
use crate::queue::WorkItem;
use crate::rooms;
use crate::routines::{self, TurnEnded, now_ms};
use crate::runner::{Broadcast, Core, Log, Runner, TurnResult};
use crate::scheduler::{self, Scheduler, SchedulerDeps, TICK_MS};
use crate::server::{Handlers, Routes, RpcError};
use crate::team::{self, HostAccess, SharedAttachments, Team, TeamDeps};

pub struct M4Deps {
    pub db: Db,
    pub broadcast: Broadcast,
    pub log: Log,
    /// The protected `shared-files` folder.
    pub shared_files: PathBuf,
    /// The team's shared-file links. `None` turns file sharing with companion images off.
    pub attachments: Option<Arc<dyn SharedAttachments>>,
    /// `cleanupSharedFiles()`: runs after a room is deleted.
    pub cleanup_shared: Arc<dyn Fn() + Send + Sync>,
    /// The daemon's composed `turnEnded` (task hooks, M4's [`M4::turn_ended`], the turn notice). `routines.delete` ends
    /// the work its routine had queued through it.
    pub turn_ended: TurnEnded,
}

pub struct M4 {
    /// One bearer token per running CLI session, shared with the `/mcp` route (and `m6`'s `/mcp/browser`).
    pub tokens: McpTokens,
    pub team: Arc<Team>,
    pub scheduler: Scheduler,
    runner: Arc<OnceLock<Runner>>,
    cleanup_shared: Arc<dyn Fn() + Send + Sync>,
    turn_ended: TurnEnded,
}

impl M4 {
    pub fn new(deps: M4Deps) -> M4 {
        let runner: Arc<OnceLock<Runner>> = Arc::new(OnceLock::new());
        let team = Arc::new(Team::new(TeamDeps {
            broadcast: deps.broadcast.clone(),
            log: deps.log.clone(),
            shared_files: deps.shared_files,
            attachments: deps.attachments,
        }));
        let (delivering, stopping) = (runner.clone(), runner.clone());
        let scheduler = Scheduler::new(SchedulerDeps {
            db: deps.db,
            broadcast: deps.broadcast,
            deliver: Arc::new(move |item: &WorkItem| {
                let runner = delivering.get().ok_or("The runner is not ready")?;
                runner
                    .deliver(item.clone())
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }),
            stop_item: Arc::new(move |item_id: &str, notice: &str| {
                match stopping.get() {
                    Some(runner) => {
                        let stopped = runner.stop_item(item_id, notice);
                        async move { stopped.await.map_err(|error| error.to_string()) }.boxed()
                    }
                    None => async { Err("The runner is not ready".to_owned()) }.boxed(),
                }
            }),
            log: deps.log,
            clock: Arc::new(now_ms),
            tick_every: Duration::from_millis(TICK_MS),
        });
        M4 {
            tokens: McpTokens::default(),
            team,
            scheduler,
            runner,
            cleanup_shared: deps.cleanup_shared,
            turn_ended: deps.turn_ended,
        }
    }

    /// Gives M4 the runner it was built before. Call once, before [`M4::install`] and [`M4::start`].
    pub fn attach(&self, runner: Runner) {
        let _ = self.runner.set(runner);
    }

    fn runner(&self) -> Result<Runner, RpcError> {
        self.runner
            .get()
            .cloned()
            .ok_or_else(|| RpcError::new(ErrorCode::Internal, "M4 has no runner: call attach first"))
    }

    /// Registers the handlers (`rooms.*`, `room.*`, `exchanges.get`, `routines.*`, `notifications.*`) and mounts `/mcp`
    /// with the team's tools followed by `extra_tools` (the Google tools, when there is a Google login).
    pub fn install(
        &self,
        handlers: &mut Handlers,
        routes: &mut Routes,
        context: &Context,
        extra_tools: Vec<McpTool>,
    ) -> Result<(), RpcError> {
        let access: Arc<dyn HostAccess> = Arc::new(self.runner()?);
        let table = std::mem::take(handlers);
        let table = rooms::register(table, context, self.cleanup_shared.clone());
        let table = routines::register(table, context, self.turn_ended.clone());
        let table = scheduler::register(table, context);
        *handlers = team::register(table, context, self.team.clone(), access.clone());
        let mut tools = self.team.tools(access);
        tools.extend(extra_tools);
        *routes = std::mem::take(routes).route("/mcp", mcp_route(self.tokens.clone(), tools));
        Ok(())
    }

    /// `team.start()` and `scheduler.start()`: after the runner's recovery and before work starts. Inside a tokio runtime.
    pub fn start(&self) -> Result<(), RpcError> {
        self.team.start();
        self.scheduler.start()
    }

    pub fn stop(&self) {
        self.scheduler.stop();
    }

    // ---- the runner's hook bodies ----

    /// The `tjommie` entry of `mcpServers`: this MCP endpoint, with a fresh token for the session.
    pub fn mcp_server(&self, bot_id: &str, mcp_url: &str) -> McpServer {
        let token = self.tokens.issue(bot_id);
        McpServer::Http {
            name: "tjommie".to_owned(),
            url: mcp_url.to_owned(),
            headers: BTreeMap::from([("Authorization".to_owned(), format!("Bearer {token}"))]),
            auto_allow: true,
        }
    }

    /// `sessionClosed`: the session's token stops working, and its autonomous chain ends.
    pub fn session_closed(&self, bot_id: &str) {
        self.tokens.revoke(bot_id);
        self.team.session_closed(bot_id);
    }

    pub fn turn_started(&self, core: &mut Core<'_>, item: &WorkItem) -> Result<(), String> {
        if item.routine_run_id.is_none() {
            return Ok(());
        }
        self.scheduler
            .run_started(core.db(), item)
            .map_err(|error| error.to_string())
    }

    pub fn turn_requeued(&self, item: &WorkItem) {
        self.scheduler.run_requeued(item);
    }

    /// A routine run is settled, then the team answers (even when settling failed, as `finally` does in `daemon.ts`).
    pub fn turn_ended(
        &self,
        core: &mut Core<'_>,
        item: &WorkItem,
        result: &TurnResult,
    ) -> Result<(), String> {
        let settled = if item.routine_run_id.is_some() {
            self.scheduler
                .run_ended(core.db(), item, result)
                .map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        let answered = self
            .team
            .turn_ended(core, item, result)
            .map_err(|error| error.to_string());
        settled.and(answered)
    }

    /// `retryRoutine`: a manual retry of a routine run, from its label message.
    pub fn retry_routine(
        &self,
        core: &mut Core<'_>,
        message: &Message,
    ) -> Result<Message, RpcError> {
        self.scheduler.retry(core, message)
    }
}
