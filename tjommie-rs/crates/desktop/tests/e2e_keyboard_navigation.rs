//! Full keyboard navigation (Phase 3 plan T5.6; owner decision 4): from launch, keys only (`key` and typing, never
//! `click`). It goes to every screen and every settings section, creates an agent, mentions it in a room and sees its
//! answer (a recorded provider fixture, invented here), works a menu and dialogs from the keyboard, and on each screen
//! and dialog fails on any control that no Tab sequence reaches. Every dialog is opened by keys from its screen (M5
//! review M8), with two exceptions: Devices' Rename and Revoke need a paired phone, which this run can't make, so
//! `settings::devices::tests::the_device_dialogs_work_from_the_keyboard` covers them with a device in the view model;
//! and `Route::Account`'s keyboard path arrives with the Chat lane's link producers. The system's folder picker can't be
//! driven by keys either, so the test driver answers it (`answer_path_prompt`) for the team's repository. Approving a
//! card (T4.2) and moving into and out of a bot's page (T3.5) are the Chat and Browser lanes' steps. The API-key account
//! it adds has an invented key, and every Credential Manager target in this temp root's namespace is deleted when the
//! test ends, failed or not (`Credentials`). Needs the daemon built with the switch, beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_keyboard_navigation --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
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

/// Opens the menu on `trigger` by keys and chooses `item`: Enter opens it on its first row, the arrows move, Enter
/// chooses.
fn menu_to(app: &mut Client, trigger: &str, item: &str) {
    activate(app, trigger);
    let started = Instant::now();
    while focused(app).as_deref() == Some(trigger) {
        assert!(started.elapsed() < Duration::from_secs(10), "{trigger}'s menu didn't open");
        sleep(Duration::from_millis(30));
    }
    for _ in 0..12 {
        if focused(app).as_deref() == Some(item) {
            app.key("enter").unwrap();
            return;
        }
        press(app, "down");
    }
    panic!("no arrow reaches {item} in {trigger}'s menu");
}

/// Opens a dialog by keys from its screen (`open` does it), Tabs once around it (`sweep_in`), then closes it with
/// Escape.
fn dialog(app: &mut Client, place: &str, prefixes: &[&str], open: impl FnOnce(&mut Client)) {
    open(app);
    app.wait("dialog.title", Predicate::Exists, 10_000).unwrap();
    sweep_in(app, place, prefixes);
    app.key("escape").unwrap();
    wait_gone(app, "dialog.title");
}

/// Opens a settings section from its nav link.
fn open_section(app: &mut Client, key: &str) {
    activate(app, &format!("settings.nav.{key}"));
    app.wait(&format!("settings.{key}.title"), Predicate::Exists, 10_000)
        .unwrap();
}

/// The ids under `prefix` with `suffix`, as the window has them now.
fn ids_like(app: &mut Client, prefix: &str, suffix: &str) -> Vec<String> {
    app.snapshot()
        .unwrap()
        .nodes
        .into_iter()
        .map(|node| node.id)
        .filter(|id| id.starts_with(prefix) && id.ends_with(suffix))
        .collect()
}

