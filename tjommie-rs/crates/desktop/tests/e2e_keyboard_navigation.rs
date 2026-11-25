//! Full keyboard navigation (Phase 3 plan T5.6; owner decision 4): from launch, keys only (`key` and typing, never
//! `click`). It goes to every screen and every settings section, creates an agent, sends it a message from its chat
//! (Shift+Enter starts a line, Enter sends) and approves the card its next turn raises, mentions it in a room and sees
//! its answer (recorded provider fixtures, invented here), works a menu and dialogs from the keyboard, and on each
//! screen and dialog fails on any control that no Tab sequence reaches. Every dialog is opened by keys from its screen
//! (M5 review M8), with two exceptions: Devices' Rename and Revoke need a paired phone, which this run can't make, so
//! `settings::devices::tests::the_device_dialogs_work_from_the_keyboard` covers them with a device in the view model;
//! and `Route::Account`'s keyboard path arrives with the Chat lane's link producers. The system's folder picker can't be
//! driven by keys either, so the test driver answers it (`answer_path_prompt`) for the team's repository, and for the
//! save path of a file card's Download. The exchange and the file card come from fixture `mcp` events (a fixture turn
//! calling the session's own `tjommie` tools): Harbour asks a second fixture agent, Lantern, through `message_bot`, and
//! shares a Markdown file the test wrote in its own folder through `share_file`. The exchange view opens from View thread
//! and closes with Escape; the card's Preview opens the viewer, and Download saves to the answered path. Moving into and
//! out of a bot's page (T3.5) is the Browser lane's step. The API-key account it adds has an invented key, and every Credential
//! Manager target in this temp root's namespace is deleted when the test ends, failed or not (`Credentials`). Needs
//! the daemon built with the switch, beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_keyboard_navigation --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::thread::sleep;
use std::time::{Duration, Instant};

use common::daemon_exe;
use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

/// The app's own log, once the test has its data root: a failing run carries what the app was doing (review of the host
/// wiring: an intermittent `tab_to` failure that no reading explained needs the run's own evidence). On a failure its last
/// lines are in the test's panic message, and so in the public job log of `ci.yml`: nothing in this test's data (an invented
/// key, temp paths) is private, and nothing the daemon writes to its stderr may be (no settings value, no token).
static APP_LOG: OnceLock<PathBuf> = OnceLock::new();

/// The last lines of the app's log.
fn log_tail() -> String {
    let Some(path) = APP_LOG.get() else {
        return "(the test has no log path yet)".to_owned();
    };
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let count = text.lines().count();
            text.lines()
                .skip(count.saturating_sub(60))
                .collect::<Vec<_>>()
                .join("\n")
        }
        Err(error) => format!("(no log at {}: {error})", path.display()),
    }
}

/// Harbour's turns, one per message, in the order the test sends them: two lines from its chat's composer (as one
/// message), a request that waits on an approval card, a request that has it ask Lantern through its `tjommie` server's
/// `message_bot` (Lantern's answer then reaches it as a message of its own, which plays the next turn), a request that has
/// it share a file of its own folder through `share_file`, and the mention in Everyone.
const HARBOUR: &str = r#"{ "version": 1, "turns": [
  { "expect": "First line\nsecond line", "events": [
    { "type": "text", "delta": "Both lines arrived." }, { "type": "text_end" } ] },
  { "expect": "check the moorings", "events": [
    { "type": "approval", "id": "ask-1", "tool": "Bash", "summary": "Run git status", "rule": "Bash(git status:*)",
      "then": { "once": [ { "type": "text", "delta": "Ran it once." }, { "type": "text_end" } ] } } ] },
  { "expect": "ask Lantern", "events": [
    { "type": "mcp", "id": "m1", "tool": "message_bot",
      "arguments": { "bot": "Lantern", "message": "Is the tide table ready?" } },
    { "type": "text", "delta": "I asked Lantern." }, { "type": "text_end" } ] },
  { "expect": "The tide table is ready.", "events": [
    { "type": "text", "delta": "Lantern says the table is ready." }, { "type": "text_end" } ] },
  { "expect": "share the tide report", "events": [
    { "type": "mcp", "id": "m2", "tool": "share_file",
      "arguments": { "path": "tide-report.md", "caption": "Tide report" } },
    { "type": "text", "delta": "I shared the report." }, { "type": "text_end" } ] },
  { "expect": "all clear?", "events": [
    { "type": "text", "delta": "Harbour here: all clear." }, { "type": "text_end" },
    { "type": "usage", "model": "sonnet", "input": 10, "output": 5 } ] } ] }"#;

