//! Port of `packages/daemon/src/tasks.test.ts`: the task service over a real repository, database and fake runner
//! (fixture: `project_support`, the port of `project-test-support.ts`). The TypeScript file has 12 `it` blocks, not 11
//! as the brief said; all 12 are ported, in the same order.
//!
//! `Object.isFrozen` assertions have no Rust counterpart: the review context is an owned value that is moved, so there
//! is no frozen state to check (see the review-criteria test).

mod common;
mod project_support;

use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::cli::BoxFuture;
use daemon::project_checks::{CheckError, CheckResult, CheckRuntime};
use daemon::project_review::{
    ReviewContext, ReviewResult, ReviewRuntime, ReviewSnapshot, review_coverage,
};
use daemon::queue::{NewWork, WorkSource, WorkStatus, enqueue, get_work_item, set_status};
use daemon::runner::ExecutionPhase;
use daemon::tasks::TasksOptions;
use project_support::{Fixture, fixture};
use protocol::projects::{
    AttemptState, CheckRun, EvaluationKind, EvaluationState, ProjectCheck, ProjectMember,
    ProjectPatch, TaskState,
};
use regex::Regex;
use rusqlite::params;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Fails the test instead of hanging the run; the TypeScript tests have 30 s timeouts.
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .expect("timed out after 30 s")
}

/// The TypeScript `afterEach`: shuts the task service down before the test ends.
async fn close(h: &Fixture) {
    within(h.tasks.shutdown()).await.unwrap();
}

/// `passingChecks` (tasks.test.ts:13): writes the synthetic output into the check's worktree, then passes.
pub struct PassingChecks;

impl CheckRuntime for PassingChecks {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        cwd: &'a str,
        _signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>> {
        Box::pin(async move {
            fs::write(
                Path::new(cwd).join("check-output.txt"),
                "synthetic output\n",
            )
            .expect("write the synthetic check output");
            Ok(CheckRun {
                check_id: check.id.clone(),
                command: check.command.clone(),
                args: check.args.clone(),
                exit_code: Some(0),
                output: "Synthetic check passed".into(),
                truncated: false,
            })
        })
    }
}

/// `passingReview` (tasks.test.ts:17): reads one file of the snapshot, then passes with the snapshot's coverage.
pub struct PassingReview;

impl ReviewRuntime for PassingReview {
    fn review<'a>(
        &'a self,
        snapshot: &'a ReviewSnapshot,
        _signal: &'a CancellationToken,
        _context: &'a ReviewContext,
    ) -> BoxFuture<'a, Result<ReviewResult, CheckError>> {
        Box::pin(async move {
            snapshot.read("src/a.txt").map_err(CheckError::Failed)?;
            Ok(ReviewResult {
                state: EvaluationState::Passed,
                reason: None,
                findings: Vec::new(),
                coverage: Some(review_coverage(snapshot).map_err(CheckError::Failed)?),
            })
        })
    }
}

/// `{ review: async () => ({ state: "passed", reason: null, findings: [] }) }` (tasks.test.ts:50): no coverage.
struct PassWithoutCoverage;

impl ReviewRuntime for PassWithoutCoverage {
    fn review<'a>(
        &'a self,
        _snapshot: &'a ReviewSnapshot,
        _signal: &'a CancellationToken,
        _context: &'a ReviewContext,
    ) -> BoxFuture<'a, Result<ReviewResult, CheckError>> {
        Box::pin(async {
            Ok(ReviewResult {
                state: EvaluationState::Passed,
                reason: None,
                findings: Vec::new(),
                coverage: None,
            })
        })
    }
}

/// Passes, and keeps the context the reviewer was given (tasks.test.ts:60).
#[derive(Default)]
struct RecordingReview {
    observed: Mutex<Option<ReviewContext>>,
}

