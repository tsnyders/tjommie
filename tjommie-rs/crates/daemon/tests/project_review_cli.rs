//! Port of `packages/daemon/src/project-review-cli.test.ts`. Fake children stand in for the reviewer CLI: no Claude
//! process runs, and no account or ACL is touched.

mod providers_fakes;

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

use daemon::cli::{BoxFuture, CliChild, CliCommand, CliExit, CliHost, LaunchOptions, Retirement};
use daemon::project_checks::CheckError;
use daemon::project_review_cli::{ReviewCliOptions, run_review_cli};
use daemon::providers::ProviderEvent;
use daemon::win::boundary::BoundaryError;
use futures_util::FutureExt;
use providers_fakes::{command, fake_child, temp_root};
use regex::Regex;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

/// Answers the one launch with a prepared outcome, as `vi.mocked(launchCli)` does in the TypeScript test.
struct ScriptedLaunch(Mutex<Option<io::Result<CliChild>>>);

impl CliHost for ScriptedLaunch {
    fn launch<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        let outcome = self.0.lock().unwrap().take().expect("one scripted launch");
        Box::pin(async move { outcome })
    }
}

fn scripted(outcome: io::Result<CliChild>) -> Arc<dyn CliHost> {
    Arc::new(ScriptedLaunch(Mutex::new(Some(outcome))))
}

fn cwd(root: &TempDir) -> String {
    root.path().to_string_lossy().into_owned()
}

/// `options(events)` in the TypeScript test: the same command, input and model, with no key.
fn options<'a>(
    host: Arc<dyn CliHost>,
    cwd: &str,
    on_event: &'a (dyn Fn(ProviderEvent) -> Result<(), String> + Send + Sync),
) -> ReviewCliOptions<'a> {
    ReviewCliOptions {
        host,
        command: command("synthetic-claude"),
        args: vec![],
        cwd: cwd.into(),
        env: BTreeMap::new(),
        input: "owned fixture".into(),
        model: "test".into(),
        signal: CancellationToken::new(),
        api_key: None,
        on_event,
        timeout_ms: None,
    }
}

#[tokio::test]
async fn records_unknown_spending_and_keeps_the_lease_when_a_paid_launch_cannot_prove_retirement() {
    let root = temp_root();
    let events = Mutex::new(Vec::new());
    let record = |event: ProviderEvent| -> Result<(), String> {
        events.lock().unwrap().push(event);
        Ok(())
    };
    let launch = scripted(Err(io::Error::other(BoundaryError::Termination)));
    match run_review_cli(options(launch, &cwd(&root), &record)).await {
        Err(CheckError::Termination(message)) => assert_eq!(
            message,
            "Reviewer launch retirement is unconfirmed; its account and scratch profile are retained."
        ),
        Err(other) => panic!("expected a termination, got: {other}"),
        Ok(_) => panic!("expected a termination, got a result"),
    }
    let recorded = events.into_inner().unwrap();
    match recorded.as_slice() {
        [ProviderEvent::Usage(usage)] => {
            assert_eq!(usage.model, "test");
            assert_eq!(usage.cost_usd, None);
            let id = usage.event_id.as_deref().unwrap_or_default();
            let shape = Regex::new(r"^[a-f0-9-]+:unknown-launch$").unwrap();
            assert!(shape.is_match(id), "{id}");
        }
        other => panic!("expected one unknown usage event, got {other:?}"),
    }
}

#[tokio::test]
async fn does_not_invent_a_charge_when_the_launcher_proves_the_child_never_started() {
    let root = temp_root();
    let events = Mutex::new(Vec::new());
    let record = |event: ProviderEvent| -> Result<(), String> {
        events.lock().unwrap().push(event);
        Ok(())
    };
    let launch = scripted(Err(io::Error::other("Missing synthetic executable")));
    match run_review_cli(options(launch, &cwd(&root), &record)).await {
        Err(CheckError::Failed(message)) => assert_eq!(message, "Missing synthetic executable"),
        Err(other) => panic!("expected the launch error, got: {other}"),
        Ok(_) => panic!("expected the launch error, got a result"),
    }
    assert!(events.into_inner().unwrap().is_empty());
}