/// Lantern's one turn: the answer to Harbour's request, which the daemon hands it as a message from Harbour.
const LANTERN: &str = r#"{ "version": 1, "turns": [
  { "expect": "Is the tide table ready?", "events": [
    { "type": "text", "delta": "The tide table is ready." }, { "type": "text_end" } ] } ] }"#;

/// The file Harbour shares: Markdown, so the viewer renders it.
const TIDE_REPORT: &str = "# Tide report\n\nLow tide at **06:40**, high tide at **12:55**.\n";

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
        // Where each screen is drawn, and the window, to tell a scroll that didn't happen from a screen drawn elsewhere.
        let screens: Vec<(String, f32)> = snapshot
            .nodes
            .iter()
            .filter(|node| node.id.starts_with("screen.") || node.id.starts_with("dialog."))
            .map(|node| (node.id.clone(), node.bounds.y))
            .collect();
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "{place}: Tab left {id} out of view at {bounds:?} (window {:?}; screens and dialogs at {screens:?})",
            snapshot.viewport
        );
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
    // What the screen said while it did not move: a connection that is not Ready disables the nav buttons, which are then
    // no tab stops.
    // The app may have gone (the case its log is most wanted for): then there is no pill to read, and no panic here.
    let status = app
        .snapshot()
        .ok()
        .and_then(|snapshot| snapshot.node("shell.status").map(|node| node.text.clone()))
        .unwrap_or_else(|| "(the app did not answer)".to_owned());
    panic!(
        "no Tab reaches {id}; it passed {passed:?}\nthe connection pill said {status:?}\n--- the app's log, last lines ---\n{}",
        log_tail()
    );
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
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{trigger}'s menu didn't open"
        );
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

/// The size the test runs at, the same on every machine: it fits a hosted runner's 1024 by 768 display.
const WINDOW: (f32, f32) = (1000., 700.);

/// Sets the window's size and waits until the app has drawn at it.
fn resize(app: &mut Client, (width, height): (f32, f32)) {
    app.resize(width, height).unwrap();
    let started = Instant::now();
    loop {
        let viewport = app.snapshot().unwrap().viewport;
        if viewport.as_ref().is_some_and(|view| {
            (view.width - width).abs() < 1. && (view.height - height).abs() < 1.
        }) {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the window isn't {width} by {height}: {viewport:?}"
        );
        sleep(Duration::from_millis(30));
    }
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
        .args([
            "-c",
            "user.name=Keyboard Test",
            "-c",
            "user.email=keyboard-test@invalid",
        ])
        .args(args)
        .current_dir(folder)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{id} is still on screen"
        );
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
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "nothing under {prefix} says {text:?}"
        );
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
    sweep_where(app, place, |id| {
        prefixes.iter().any(|prefix| id.starts_with(prefix))
    });
}

/// `sweep_in` for the controls `own` says are the place's: a sheet's, whose ids it shares with what it covers.
fn sweep_where(app: &mut Client, place: &str, own: impl Fn(&str) -> bool) {
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
        .filter(|node| own(&node.id))
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
    assert!(
        missed.is_empty(),
        "{place}: no Tab reaches {missed:?} (it reached {seen:?})"
    );
    let unavailable: Vec<&String> = seen
        .iter()
        .filter(|id| snapshot.node(id).is_some_and(|node| !node.enabled))
        .collect();
    assert!(
        unavailable.is_empty(),
        "{place}: Tab stops on unavailable {unavailable:?}"
    );
}