impl ReviewRuntime for RecordingReview {
    fn review<'a>(
        &'a self,
        snapshot: &'a ReviewSnapshot,
        _signal: &'a CancellationToken,
        context: &'a ReviewContext,
    ) -> BoxFuture<'a, Result<ReviewResult, CheckError>> {
        Box::pin(async move {
            *self.observed.lock().unwrap() = Some(context.clone());
            Ok(ReviewResult {
                state: EvaluationState::Passed,
                reason: None,
                findings: Vec::new(),
                coverage: Some(review_coverage(snapshot).map_err(CheckError::Failed)?),
            })
        })
    }
}

/// Announces that the review started, then waits until the test releases it (tasks.test.ts:73).
struct HeldReview {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl ReviewRuntime for HeldReview {
    fn review<'a>(
        &'a self,
        snapshot: &'a ReviewSnapshot,
        _signal: &'a CancellationToken,
        _context: &'a ReviewContext,
    ) -> BoxFuture<'a, Result<ReviewResult, CheckError>> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            Ok(ReviewResult {
                state: EvaluationState::Passed,
                reason: None,
                findings: Vec::new(),
                coverage: Some(review_coverage(snapshot).map_err(CheckError::Failed)?),
            })
        })
    }
}

/// Announces that the check started, then runs until its signal is cancelled (tasks.test.ts:132).
struct HangingChecks {
    started: Arc<Notify>,
}

impl CheckRuntime for HangingChecks {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        _cwd: &'a str,
        signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>> {
        Box::pin(async move {
            self.started.notify_one();
            signal.cancelled().await;
            Ok(CheckRun {
                check_id: check.id.clone(),
                command: check.command.clone(),
                args: check.args.clone(),
                exit_code: None,
                output: "cancelled".into(),
                truncated: false,
            })
        })
    }
}

#[tokio::test]
async fn creates_independent_persisted_task_contexts_only_after_explicit_start_and_rejects_busy_bot_dispatch()
 {
    let h = fixture(TasksOptions::default());
    let first = h.new_task(0);
    let second = h.new_task(1);
    let before = h.owner();
    let a = h.tasks.start(&first.id, first.revision, "a").unwrap();
    let b = h.tasks.start(&second.id, second.revision, "b").unwrap();
    assert_ne!(a.worktree_path, b.worktree_path);
    assert_ne!(a.context_key, b.context_key);
    assert_eq!(
        h.tasks.start(&first.id, first.revision, "a").unwrap().id,
        a.id
    );
    let delivered = h.runner.delivered();
    assert_eq!(delivered.len(), 2);
    let context = h
        .tasks
        .execution_context(&h.db.lock().unwrap(), &h.bots[0], &delivered[0])
        .unwrap();
    assert_eq!(
        context.map(|context| context.cwd),
        Some(a.worktree_path.clone())
    );
    let wrong_bot = h
        .tasks
        .execution_context(&h.db.lock().unwrap(), &h.bots[1], &delivered[0])
        .unwrap_err();
    assert!(wrong_bot.message.contains("no longer active"));
    let third = h.new_task(0);
    let busy = h
        .tasks
        .start(&third.id, third.revision, "busy")
        .unwrap_err();
    assert!(
        Regex::new("queued or running|owns")
            .unwrap()
            .is_match(&busy.message)
    );
    assert_eq!(h.owner(), before);
}

