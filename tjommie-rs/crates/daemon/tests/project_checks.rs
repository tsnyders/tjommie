//! Port of `packages/daemon/src/project-checks.test.ts`. The worker-launch check uses a fake host that fails the way
//! the worker boundary fails. The two real-process checks need Core's `SystemCli` and are in `project_checks_host.rs`.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use daemon::cli::{BoxFuture, CliChild, CliCommand, CliHost, LaunchOptions};
use daemon::project_checks::{CheckError, CheckRuntime, HostCheckRuntime};
use daemon::win::boundary::BoundaryError;
use protocol::projects::ProjectCheck;
use tokio_util::sync::CancellationToken;

const LAUNCH_TERMINATED: &str =
    "Could not confirm that the owned check launch stopped. Its worktree remains reserved.";

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

/// A check that is already cancelled never reaches the host (project-checks.ts:25).
#[tokio::test]
async fn never_launches_an_already_cancelled_check() {
    let host = Arc::new(FailingHost {
        error: BoundaryError::Other("must not be asked".into()),
        launches: AtomicUsize::new(0),
    });
    let runtime = HostCheckRuntime::new(host.clone());
    let cancelled = CancellationToken::new();
    cancelled.cancel();

    let error = runtime
        .run(&synthetic_check("cmd", &["/C", "exit"]), ".", &cancelled)
        .await
        .unwrap_err();

    assert_eq!(error, CheckError::Failed("Check cancelled.".into()));
    assert_eq!(host.launches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn never_falls_back_to_owner_execution_when_the_worker_launch_fails() {
    // Both flavours of the TypeScript spawn mock: a termination that can't be confirmed, and an ordinary refusal.
    // The runtime holds only the host it was given, so "no owner fallback" is: exactly one launch, then the error.
    // The TypeScript marker file is not ported: a fake host runs nothing, so it could never appear. That a refused
    // worker launch never starts an owner process is the boundary's own test (`win::boundary`).
    for unknown in [false, true] {
        let root = tempfile::tempdir().unwrap();
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
    }
}