/// Polls `done` for up to a minute. A failure names `place` and says what the chat's errors, notices and problems say,
/// such as a fixture turn's own error.
fn until(app: &mut Client, place: &str, what: &str, mut done: impl FnMut(&mut Client) -> bool) {
    let started = Instant::now();
    while !done(app) {
        if started.elapsed() > Duration::from_secs(60) {
            let said: Vec<String> = app
                .snapshot()
                .unwrap()
                .nodes
                .into_iter()
                .filter(|node| {
                    node.id.starts_with("error.")
                        || node.id.starts_with("notice.")
                        || node.id.ends_with(".problem")
                })
                .map(|node| node.text)
                .collect();
            panic!("{place}: {what}; the chat says {said:?}");
        }
        sleep(Duration::from_millis(100));
    }
}

/// How many of the agent's replies its chat shows (`prose.<message id>`).
fn replies(app: &mut Client) -> usize {
    ids_like(app, "prose.", "")
        .iter()
        .filter(|id| id.matches('.').count() == 1)
        .count()
}

/// The chat's composer by keys (review C5): Tab reaches its field, Shift+Enter starts a new line, Enter sends, and the
/// two lines show as one bubble of yours (Harbour's fixture turn expects them as one message too). Harbour answers.
fn composer(app: &mut Client, bot: &str) {
    tab_to(app, &format!("composer.{bot}.field"));
    app.text("First line").unwrap();
    app.key("shift-enter").unwrap();
    app.text("second line").unwrap();
    app.key("enter").unwrap();
    wait_for_text(app, "bubble.", "First line\nsecond line");
    assert_eq!(
        ids_like(app, "bubble.", "").len(),
        1,
        "Composer: Shift+Enter sent a message of its own"
    );
    until(app, "Composer", "Harbour didn't answer", |app| {
        replies(app) > 0
    });
}

/// An approval card by keys (review C5): a message sent from the composer brings Harbour's card, whose every control is
/// a Tab stop; Enter on Approve answers it, its record replaces it, and the focus lands on the composer's field (M4
/// review A4). Harbour's turn then goes on.
fn approval(app: &mut Client, bot: &str) {
    let field = format!("composer.{bot}.field");
    if focused(app).as_deref() != Some(field.as_str()) {
        tab_to(app, &field);
    }
    let before = replies(app);
    app.text("Please check the moorings").unwrap();
    app.key("enter").unwrap();
    until(app, "Approval", "no card came", |app| {
        !ids_like(app, "approval.", ".approve").is_empty()
    });
    let approve = ids_like(app, "approval.", ".approve").pop().unwrap();
    let card = approve.trim_end_matches(".approve").to_owned();
    sweep(app, "Chat, approval card");
    activate(app, &approve);
    app.wait(
        &card,
        Predicate::Text("Approved: Run git status".into()),
        10_000,
    )
    .unwrap();
    until(
        app,
        "Approval",
        "the focus isn't on the composer's field",
        |app| focused(app).is_some_and(|id| id == field),
    );
    until(app, "Approval", "Harbour's turn didn't go on", |app| {
        replies(app) > before
    });
}

/// The id of the agent whose chat is open: the one whose composer is on screen.
fn chat_bot(app: &mut Client) -> String {
    let started = Instant::now();
    loop {
        if let Some(field) = ids_like(app, "composer.", ".field").pop() {
            return field
                .trim_start_matches("composer.")
                .trim_end_matches(".field")
                .to_owned();
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no chat's composer is on screen"
        );
        sleep(Duration::from_millis(100));
    }
}

/// Types `text` in the open chat's composer and sends it with Enter.
fn say(app: &mut Client, bot: &str, text: &str) {
    let field = format!("composer.{bot}.field");
    if focused(app).as_deref() != Some(field.as_str()) {
        tab_to(app, &field);
    }
    app.text(text).unwrap();
    app.key("enter").unwrap();
}

