//! Full keyboard navigation (Phase 3 plan T5.6; owner decision 4): from launch, keys only (`key` and typing, never
//! `click`). It goes to every screen and every settings section, creates an agent, mentions it in a room and sees its
//! answer (a recorded provider fixture, invented here), works a menu and dialogs from the keyboard, and on each screen
//! and dialog fails on any control that no Tab sequence reaches. Approving a card (T4.2) and moving into and out of a
//! bot's page (T3.5) are the Chat and Browser lanes' steps. Needs the daemon built with the switch, beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_keyboard_navigation --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

fn daemon_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tjommie-desktop")).with_file_name("tjommie-daemon.exe")
}

/// Harbour's one turn.
const HARBOUR: &str = r#"{ "version": 1, "turns": [ { "events": [
  { "type": "text", "delta": "Harbour here: all clear." }, { "type": "text_end" },
  { "type": "usage", "model": "sonnet", "input": 10, "output": 5 } ] } ] }"#;

const SECTIONS: [&str; 12] = [
    "general",
    "appearance",
    "notifications",
    "files",
    "voice",
    "logins",
    "bots",
    "accounts",
    "usage",
    "devices",
    "diagnostics",
    "about",
];

fn focused(app: &mut Client) -> Option<String> {
    app.snapshot()
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.focused)
        .map(|node| node.id)
}

/// Presses `chord`, then waits for the focus to move (the next frame shows it); the focused control after.
fn press(app: &mut Client, chord: &str) -> Option<String> {
    let before = focused(app);
    app.key(chord).unwrap();
    let started = Instant::now();
    loop {
        let now = focused(app);
        if now != before || started.elapsed() > Duration::from_millis(600) {
            return now;
        }
        sleep(Duration::from_millis(15));
    }
}

/// The control Tab moved to shows (T5.6): its middle is inside the window, never below the fold. The scroll that
/// brings it into view comes a frame after the focus does, so this waits a moment for it.
fn wait_in_view(app: &mut Client, place: &str, id: &str) {
    let started = Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        let shown = match (snapshot.node(id), snapshot.viewport.as_ref()) {
            (Some(node), Some(view)) => {
                let middle = node.bounds.y + node.bounds.height / 2.0;
                middle >= view.y && middle <= view.y + view.height
            }
            // Gone already, or nothing to measure against.
            _ => true,
        };
        if shown {
            return;
        }
        let bounds = snapshot.node(id).map(|node| node.bounds);
        assert!(started.elapsed() < Duration::from_millis(600), "{place}: Tab left {id} out of view at {bounds:?}");
        sleep(Duration::from_millis(15));
    }
}

/// Tab until `id` has the focus.
fn tab_to(app: &mut Client, id: &str) {
    let mut passed = Vec::new();
    for _ in 0..250 {
        let now = press(app, "tab");
        if let Some(now) = now.as_deref() {
            wait_in_view(app, "tab_to", now);
        }
        if now.as_deref() == Some(id) {
            return;
        }
        passed.extend(now);
    }
    panic!("no Tab reaches {id}; it passed {passed:?}");
}

/// Tab to `id`, then Enter.
fn activate(app: &mut Client, id: &str) {
    tab_to(app, id);
    app.key("enter").unwrap();
}

fn wait_gone(app: &mut Client, id: &str) {
    let started = Instant::now();
    while app.snapshot().unwrap().node(id).is_some() {
        assert!(started.elapsed() < Duration::from_secs(10), "{id} is still on screen");
        sleep(Duration::from_millis(50));
    }
}

fn wait_for_text(app: &mut Client, prefix: &str, text: &str) {
    let started = Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        if snapshot
            .nodes
            .iter()
            .any(|node| node.id.starts_with(prefix) && node.text == text)
        {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "nothing under {prefix} says {text:?}");
        sleep(Duration::from_millis(200));
    }
}

/// Tabs once around the window and fails on any control no Tab reached.
fn sweep(app: &mut Client, place: &str) {
    sweep_in(app, place, &[""]);
}

/// Tabs once around an open dialog (Tab stays inside it) and fails on any of its controls (ids starting with one of
/// `prefixes`) no Tab reached; the page behind it is out of reach, as it should be.
fn sweep_in(app: &mut Client, place: &str, prefixes: &[&str]) {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..400 {
        let Some(now) = press(app, "tab") else {
            continue;
        };
        wait_in_view(app, place, &now);
        if seen.first() == Some(&now) {
            break;
        }
        if !seen.contains(&now) {
            seen.push(now);
        }
    }
    let group = |id: &str| id.rsplit_once('.').map_or(id, |(head, _)| head).to_owned();
    let snapshot = app.snapshot().unwrap();
    let missed: Vec<String> = snapshot
        .nodes
        .iter()
        .filter(|node| node.interactive && !seen.contains(&node.id))
        .filter(|node| prefixes.iter().any(|prefix| node.id.starts_with(prefix)))
        // A radio group is one stop, its chosen choice; the arrows move to the others.
        .filter(|node| {
            !(node.role == "RadioButton" && seen.iter().any(|id| group(id) == group(&node.id)))
        })
        // The window's caption buttons are Windows' own (Alt+Space, Win+arrows).
        .filter(|node| !node.id.starts_with("caption."))
        // A control that's unavailable now is no Tab stop (M5 review M2).
        .filter(|node| node.enabled)
        .map(|node| node.id.clone())
        .collect();
    assert!(missed.is_empty(), "{place}: no Tab reaches {missed:?} (it reached {seen:?})");
    let unavailable: Vec<&String> = seen
        .iter()
        .filter(|id| snapshot.node(id).is_some_and(|node| !node.enabled))
        .collect();
    assert!(unavailable.is_empty(), "{place}: Tab stops on unavailable {unavailable:?}");
}