#[tokio::test]
async fn waits_for_worker_job_proof_and_records_unknown_spending_without_final_usage() {
    let root = temp_root();
    let events = Mutex::new(Vec::new());
    let record = |event: ProviderEvent| -> Result<(), String> {
        events.lock().unwrap().push(event);
        Ok(())
    };
    // The child printed nothing and has exited, but its job could not be shown to have stopped. As `win::boundary`
    // builds a worker child, the exit says so and `retirement` carries the proof, which is what the reviewer waits on.
    let (mut child, fake) = fake_child(4242);
    drop(fake.stdout);
    drop(fake.stderr);
    fake.exit
        .fire(CliExit::Failed(BoundaryError::Termination.to_string()));
    child.retirement = Some(unconfirmed_retirement());
    let launch = scripted(Ok(child));
    match run_review_cli(options(launch, &cwd(&root), &record)).await {
        Err(CheckError::Termination(message)) => assert_eq!(
            message,
            "Reviewer job retirement is unconfirmed; its account and scratch profile are retained."
        ),
        Err(other) => panic!("expected a termination, got: {other}"),
        Ok(_) => panic!("expected a termination, got a result"),
    }
    let recorded = events.into_inner().unwrap();
    match recorded.as_slice() {
        [ProviderEvent::Usage(usage)] => assert_eq!(usage.cost_usd, None),
        other => panic!("expected one unknown usage event, got {other:?}"),
    }
}

/// A worker child whose tree could not be shown to have stopped (`workerProcessRetired` rejecting).
fn unconfirmed_retirement() -> Retirement {
    async { Err(BoundaryError::Termination.to_string()) }
        .boxed()
        .shared()
}

/// The exit is clean, but the retirement proof is not: `waitForCliExit` throws, so the account stays reserved.
#[tokio::test]
async fn retains_the_account_when_the_exit_is_clean_but_the_retirement_proof_fails() {
    let root = temp_root();
    let record = |_: ProviderEvent| -> Result<(), String> { Ok(()) };
    let (mut child, fake) = fake_child(4242);
    drop(fake.stdout);
    drop(fake.stderr);
    fake.exit.fire(CliExit::Exited {
        code: Some(0),
        signal: None,
    });
    child.retirement = Some(unconfirmed_retirement());
    let launch = scripted(Ok(child));
    match run_review_cli(options(launch, &cwd(&root), &record)).await {
        Err(CheckError::Termination(message)) => assert_eq!(
            message,
            "Reviewer job retirement is unconfirmed; its account and scratch profile are retained."
        ),
        Err(other) => panic!("expected a termination, got: {other}"),
        Ok(_) => panic!("expected a termination, got a result"),
    }
}

/// project-review-cli.ts:64: a child `error` event rejects with a plain `Error`, which retains nothing.
#[tokio::test]
async fn reports_a_child_that_failed_to_start_as_a_plain_failure_not_a_retained_termination() {
    let root = temp_root();
    let record = |_: ProviderEvent| -> Result<(), String> { Ok(()) };
    let (child, fake) = fake_child(4242);
    drop(fake.stdout);
    drop(fake.stderr);
    fake.exit.fire(CliExit::Failed("spawn ENOENT".into()));
    let launch = scripted(Ok(child));
    match run_review_cli(options(launch, &cwd(&root), &record)).await {
        Err(CheckError::Failed(message)) => assert_eq!(message, "review_provider_start_failed"),
        Err(other) => panic!("expected a plain failure, got: {other}"),
        Ok(_) => panic!("expected a plain failure, got a result"),
    }
}

#[cfg(windows)]
#[tokio::test]
async fn does_not_reuse_owner_policy_evidence_for_a_different_worker_sid() {
    use daemon::project_review_policy::{WorkerPolicyObserver, review_machine_policy_hash};
    use daemon::win::boundary::PolicyReceipt;
    use std::path::PathBuf;

    /// No worker has been observed, but a worker accounts folder exists.
    struct AccountsWithoutObserver;

    impl WorkerPolicyObserver for AccountsWithoutObserver {
        fn observe<'a>(&'a self) -> BoxFuture<'a, Result<Option<PolicyReceipt>, String>> {
            Box::pin(async { Ok(None) })
        }

        fn accounts_root(&self) -> Option<PathBuf> {
            Some(PathBuf::from(r"C:\synthetic-worker\accounts"))
        }
    }

    let accounts = AccountsWithoutObserver;
    let observer: &dyn WorkerPolicyObserver = &accounts;
    match review_machine_policy_hash(Some(observer)).await {
        Err(message) => assert_eq!(message, "review_worker_policy_observer_unavailable"),
        Ok(hash) => panic!("expected the observer to be unavailable, got {hash}"),
    }
}