/// An exchange by keys: a request has Harbour ask Lantern (its fixture turn calls `message_bot` on the session's own
/// `tjommie` server), the chat shows the hand-off row with its View thread link, and Lantern's answer reaches Harbour as a
/// message of its own that plays Harbour's next turn. Enter on View thread opens the exchange, every control in it is a Tab
/// stop, and Escape closes it and gives the keyboard back to the link. At the test's 1000 by 700 the content is too narrow
/// for both (`shell::overlaid`), so the exchange lies over the chat as a sheet that keeps Tab inside it, as a dialog does:
/// the round reaches its own controls, and none of the chat's behind it (View thread among them).
fn exchange(app: &mut Client, bot: &str) {
    let before = replies(app);
    say(app, bot, "Please ask Lantern about the tide table");
    until(app, "Exchange", "no View thread link came", |app| {
        !ids_like(app, "exchange.", ".open").is_empty()
    });
    until(
        app,
        "Exchange",
        "Lantern's answer didn't reach Harbour",
        |app| replies(app) >= before + 2,
    );
    let open = ids_like(app, "exchange.", ".open").pop().unwrap();
    let view = format!("{}.view", open.trim_end_matches(".open"));
    activate(app, &open);
    app.wait(&view, Predicate::Exists, 10_000).unwrap();
    // The exchange's own words: who asked whom.
    app.wait(
        &format!("{}.title", open.trim_end_matches(".open")),
        Predicate::Text("Harbour asked Lantern".into()),
        10_000,
    )
    .unwrap();
    let own = format!("{}.", open.trim_end_matches(".open"));
    sweep_where(app, "Chat, exchange", |id| {
        id.starts_with(&own) && id != open
    });
    app.key("escape").unwrap();
    wait_gone(app, &view);
    until(
        app,
        "Exchange",
        "the focus isn't back on View thread",
        |app| focused(app).is_some_and(|id| id == open),
    );
}

/// A shared file's card by keys: Harbour's fixture turn calls `share_file` on a file the test wrote in Harbour's own folder,
/// so a card of Harbour's shows with Preview and Download. Enter on Preview opens the viewer, which renders the Markdown
/// through the transfer, every control in it is a Tab stop, and Escape closes it. Download saves to the path the test
/// answers (the system's save dialog can't be driven by keys) and says it saved; the file there is the one shared.
fn file_card(app: &mut Client, bot: &str, root: &Path) {
    // The agent's own folder under the data root: `share_file` reads a path in the agent's folders, relative to its own.
    std::fs::write(
        root.join("bots").join(bot).join("tide-report.md"),
        TIDE_REPORT,
    )
    .unwrap();
    say(app, bot, "Please share the tide report");
    until(app, "File card", "no file card came", |app| {
        !ids_like(app, "file.", ".preview").is_empty()
    });
    let preview = ids_like(app, "file.", ".preview").pop().unwrap();
    let card = preview.trim_end_matches(".preview").to_owned();
    assert_eq!(
        app.snapshot().unwrap().node(&card).unwrap().text,
        "tide-report.md"
    );
    sweep(app, "Chat, file card");
    activate(app, &preview);
    let viewer = format!("{card}.viewer");
    app.wait(&viewer, Predicate::Exists, 10_000).unwrap();
    until(app, "File viewer", "the file didn't load", |app| {
        let snapshot = app.snapshot().unwrap();
        snapshot.node(&format!("{viewer}.problem")).is_none()
            && snapshot.node(&format!("{viewer}.status")).is_none()
    });
    sweep_in(app, "File viewer", &[viewer.as_str()]);
    app.key("escape").unwrap();
    wait_gone(app, &viewer);
    // Download: the answered path is where the file is saved.
    let saved = root.join("downloads").join("tide-report.md");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    app.answer_path_prompt(&saved).unwrap();
    activate(app, &format!("{card}.download"));
    app.wait(
        &format!("{card}.status"),
        Predicate::Text("Saved on this device.".into()),
        30_000,
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), TIDE_REPORT);
}

