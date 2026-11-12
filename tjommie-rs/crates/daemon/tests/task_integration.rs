//! Port of `packages/daemon/src/task-integration.test.ts`: merging accepted task commits into the target branch, over a
//! real repository, database and fake runner (fixture: `project_support`, the port of `project-test-support.ts`).
//!
//! The TypeScript file defines its own `checks` and `review` fakes (it does not import `tasks.test.ts`), so the
//! ones here are local copies of the same behaviour.

mod common;
mod project_support;

use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use daemon::cli::BoxFuture;
use daemon::project_checks::{CheckError, CheckResult, CheckRuntime};
use daemon::project_review::{
    ReviewContext, ReviewResult, ReviewRuntime, ReviewSnapshot, review_coverage,
};
use daemon::task_integration::TaskIntegrations;
use daemon::tasks::TasksOptions;
use project_support::{Fixture, eventually, fixture};
use protocol::projects::{
    CheckRun, EvaluationKind, EvaluationState, IntegrationState, ProjectCheck, ProjectPatch,
    TaskAttempt, TaskIntegration, TasksIntegrate,
};
use regex::Regex;
use tokio_util::sync::CancellationToken;

/// Fails the test instead of hanging the run; the TypeScript tests have 30 s timeouts.
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .expect("timed out after 30 s")
}

/// The TypeScript `afterEach`: the integration service first, then the task service.
async fn close(h: &Fixture, service: &TaskIntegrations) {
    within(service.shutdown()).await.unwrap();
    within(h.tasks.shutdown()).await.unwrap();
}

fn check_run(check: &ProjectCheck, exit_code: Option<i64>, output: &str) -> CheckRun {
    CheckRun {
        check_id: check.id.clone(),
        command: check.command.clone(),
        args: check.args.clone(),
        exit_code,
        output: output.into(),
        truncated: false,
    }
}

/// `checks` (task-integration.test.ts:9): every check passes with synthetic output, and writes nothing.
struct SyntheticPass;

impl CheckRuntime for SyntheticPass {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        _cwd: &'a str,
        _signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>> {
        Box::pin(async move { Ok(check_run(check, Some(0), "synthetic pass")) })
    }
}

/// `failing` (task-integration.test.ts:73): every check exits with code 1.
struct FailingChecks;

impl CheckRuntime for FailingChecks {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        _cwd: &'a str,
        _signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>> {
        Box::pin(async move { Ok(check_run(check, Some(1), "synthetic failure")) })
    }
}

/// Counts the checks it is asked to run, and passes them (task-integration.test.ts:82).
#[derive(Default)]
struct CountingChecks {
    runs: AtomicUsize,
}

impl CheckRuntime for CountingChecks {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        _cwd: &'a str,
        _signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>> {
        Box::pin(async move {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(check_run(check, Some(0), "synthetic pass"))
        })
    }
}

/// The first combined check passes, and the second one fails to return a result (a thrown error in the TypeScript).
#[derive(Default)]
struct FailsSecondCheck {
    calls: AtomicUsize,
}

impl CheckRuntime for FailsSecondCheck {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        _cwd: &'a str,
        _signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(CheckError::Failed(
                    "Synthetic interruption before check result".into(),
                ));
            }
            Ok(check_run(check, Some(0), "synthetic pass"))
        })
    }
}

/// `review` (task-integration.test.ts:10): reads one file of the snapshot, then passes with its coverage.
struct PassingReview;

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

/// `projectFixture({ checks, review })` (task-integration.test.ts:19).
fn integration_fixture() -> Fixture {
    fixture(TasksOptions {
        checks: Some(Arc::new(SyntheticPass)),
        review: Some(Arc::new(PassingReview)),
        ..TasksOptions::default()
    })
}

/// `new TaskIntegrations(h.tasks, checks)` (task-integration.test.ts:38). The TypeScript constructor takes no host
/// checks, so none is passed: every test here supplies its own runtime.
fn integrations(h: &Fixture, checks: Arc<dyn CheckRuntime>) -> Arc<TaskIntegrations> {
    TaskIntegrations::new(h.tasks.clone(), Some(checks), None)
}

