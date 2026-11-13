//! `task-integration.ts`: merging an accepted task commit into the project's target branch. The candidate is built and
//! checked in an owned worktree, and only a verified candidate is promoted. Locking follows `tasks.rs`: methods taking
//! a `&Connection` expect the caller to hold the database lock; the async ones lock in short sections, and git and the
//! checks run with it released.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use protocol::model::Autonomy;
use protocol::projects::{IntegrationState, TaskIntegration, TasksIntegrate};
use protocol::{ErrorCode, Event};
use rusqlite::{Connection, params};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::bots::{StoredBot, get_stored_bot};
use crate::ids::{new_id, now_iso};
use crate::project_checks::{CheckError, CheckRuntime};
use crate::projects::{data_list, data_of, exists, to_json, transaction, wire_text};
use crate::server::RpcError;
use crate::tasks::{Tasks, bad_state, blocking, failed};
use crate::worktrees::{ManagedWorktree, Repository};

#[derive(Default)]
struct State {
    running: HashMap<String, CancellationToken>,
    closing: bool,
    termination_unknown: bool,
}

pub struct TaskIntegrations {
    pub tasks: Arc<Tasks>,
    checks: Option<Arc<dyn CheckRuntime>>,
    /// `new HostCheckRuntime()`: used when `checks` is unset and the tester's autonomy is `full`.
    host_checks: Option<Arc<dyn CheckRuntime>>,
    state: Mutex<State>,
    /// How many preparations are queued or running; `shutdown` waits for zero.
    settling: watch::Sender<usize>,
}

/// Counts one preparation, from `start` until the task that runs it ends.
struct Settling(Arc<TaskIntegrations>);

impl Settling {
    fn new(integrations: &Arc<TaskIntegrations>) -> Self {
        integrations.settling.send_modify(|count| *count += 1);
        Self(integrations.clone())
    }
}

impl Drop for Settling {
    fn drop(&mut self) {
        self.0.settling.send_modify(|count| *count -= 1);
    }
}

/// What a preparation needs once its first section has run.
struct Prepared {
    attempt_revision: u64,
    attempt_id: String,
    project: protocol::projects::Project,
    repo: Repository,
    tester: StoredBot,
    checks: Arc<dyn CheckRuntime>,
    controller: CancellationToken,
}