#[test]
fn e2e_keyboard_navigation() {
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
    let _ = APP_LOG.set(root.path().join("logs").join("desktop.log"));
    // Declared before the app, so it's dropped after the app and its daemon have gone.
    let credentials = Credentials::of(root.path());
    let fixtures = root.path().join("provider-fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    std::fs::write(fixtures.join("Harbour.json"), HARBOUR).unwrap();
    std::fs::write(fixtures.join("Lantern.json"), LANTERN).unwrap();
    let mut app = Client::launch_with(
        &common::desktop_exe(),
        root.path(),
        &[
            ("TJOMMIE_DAEMON_EXE", daemon_exe().to_str().unwrap()),
            ("TJOMMIE_PROVIDER_FIXTURES", fixtures.to_str().unwrap()),
        ],
    )
    .unwrap();
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
    resize(&mut app, WINDOW);

    // The sidebar's sections, in visual order; the arrows move between them too.
    tab_to(&mut app, "sidebar.nav.agents");
    let order: Vec<Option<String>> = (0..4).map(|_| press(&mut app, "tab")).collect();
    assert_eq!(
        order,
        ["rooms", "projects", "routines", "settings"].map(|key| Some(format!("sidebar.nav.{key}")))
    );
    assert_eq!(
        press(&mut app, "up").as_deref(),
        Some("sidebar.nav.routines")
    );
    assert_eq!(
        press(&mut app, "down").as_deref(),
        Some("sidebar.nav.settings")
    );
    // No agent yet, so Agents shows the first-run setup. Its cards settle once the fixtures daemon reports both
    // providers signed in; the sweep waits for that, or it races the report.
    for provider in ["claude", "codex"] {
        app.wait(
            &format!("setup.{provider}.other_account"),
            Predicate::Exists,
            30_000,
        )
        .unwrap();
    }
    sweep(&mut app, "Agents");

    // Lantern first, from the keyboard, so Harbour can be allowed to message it: its chat opens, and the Agents screen
    // again for the next New agent.
    activate(&mut app, "agents.new");
    app.wait("screen.new", Predicate::Exists, 10_000).unwrap();
    tab_to(&mut app, "bot_form.name");
    app.text("Lantern").unwrap();
    activate(&mut app, "new_bot.create");
    app.wait("screen.chat", Predicate::Exists, 30_000).unwrap();
    let lantern = chat_bot(&mut app);
    activate(&mut app, "sidebar.nav.agents");
    app.wait("agents.new", Predicate::Exists, 10_000).unwrap();

    // A new agent, made from the keyboard: Can message holds Lantern, and its chat opens.
    activate(&mut app, "agents.new");
    app.wait("screen.new", Predicate::Exists, 10_000).unwrap();
    tab_to(&mut app, "bot_form.name");
    app.text("Harbour").unwrap();
    // The row opens on Enter, and Lantern's switch is the only one in it.
    activate(&mut app, "bot_form.can_message");
    let allow = format!("bot_form.can_message.{lantern}");
    activate(&mut app, &allow);
    until(&mut app, "New agent", "Lantern isn't allowed", |app| {
        app.snapshot()
            .unwrap()
            .node(&allow)
            .is_some_and(|node| node.toggled)
    });
    sweep(&mut app, "New agent");
    activate(&mut app, "new_bot.create");
    app.wait("screen.chat", Predicate::Exists, 30_000).unwrap();
    sweep(&mut app, "Chat");

    // The chat by keys (review C5): its composer, then an approval card, then an exchange with Lantern and a file card.
    let bot = chat_bot(&mut app);
    assert_ne!(bot, lantern, "Harbour's chat is open");
    composer(&mut app, &bot);
    approval(&mut app, &bot);
    exchange(&mut app, &bot);
    file_card(&mut app, &bot, root.path());

    // Its card's More menu, from the keyboard: Enter opens it on its first row, the arrows move, Enter chooses.
    activate(&mut app, "sidebar.nav.agents");
    app.wait("screen.agents", Predicate::Exists, 10_000)
        .unwrap();
    activate(&mut app, &format!("agents.card.{bot}.more"));
    app.wait(
        &format!("agents.menu.{bot}.chat"),
        Predicate::Focused,
        10_000,
    )
    .unwrap();
    assert_eq!(
        press(&mut app, "down"),
        Some(format!("agents.menu.{bot}.pause"))
    );
    assert_eq!(
        press(&mut app, "down"),
        Some(format!("agents.menu.{bot}.settings"))
    );
    app.key("enter").unwrap();
    app.wait("screen.bot_settings", Predicate::Exists, 10_000)
        .unwrap();
    sweep(&mut app, "Agent settings");
    activate(&mut app, "bot_settings.delete");
    app.wait("dialog.confirm", Predicate::Exists, 10_000)
        .unwrap();
    sweep_in(&mut app, "Delete agent dialog", &["dialog."]);
    app.key("escape").unwrap();
    wait_gone(&mut app, "dialog.confirm");
    // The routine editor, from Add routine (its row opens first if it's closed).
    if app
        .snapshot()
        .unwrap()
        .node("bot_settings.routines.add")
        .is_none()
    {
        activate(&mut app, "bot_form.routines");
        app.wait("bot_settings.routines.add", Predicate::Exists, 10_000)
            .unwrap();
    }
    dialog(
        &mut app,
        "Routine editor",
        &["dialog.", "routine."],
        |app| activate(app, "bot_settings.routines.add"),
    );

    // Rooms: the list, the New room dialog, and Everyone, where Harbour answers a mention.
    activate(&mut app, "sidebar.nav.rooms");
    app.wait("screen.rooms", Predicate::Exists, 10_000).unwrap();
    sweep(&mut app, "Rooms");
    activate(&mut app, "rooms.new");
    app.wait("room_form.name", Predicate::Exists, 10_000)
        .unwrap();
    sweep_in(&mut app, "New room dialog", &["dialog.", "room_form."]);
    // A room of its own, made from the keyboard, for Edit room and Delete room: a new room opens once it's made.
    tab_to(&mut app, "room_form.name");
    app.text("Lighthouse").unwrap();
    activate(&mut app, &format!("room_form.members.{bot}"));
    activate(&mut app, "room_form.save");
    app.wait("screen.room", Predicate::Text("Lighthouse".into()), 10_000)
        .unwrap();
    sweep(&mut app, "Lighthouse");
    dialog(
        &mut app,
        "Edit room dialog",
        &["dialog.", "room_form."],
        |app| menu_to(app, "room.more", "room.menu.edit"),
    );
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
    app.wait("screen.projects", Predicate::Exists, 10_000)
        .unwrap();
    sweep(&mut app, "Projects");
    activate(&mut app, "projects.new");
    app.wait("project_form.name", Predicate::Exists, 10_000)
        .unwrap();
    sweep_in(
        &mut app,
        "New project team dialog",
        &["dialog.", "project_form."],
    );
    // A team of its own, for New task and Archive team. Its repository is a git folder made here; the system's folder
    // picker can't be driven by keys, so the test driver answers it. The rest is keys.
    let repository = root.path().join("breakwater");
    std::fs::create_dir_all(&repository).unwrap();
    git(&repository, &["init", "--quiet", "--initial-branch=main"]);
    git(
        &repository,
        &["commit", "--quiet", "--allow-empty", "-m", "Start"],
    );
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
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the new team isn't in the list"
        );
        sleep(Duration::from_millis(100));
    };
    activate(&mut app, &team);
    app.wait(
        "projects.name",
        Predicate::Text("Breakwater".into()),
        10_000,
    )
    .unwrap();
    dialog(
        &mut app,
        "New task dialog",
        &["dialog.", "task_form."],
        |app| activate(app, "projects.new_task"),
    );
    // At the app's smallest window (360 by 600) the New task dialog is taller than the window: its body scrolls, and
    // Tab brings each of its controls into view. It opens at the test's size and the window shrinks under it (the
    // screens' own layout at 360 wide is another matter).
    activate(&mut app, "projects.new_task");
    app.wait("dialog.title", Predicate::Exists, 10_000).unwrap();
    resize(&mut app, (360., 600.));
    sweep_in(
        &mut app,
        "New task dialog, smallest window",
        &["dialog.", "task_form."],
    );
    app.key("escape").unwrap();
    wait_gone(&mut app, "dialog.title");
    resize(&mut app, WINDOW);
    dialog(&mut app, "Archive team dialog", &["dialog."], |app| {
        activate(app, "projects.archive")
    });
    activate(&mut app, "sidebar.nav.routines");
    app.wait("screen.routines", Predicate::Exists, 10_000)
        .unwrap();
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
    dialog(
        &mut app,
        "Third-party notices dialog",
        &["dialog.", "settings.about.notices."],
        |app| activate(app, "settings.about.notices"),
    );
    open_section(&mut app, "devices");
    dialog(
        &mut app,
        "Pair a device dialog",
        &["dialog.", "settings.devices.pair."],
        |app| activate(app, "settings.devices.pair"),
    );

    // The seven account dialogs. The fixtures daemon signs in a subscription account per provider (Harbour uses
    // Claude's); an API-key account, added here with an invented key, has Replace key and Spending cap.
    open_section(&mut app, "accounts");
    let form = ["dialog.", "account_form."];
    dialog(&mut app, "Add subscription dialog", &form, |app| {
        menu_to(
            app,
            "settings.accounts.claude.add",
            "settings.accounts.claude.add.subscription",
        )
    });
    let before = ids_like(&mut app, "settings.accounts.", ".more");
    menu_to(
        &mut app,
        "settings.accounts.claude.add",
        "settings.accounts.claude.add.key",
    );
    if app.wait("dialog.title", Predicate::Exists, 30_000).is_err() {
        // What the window and the app's log say, for a run nobody watched.
        let snapshot = app.snapshot().unwrap();
        let shown: Vec<(String, String, bool)> = snapshot
            .nodes
            .iter()
            .filter(|node| node.id.starts_with("dialog.") || node.id.contains(".add"))
            .map(|node| (node.id.clone(), node.text.clone(), node.focused))
            .collect();
        panic!(
            "the Add API key dialog didn't open: {shown:?}\n--- the app's log, last lines ---\n{}",
            log_tail()
        );
    }
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
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the API-key account didn't appear"
        );
        sleep(Duration::from_millis(100));
    };
    assert!(
        credentials
            .targets()
            .iter()
            .any(|target| target.contains("/account/")),
        "the cleanup must see this run's key in Credential Manager: its namespace isn't the daemon's"
    );
    let more = format!("{key_account}.more");
    for (place, item) in [
        ("Replace key dialog", "replace_key"),
        ("Spending cap dialog", "edit_cap"),
        ("Rename account dialog", "rename"),
    ] {
        dialog(&mut app, place, &form, |app| {
            menu_to(app, &more, &format!("{key_account}.{item}"))
        });
    }
    let used = ids_like(&mut app, "settings.accounts.", ".users")
        .into_iter()
        .find(|id| {
            app.snapshot()
                .unwrap()
                .node(id)
                .is_some_and(|node| node.text == "2 agents")
        })
        .expect("Harbour's and Lantern's account")
        .trim_end_matches(".users")
        .to_owned();
    dialog(
        &mut app,
        "Can't remove dialog",
        &["dialog.", "account_form.", "account_remove."],
        |app| menu_to(app, &format!("{used}.more"), &format!("{used}.remove")),
    );
    // Remove the API-key account for real: the guard deletes its key from Credential Manager either way.
    menu_to(&mut app, &more, &format!("{key_account}.remove"));
    app.wait("dialog.title", Predicate::Exists, 10_000).unwrap();
    sweep_in(&mut app, "Remove account dialog", &form);
    activate(&mut app, "account_form.save");
    wait_gone(&mut app, &more);
}
