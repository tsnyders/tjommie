//! The live connection through the test driver (Phase 3 plan M5): the app starts its own daemon on its temp root and
//! connects, so the connection screen gives way to Agents. Needs the debug daemon beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_live --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

/// The debug daemon the app starts: beside the app in the same target folder.
pub fn daemon_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tjommie-desktop")).with_file_name("tjommie-daemon.exe")
}

#[test]
fn e2e_live_connects_to_its_own_daemon() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    assert!(
        daemon_exe().exists(),
        "build the daemon first: cargo build -p daemon --bin tjommie-daemon --locked"
    );
    let root = owner_safety::test_root().unwrap();
    let mut app = Client::launch(env!("CARGO_BIN_EXE_tjommie-desktop"), root.path()).unwrap();

    // Connected: the route's screen shows, and the connection screen is gone.
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
    let snapshot = app.snapshot().unwrap();
    assert!(
        snapshot.node("connection.title").is_none(),
        "still on the connection screen: {:?}",
        snapshot.node("connection.title")
    );
    // The daemon wrote into the temp root, and only there.
    assert!(root.path().join("data").exists() || root.path().join("tjommie.db").exists());
}

#[test]
fn e2e_live_without_a_daemon_still_opens_and_says_why() {
    assert_eq!(identity::CURRENT.name, "e2e");
    let root = owner_safety::test_root().unwrap();
    let missing = root.path().join("no-daemon-here.exe");
    let mut app = Client::launch_with(
        env!("CARGO_BIN_EXE_tjommie-desktop"),
        root.path(),
        &[("TJOMMIE_DAEMON_EXE", missing.to_str().unwrap())],
    )
    .unwrap();

    // The window opens on the connection screen, which says the service didn't start, and why, with Try again.
    app.wait(
        "connection.title",
        Predicate::Text("Tjommie's background service didn't start".into()),
        30_000,
    )
    .unwrap();
    let snapshot = app.snapshot().unwrap();
    assert!(snapshot.node("connection.retry").is_some(), "Try again");
    assert!(snapshot.node("sidebar.nav.agents").is_some(), "the sidebar");
    let said: Vec<&str> = snapshot
        .nodes
        .iter()
        .map(|node| node.text.as_str())
        .collect();
    assert!(
        said.iter()
            .any(|text| text.contains("names a daemon that is not there")),
        "the reason: {said:?}"
    );

    // Try again starts the service again from the UI thread (M5 review C1). It fails the same way and says so, with no
    // crash notice and no endless "Starting Tjommie".
    app.click("connection.retry").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3));
    app.wait(
        "connection.title",
        Predicate::Text("Tjommie's background service didn't start".into()),
        30_000,
    )
    .unwrap();
    let after = app.snapshot().unwrap();
    assert!(after.node("connection.retry").is_some(), "Try again, again");
    assert!(
        after
            .nodes
            .iter()
            .all(|node| !node.text.contains("Something went wrong")),
        "no crash notice"
    );
}
