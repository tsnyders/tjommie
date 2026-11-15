//! `tjommie-daemon accept-worker` proves on this PC that both provider CLIs start under the worker accounts, so it starts
//! the real Claude and Codex. While `TJOMMIE_PROVIDER_FIXTURES` is set (empty included, as everywhere else) the run is
//! meant to start none, so the subcommand refuses, in a build with the `provider-fixtures` feature and in one without (a
//! variable set for a run that doesn't know it is ignored must never be what lets a real provider start). The refusal is
//! in the library function that implements the command, so a caller other than `main.rs` is covered too.
//!
//! The runs of the exe pass an extra argument, `--probe`. The refusal comes first; without it the daemon answers with its
//! usage text, so a daemon that lost the refusal still starts nothing in these tests. The library calls use an empty
//! temporary data root: past the refusal, the first thing the function does is look for the worker configuration there,
//! find none and stop, before any Windows call or CLI.

mod common;

use std::ffi::OsString;

use common::{TempRoot, launch};
use daemon::win::acceptance::accept_worker_in;

const VARIABLE: &str = "TJOMMIE_PROVIDER_FIXTURES";

const REFUSAL: &str = "TJOMMIE_PROVIDER_FIXTURES is set, and accept-worker starts the real Claude and Codex. Unset it first.";

const USAGE: &str =
    "Usage: tjommie-daemon accept-worker (as the Tjommie owner, not elevated, with Tjommie closed)";

#[tokio::test]
async fn accept_worker_refuses_to_run_while_the_fixtures_variable_is_set() {
    for value in ["C:\\fixtures-folder-9c1d", "relative-folder-9c1d", " ", ""] {
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
    // The control: an unset variable changes nothing, so the same run gets the daemon's own answer.
    let root = TempRoot::new();
    let mut daemon = launch(&["accept-worker", "--probe"], root, &[]);
    let status = daemon.exited().await;
    assert_eq!(status.code(), Some(1));
    assert_eq!(daemon.stderr().trim(), USAGE);
}

/// An environment lookup that holds only the fixtures variable, if `value` is given. The process environment is never
/// touched.
fn lookup(value: Option<&'static str>) -> impl Fn(&str) -> Option<OsString> {
    move |name| {
        assert_eq!(name, VARIABLE, "only the fixtures variable is read");
        value.map(OsString::from)
    }
}

#[tokio::test]
async fn the_library_entry_refuses_too_so_any_caller_is_covered() {
    for value in ["C:\\fixtures-folder-9c1d", "relative-folder-9c1d", " ", ""] {
        let root = TempRoot::new();
        let refused = accept_worker_in(&root.0, lookup(Some(value))).await;
        assert_eq!(refused, Err(REFUSAL.to_owned()), "{value:?}");
        // Nothing was started, read or written.
        assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0, "{value:?}");
    }
}

#[tokio::test]
async fn the_library_entry_without_the_variable_gets_as_far_as_its_own_first_check() {
    let root = TempRoot::new();
    let answer = accept_worker_in(&root.0, lookup(None))
        .await
        .expect_err("an empty data root has no worker configuration");
    assert!(
        answer.starts_with("There is no worker configuration in "),
        "{answer}"
    );
}