/// `accepted(h, index)` (task-integration.test.ts:20): developer `index` commits one file, then its review and checks
/// both pass.
async fn accepted(h: &Fixture, index: usize) -> TaskAttempt {
    let task = h.new_task(index);
    let (file, bytes) = if index == 0 {
        ("src/a.txt", "A1\n")
    } else {
        ("src/b.txt", "B1\n")
    };
    let begun = h
        .tasks
        .start(&task.id, task.revision, &format!("start-{index}"))
        .unwrap();
    let attempt = h.complete(&begun, file, bytes).await;
    let sha = attempt.head_sha.clone().unwrap();
    within(h.tasks.evaluate(&attempt.id, attempt.revision, &sha, EvaluationKind::Review))
        .await
        .unwrap();
    within(h.tasks.evaluate(&attempt.id, attempt.revision, &sha, EvaluationKind::Test))
        .await
        .unwrap();
    attempt
}

/// `finished(service, id)` (task-integration.test.ts:24): waits until the integration is integrated or blocked.
async fn finished(h: &Fixture, service: &TaskIntegrations, id: &str) -> TaskIntegration {
    eventually("the integration to finish", || {
        let record = service.get(&h.db.lock().unwrap(), id).unwrap();
        matches!(record.state, IntegrationState::Integrated | IntegrationState::Blocked)
            .then_some(record)
    })
    .await
}

/// `service.start({ attemptId, expectedRevision, sourceSha, expectedTargetSha, requestId })`.
fn integrate(attempt: &TaskAttempt, expected_target: &str, request: &str) -> TasksIntegrate {
    TasksIntegrate {
        attempt_id: attempt.id.clone(),
        expected_revision: attempt.revision,
        source_sha: attempt.head_sha.clone().unwrap(),
        expected_target_sha: expected_target.into(),
        request_id: request.into(),
    }
}

#[tokio::test]
async fn refuses_to_promote_a_passing_prefix_after_a_later_combined_check_fails_to_return() {
    let h = integration_fixture();
    h.git(&h.repo_path, &["branch", "delivery", h.base.as_str()]);
    let second = ProjectCheck {
        id: "second".into(),
        ..h.project.input.checks[0].clone()
    };
    h.projects
        .update(
            &h.db.lock().unwrap(),
            &h.project.id,
            1,
            ProjectPatch {
                target_ref: Some("refs/heads/delivery".into()),
                checks: Some(vec![h.project.input.checks[0].clone(), second]),
                ..ProjectPatch::default()
            },
        )
        .unwrap();
    let attempt = accepted(&h, 0).await;
    let service = integrations(&h, Arc::new(FailsSecondCheck::default()));
    let record = service
        .start(&integrate(&attempt, &h.base, "partial-checks"))
        .unwrap();
    let blocked = finished(&h, &service, &record.id).await;
    assert_eq!(blocked.checks.len(), 1);
    assert_eq!(blocked.checks[0].exit_code, Some(0));
    assert!(!blocked.checks_completed);
    service.recover(&h.db.lock().unwrap()).unwrap();
    let refused = service
        .promote(&h.db.lock().unwrap(), &record.id, &h.base)
        .unwrap_err();
    assert!(refused.message.contains("verified candidate"));
    assert_eq!(h.git(&h.repo_path, &["rev-parse", "delivery"]), h.base);
    close(&h, &service).await;
}

