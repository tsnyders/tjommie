//! A daemon built WITHOUT the `provider-fixtures` feature must refuse to start when `TJOMMIE_PROVIDER_FIXTURES` is set.
//! Ignoring it would be the one dangerous outcome of the switch: an end-to-end run that believes it is replaying
//! recorded conversations would start the real Claude and Codex, with real accounts and paid turns. This file is
//! compiled and run only without the feature (the feature build's own handling is in `provider_fixtures_startup.rs`).
#![cfg(not(feature = "provider-fixtures"))]

mod common;

use std::path::Path;

use common::{LISTENING, TempRoot, connect, launch};
use serde_json::json;

const VARIABLE: &str = "TJOMMIE_PROVIDER_FIXTURES";

const REFUSAL: &str = "TJOMMIE_PROVIDER_FIXTURES is set, but this daemon was built without the provider-fixtures feature, so it would use the real Claude and Codex. Unset it, or build the daemon with --features provider-fixtures (tests only).";

fn token(root: &Path) -> String {
    std::fs::read_to_string(root.join("daemon-token"))
        .unwrap()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn refuses_to_start_when_the_variable_is_set_and_the_feature_is_not_built() {
    let scratch = TempRoot::new();
    let existing = scratch.0.to_string_lossy().into_owned();
    let missing = scratch
        .0
        .join("no-such-folder-5d27c1")
        .to_string_lossy()
        .into_owned();
    // Whatever it names, a good folder, a bad one or a path that is not even absolute, and a blank that is still
    // something.
    for value in [
        existing,
        missing,
        "relative-folder-5d27c1".to_owned(),
        " ".to_owned(),
    ] {
        let root = TempRoot::new();
        let home = root.0.clone();
        let mut daemon = launch(&["--dev"], root, &[(VARIABLE, &value)]);
        let status = daemon.exited().await;
        assert_eq!(status.code(), Some(1), "{value:?}");
        // The notices that say a test hook is on are filtered out; the refusal is all that is left.
        let stderr = daemon.stderr();
        assert_eq!(stderr.trim(), REFUSAL, "{value:?}");
        assert!(
            value.trim().is_empty() || !stderr.contains(value.trim()),
            "the value is never echoed: {stderr}"
        );
        assert!(!daemon.stdout().contains(LISTENING), "{value:?}");
        // Before any port or data: the data root was left as it was found, empty.
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "{value:?}");
    }
}

#[tokio::test]
async fn an_empty_or_unset_variable_changes_nothing() {
    // PATH is an empty folder, so even a daemon that went wrong could find no provider to start.
    let nowhere = TempRoot::new();
    let path = nowhere.0.to_string_lossy().into_owned();
    for variable in [Some(""), None] {
        let root = TempRoot::new();
        let home = root.0.clone();
        let mut env = vec![("PATH", path.as_str())];
        env.extend(variable.map(|value| (VARIABLE, value)));
        let daemon = launch(&["--dev"], root, &env);
        let port = daemon.ready().await;
        assert!(
            !daemon.stderr().contains(REFUSAL)
                && !daemon.stderr_raw().contains("provider fixtures"),
            "{variable:?}: {}",
            daemon.stderr_raw()
        );
        let mut client = connect(port).await;
        client.hello(&token(&home)).await;
        let listed = client.call(1, "bots.list", json!({})).await;
        assert_eq!(listed["ok"], true, "{variable:?}: {listed}");
        client.close().await;
    }
}
