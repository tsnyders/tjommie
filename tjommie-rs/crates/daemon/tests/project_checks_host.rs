//! The two real-process tests of `project-checks.test.ts`: they run PowerShell through Core's real `SystemCli`.

use std::sync::Arc;

use daemon::cli::SystemCli;
use daemon::project_checks::{CheckError, CheckRuntime, HostCheckRuntime};
use protocol::projects::ProjectCheck;
use tokio_util::sync::CancellationToken;

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

#[tokio::test]
async fn runs_an_explicit_argument_array_with_a_scrubbed_environment_and_bounded_output() {
    let root = tempfile::tempdir().unwrap();
    // The TypeScript check is `node -e` printing 300,000 "x" and exiting 3 if a secret leaked. Here PowerShell does
    // the same. PowerShell starts more slowly than node, so the timeout is 10 s instead of 2 s. The sentinel is
    // set in this process first: the scrub is only shown if the child still doesn't see it.
    // SAFETY: edition 2024; this test file has one test that touches the environment, and nothing else reads it.
    unsafe {
        std::env::set_var(
            "ANTHROPIC_API_KEY",
            "synthetic-secret-must-not-reach-a-check",
        )
    };
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
