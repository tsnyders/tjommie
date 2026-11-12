//! Port of `packages/daemon/src/project-checks.test.ts`. The real-process checks go through the real host; the
//! worker-launch check uses a fake host that fails the way the worker boundary fails.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use daemon::cli::{BoxFuture, CliChild, CliCommand, CliHost, LaunchOptions};
// TODO(M5-integration): `SystemCli` is Core's real host (rs-core cli.rs:590, `impl CliHost for SystemCli` at :809).
// It is not in this worktree's cli.rs yet; the merge provides it.
use daemon::cli::SystemCli;
use daemon::project_checks::{CheckError, CheckRuntime, HostCheckRuntime};
use daemon::win::boundary::BoundaryError;
use protocol::projects::ProjectCheck;
use tokio_util::sync::CancellationToken;

const LAUNCH_TERMINATED: &str =
    "Could not confirm that the owned check launch stopped. Its worktree remains reserved.";
const LAUNCH_FAILED_PREFIX: &str = "The configured check could not start.";

/// The synthetic check of the TypeScript tests, with a two-second timeout.
fn synthetic_check(command: &str, args: &[&str]) -> ProjectCheck {
    ProjectCheck {
        id: "check".into(),
        label: "Synthetic check".into(),
        command: command.into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        timeout_ms: 2000,
    }
}

/// A host whose launch fails the way `CliHost for WorkerBoundary` fails: `io::Error::other` of a `BoundaryError`.
struct FailingHost {
    error: BoundaryError,
    launches: AtomicUsize,
}

impl CliHost for FailingHost {
    fn launch<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let error = self.error.clone();
        Box::pin(async move { Err(io::Error::other(error)) })
    }
}

#[tokio::test]
async fn runs_an_explicit_argument_array_with_a_scrubbed_environment_and_bounded_output() {
    let root = tempfile::tempdir().unwrap();
    // The TypeScript check is `node -e` printing 300,000 "x" and exiting 3 if a secret leaked. Here PowerShell does
    // the same. PowerShell starts more slowly than node, so the timeout is 10 s instead of 2 s.
    let script = "if ($env:ANTHROPIC_API_KEY) { exit 3 }; [Console]::Out.Write('x' * 300000)";
    let mut check = synthetic_check(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    );
    check.timeout_ms = 10_000;
    let runtime = HostCheckRuntime::new(Arc::new(SystemCli::new()));
    let cancel = CancellationToken::new();

    let result = runtime
        .run(&check, &root.path().to_string_lossy(), &cancel)
        .await
        .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result.truncated);
    assert!(result.output.len() <= 262_144);
    assert_eq!(result.output, "x".repeat(262_000));
}

#[tokio::test]
async fn never_runs_an_already_cancelled_check_and_reports_a_missing_executable_without_echoing_arguments()
 {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_string_lossy().into_owned();
    let runtime = HostCheckRuntime::new(Arc::new(SystemCli::new()));
    let missing = synthetic_check(
        "tjommie-missing-synthetic-check",
        &["synthetic-private-argument"],
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();

    let error = runtime.run(&missing, &cwd, &cancelled).await.unwrap_err();
    assert!(error.to_string().contains("cancelled"));

    let error = runtime
        .run(&missing, &cwd, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, CheckError::Failed(message) if message.starts_with(LAUNCH_FAILED_PREFIX))
    );
    assert!(!error.to_string().contains("synthetic-private-argument"));
}

#[tokio::test]
async fn never_falls_back_to_owner_execution_when_the_worker_launch_fails() {
    // Both flavours of the TypeScript spawn mock: a termination that can't be confirmed, and an ordinary refusal.
    for unknown in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("owner-process-must-not-write");
        let error = if unknown {
            BoundaryError::Termination
        } else {
            BoundaryError::Other("synthetic worker refusal".into())
        };
        let host = Arc::new(FailingHost {
            error,
            launches: AtomicUsize::new(0),
        });
        let runtime = HostCheckRuntime::new(host.clone());
        // The fake host never runs anything, so the marker can only stay absent; the check is kept for the shape.
        let check = synthetic_check("cmd", &["/C", "echo owner bypass"]);

        let result = runtime
            .run(
                &check,
                &root.path().to_string_lossy(),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();

        if unknown {
            assert_eq!(result, CheckError::Termination(LAUNCH_TERMINATED.into()));
        } else {
            assert!(
                matches!(&result, CheckError::Failed(message) if message.contains("protected worker runtime"))
            );
        }
        assert_eq!(host.launches.load(Ordering::SeqCst), 1);
        assert!(!marker.exists());
    }
}
