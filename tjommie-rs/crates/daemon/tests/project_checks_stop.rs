//! Port of `packages/daemon/src/project-checks-stop.test.ts`. The owned check's stop is refused and the check never
//! closes: the uncertain stop must be reported promptly, not after the check's own timeout.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::cli::{
    BoxFuture, ChildExit, CliChild, CliCommand, CliExit, CliHost, KillTree, LaunchOptions,
};
use daemon::project_checks::{CheckError, CheckRuntime, HostCheckRuntime};
use daemon::win::boundary::BoundaryError;
use protocol::projects::ProjectCheck;
use tokio::io::{AsyncBufReadExt, BufReader, DuplexStream, duplex};
use tokio_util::sync::CancellationToken;

/// A child whose stop is refused, and which never exits or closes its output. The test keeps the stdin reader (to see
/// the end of the check's input) and the output ends, so nothing closes by accident.
struct RefusingChild {
    child: CliChild,
    stdin: BufReader<DuplexStream>,
    _stdout: DuplexStream,
    _stderr: DuplexStream,
}

fn refusing_child(pid: u32) -> RefusingChild {
    let (stdin, stdin_test) = duplex(1 << 20);
    let (stdout, stdout_test) = duplex(1 << 20);
    let (stderr, stderr_test) = duplex(1 << 20);
    let kill_tree: KillTree = Arc::new(|| {
        Box::pin(async { Err("Synthetic termination refusal".to_string()) })
    });
    RefusingChild {
        child: CliChild {
            pid: Some(pid),
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: Box::new(stderr),
            exit: ChildExit::new(),
            kill_tree,
        },
        stdin: BufReader::new(stdin_test),
        _stdout: stdout_test,
        _stderr: stderr_test,
    }
}

/// Hands out one child, as `launchCli` resolving once does in the TypeScript test.
struct OneChild(Mutex<Option<CliChild>>);

impl CliHost for OneChild {
    fn launch<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        let child = self.0.lock().unwrap().take();
        Box::pin(async move { child.ok_or_else(|| io::Error::other("no synthetic child left")) })
    }
}

fn synthetic_check() -> ProjectCheck {
    ProjectCheck {
        id: "check".into(),
        label: "Synthetic check".into(),
        command: "synthetic-no-executable".into(),
        args: Vec::new(),
        timeout_ms: 60_000,
    }
}

/// project-checks.ts:34: a child `error` event rejects with a plain `Error`, which is not a termination. Only the
/// worker boundary's unconfirmed retirement is, and it carries its own text.
#[tokio::test]
async fn reports_a_child_that_failed_to_start_as_a_plain_failure_not_a_termination() {
    // The test ends of the three pipes drop at once, so the child's output closes, as a child that never ran would.
    let RefusingChild { child, .. } = refusing_child(4242421);
    child.exit.fire(CliExit::Failed("spawn ENOENT".into()));
    let runtime = HostCheckRuntime::new(Arc::new(OneChild(Mutex::new(Some(child)))));
    let controller = CancellationToken::new();

    let error = tokio::time::timeout(
        Duration::from_millis(500),
        runtime.run(&synthetic_check(), ".", &controller),
    )
    .await
    .expect("a failed child settles at once")
    .unwrap_err();

    assert_eq!(
        error,
        CheckError::Failed("The configured check could not start.".into())
    );
}

/// The same `Failed` event with the worker boundary's own text is the unconfirmed retirement.
#[tokio::test]
async fn reports_a_worker_that_could_not_confirm_its_retirement_as_a_termination() {
    let RefusingChild { child, .. } = refusing_child(4242422);
    child.exit.fire(CliExit::Failed(BoundaryError::Termination.to_string()));
    let runtime = HostCheckRuntime::new(Arc::new(OneChild(Mutex::new(Some(child)))));
    let controller = CancellationToken::new();

    let error = tokio::time::timeout(
        Duration::from_millis(500),
        runtime.run(&synthetic_check(), ".", &controller),
    )
    .await
    .expect("a failed child settles at once")
    .unwrap_err();

    assert!(matches!(error, CheckError::Termination(_)));
}

#[tokio::test]
async fn reports_uncertain_stop_promptly_even_when_the_owned_check_never_closes() {
    let RefusingChild {
        child,
        mut stdin,
        _stdout,
        _stderr,
    } = refusing_child(4242420);
    let host = Arc::new(OneChild(Mutex::new(Some(child))));
    let runtime = HostCheckRuntime::new(host);
    let check = synthetic_check();
    let controller = CancellationToken::new();
    let run = runtime.run(&check, ".", &controller);
    let cancel_after_stdin_ends = async {
        // The runtime ends the check's stdin first (the TypeScript test waits for `writableEnded`).
        let mut line = String::new();
        let read = stdin.read_line(&mut line).await.unwrap();
        assert_eq!(read, 0);
        controller.cancel();
    };

    let (result, ()) = tokio::join!(
        tokio::time::timeout(Duration::from_millis(500), run),
        cancel_after_stdin_ends,
    );

    let error = result
        .expect("the uncertain termination is reported within 500 ms")
        .unwrap_err();
    assert!(matches!(error, CheckError::Termination(_)));
    assert_eq!(
        error,
        CheckError::Termination(
            "Could not confirm that the owned check process tree stopped.".into()
        )
    );
}