#[test]
fn e2e_keyboard_navigation() {
    assert_eq!(identity::CURRENT.name, "e2e", "build the e2e app: TJOMMIE_IDENTITY=e2e");
    assert!(
        daemon_exe().exists(),
        "build the daemon first: cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked"
    );
    let root = owner_safety::test_root().unwrap();
    let fixtures = root.path().join("provider-fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    std::fs::write(fixtures.join("Harbour.json"), HARBOUR).unwrap();
    let mut app = Client::launch_with(
        env!("CARGO_BIN_EXE_tjommie-desktop"),
        root.path(),
        &[
            ("TJOMMIE_DAEMON_EXE", daemon_exe().to_str().unwrap()),
            ("TJOMMIE_PROVIDER_FIXTURES", fixtures.to_str().unwrap()),
        ],
    )
    .unwrap();
    app.wait("screen.agents", Predicate::Exists, 60_000).unwrap();

    // The sidebar's sections, in visual order; the arrows move between them too.
    tab_to(&mut app, "sidebar.nav.agents");
    let order: Vec<Option<String>> = (0..4).map(|_| press(&mut app, "tab")).collect();
    assert_eq!(
        order,
        ["rooms", "projects", "routines", "settings"]
            .map(|key| Some(format!("sidebar.nav.{key}")))
    );
    assert_eq!(press(&mut app, "up").as_deref(), Some("sidebar.nav.routines"));
    assert_eq!(press(&mut app, "down").as_deref(), Some("sidebar.nav.settings"));
    sweep(&mut app, "Agents");

    // A new agent, made from the keyboard: its chat opens.
    activate(&mut app, "agents.new");
    app.wait("screen.new", Predicate::Exists, 10_000).unwrap();
    tab_to(&mut app, "bot_form.name");
    app.text("Harbour").unwrap();
    sweep(&mut app, "New agent");
    activate(&mut app, "new_bot.create");
    app.wait("screen.chat", Predicate::Exists, 30_000).unwrap();
    sweep(&mut app, "Chat");

    // Its card's More menu, from the keyboard: Enter opens it on its first row, the arrows move, Enter chooses.
    activate(&mut app, "sidebar.nav.agents");
    app.wait("screen.agents", Predicate::Exists, 10_000).unwrap();
    let card = app
        .snapshot()
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.id.starts_with("agents.card.") && node.id.matches('.').count() == 2)
        .map(|node| node.id)
        .expect("Harbour's card");
    let bot = card.trim_start_matches("agents.card.").to_owned();
    activate(&mut app, &format!("{card}.more"));
    app.wait(&format!("agents.menu.{bot}.chat"), Predicate::Focused, 10_000)
        .unwrap();
    assert_eq!(press(&mut app, "down"), Some(format!("agents.menu.{bot}.pause")));
    assert_eq!(press(&mut app, "down"), Some(format!("agents.menu.{bot}.settings")));
    app.key("enter").unwrap();
    app.wait("screen.bot_settings", Predicate::Exists, 10_000).unwrap();
    sweep(&mut app, "Agent settings");
    activate(&mut app, "bot_settings.delete");
    app.wait("dialog.confirm", Predicate::Exists, 10_000).unwrap();
    sweep_in(&mut app, "Delete agent dialog", &["dialog."]);
    app.key("escape").unwrap();
    wait_gone(&mut app, "dialog.confirm");

    // Rooms: the list, the New room dialog, and Everyone, where Harbour answers a mention.
    activate(&mut app, "sidebar.nav.rooms");
    app.wait("screen.rooms", Predicate::Exists, 10_000).unwrap();
    sweep(&mut app, "Rooms");
    activate(&mut app, "rooms.new");
    app.wait("room_form.name", Predicate::Exists, 10_000).unwrap();
    sweep_in(&mut app, "New room dialog", &["dialog.", "room_form."]);
    app.key("escape").unwrap();
    wait_gone(&mut app, "room_form.name");
    activate(&mut app, "rooms.room.team.open");
    app.wait("screen.room", Predicate::Text("Everyone".into()), 10_000)
        .unwrap();
    sweep(&mut app, "Everyone");
    tab_to(&mut app, "room.composer.field");
    app.text("@Har").unwrap();
    app.wait("room.mentions.0", Predicate::Text("Harbour".into()), 10_000)
        .unwrap();
    app.key("enter").unwrap();
    app.text("all clear?").unwrap();
    app.key("enter").unwrap();
    wait_for_text(&mut app, "room.post.", "You: @Harbour all clear?");
    wait_for_text(&mut app, "room.post.", "Harbour: Harbour here: all clear.");

    // Projects and its New project team dialog; Routines.
    activate(&mut app, "sidebar.nav.projects");
    app.wait("screen.projects", Predicate::Exists, 10_000).unwrap();
    sweep(&mut app, "Projects");
    activate(&mut app, "projects.new");
    app.wait("project_form.name", Predicate::Exists, 10_000).unwrap();
    sweep_in(&mut app, "New project team dialog", &["dialog.", "project_form."]);
    app.key("escape").unwrap();
    wait_gone(&mut app, "project_form.name");
    activate(&mut app, "sidebar.nav.routines");
    app.wait("screen.routines", Predicate::Exists, 10_000).unwrap();
    sweep(&mut app, "Routines");

    // Every settings section.
    activate(&mut app, "sidebar.nav.settings");
    for key in SECTIONS {
        activate(&mut app, &format!("settings.nav.{key}"));
        app.wait(&format!("settings.{key}.title"), Predicate::Exists, 10_000)
            .unwrap();
        sweep(&mut app, &format!("Settings, {key}"));
    }
}
