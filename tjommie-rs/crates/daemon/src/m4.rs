//! M4 collaboration in one place, like `m6.rs`: the team (rooms, exchanges, the five tools), the MCP endpoint, routines,
//! the scheduler and the notification handlers. This is the part of `daemon.ts` (`:243-302`, `:568-618`, `:648`) that wires
//! them together.
//!
//! **Order.** The runner needs M4's hooks when it is built, and M4's tools and wake-ups need the runner, so M4 is made
//! first ([`M4::new`]), the runner is built with the hook bodies below, and then [`M4::attach`] gives M4 the runner and
//! [`M4::install`] registers the handlers and the `/mcp` route. [`M4::start_team`] and [`M4::start_scheduler`] come with the daemon's other starts.
//!
//! **Hooks.** The runner calls these with its locks held. The daemon's own `turnEnded` also runs the task hooks first and
//! the turn notice after, which belong to other tasks, so Core's hooks call these methods inside their own closures:
//!
//! | `RunnerHooks` field | call |
//! |---|---|
//! | `mcp_servers` | [`M4::mcp_servers`] (the `tjommie` and `browser` entries) |
//! | `session_closed` | [`M4::session_closed`] |
//! | `turn_started` | [`M4::turn_started`] |
//! | `turn_requeued` | [`M4::turn_requeued`] |
//! | `turn_ended` | [`M4::turn_ended`] (after the task hook, before the notice) |
//! | `retry_routine` | [`M4::retry_routine`] |

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::FutureExt;
use protocol::ErrorCode;
use protocol::model::{Approval, ApprovalDecision, Autonomy, Message};

use crate::bots::StoredBot;
use crate::context::{Context, Db};
use crate::mcp::{McpTokens, McpTool, mcp_route};
use crate::providers::McpServer;
use crate::queue::WorkItem;
use crate::rooms;
use crate::routines::{self, now_ms};
use crate::runner::{Broadcast, Core, Log, Runner, TurnResult};
use crate::scheduler::{self, Scheduler, SchedulerDeps, TICK_MS};
use crate::server::{Handlers, Routes, RpcError};
use crate::team::{self, BotDeletionHandle, HostAccess, SharedAttachments, Team, TeamDeps};

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
    /// the work its routine had queued through it. It is given the M4 to call rather than capturing it: M4 holds this
    /// closure, so a closure holding M4 would keep both alive for ever.
    pub turn_ended: DaemonTurnEnded,
}