/// Runs git in `folder`, with an invented identity, and fails if it does.
fn git(folder: &Path, args: &[&str]) {
    let out = desktop_host::spawn::command("git")
        .args(["-c", "user.name=Keyboard Test", "-c", "user.email=keyboard-test@invalid"])
        .args(args)
        .current_dir(folder)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// This run's Credential Manager targets: `Tjommie/<namespace>/…`, where the namespace is the sha256 of this temp root
/// (the daemon's `Namespace`). Dropped at the end of the test, failed or not, it deletes every one of them: the
/// API-key account writes its invented key there. Nothing outside this root's namespace is touched.
struct Credentials {
    namespaces: Vec<String>,
}

impl Credentials {
    fn of(root: &Path) -> Self {
        let namespaces = [root.to_path_buf(), root.join("data")]
            .iter()
            .filter_map(|path| desktop_host::vault::credential::namespace(path).ok())
            .collect();
        Credentials { namespaces }
    }

    /// This run's targets, as `cmdkey /list` has them now.
    fn targets(&self) -> Vec<String> {
        let listed = desktop_host::spawn::command("cmdkey")
            .arg("/list")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .unwrap_or_default();
        listed
            .split_whitespace()
            .filter_map(|word| word.find("Tjommie/").and_then(|at| word.get(at..)))
            .filter(|target| {
                self.namespaces
                    .iter()
                    .any(|namespace| target.starts_with(&format!("Tjommie/{namespace}")))
            })
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        for target in self.targets() {
            let _ = desktop_host::vault::credential::delete_blob(&target);
        }
    }
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
    // Declared before the app, so it's dropped after the app and its daemon have gone.
    let credentials = Credentials::of(root.path());
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
    // The routine editor, from Add routine (its row opens first if it's closed).
    if app.snapshot().unwrap().node("bot_settings.routines.add").is_none() {
        activate(&mut app, "bot_form.routines");
        app.wait("bot_settings.routines.add", Predicate::Exists, 10_000)
            .unwrap();
    }
    dialog(&mut app, "Routine editor", &["dialog.", "routine."], |app| {
        activate(app, "bot_settings.routines.add")
    });

    // Rooms: the list, the New room dialog, and Everyone, where Harbour answers a mention.
    activate(&mut app, "sidebar.nav.rooms");
    app.wait("screen.rooms", Predicate::Exists, 10_000).unwrap();
    sweep(&mut app, "Rooms");
    activate(&mut app, "rooms.new");
    app.wait("room_form.name", Predicate::Exists, 10_000).unwrap();
    sweep_in(&mut app, "New room dialog", &["dialog.", "room_form."]);
    // A room of its own, made from the keyboard, for Edit room and Delete room: a new room opens once it's made.
    tab_to(&mut app, "room_form.name");
    app.text("Lighthouse").unwrap();
    activate(&mut app, &format!("room_form.members.{bot}"));
    activate(&mut app, "room_form.save");
    app.wait("screen.room", Predicate::Text("Lighthouse".into()), 10_000)
        .unwrap();
    sweep(&mut app, "Lighthouse");
    dialog(&mut app, "Edit room dialog", &["dialog.", "room_form."], |app| {
        menu_to(app, "room.more", "room.menu.edit")
    });
    dialog(&mut app, "Delete room dialog", &["dialog."], |app| {
        menu_to(app, "room.more", "room.menu.delete")
    });
    activate(&mut app, "sidebar.nav.rooms");
    app.wait("screen.rooms", Predicate::Exists, 10_000).unwrap();
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
    // A team of its own, for New task and Archive team. Its repository is a git folder made here; the system's folder
    // picker can't be driven by keys, so the test driver answers it. The rest is keys.
    let repository = root.path().join("breakwater");
    std::fs::create_dir_all(&repository).unwrap();
    git(&repository, &["init", "--quiet", "--initial-branch=main"]);
    git(&repository, &["commit", "--quiet", "--allow-empty", "-m", "Start"]);
    app.answer_path_prompt(&repository).unwrap();
    activate(&mut app, "project_form.register");
    tab_to(&mut app, "project_form.name");
    app.text("Breakwater").unwrap();
    activate(&mut app, &format!("project_form.role.{bot}.developer"));
    activate(&mut app, "project_form.save");
    wait_gone(&mut app, "project_form.name");
    // The list reads again after the save: wait for the team to be in it.
    let started = Instant::now();
    let team = loop {
        let listed = ids_like(&mut app, "projects.team.", "")
            .into_iter()
            .find(|id| id.matches('.').count() == 2);
        if let Some(team) = listed {
            break team;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the new team isn't in the list");
        sleep(Duration::from_millis(100));
    };
    activate(&mut app, &team);
    app.wait("projects.name", Predicate::Text("Breakwater".into()), 10_000)
        .unwrap();
    dialog(&mut app, "New task dialog", &["dialog.", "task_form."], |app| {
        activate(app, "projects.new_task")
    });
    dialog(&mut app, "Archive team dialog", &["dialog."], |app| {
        activate(app, "projects.archive")
    });
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

    // Settings' dialogs, each opened by keys from its section.
    open_section(&mut app, "about");
    dialog(&mut app, "Third-party notices dialog", &["dialog.", "settings.about.notices."], |app| {
        activate(app, "settings.about.notices")
    });
    open_section(&mut app, "devices");
    dialog(&mut app, "Pair a device dialog", &["dialog.", "settings.devices.pair."], |app| {
        activate(app, "settings.devices.pair")
    });

    // The seven account dialogs. The fixtures daemon signs in a subscription account per provider (Harbour uses
    // Claude's); an API-key account, added here with an invented key, has Replace key and Spending cap.
    open_section(&mut app, "accounts");
    let form = ["dialog.", "account_form."];
    dialog(&mut app, "Add subscription dialog", &form, |app| {
        menu_to(app, "settings.accounts.claude.add", "settings.accounts.claude.add.subscription")
    });
    let before = ids_like(&mut app, "settings.accounts.", ".more");
    menu_to(&mut app, "settings.accounts.claude.add", "settings.accounts.claude.add.key");
    app.wait("dialog.title", Predicate::Exists, 10_000).unwrap();
    sweep_in(&mut app, "Add API key dialog", &form);
    tab_to(&mut app, "account_form.name");
    app.text("Invented key").unwrap();
    tab_to(&mut app, "account_form.key");
    app.text("invented-e2e-key-not-real").unwrap();
    tab_to(&mut app, "account_form.cap");
    app.text("25").unwrap();
    activate(&mut app, "account_form.save");
    wait_gone(&mut app, "dialog.title");
    let started = Instant::now();
    let key_account = loop {
        let added = ids_like(&mut app, "settings.accounts.", ".more")
            .into_iter()
            .find(|id| !before.contains(id));
        if let Some(more) = added {
            break more.trim_end_matches(".more").to_owned();
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the API-key account didn't appear");
        sleep(Duration::from_millis(100));
    };
    assert!(
        credentials.targets().iter().any(|target| target.contains("/account/")),
        "the cleanup must see this run's key in Credential Manager: its namespace isn't the daemon's"
    );
    let more = format!("{key_account}.more");
    for (place, item) in [
        ("Replace key dialog", "replace_key"),
        ("Spending cap dialog", "edit_cap"),
        ("Rename account dialog", "rename"),
    ] {
        dialog(&mut app, place, &form, |app| menu_to(app, &more, &format!("{key_account}.{item}")));
    }
    let used = ids_like(&mut app, "settings.accounts.", ".users")
        .into_iter()
        .find(|id| app.snapshot().unwrap().node(id).is_some_and(|node| node.text == "1 agent"))
        .expect("Harbour's account")
        .trim_end_matches(".users")
        .to_owned();
    dialog(&mut app, "Can't remove dialog", &["dialog.", "account_form.", "account_remove."], |app| {
        menu_to(app, &format!("{used}.more"), &format!("{used}.remove"))
    });
    // Remove the API-key account for real: the guard deletes its key from Credential Manager either way.
    menu_to(&mut app, &more, &format!("{key_account}.remove"));
    app.wait("dialog.title", Predicate::Exists, 10_000).unwrap();
    sweep_in(&mut app, "Remove account dialog", &form);
    activate(&mut app, "account_form.save");
    wait_gone(&mut app, &more);
}
