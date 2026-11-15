//! The real `tjommie-daemon.exe`, built with the `provider-fixtures` feature, started the way a desktop or a developer
//! starts it: `TJOMMIE_PROVIDER_FIXTURES` decides whether it plays recorded conversations. With a good folder it says so
//! and reads as installed and signed in; with a bad value it refuses to start before it touches anything; unset, it
//! is the normal daemon with the real providers (here with an empty PATH, so none can be found, let alone started).
#![cfg(feature = "provider-fixtures")]

mod common;

use std::path::Path;

use common::{LISTENING, TempRoot, connect, launch};
use serde_json::json;

const VARIABLE: &str = "TJOMMIE_PROVIDER_FIXTURES";

fn shipped() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/provider")
        .to_string_lossy()
        .into_owned()
}

/// What the dev daemon wrote as its token, once it listens.
fn token(root: &Path) -> String {
    std::fs::read_to_string(root.join("daemon-token"))
        .unwrap()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn refuses_to_start_unless_the_variable_is_an_absolute_existing_folder() {
    let scratch = TempRoot::new();
    let file = scratch.0.join("a-file");
    std::fs::write(&file, "{}").unwrap();
    let missing = scratch.0.join("missing");
    let cases = [
        (String::new(), "is set but empty"),
        ("fixtures".to_owned(), "must be an absolute path"),
        (
            missing.to_string_lossy().into_owned(),
            "must name an existing folder",
        ),
        (
            file.to_string_lossy().into_owned(),
            "must name an existing folder",
        ),
    ];
    for (value, message) in cases {
        let root = TempRoot::new();
        let home = root.0.clone();
        let mut daemon = launch(&["--dev"], root, &[(VARIABLE, &value)]);
        let status = daemon.exited().await;
        assert_eq!(status.code(), Some(1), "{value:?}");
        let stderr = daemon.stderr();
        assert!(stderr.contains(message), "{value:?}: {stderr}");
        assert!(stderr.contains(VARIABLE), "{value:?}: {stderr}");
        assert!(!daemon.stdout().contains(LISTENING), "{value:?}");
        assert!(
            !stderr.contains("provider fixtures ON"),
            "{value:?}: refused values never turn it on"
        );
        // Before any port or data: the data root was left as it was found, empty.
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "{value:?}");
    }
}

#[tokio::test]
async fn a_good_folder_turns_fixtures_on_says_so_and_needs_no_setup() {
    let root = TempRoot::new();
    let home = root.0.clone();
    let dir = shipped();
    let daemon = launch(&["--dev"], root, &[(VARIABLE, &dir)]);
    let port = daemon.ready().await;

    let stderr = daemon.stderr_raw();
    let line =
        format!("Tjommie daemon: provider fixtures ON ({dir}): no real provider will be started.");
    assert_eq!(
        stderr.lines().filter(|text| *text == line).count(),
        1,
        "the one startup line, once: {stderr}"
    );
    assert!(
        !stderr.contains("fixtures@example.com") && !stderr.contains("Hello from the fixture"),
        "fixture contents are never logged"
    );

    let mut client = connect(port).await;
    client.hello(&token(&home)).await;
    let setup = client.call(1, "setup.status", json!({})).await;
    for entry in setup["result"].as_array().unwrap() {
        assert_eq!(entry["installed"], true, "{entry}");
        assert_eq!(entry["signedIn"], true, "{entry}");
    }
    let bot = client
        .call(
            2,
            "bots.create",
            json!({ "name": "Ada", "color": "blue", "shape": "circle", "provider": "claude", "model": "sonnet",
                "instructions": "", "autonomy": "ask", "browser": false }),
        )
        .await;
    assert_eq!(bot["result"]["state"], "idle", "{bot}");
    client.close().await;
}

#[tokio::test]
async fn without_the_variable_the_real_providers_are_used() {
    // The feature is compiled in, the variable is unset: the normal daemon, whose providers are found on PATH. PATH is an
    // empty folder, so none is found (and none can be started).
    let nowhere = TempRoot::new();
    let root = TempRoot::new();
    let home = root.0.clone();
    let daemon = launch(&["--dev"], root, &[("PATH", &nowhere.0.to_string_lossy())]);
    let port = daemon.ready().await;
    assert!(
        !daemon.stderr_raw().contains("provider fixtures"),
        "{}",
        daemon.stderr_raw()
    );

    let mut client = connect(port).await;
    client.hello(&token(&home)).await;
    let setup = client.call(1, "setup.status", json!({})).await;
    for entry in setup["result"].as_array().unwrap() {
        assert_eq!(entry["installed"], false, "{entry}");
        assert_eq!(entry["signedIn"], false, "{entry}");
    }
    client.close().await;
}