/// [`M4Deps::turn_ended`]: the daemon's `turnEnded`, run with the M4 and the runner's [`Core`].
pub type DaemonTurnEnded =
    Arc<dyn Fn(&M4, &mut Core<'_>, &WorkItem, &TurnResult) -> Result<(), String> + Send + Sync>;

/// The runner M4 works with, once [`M4::attach`] gives it and until [`M4::stop`] takes it back. The runner's hooks hold
/// M4, so M4 must let go of the runner or the two keep each other (and the database handle) alive after shutdown.
type RunnerSlot = Arc<Mutex<Option<Runner>>>;

fn current(slot: &RunnerSlot) -> Option<Runner> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

pub struct M4 {
    /// One bearer token per running CLI session, shared with the `/mcp` route (and `m6`'s `/mcp/browser`).
    pub tokens: McpTokens,
    pub team: Arc<Team>,
    pub scheduler: Scheduler,
    runner: RunnerSlot,
    cleanup_shared: Arc<dyn Fn() + Send + Sync>,
    daemon_turn_ended: DaemonTurnEnded,
    log: Log,
}

impl M4 {
    pub fn new(deps: M4Deps) -> M4 {
        let runner: RunnerSlot = Arc::default();
        let team = Arc::new(Team::new(TeamDeps {
            broadcast: deps.broadcast.clone(),
            log: deps.log.clone(),
            shared_files: deps.shared_files,
            attachments: deps.attachments,
        }));
        let (delivering, stopping) = (runner.clone(), runner.clone());
        let log = deps.log.clone();
        let scheduler = Scheduler::new(SchedulerDeps {
            db: deps.db,
            broadcast: deps.broadcast,
            deliver: Arc::new(move |item: &WorkItem| {
                let runner = current(&delivering).ok_or("The runner is not ready")?;
                runner
                    .deliver(item.clone())
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }),
            stop_item: Arc::new(
                move |item_id: &str, notice: &str| match current(&stopping) {
                    Some(runner) => {
                        let stopped = runner.stop_item(item_id, notice);
                        async move { stopped.await.map_err(|error| error.to_string()) }.boxed()
                    }
                    None => async { Err("The runner is not ready".to_owned()) }.boxed(),
                },
            ),
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
            daemon_turn_ended: deps.turn_ended,
            log,
        }
    }

    /// Gives M4 the runner it was built before. Call once, before [`M4::install`] and [`M4::start_scheduler`]; a second
    /// call replaces it.
    pub fn attach(&self, runner: Runner) {
        *self.runner.lock().unwrap_or_else(PoisonError::into_inner) = Some(runner);
    }

    /// Gives the team its shared-file links, when the real `Attachments` is made after M4 (as in startup, where M6
    /// makes it).
    pub fn attach_attachments(&self, attachments: Arc<dyn SharedAttachments>) {
        self.team.attach_attachments(attachments);
    }

    fn runner(&self) -> Result<Runner, RpcError> {
        current(&self.runner).ok_or_else(|| {
            RpcError::new(
                ErrorCode::Internal,
                "M4 has no runner: it is not attached yet, or has stopped",
            )
        })
    }

    /// Registers the handlers (`rooms.*`, `room.*`, `exchanges.get`, `routines.*`, `notifications.*`) and mounts `/mcp`
    /// with the team's tools followed by `extra_tools` (the Google tools, when there is a Google login).
    pub fn install(
        self: &Arc<Self>,
        handlers: &mut Handlers,
        routes: &mut Routes,
        context: &Context,
        extra_tools: Vec<McpTool>,
    ) -> Result<(), RpcError> {
        let access: Arc<dyn HostAccess> = Arc::new(self.runner()?);
        // `routines.delete` ends the work its routine had queued through the daemon's `turnEnded`, outside any lock. These
        // closures belong to the handlers, so holding M4 here is no cycle.
        let ended = {
            let (m4, slot) = (self.clone(), self.runner.clone());
            Arc::new(move |item: &WorkItem, result: &TurnResult| {
                if let Some(runner) = current(&slot)
                    && let Err(error) =
                        runner.with(|core| (m4.daemon_turn_ended)(&m4, core, item, result))
                {
                    (m4.log)("turnEnded", &error);
                }
            })
        };
        let table = std::mem::take(handlers);
        let table = rooms::register(table, context, self.cleanup_shared.clone());
        let table = routines::register(table, context, ended);
        let table = scheduler::register(table, context);
        *handlers = team::register(table, context, self.team.clone(), access.clone());
        let mut tools = self.team.tools(access);
        tools.extend(extra_tools);
        *routes = std::mem::take(routes).route("/mcp", mcp_route(self.tokens.clone(), tools));
        Ok(())
    }

    /// `team.start()`: removes stale file snapshots. `daemon.ts` runs it just before `runner.recover()`.
    pub fn start_team(&self) {
        self.team.start();
    }

    /// `scheduler.start()`: `daemon.ts` runs it right after `runner.start()`. Inside a tokio runtime.
    pub fn start_scheduler(&self) -> Result<(), RpcError> {
        self.scheduler.start()
    }

    /// Stops the scheduler and lets go of the runner, which breaks the loop the runner's hooks make with M4. Work the
    /// runner still ends while it shuts down reaches the team through the hooks' own `Core`, not through the runner.
    pub fn stop(&self) {
        self.scheduler.stop();
        self.runner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    // ---- the runner's hook bodies ----

    /// The `mcpServers` list of `daemon.ts`: `tjommie` (this MCP endpoint), and `browser` (`mcp_url` + `/browser`, M6's
    /// proxy) when the bot's browser is on. A fresh token serves both, and browser tools ask nothing unless the bot's
    /// autonomy is `ask`.
    pub fn mcp_servers(&self, bot: &StoredBot, mcp_url: &str) -> Vec<McpServer> {
        let token = self.tokens.issue(&bot.id);
        let mut servers = vec![http_server("tjommie", mcp_url, &token, true)];
        if bot.browser {
            let url = format!("{mcp_url}/browser");
            servers.push(http_server(
                "browser",
                &url,
                &token,
                bot.autonomy != Autonomy::Ask,
            ));
        }
        servers
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

    // ---- what the handlers call, through the runner's host ----

    /// `team.botDeleting(id)`: captures the bot's open exchanges and snapshot folders, before its row goes.
    pub fn bot_deleting(&self, bot_id: &str) -> Result<BotDeletionHandle, RpcError> {
        let runner = self.runner()?;
        team::with_host(&runner, |host| self.team.bot_deleting(host, bot_id))
    }

    /// `team.botDeleted(handle)`: after the bot's row is gone, answers its open exchanges and frees its snapshots.
    pub fn bot_deleted(&self, handle: &BotDeletionHandle) -> Result<(), RpcError> {
        let runner = self.runner()?;
        team::with_host(&runner, |host| self.team.bot_deleted(host, handle))
    }

    /// `team.answerRoutineProposal`: `approvals.respond` for a routine proposal. The caller pumps the bot's queue.
    pub fn answer_routine_proposal(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<Approval, RpcError> {
        let runner = self.runner()?;
        team::with_host(&runner, |host| {
            self.team
                .answer_routine_proposal(host, approval_id, decision)
        })
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

fn http_server(name: &str, url: &str, token: &str, auto_allow: bool) -> McpServer {
    McpServer::Http {
        name: name.to_owned(),
        url: url.to_owned(),
        headers: BTreeMap::from([("Authorization".to_owned(), format!("Bearer {token}"))]),
        auto_allow,
    }
}
