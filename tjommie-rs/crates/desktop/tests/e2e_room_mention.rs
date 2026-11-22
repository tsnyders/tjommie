//! A room mention end to end (Phase 3 plan M5, T5.2; parity row 4): make an agent, open Everyone from the rooms list,
//! mention the agent with the mention list's keys, and see its answer posted in the room. The agent's turn is a
//! recorded fixture, invented here; no provider runs. Needs the daemon built with the switch, beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_room_mention --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use common::daemon_exe;
use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

/// Harbour's one turn: a short answer, streamed in two parts.
const HARBOUR: &str = r#"{ "version": 1, "turns": [ { "events": [
  { "type": "text", "delta": "Harbour here: the shipping " },
  { "type": "text", "delta": "schedule is on track." },
  { "type": "text_end" },
  { "type": "usage", "model": "sonnet", "input": 20, "output": 9 } ] } ] }"#;

/// Waits until a node whose id starts with `prefix` says `text`.
fn wait_for_text(app: &mut Client, prefix: &str, text: &str, timeout: Duration) {
    let started = Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        let shown: Vec<(String, String)> = snapshot
            .nodes
            .iter()
            .filter(|node| node.id.starts_with(prefix))
            .map(|node| (node.id.clone(), node.text.clone()))
            .collect();
        if shown.iter().any(|(_, said)| said == text) {
            return;
        }
        assert!(
            started.elapsed() < timeout,
            "nothing under {prefix} says {text:?}: {shown:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn e2e_room_mention() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    assert!(
        daemon_exe().exists(),
        "build the daemon first: cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked"
    );
    let root = owner_safety::test_root().unwrap();
    let fixtures = root.path().join("provider-fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    std::fs::write(fixtures.join("Harbour.json"), HARBOUR).unwrap();
    let mut app = Client::launch_with(
        &common::desktop_exe(),
        root.path(),
        &[
            ("TJOMMIE_DAEMON_EXE", daemon_exe().to_str().unwrap()),
            ("TJOMMIE_PROVIDER_FIXTURES", fixtures.to_str().unwrap()),
        ],
    )
    .unwrap();
    // A daemon without the switch refuses the variable and never starts: this wait is where that shows.
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();

    // The agent.
    app.click("agents.new").unwrap();
    app.wait("screen.new", Predicate::Exists, 10_000).unwrap();
    app.click("bot_form.name").unwrap();
    app.text("Harbour").unwrap();
    app.click("new_bot.create").unwrap();
    app.wait("screen.chat", Predicate::Exists, 30_000).unwrap();

    // Everyone, from the rooms list.
    app.click("sidebar.nav.rooms").unwrap();
    app.wait("rooms.room.team.open", Predicate::Exists, 10_000)
        .unwrap();
    app.click("rooms.room.team.open").unwrap();
    app.wait("screen.room", Predicate::Text("Everyone".into()), 10_000)
        .unwrap();
    app.wait("room.empty", Predicate::Exists, 30_000).unwrap();

    // "@Har" offers Harbour; Enter picks it rather than sending, and the next Enter sends.
    app.click("room.composer.field").unwrap();
    app.text("@Har").unwrap();
    app.wait("room.mentions.0", Predicate::Text("Harbour".into()), 10_000)
        .unwrap();
    app.key("enter").unwrap();
    app.text("how is the shipping schedule?").unwrap();
    app.key("enter").unwrap();
    wait_for_text(
        &mut app,
        "room.post.",
        "You: @Harbour how is the shipping schedule?",
        Duration::from_secs(30),
    );

    // Harbour answers in the room, from its fixture.
    wait_for_text(
        &mut app,
        "room.post.",
        "Harbour: Harbour here: the shipping schedule is on track.",
        Duration::from_secs(60),
    );
}