impl TaskIntegrations {
    /// `new TaskIntegrations(tasks, checks)`. Must be called inside a Tokio runtime (through `tasks`).
    pub fn new(
        tasks: Arc<Tasks>,
        checks: Option<Arc<dyn CheckRuntime>>,
        host_checks: Option<Arc<dyn CheckRuntime>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            tasks,
            checks,
            host_checks,
            state: Mutex::default(),
            settling: watch::channel(0).0,
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn closing(&self) -> bool {
        self.state().closing
    }

    pub fn get(&self, db: &Connection, id: &str) -> Result<TaskIntegration, RpcError> {
        data_of(db, "SELECT data FROM task_integrations WHERE id = ?1", [id])
    }

    /// `#save`.
    fn save(&self, db: &Connection, record: &TaskIntegration) -> Result<TaskIntegration, RpcError> {
        db.execute(
            "UPDATE task_integrations SET state = ?1, data = ?2 WHERE id = ?3",
            params![wire_text(&record.state), to_json(record)?, record.id],
        )?;
        self.tasks
            .emit(Event::TaskIntegrationUpdated(record.clone()));
        Ok(record.clone())
    }

    /// `#blocked`.
    fn blocked(
        &self,
        db: &Connection,
        record: &TaskIntegration,
        reason: &str,
    ) -> Result<TaskIntegration, RpcError> {
        self.save(
            db,
            &TaskIntegration {
                state: IntegrationState::Blocked,
                reason: Some(reason.to_owned()),
                updated_at: now_iso(),
                ..record.clone()
            },
        )
    }

    /// `tasks.integrate`: records the integration and prepares it in the background.
    pub fn start(self: &Arc<Self>, input: &TasksIntegrate) -> Result<TaskIntegration, RpcError> {
        if self.closing() {
            return Err(bad_state("The integration service is closing."));
        }
        let record = {
            let db = self.tasks.lock();
            let repeated: Option<(String, String, String)> =
                rusqlite::OptionalExtension::optional(db.query_row(
                    "SELECT task_id, result_id, operation FROM task_requests WHERE id = ?1",
                    [&input.request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ))?;
            let existing = self.tasks.attempt(&db, &input.attempt_id)?;
            let task = self.tasks.task(&db, &existing.task_id)?;
            if let Some((task_id, result_id, operation)) = repeated {
                if task_id != task.id || operation != "integrate" {
                    return Err(RpcError::new(
                        ErrorCode::BadParams,
                        "This request ID was already used for another action.",
                    ));
                }
                return self.get(&db, &result_id);
            }
            let attempt = self.tasks.exact(
                &db,
                &input.attempt_id,
                input.expected_revision,
                &input.source_sha,
            )?;
            let project = self.tasks.projects.active(&db, &task.input.project_id)?;
            let repo = self
                .tasks
                .projects
                .repository(&db, &project.input.repository_id)?;
            if exists(
                &db,
                "SELECT 1 FROM task_integrations WHERE repository_id = ?1 AND target_ref = ?2 AND state IN ('preparing', 'checking', 'ready')",
                params![repo.id, project.input.target_ref],
            )? {
                return Err(RpcError::new(
                    ErrorCode::Busy,
                    "Another integration owns this target.",
                ));
            }
            let id = new_id();
            let work = self.tasks.projects.worktrees.describe(
                &format!("integration-{id}"),
                &input.expected_target_sha,
                None,
            )?;
            let timestamp = now_iso();
            let record = TaskIntegration {
                id: id.clone(),
                attempt_id: attempt.id.clone(),
                revision: attempt.revision,
                accepted_sha: input.source_sha.clone(),
                expected_target_sha: input.expected_target_sha.clone(),
                checks_completed: false,
                project_revision: project.revision,
                target_ref: project.input.target_ref.clone(),
                candidate_sha: None,
                branch: work.branch,
                worktree_path: work.path,
                promotion_path: None,
                state: IntegrationState::Preparing,
                reason: None,
                checks: Vec::new(),
                created_at: timestamp.clone(),
                updated_at: timestamp,
            };
            transaction(&db, || {
                db.execute(
                    "INSERT INTO task_integrations VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        id,
                        attempt.id,
                        repo.id,
                        wire_text(&record.state),
                        record.target_ref,
                        to_json(&record)?
                    ],
                )?;
                db.execute(
                    "INSERT INTO task_requests VALUES (?1, ?2, ?3, ?4)",
                    params![input.request_id, "integrate", task.id, id],
                )?;
                Ok(())
            })?;
            self.tasks
                .emit(Event::TaskIntegrationUpdated(record.clone()));
            record
        };
        // `setImmediate`: the response goes out first; counted now so `shutdown` waits for it.
        let settling = Settling::new(self);
        let (this, queued, id) = (self.clone(), record.clone(), record.id.clone());
        self.tasks.handle().spawn(async move {
            let _settling = settling;
            if this.prepare(queued).await.is_err() {
                let this = this.clone();
                let _ = blocking(move || -> Result<(), RpcError> {
                    let db = this.tasks.lock();
                    let record = this.get(&db, &id)?;
                    this.blocked(
                        &db,
                        &record,
                        "Integration could not finish. Retained work needs inspection.",
                    )?;
                    Ok(())
                })
                .await;
            }
        });
        Ok(record)
    }

    /// `#prepare`: builds the candidate, runs every combined check, then promotes it.
    async fn prepare(self: &Arc<Self>, record: TaskIntegration) -> Result<(), CheckError> {
        let this = self.clone();
        let first = record.clone();
        let started = blocking(move || this.begin_preparation(&first))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        let Some(prepared) = started else {
            return Ok(());
        };
        let mut terminated = false;
        let result = self.run_preparation(&prepared, record.clone()).await;
        if matches!(result, Err(CheckError::Termination(_))) {
            terminated = true;
            self.state().termination_unknown = true;
        }
        self.state().running.remove(&record.id);
        if !terminated {
            let this = self.clone();
            let (bot_id, owner_id) = (prepared.tester.id.clone(), record.id.clone());
            blocking(move || this.tasks.release(&this.tasks.lock(), &bot_id, &owner_id))
                .await
                .map_err(failed)?
                .map_err(failed)?;
        }
        result
    }

    /// The first section of `#prepare`, with the lock held. `None` means the integration was blocked (or stopped).
    fn begin_preparation(&self, record: &TaskIntegration) -> Result<Option<Prepared>, RpcError> {
        let db = self.tasks.lock();
        if self.closing() {
            self.blocked(
                &db,
                record,
                "Integration stopped before preparation; no project checks were run.",
            )?;
            return Ok(None);
        }
        let attempt = self.tasks.exact(
            &db,
            &record.attempt_id,
            record.revision,
            &record.accepted_sha,
        )?;
        let task = self.tasks.task(&db, &attempt.task_id)?;
        let project = self.tasks.projects.active(&db, &task.input.project_id)?;
        if !self.tasks.accepted(&db, &attempt)? {
            self.blocked(
                &db,
                record,
                "The exact task commit needs passing independent review and configured checks.",
            )?;
            return Ok(None);
        }
        let repo = self
            .tasks
            .projects
            .repository(&db, &project.input.repository_id)?;
        let trees = &self.tasks.projects.worktrees;
        if trees.resolve_ref(&repo, &record.target_ref)? != record.expected_target_sha {
            self.blocked(&db, record, "target_head_moved")?;
            return Ok(None);
        }
        let tester = match &task.input.tester_id {
            Some(id) => Some(get_stored_bot(&db, id)?),
            None => None,
        };
        let checks = self.checks.clone().or_else(|| {
            tester
                .as_ref()
                .filter(|tester| tester.autonomy == Autonomy::Full)
                .and_then(|_| self.host_checks.clone())
        });
        let (Some(tester), Some(checks)) = (tester, checks) else {
            self.blocked(&db, record, "check_isolation_unavailable")?;
            return Ok(None);
        };
        if project.input.checks.is_empty() {
            self.blocked(&db, record, "check_isolation_unavailable")?;
            return Ok(None);
        }
        self.tasks
            .lease(&db, &tester.id, &record.id, "integration")?;
        let controller = CancellationToken::new();
        self.state()
            .running
            .insert(record.id.clone(), controller.clone());
        Ok(Some(Prepared {
            attempt_revision: attempt.revision,
            attempt_id: attempt.id,
            project,
            repo,
            tester,
            checks,
            controller,
        }))
    }

    /// Saves the record under the lock.
    async fn save_record(self: &Arc<Self>, record: &TaskIntegration) -> Result<(), CheckError> {
        let (this, record) = (self.clone(), record.clone());
        blocking(move || this.save(&this.tasks.lock(), &record))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        Ok(())
    }

    /// Blocks the record under the lock.
    async fn block_record(
        self: &Arc<Self>,
        record: &TaskIntegration,
        reason: &'static str,
    ) -> Result<(), CheckError> {
        let (this, record) = (self.clone(), record.clone());
        blocking(move || this.blocked(&this.tasks.lock(), &record, reason))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        Ok(())
    }

    /// The `try` body of `#prepare`.
    async fn run_preparation(
        self: &Arc<Self>,
        prepared: &Prepared,
        mut record: TaskIntegration,
    ) -> Result<(), CheckError> {
        let trees = self.tasks.projects.worktrees.clone();
        let work: ManagedWorktree = trees
            .describe(
                &format!("integration-{}", record.id),
                &record.expected_target_sha,
                Some(&record.branch),
            )
            .map_err(failed)?;
        let (merge_trees, repo, merge_work) = (trees.clone(), prepared.repo.clone(), work.clone());
        let (target, accepted) = (
            record.expected_target_sha.clone(),
            record.accepted_sha.clone(),
        );
        let merged =
            blocking(move || merge_trees.merge_candidate(&repo, &merge_work, &target, &accepted))
                .await
                .map_err(failed)?
                .map_err(failed)?;
        if merged.conflict {
            return self
                .block_record(
                    &record,
                    "conflict: resolve the retained candidate files, then create a new reviewed task commit; no side was selected.",
                )
                .await;
        }
        record.candidate_sha = merged.sha;
        record.state = IntegrationState::Checking;
        record.updated_at = now_iso();
        self.save_record(&record).await?;
        for check in &prepared.project.input.checks {
            let result = prepared
                .checks
                .run(check, &work.path, &prepared.controller)
                .await?;
            let passed = result.exit_code == Some(0);
            record.checks.push(result);
            record.updated_at = now_iso();
            self.save_record(&record).await?;
            if !passed {
                return self.block_record(&record, "combined_checks_failed").await;
            }
        }
        let this = self.clone();
        let (attempt_id, revision, accepted) = (
            prepared.attempt_id.clone(),
            prepared.attempt_revision,
            record.accepted_sha.clone(),
        );
        blocking(move || {
            this.tasks
                .exact(&this.tasks.lock(), &attempt_id, revision, &accepted)
        })
        .await
        .map_err(failed)?
        .map_err(failed)?;
        if prepared.controller.is_cancelled() {
            return self
                .block_record(
                    &record,
                    "Integration stopped; retained candidate is unchanged.",
                )
                .await;
        }
        let candidate = record.candidate_sha.clone();
        let (dirty_trees, repo, dirty_work) = (trees, prepared.repo.clone(), work);
        let changed = blocking(move || -> Result<bool, RpcError> {
            match candidate {
                None => Ok(true),
                Some(sha) => dirty_trees.dirty(&repo, &dirty_work, Some(&sha), true),
            }
        })
        .await
        .map_err(failed)?
        .map_err(failed)?;
        if changed {
            return self
                .block_record(&record, "Combined checks changed the candidate source.")
                .await;
        }
        record.state = IntegrationState::Ready;
        record.checks_completed = true;
        record.updated_at = now_iso();
        self.save_record(&record).await?;
        let this = self.clone();
        let (id, expected) = (record.id.clone(), record.expected_target_sha.clone());
        blocking(move || this.promote(&this.tasks.lock(), &id, &expected))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        Ok(())
    }

    /// `tasks.promote`: advances the target branch to the verified candidate, in an owned worktree.
    pub fn promote(
        &self,
        db: &Connection,
        id: &str,
        expected_target_sha: &str,
    ) -> Result<TaskIntegration, RpcError> {
        if self.closing() {
            return Err(bad_state("The integration service is closing."));
        }
        let record = self.get(db, id)?;
        if record.state == IntegrationState::Integrated {
            return Ok(record);
        }
        let verified = matches!(
            record.state,
            IntegrationState::Ready | IntegrationState::Blocked
        ) && record.candidate_sha.is_some()
            && record.checks_completed
            && !record.checks.is_empty()
            && record.checks.iter().all(|check| check.exit_code == Some(0));
        if !verified {
            return Err(bad_state(
                "This integration has no verified candidate to promote.",
            ));
        }
        if expected_target_sha != record.expected_target_sha {
            return Err(bad_state(
                "Prepare a new integration for the changed target head.",
            ));
        }
        let attempt = self.tasks.exact(
            db,
            &record.attempt_id,
            record.revision,
            &record.accepted_sha,
        )?;
        if !self.tasks.accepted(db, &attempt)? {
            return self.blocked(db, &record, "Review or tests are stale.");
        }
        let task = self.tasks.task(db, &attempt.task_id)?;
        let project = self.tasks.projects.active(db, &task.input.project_id)?;
        let repo = self
            .tasks
            .projects
            .repository(db, &project.input.repository_id)?;
        let trees = &self.tasks.projects.worktrees;
        if project.input.target_ref != record.target_ref
            || project.revision != record.project_revision
        {
            return self.blocked(
                db,
                &record,
                "The project's target or check configuration changed.",
            );
        }
        let configured = &project.input.checks;
        let mismatch = record.checks.len() != configured.len()
            || configured
                .iter()
                .zip(&record.checks)
                .any(|(check, result)| {
                    result.check_id != check.id
                        || result.command != check.command
                        || result.args != check.args
                });
        if mismatch {
            return Err(bad_state(
                "The candidate has not passed every configured combined check.",
            ));
        }
        if exists(
            db,
            "SELECT 1 FROM task_integrations WHERE repository_id = ?1 AND target_ref = ?2 AND id <> ?3 AND state IN ('preparing', 'checking', 'ready')",
            params![repo.id, record.target_ref, record.id],
        )? {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "Another integration owns this target.",
            ));
        }
        let work = trees.describe(
            &format!("integration-{}", record.id),
            &record.expected_target_sha,
            Some(&record.branch),
        )?;
        let candidate = record.candidate_sha.clone().unwrap_or_default();
        if trees.head(&repo, &work)? != candidate
            || trees.dirty(&repo, &work, Some(&candidate), true)?
        {
            return self.blocked(db, &record, "The verified candidate changed.");
        }
        let promoted = trees.promote(
            &repo,
            &record.target_ref,
            &record.expected_target_sha,
            &candidate,
        )?;
        if let Some(reason) = promoted.reason {
            return self.save(
                db,
                &TaskIntegration {
                    state: IntegrationState::Blocked,
                    promotion_path: promoted.path,
                    reason: Some(reason),
                    updated_at: now_iso(),
                    ..record
                },
            );
        }
        self.save(
            db,
            &TaskIntegration {
                state: IntegrationState::Integrated,
                promotion_path: promoted.path,
                reason: None,
                updated_at: now_iso(),
                ..record
            },
        )
    }

    /// `stopAttempt`: cancels every running preparation of the attempt.
    pub fn stop_attempt(&self, db: &Connection, attempt_id: &str) -> Result<(), RpcError> {
        let running: Vec<(String, CancellationToken)> = self
            .state()
            .running
            .iter()
            .map(|(id, token)| (id.clone(), token.clone()))
            .collect();
        for (id, token) in running {
            if self.get(db, &id)?.attempt_id == attempt_id {
                token.cancel();
            }
        }
        Ok(())
    }

    /// `recover`: after a restart nothing that was preparing is still running.
    pub fn recover(&self, db: &Connection) -> Result<(), RpcError> {
        let records = data_list::<TaskIntegration>(
            db,
            "SELECT data FROM task_integrations WHERE state IN ('preparing', 'checking', 'ready')",
            [],
        )?;
        for record in records {
            self.blocked(
                db,
                &record,
                "Daemon restarted during integration. Inspect retained work and prepare again.",
            )?;
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        {
            let mut state = self.state();
            state.closing = true;
            for controller in state.running.values() {
                controller.cancel();
            }
        }
        let mut idle = self.settling.subscribe();
        let _ = idle.wait_for(|count| *count == 0).await;
        if self.state().termination_unknown {
            return Err("An owned integration check could not be confirmed stopped; retain its state for reconciliation.".into());
        }
        Ok(())
    }
}