#[tokio::test]
async fn keeps_project_room_and_bot_request_discussions_in_the_bot_home_instead_of_the_user_checkout()
 {
    let h = fixture(TasksOptions::default());
    let repo = h.repo_path.to_string_lossy().into_owned();
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE bots SET project_folder=?1 WHERE id=?2",
            params![repo, h.bots[0].id],
        )
        .unwrap();
    for source in [WorkSource::Room, WorkSource::BotRequest] {
        let item = enqueue(
            &h.db.lock().unwrap(),
            &NewWork {
                conversation_id: h.project.input.room_id.clone(),
                ..NewWork::new(&h.bots[0].id, source, "Discuss the project")
            },
        )
        .unwrap();
        let mut bot = h.bots[0].clone();
        bot.project_folder = Some(repo.clone());
        let context = h
            .tasks
            .execution_context(&h.db.lock().unwrap(), &bot, &item)
            .unwrap()
            .expect("a discussion context");
        assert_eq!(context.phase, ExecutionPhase::Discussion);
        assert_eq!(context.cwd, h.bots[0].home_folder);
        assert_ne!(context.cwd, repo);
    }
    let stored: Option<String> =
        h.db.lock()
            .unwrap()
            .query_row(
                "SELECT project_folder FROM bots WHERE id=?1",
                [&h.bots[0].id],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(stored.as_deref(), Some(repo.as_str()));
    assert!(h.runner.delivered().is_empty());
}

#[tokio::test]
async fn persists_reviewer_unavailable_without_launching_a_normal_provider_or_accepting_the_task() {
    let h = fixture(TasksOptions::default());
    let task = h.new_task(0);
    let begun = h.tasks.start(&task.id, task.revision, "start").unwrap();
    let a = h.complete(&begun, "src/a.txt", "A1\n").await;
    let review = within(h.tasks.evaluate(
        &a.id,
        a.revision,
        a.head_sha.as_deref().unwrap(),
        EvaluationKind::Review,
    ))
    .await
    .unwrap();
    assert_eq!(review.state, EvaluationState::Unavailable);
    assert_eq!(
        review.reason.as_deref(),
        Some("review_isolation_unavailable")
    );
    assert!(!h.tasks.accepted(&h.db.lock().unwrap(), &a).unwrap());
    assert_eq!(h.runner.delivered().len(), 1);
    assert_eq!(h.task(&task.id).state, TaskState::Review);
    close(&h).await;
}

#[tokio::test]
async fn refuses_a_passing_review_without_complete_coverage_and_hydrates_legacy_coverage_as_null() {
    let h = fixture(TasksOptions {
        review: Some(Arc::new(PassWithoutCoverage)),
        ..TasksOptions::default()
    });
    let task = h.new_task(0);
    let begun = h
        .tasks
        .start(&task.id, task.revision, "missing-coverage")
        .unwrap();
    let attempt = h.complete(&begun, "src/a.txt", "A1\n").await;
    let result = within(h.tasks.evaluate(
        &attempt.id,
        attempt.revision,
        attempt.head_sha.as_deref().unwrap(),
        EvaluationKind::Review,
    ))
    .await
    .unwrap();
    assert_eq!(result.state, EvaluationState::Unavailable);
    assert!(!h.tasks.accepted(&h.db.lock().unwrap(), &attempt).unwrap());
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE task_evaluations SET data=json_remove(data,'$.reviewCoverage') WHERE id=?1",
            [&result.id],
        )
        .unwrap();
    let detail = h.tasks.detail(&h.db.lock().unwrap(), &task.id).unwrap();
    assert!(matches!(detail.evaluations[0].review_coverage, Some(None)));
    close(&h).await;
}