#[tokio::test]
async fn integrates_two_exact_accepted_tasks_with_combined_checks_while_preserving_a_dirty_user_checkout() {
    let h = integration_fixture();
    h.git(&h.repo_path, &["branch", "delivery", h.base.as_str()]);
    h.projects
        .update(
            &h.db.lock().unwrap(),
            &h.project.id,
            1,
            ProjectPatch {
                target_ref: Some("refs/heads/delivery".into()),
                ..ProjectPatch::default()
            },
        )
        .unwrap();
    let staged = h.repo_path.join("src").join("b.txt");
    fs::write(&staged, "owner staged\n").unwrap();
    h.git(&h.repo_path, &["add", "src/b.txt"]);
    fs::write(&staged, "owner staged and unstaged\n").unwrap();
    fs::write(h.repo_path.join("owner-untracked"), "retain\n").unwrap();
    let before = h.owner();
    let first = accepted(&h, 0).await;
    let second = accepted(&h, 1).await;
    let service = integrations(&h, Arc::new(SyntheticPass));
    let a = service
        .start(&integrate(&first, &h.base, "integrate-a"))
        .unwrap();
    let integrated_a = finished(&h, &service, &a.id).await;
    assert!(integrated_a.reason.is_none());
    assert_eq!(integrated_a.state, IntegrationState::Integrated);
    let a_candidate = integrated_a.candidate_sha.clone().unwrap();
    let b = service
        .start(&integrate(&second, &a_candidate, "integrate-b"))
        .unwrap();
    let integrated_b = finished(&h, &service, &b.id).await;
    assert!(integrated_b.reason.is_none());
    assert_eq!(integrated_b.state, IntegrationState::Integrated);
    let candidate = integrated_b.candidate_sha.clone().unwrap();
    assert_eq!(h.git(&h.repo_path, &["rev-parse", "delivery"]), candidate);
    let first_head = first.head_sha.clone().unwrap();
    assert_eq!(
        h.git(
            &h.repo_path,
            &["merge-base", "--is-ancestor", first_head.as_str(), candidate.as_str()]
        ),
        ""
    );
    let second_head = second.head_sha.clone().unwrap();
    assert_eq!(
        h.git(
            &h.repo_path,
            &["merge-base", "--is-ancestor", second_head.as_str(), candidate.as_str()]
        ),
        ""
    );
    assert_eq!(h.owner(), before);
    close(&h, &service).await;
}

#[tokio::test]
async fn retains_a_verified_candidate_when_the_user_has_the_target_checked_out_and_rejects_stale_source_promotion()
{
    let h = integration_fixture();
    let attempt = accepted(&h, 0).await;
    let before = h.owner();
    let service = integrations(&h, Arc::new(SyntheticPass));
    let record = service
        .start(&integrate(&attempt, &h.base, "integrate"))
        .unwrap();
    let blocked = finished(&h, &service, &record.id).await;
    assert_eq!(blocked.reason.as_deref(), Some("target_checked_out"));
    assert!(
        blocked
            .candidate_sha
            .as_deref()
            .is_some_and(|sha| !sha.is_empty())
    );
    assert_eq!(h.owner(), before);
    fs::write(
        Path::new(&attempt.worktree_path).join("src").join("a.txt"),
        "later source edit\n",
    )
    .unwrap();
    let stale = service
        .promote(&h.db.lock().unwrap(), &record.id, &h.base)
        .unwrap_err();
    assert!(Regex::new("changed|dirty").unwrap().is_match(&stale.message));
    assert!(Path::new(&blocked.worktree_path).exists());
    close(&h, &service).await;
}

#[tokio::test]
async fn blocks_failed_combined_checks_and_target_head_drift_without_promoting_or_discarding_candidates()
{
    let h = integration_fixture();
    let attempt = accepted(&h, 0).await;
    let service = integrations(&h, Arc::new(FailingChecks));
    let failed = service
        .start(&integrate(&attempt, &h.base, "failed"))
        .unwrap();
    let result = finished(&h, &service, &failed.id).await;
    assert_eq!(result.reason.as_deref(), Some("combined_checks_failed"));
    assert!(Path::new(&result.worktree_path).exists());
    assert_eq!(h.git(&h.repo_path, &["rev-parse", "main"]), h.base);
    let attempt_head = attempt.head_sha.clone().unwrap();
    let drift = service
        .start(&integrate(&attempt, &attempt_head, "drift"))
        .unwrap();
    let drifted = finished(&h, &service, &drift.id).await;
    assert_eq!(drifted.reason.as_deref(), Some("target_head_moved"));
    close(&h, &service).await;
}

#[tokio::test]
async fn stops_a_queued_integration_during_shutdown_before_running_configured_project_code() {
    let h = integration_fixture();
    let attempt = accepted(&h, 0).await;
    let counting = Arc::new(CountingChecks::default());
    let service = integrations(&h, counting.clone());
    let record = service
        .start(&integrate(&attempt, &h.base, "close"))
        .unwrap();
    within(service.shutdown()).await.unwrap();
    let stored = service.get(&h.db.lock().unwrap(), &record.id).unwrap();
    assert!(
        stored
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("stopped before preparation"))
    );
    assert_eq!(counting.runs.load(Ordering::SeqCst), 0);
    assert!(!Path::new(&record.worktree_path).exists());
    within(h.tasks.shutdown()).await.unwrap();
}
