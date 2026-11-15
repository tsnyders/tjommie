//! `tjommie-daemon accept-worker` proves on this PC that both provider CLIs start under the worker accounts, so it starts
//! the real Claude and Codex. While `TJOMMIE_PROVIDER_FIXTURES` is set the run is meant to start none, so the subcommand
//! refuses, in a build with the `provider-fixtures` feature and in one without (a variable set for a run that doesn't
//! know it is ignored must never be what lets a real provider start).
//!
//! The runs here pass an extra argument, `--probe`. The refusal comes first; without it the daemon answers with its usage
//! text, so a daemon that lost the refusal still starts nothing in these tests.

mod common;

use common::{TempRoot, launch};

const VARIABLE: &str = "TJOMMIE_PROVIDER_FIXTURES";

const REFUSAL: &str = "TJOMMIE_PROVIDER_FIXTURES is set, and accept-worker starts the real Claude and Codex. Unset it first.";

const USAGE: &str =
    "Usage: tjommie-daemon accept-worker (as the Tjommie owner, not elevated, with Tjommie closed)";

#[tokio::test]
async fn accept_worker_refuses_to_run_while_the_fixtures_variable_is_set() {
    for value in ["C:\\fixtures-folder-9c1d", "relative-folder-9c1d", " "] {
        let root = TempRoot::new();
        let home = root.0.clone();
        let mut daemon = launch(&["accept-worker", "--probe"], root, &[(VARIABLE, value)]);
        let status = daemon.exited().await;
        assert_eq!(status.code(), Some(1), "{value:?}");
        let (stdout, stderr) = (daemon.stdout(), daemon.stderr());
        assert_eq!(stderr.trim(), REFUSAL, "{value:?}");
        assert!(
            value.trim().is_empty() || !stderr.contains(value.trim()),
            "the value is never echoed: {stderr}"
        );
        assert!(
            !stdout.contains("Checking the Windows worker accounts"),
            "{stdout}"
        );
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "{value:?}");
    }
}

#[tokio::test]
async fn without_the_variable_accept_worker_gets_as_far_as_its_own_checks() {
    // The control: an unset or empty variable changes nothing, so the same runs get the daemon's own answer.
    for variable in [None, Some("")] {
        let root = TempRoot::new();
        let env: Vec<(&str, &str)> = variable
            .map(|value| (VARIABLE, value))
            .into_iter()
            .collect();
        let mut daemon = launch(&["accept-worker", "--probe"], root, &env);
        let status = daemon.exited().await;
        assert_eq!(status.code(), Some(1), "{variable:?}");
        assert_eq!(daemon.stderr().trim(), USAGE, "{variable:?}");
    }
}