#[tokio::test]
async fn supplies_frozen_server_owned_reviewer_role_task_and_checklist_criteria_with_their_revisions()
 {
    let review = Arc::new(RecordingReview::default());
    let h = fixture(TasksOptions {
        review: Some(review.clone()),
        ..TasksOptions::default()
    });
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE bots SET instructions=?1 WHERE id=?2",
            params!["Check error handling", h.bots[2].id],
        )
        .unwrap();
    let members: Vec<ProjectMember> = h
        .project
        .input
        .members
        .iter()
        .map(|member| ProjectMember {
            instructions: if member.bot_id == h.bots[2].id {
                "Review compatibility".into()
            } else {
                member.instructions.clone()
            },
            ..member.clone()
        })
        .collect();
    let project = h
        .projects
        .update(
            &h.db.lock().unwrap(),
            &h.project.id,
            h.project.revision,
            ProjectPatch {
                members: Some(members),
                ..ProjectPatch::default()
            },
        )
        .unwrap();
    let task = h.new_task(0);
    let begun = h.tasks.start(&task.id, task.revision, "criteria").unwrap();
    let attempt = h.complete(&begun, "src/a.txt", "A1\n").await;
    let current_task = h.task(&task.id);
    let result = within(h.tasks.evaluate(
        &attempt.id,
        attempt.revision,
        attempt.head_sha.as_deref().unwrap(),
        EvaluationKind::Review,
    ))
    .await
    .unwrap();
    assert_eq!(result.state, EvaluationState::Passed);
    let observed = review
        .observed
        .lock()
        .unwrap()
        .clone()
        .expect("the reviewer was called");
    assert_eq!(observed.bot_id, h.bots[2].id);
    assert_eq!(observed.project_revision, project.revision);
    assert_eq!(observed.task_revision, current_task.revision);
    assert_eq!(observed.revision, attempt.revision);
    assert_eq!(observed.instructions.reviewer, "Check error handling");
    assert_eq!(observed.instructions.role, "Review compatibility");
    assert_eq!(observed.instructions.title, task.input.title);
    assert_eq!(observed.instructions.task, task.input.instructions);
    assert_eq!(observed.instructions.acceptance, task.input.acceptance);
    close(&h).await;
}

#[tokio::test]
async fn keeps_a_completed_model_reply_stale_if_trusted_reviewer_instructions_changed_during_review()
 {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let h = fixture(TasksOptions {
        review: Some(Arc::new(HeldReview {
            started: started.clone(),
            release: release.clone(),
        })),
        ..TasksOptions::default()
    });
    let task = h.new_task(0);
    let begun = h
        .tasks
        .start(&task.id, task.revision, "criteria-drift")
        .unwrap();
    let attempt = h.complete(&begun, "src/a.txt", "A1\n").await;
    let tasks = h.tasks.clone();
    let (id, revision, sha) = (
        attempt.id.clone(),
        attempt.revision,
        attempt.head_sha.clone().unwrap(),
    );
    let operation = tokio::spawn(async move {
        tasks
            .evaluate(&id, revision, &sha, EvaluationKind::Review)
            .await
    });
    within(started.notified()).await;
    h.db.lock()
        .unwrap()
        .execute(
            "UPDATE bots SET instructions=?1 WHERE id=?2",
            params!["New acceptance requirements", h.bots[2].id],
        )
        .unwrap();
    release.notify_one();
    let result = within(operation).await.unwrap().unwrap();
    assert_eq!(result.state, EvaluationState::Stale);
    assert_eq!(
        result.reason.as_deref(),
        Some("The review criteria changed during evaluation.")
    );
    let detail = h.tasks.detail(&h.db.lock().unwrap(), &task.id).unwrap();
    assert_eq!(detail.evaluations[0].state, EvaluationState::Stale);
    assert!(!h.tasks.accepted(&h.db.lock().unwrap(), &attempt).unwrap());
    assert!(
        !h.tasks
            .bot_busy(&h.db.lock().unwrap(), &h.bots[2].id)
            .unwrap()
    );
    close(&h).await;
}

