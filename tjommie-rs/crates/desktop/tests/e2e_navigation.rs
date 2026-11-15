//! The shell's navigation through the test driver (Phase 3 plan T2.1). Run with the e2e identity:
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_navigation --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

const SECTIONS: [&str; 5] = ["agents", "rooms", "projects", "routines", "settings"];

/// The sidebar's sections the snapshot marks selected.
fn selected(app: &mut Client) -> Vec<&'static str> {
    let snapshot = app.snapshot().unwrap();
    SECTIONS
        .into_iter()
        .filter(|key| {
            snapshot
                .node(&format!("sidebar.nav.{key}"))
                .is_some_and(|node| node.selected)
        })
        .collect()
}

#[test]
fn e2e_navigation() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    let root = owner_safety::test_root().unwrap();
    let mut app = Client::launch(env!("CARGO_BIN_EXE_tjommie-desktop"), root.path()).unwrap();

    // The app opens on Agents. No store is wired to a daemon yet (T1.4), so it is connecting: the connection screen
    // fills the main column, and the sidebar still works.
    app.wait("sidebar.nav.agents", Predicate::Selected, 30_000)
        .unwrap();
    let snapshot = app.snapshot().unwrap();
    let title = snapshot
        .node("connection.title")
        .expect("the connection screen");
    assert_eq!(
        (title.role.as_str(), title.text.as_str()),
        ("Heading", "Starting Tjommie")
    );
    let agents = snapshot.node("sidebar.nav.agents").unwrap();
    assert_eq!(
        (agents.role.as_str(), agents.text.as_str()),
        ("Link", "Agents")
    );

    // A section, clicked, is the one selected.
    for key in ["rooms", "projects", "routines", "settings", "agents"] {
        let id = format!("sidebar.nav.{key}");
        app.click(&id).unwrap();
        app.wait(&id, Predicate::Selected, 10_000).unwrap();
        assert_eq!(selected(&mut app), [key]);
    }

    // The foot: the host button on This PC, and New agent, which opens a screen no section owns.
    let snapshot = app.snapshot().unwrap();
    assert_eq!(
        snapshot.node("sidebar.host").unwrap().text,
        "Switch PC, now on This PC"
    );
    assert_eq!(snapshot.node("sidebar.new").unwrap().text, "New agent");
    app.click("sidebar.new").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !selected(&mut app).is_empty() {
        assert!(Instant::now() < deadline, "a section stayed selected");
        std::thread::sleep(Duration::from_millis(25));
    }
}