#[tokio::test]
async fn binds_review_and_isolated_checks_to_exact_commit_and_revision_and_invalidates_them_when_code_changes()
 {
    let h = fixture(TasksOptions {
        checks: Some(Arc::new(PassingChecks)),
        review: Some(Arc::new(PassingReview)),
        ..TasksOptions::default()
    });
    let task = h.new_task(0);
    let begun = h.tasks.start(&task.id, task.revision, "start").unwrap();
    let a = h.complete(&begun, "src/a.txt", "A1\n").await;
    let sha = a.head_sha.clone().unwrap();
    let review = within(
        h.tasks
            .evaluate(&a.id, a.revision, &sha, EvaluationKind::Review),
    )
    .await
    .unwrap();
    assert_eq!(review.state, EvaluationState::Passed);
    let tests = within(
        h.tasks
            .evaluate(&a.id, a.revision, &sha, EvaluationKind::Test),
    )
    .await
    .unwrap();
    assert_eq!(tests.state, EvaluationState::Passed);
    let check_worktree = tests.worktree_path.clone().unwrap();
    assert_ne!(check_worktree, a.worktree_path);
    let output = fs::read_to_string(Path::new(&check_worktree).join("check-output.txt")).unwrap();
    assert!(output.contains("synthetic"));
    assert!(h.tasks.accepted(&h.db.lock().unwrap(), &a).unwrap());
    assert_eq!(h.task(&task.id).state, TaskState::Done);
    fs::write(
        Path::new(&a.worktree_path).join("src").join("a.txt"),
        "uncommitted fix\n",
    )
    .unwrap();
    h.tasks.detail(&h.db.lock().unwrap(), &task.id).unwrap();
    assert_eq!(h.attempt(&a.id).revision, a.revision + 1);
    let detail = h.tasks.detail(&h.db.lock().unwrap(), &task.id).unwrap();
    assert!(
        detail
            .evaluations
            .iter()
            .all(|evaluation| evaluation.state == EvaluationState::Stale)
    );
    assert!(
        h.tasks
            .exact(&h.db.lock().unwrap(), &a.id, a.revision, &sha)
            .is_err()
    );
    close(&h).await;
}

#[tokio::test]
async fn stop_preserves_edits_and_only_explicit_resume_requeues_an_attempt() {
    let h = fixture(TasksOptions::default());
    let task = h.new_task(0);
    let a = h.tasks.start(&task.id, task.revision, "start").unwrap();
    fs::write(
        Path::new(&a.worktree_path).join("src").join("a.txt"),
        "retained working edits\n",
    )
    .unwrap();
    let stopped = within(h.tasks.stop(&task.id, h.task(&task.id).revision, false))
        .await
        .unwrap();
    assert_eq!(h.attempt(&a.id).state, AttemptState::Interrupted);
    let kept = fs::read_to_string(Path::new(&a.worktree_path).join("src").join("a.txt")).unwrap();
    assert!(kept.contains("retained"));
    let resumed = h
        .tasks
        .resume(&task.id, stopped.revision, "resume")
        .unwrap();
    assert_eq!(resumed.id, a.id);
    assert_eq!(resumed.revision, a.revision + 1);
    assert_eq!(h.runner.delivered().len(), 2);
    close(&h).await;
}

#[tokio::test]
async fn blocks_an_interrupted_running_attempt_on_recovery_and_never_reruns_completed_work() {
    let h = fixture(TasksOptions::default());
    let task = h.new_task(0);
    let a = h.tasks.start(&task.id, task.revision, "start").unwrap();
    let work_id = a.work_item_id.clone().unwrap();
    set_status(&h.db.lock().unwrap(), &work_id, WorkStatus::Interrupted).unwrap();
    h.tasks.recover(&h.db.lock().unwrap()).unwrap();
    assert_eq!(h.attempt(&a.id).state, AttemptState::Interrupted);
    assert_eq!(h.runner.delivered().len(), 1);
    let item = get_work_item(&h.db.lock().unwrap(), &work_id).unwrap();
    assert_eq!(item.status, WorkStatus::Interrupted);
}

#[tokio::test]
async fn previews_and_refuses_cleanup_of_all_untracked_files_including_ignored_output_and_retains_branches_after_clean_cleanup()
 {
    let h = fixture(TasksOptions::default());
    let task = h.new_task(0);
    let begun = h.tasks.start(&task.id, task.revision, "start").unwrap();
    let attempt = h.complete(&begun, "src/a.txt", "A1\n").await;
    let head = attempt.head_sha.clone().unwrap();
    let output = Path::new(&attempt.worktree_path).join("check-output.txt");
    fs::write(&output, "ignored bytes to retain\n").unwrap();
    let preview = h
        .tasks
        .cleanup_preview(&h.db.lock().unwrap(), &attempt.id)
        .unwrap();
    assert!(!preview.can_remove);
    assert!(preview.files.contains(&"check-output.txt".to_string()));
    let refused = h
        .tasks
        .cleanup(&h.db.lock().unwrap(), &attempt.id, &head)
        .unwrap_err();
    assert!(refused.message.contains("retained work"));
    assert!(fs::read_to_string(&output).unwrap().contains("retain"));
    fs::remove_file(&output).unwrap();
    let clean = h
        .tasks
        .cleanup_preview(&h.db.lock().unwrap(), &attempt.id)
        .unwrap();
    assert!(clean.can_remove);
    let cleaned = h
        .tasks
        .cleanup(&h.db.lock().unwrap(), &attempt.id, &head)
        .unwrap();
    assert_eq!(cleaned.state, AttemptState::Cleaned);
    assert_eq!(
        h.git(&h.repo_path, &["rev-parse", attempt.branch.as_str()]),
        head
    );
    let detail = h.tasks.detail(&h.db.lock().unwrap(), &task.id).unwrap();
    assert_eq!(detail.attempts[0].state, AttemptState::Cleaned);
    close(&h).await;
}

#[tokio::test]
async fn does_not_wait_for_or_interrupt_a_different_live_task_when_cancelling_completed_work() {
    let h = fixture(TasksOptions::default());
    let old_task = h.new_task(0);
    let begun = h
        .tasks
        .start(&old_task.id, old_task.revision, "old")
        .unwrap();
    let old = h.complete(&begun, "src/a.txt", "A1\n").await;
    let next_task = h.new_task(0);
    let next = h
        .tasks
        .start(&next_task.id, next_task.revision, "next")
        .unwrap();
    h.runner.hang_retired.store(true, Ordering::SeqCst);
    let cancelled = within(
        h.tasks
            .stop(&old_task.id, h.task(&old_task.id).revision, true),
    )
    .await
    .unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert_eq!(h.attempt(&next.id).state, AttemptState::Running);
    let next_item =
        get_work_item(&h.db.lock().unwrap(), next.work_item_id.as_deref().unwrap()).unwrap();
    assert_eq!(next_item.status, WorkStatus::Running);
    assert_eq!(h.attempt(&old.id).state, AttemptState::Cancelled);
    close(&h).await;
}

#[tokio::test]
async fn cancels_and_awaits_owned_evaluations_during_shutdown_before_releasing_their_bot_lease() {
    let started = Arc::new(Notify::new());
    let h = fixture(TasksOptions {
        checks: Some(Arc::new(HangingChecks {
            started: started.clone(),
        })),
        ..TasksOptions::default()
    });
    let task = h.new_task(0);
    let begun = h.tasks.start(&task.id, task.revision, "start").unwrap();
    let attempt = h.complete(&begun, "src/a.txt", "A1\n").await;
    let tasks = h.tasks.clone();
    let (id, revision, sha) = (
        attempt.id.clone(),
        attempt.revision,
        attempt.head_sha.clone().unwrap(),
    );
    let evaluation = tokio::spawn(async move {
        tasks
            .evaluate(&id, revision, &sha, EvaluationKind::Test)
            .await
    });
    within(started.notified()).await;
    assert!(
        h.tasks
            .bot_busy(&h.db.lock().unwrap(), &h.bots[3].id)
            .unwrap()
    );
    within(h.tasks.shutdown()).await.unwrap();
    let result = within(evaluation).await.unwrap().unwrap();
    assert_eq!(result.state, EvaluationState::Stale);
    assert!(
        !h.tasks
            .bot_busy(&h.db.lock().unwrap(), &h.bots[3].id)
            .unwrap()
    );
    let closed = h
        .tasks
        .start(&task.id, h.task(&task.id).revision, "closed")
        .unwrap_err();
    assert!(closed.message.contains("closing"));
}
