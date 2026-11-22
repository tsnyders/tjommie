//! The README's pictures (`docs/screens/SHOTS.md`). One test starts the e2e app on a temporary data root with the provider
//! fixtures daemon, builds the invented team the shot list names (agents Forge, Scout, Atlas and Quill, the rooms, a project
//! team, two routines) through the UI, and keeps a PNG of the app's own window for each screen the list gives: 1000 by 700
//! (`app.resize`), dark, and Agents once more in light. Every conversation is a recorded fixture, so no provider runs and
//! no account is touched. One picture, the first-run screen that says Claude Code and Codex are not installed, is taken with
//! the normal daemon on a machine that has neither, and only when `TJOMMIE_SHOTS_PLAIN_DAEMON` names that daemon's exe: a
//! PC that has them would have the daemon look at the real ones, so run it on a hosted runner only (`readme-shots.yml`).
//!
//! The pictures are written to `target/readme-shots/<name>.png`. **They are taken on a GitHub hosted runner, never on a
//! person's PC**: the sidebar's host button shows the Windows account's first letter, and Diagnostics and Devices show the
//! host name. Run on a PC, this test only proves that the capture works; delete what it wrote and never commit it. It is
//! ignored by default (`ci.yml` runs every `e2e_*` test, and this one takes the pictures instead of checking anything):
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_readme_shots --locked -- --ignored --nocapture`.
//!
//! A picture that can't be taken is reported (with a `_failed-<name>.png` of what the window showed, which is in the public
//! artifact too, and is not a README picture) and does not stop the others, so a run keeps what it got; a part of the tour
//! that is not a picture (a daemon that must start, the agents that must be made) ends that part and not the run, and the
//! summary of what was and was not taken is printed whatever happened. The test fails when any picture is missing. Devices
//! leaves no `_failed-` picture, and is not photographed at all unless the relay address was replaced first: the app's
//! default relay is a person's account name on a workers.dev host.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use common::daemon_exe;
use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};
use serde_json::{Value, json};

/// The size every e2e runs at: it fits a hosted runner's 1024 by 768 display.
const WINDOW: (f32, f32) = (1000., 700.);

/// Every picture the README uses, in the order they are taken.
const PICTURES: [&str; 19] = [
    "setup-not-installed-dark",
    "setup-signed-in-dark",
    "new-agent-dark",
    "chat-dark",
    "room-dark",
    "exchange-dark",
    "projects-dark",
    "project-task-dark",
    "routines-dark",
    "routine-editor-dark",
    "browser-dark",
    "logins-menu-dark",
    "devices-dark",
    "pair-dialog-dark",
    "accounts-dark",
    "usage-dark",
    "agents-dark",
    "approval-dark",
    "agents-light",
];

// ---------------------------------------------------------------------------------------------------------------------
// The driver's small helpers

fn focused(app: &mut Client) -> Option<String> {
    app.snapshot()
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.focused)
        .map(|node| node.id)
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

/// Tab to `id`, then Enter (Space for a switch): by keys, because a click below the fold lands outside the window.
fn activate(app: &mut Client, id: &str) {
    common::tab_to(app, id);
    app.key("enter").unwrap();
}

/// Polls `done` for up to a minute; a failure says `what`.
fn until(app: &mut Client, what: &str, mut done: impl FnMut(&mut Client) -> bool) {
    let started = Instant::now();
    while !done(app) {
        assert!(started.elapsed() < Duration::from_secs(60), "{what}");
        sleep(Duration::from_millis(100));
    }
}

fn wait_gone(app: &mut Client, id: &str) {
    until(app, &format!("{id} is still on screen"), |app| {
        app.snapshot().unwrap().node(id).is_none()
    });
}

/// The id of the agent whose chat is open: the one whose composer is on screen.
fn chat_bot(app: &mut Client) -> String {
    let mut found = None;
    until(app, "no chat's composer is on screen", |app| {
        found = ids_like(app, "composer.", ".field").pop();
        found.is_some()
    });
    found
        .unwrap()
        .trim_start_matches("composer.")
        .trim_end_matches(".field")
        .to_owned()
}

/// Types `text` in the open chat's composer and sends it with Enter.
fn say(app: &mut Client, text: &str) {
    let bot = chat_bot(app);
    let field = format!("composer.{bot}.field");
    if focused(app).as_deref() != Some(field.as_str()) {
        common::tab_to(app, &field);
    }
    app.text(text).unwrap();
    app.key("enter").unwrap();
}

/// Clicks `id` on the screen `from` until `shown` is on screen: a click that lands while the app is busy can go unheard, so
/// it is made again, as long as the click's own screen is still up (a slow answer is waited for, not clicked twice).
fn click_until(app: &mut Client, id: &str, from: &str, shown: &str) {
    for _ in 0..3 {
        if app.snapshot().unwrap().node(from).is_none() {
            break;
        }
        app.click(id).unwrap();
        if app.wait(shown, Predicate::Exists, 8_000).is_ok() {
            return;
        }
    }
    app.wait(shown, Predicate::Exists, 30_000).unwrap();
}

/// Opens an agent's chat from the sidebar's Recents, and waits until it is that agent's: from another chat the Chat screen
/// is already up, so only the header's name says the click took. A click that lands while the app is busy can go unheard,
/// so it is made again.
fn open_chat(app: &mut Client, name: &str) {
    let recent = common::recent(app, name);
    let id = recent.trim_start_matches("sidebar.recent.");
    for _ in 0..3 {
        app.click(&recent).unwrap();
        if app
            .wait(&format!("chat.{id}.name"), Predicate::Exists, 5_000)
            .is_ok()
        {
            return;
        }
    }
    panic!("the chat of {name} never showed");
}

/// Opens a Settings section from its nav link, with the same second chance as a click that went unheard.
fn settings_section(app: &mut Client, key: &str) {
    open(app, "sidebar.nav.settings", "settings.nav.general");
    for _ in 0..3 {
        app.click(&format!("settings.nav.{key}")).unwrap();
        if app
            .wait(&format!("settings.{key}.title"), Predicate::Exists, 5_000)
            .is_ok()
        {
            return;
        }
    }
    panic!("Settings, {key} never showed");
}

/// Opens a screen from the sidebar.
fn open(app: &mut Client, nav: &str, screen: &str) {
    for _ in 0..3 {
        app.click(nav).unwrap();
        if app.wait(screen, Predicate::Exists, 5_000).is_ok() {
            return;
        }
    }
    panic!("{screen} never showed after clicking {nav}");
}

// ---------------------------------------------------------------------------------------------------------------------
// The pictures

struct Shots {
    dir: PathBuf,
    taken: Vec<&'static str>,
    failed: Vec<(&'static str, String)>,
}

impl Shots {
    fn new() -> Shots {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/readme-shots");
        std::fs::create_dir_all(&dir).unwrap();
        // A picture left by an earlier run must not pass for this run's.
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "png") {
                std::fs::remove_file(entry.path()).unwrap();
            }
        }
        Shots {
            dir,
            taken: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// `prepare` brings the screen to its state; the picture follows once the screen has settled (its entrance motion is
    /// 500 ms). A panic in `prepare` is this picture's failure and nothing else's.
    fn take(&mut self, name: &'static str, app: &mut Client, prepare: impl FnOnce(&mut Client)) {
        self.take_with(name, app, true, prepare);
    }

    /// [`Shots::take`] for a screen that may show something that must not leave the run, and so leaves no `_failed-` picture
    /// of it either (that picture is in the public artifact, and is photographed from wherever the step stopped).
    fn take_without_failure_picture(
        &mut self,
        name: &'static str,
        app: &mut Client,
        prepare: impl FnOnce(&mut Client),
    ) {
        self.take_with(name, app, false, prepare);
    }

    fn take_with(
        &mut self,
        name: &'static str,
        app: &mut Client,
        failure_picture: bool,
        prepare: impl FnOnce(&mut Client),
    ) {
        let out = self.dir.join(format!("{name}.png"));
        let result = catch_unwind(AssertUnwindSafe(|| {
            prepare(app);
            sleep(Duration::from_millis(700));
            common::picture(&out);
        }));
        match result {
            Ok(()) => self.taken.push(name),
            Err(why) => {
                let why = panic_text(why.as_ref());
                println!("NOT TAKEN {name}: {why}");
                if failure_picture {
                    // What the window showed when it went wrong, for whoever reads the artifact (never a README picture:
                    // its name starts with an underscore).
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        common::picture(&self.dir.join(format!("_failed-{name}.png")));
                    }));
                }
                self.failed.push((name, why));
            }
        }
    }

    /// A picture that is not attempted, and why.
    fn skip(&mut self, name: &'static str, why: &str) {
        println!("NOT TAKEN {name}: {why}");
        self.failed.push((name, why.to_owned()));
    }

    /// Runs a part of the tour that is not itself a picture (a daemon that must start, the agents that must be made). A
    /// panic in it ends that part and not the run: the pictures taken so far are kept, and the summary is still printed.
    /// Whether it finished.
    fn attempt(&mut self, what: &'static str, part: impl FnOnce(&mut Shots)) -> bool {
        match catch_unwind(AssertUnwindSafe(|| part(self))) {
            Ok(()) => true,
            Err(why) => {
                let why = panic_text(why.as_ref());
                println!("STEP FAILED {what}: {why}");
                self.failed.push((what, why));
                false
            }
        }
    }

    /// The summary, printed whatever went wrong before it, and the run's verdict: every picture of the shot list or a
    /// failure that names the missing ones.
    fn finish(&self) {
        println!("taken: {:?}", self.taken);
        let missing: Vec<&str> = PICTURES
            .iter()
            .copied()
            .filter(|name| !self.taken.contains(name))
            .collect();
        println!("not taken: {missing:?}");
        assert!(missing.is_empty(), "pictures not taken: {missing:?}");
    }
}

/// The text of a panic, for the run's own log.
fn panic_text(why: &(dyn std::any::Any + Send)) -> String {
    why.downcast_ref::<String>()
        .cloned()
        .or_else(|| why.downcast_ref::<&str>().map(|why| (*why).to_owned()))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------------------
// The invented team

/// A loopback page for Forge's browser: titled Example Orders.
struct Orders {
    port: u16,
}

impl Orders {
    fn start() -> Orders {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = [0u8; 2048];
                    let _ = stream.read(&mut request);
                    let body = "<!doctype html><title>Example Orders</title>\
                        <body style=\"font-family: Segoe UI, sans-serif; margin: 40px; color: #222\">\
                        <h1>Example Orders</h1>\
                        <table cellpadding=\"10\" style=\"border-collapse: collapse; font-size: 18px\">\
                        <tr style=\"background: #eee\"><th align=\"left\">Order</th><th align=\"left\">Customer</th><th align=\"left\">Total</th></tr>\
                        <tr><td>1041</td><td>Harbour Books</td><td>$128.00</td></tr>\
                        <tr><td>1042</td><td>Lantern Cafe</td><td>$64.50</td></tr>\
                        <tr><td>1043</td><td>North Pier Tackle</td><td>$212.25</td></tr>\
                        </table></body>";
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes());
                });
            }
        });
        Orders { port }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

fn text(delta: &str) -> Vec<Value> {
    vec![
        json!({ "type": "text", "delta": delta }),
        json!({ "type": "text_end" }),
    ]
}

fn usage(model: &str, input: u64, cached: u64, output: u64) -> Value {
    json!({ "type": "usage", "model": model, "input": input, "cached_input": cached, "output": output })
}

/// One agent's recorded turns, played one per message in the order the shots send them.
fn fixture(turns: Vec<Value>) -> String {
    json!({ "version": 1, "turns": turns }).to_string()
}

fn turn(expect: &str, events: Vec<Value>) -> Value {
    json!({ "expect": expect, "events": events })
}

/// The files of recorded turns, by agent. Each agent's turns are used in this order:
///
/// * Forge: its chat (a tool line, a Markdown reply, a file card, usage), a mention in Everyone, a request to ask Scout, Scout's
///   answer coming back.
/// * Scout: a mention in Everyone, the request Forge sends, and last a slow turn that stays "working" for the Agents picture.
/// * Atlas: a request that waits on an approval card nobody answers.
/// * Quill: a page opened in its browser, in a chat of its own.
fn fixtures(page: &str) -> Vec<(&'static str, String)> {
    let forge = fixture(vec![
        turn(
            "order code",
            [
                vec![
                    json!({ "type": "tool", "id": "t1", "name": "Reading a file", "summary": "src/orders.rs" }),
                    json!({ "type": "tool_done", "id": "t1", "ok": true }),
                ],
                text("I read the order code. Two things stand out:\n\n- Search only matches the order number.\n- Dates print as `11/3/26`, which is ambiguous."),
                vec![
                    json!({ "type": "mcp", "id": "m1", "tool": "share_file", "arguments": { "path": "summary.md", "caption": "Orders summary" } }),
                    usage("sonnet", 12_400, 9_000, 640),
                ],
            ]
            .concat(),
        ),
        turn(
            "builds green",
            [
                text("Yes, the last three builds are green."),
                vec![usage("haiku", 900, 0, 110)],
            ]
            .concat(),
        ),
        turn(
            "ask Scout",
            [
                vec![json!({ "type": "mcp", "id": "m2", "tool": "message_bot", "arguments": { "bot": "Scout", "message": "Could you review the order search change in Orders app? I want to know if it is safe to merge." } })],
                text("I asked Scout to review it."),
            ]
            .concat(),
        ),
        turn(
            "safe to merge",
            [
                text("Scout says the search change is safe to merge."),
                vec![usage("sonnet", 3_100, 2_000, 180)],
            ]
            .concat(),
        ),
    ]);
    // Quill is the agent with the browser: a chat of its own, short enough that the narrow column beside the browser panel
    // shows all of it (an older tool line there wraps a letter at a time, which a longer history would put in the picture).
    let quill = fixture(vec![turn(
        "orders page",
        [
            vec![json!({ "type": "browser", "id": "b1", "tool": "browser_navigate", "arguments": { "url": page } })],
            text("The Orders page is open. It lists three orders: 1041 for Harbour Books, 1042 for Lantern Cafe and 1043 for North Pier Tackle, 404.75 in all."),
        ]
        .concat(),
    )]);
    let scout = fixture(vec![
        turn(
            "release notes",
            [
                text("The release notes are drafted and wait for your review."),
                vec![usage("haiku", 1_500, 400, 140)],
            ]
            .concat(),
        ),
        turn(
            "review the order search change",
            [
                text("I read the change. It is safe to merge: the search covers names and numbers now, and the tests pass."),
                vec![usage("sonnet", 8_200, 5_000, 420)],
            ]
            .concat(),
        ),
        // Slow: each of its five events waits 60 s (the most a file allows), so Scout is working for five minutes: the three
        // pictures that show it working (Agents in dark and in light, and Atlas's approval between them) take about two, with
        // room for the retries that open a chat or a Settings section.
        json!({ "expect": "run the tests", "delay_ms": 60_000, "events": [
            json!({ "type": "text", "delta": "Running the test suite now. " }),
            json!({ "type": "text", "delta": "Unit tests first. " }),
            json!({ "type": "text", "delta": "Then the end-to-end set. " }),
            json!({ "type": "text", "delta": "Nearly done. " }),
            json!({ "type": "text_end" }),
        ] }),
    ]);
    let atlas = fixture(vec![turn(
        "repository status",
        [
            text("I will look at the repository first."),
            vec![
                json!({ "type": "approval", "id": "ask-1", "tool": "Bash", "summary": "Run git status",
                    "detail": "{ \"command\": \"git status\" }", "rule": "Bash(git status:*)" }),
            ],
        ]
        .concat(),
    )]);
    vec![
        ("Forge.json", forge),
        ("Quill.json", quill),
        ("Scout.json", scout),
        ("Atlas.json", atlas),
        ("default.json", common::QUIET.to_owned()),
    ]
}

/// What an agent is made with.
struct Spec {
    name: &'static str,
    instructions: &'static str,
    /// Always ask, not Own folder.
    ask: bool,
    browser: bool,
    /// Allowed to message these agents.
    can_message: &'static [&'static str],
}

/// Makes an agent through the New agent screen and returns its id; its chat is open after.
fn new_agent(app: &mut Client, spec: &Spec, others: &[(&str, String)]) -> String {
    open(app, "sidebar.nav.agents", "screen.agents");
    app.wait("agents.new", Predicate::Exists, 10_000).unwrap();
    click_until(app, "agents.new", "screen.agents", "screen.new");
    common::tab_to(app, "bot_form.name");
    app.text(spec.name).unwrap();
    common::tab_to(app, "bot_form.instructions");
    app.text(spec.instructions).unwrap();
    if spec.ask {
        choose_always_ask(app);
    }
    if spec.browser {
        activate(app, "bot_form.browser");
    }
    if !spec.can_message.is_empty() {
        activate(app, "bot_form.can_message");
        for name in spec.can_message {
            let id = &others.iter().find(|(known, _)| known == name).unwrap().1;
            activate(app, &format!("bot_form.can_message.{id}"));
        }
    }
    click_until(app, "new_bot.create", "screen.new", "screen.chat");
    chat_bot(app)
}

// ---------------------------------------------------------------------------------------------------------------------

/// The first-run screen with the normal daemon, on a machine with neither Claude Code nor Codex.
fn not_installed(shots: &mut Shots, plain: &Path) {
    let root = owner_safety::test_root().unwrap();
    let folder = root.path().join("none");
    std::fs::create_dir_all(&folder).unwrap();
    let mut app = Client::launch_with(
        &common::desktop_exe(),
        root.path(),
        &[("TJOMMIE_DAEMON_EXE", plain.to_str().unwrap())],
    )
    .unwrap();
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
    app.resize(WINDOW.0, WINDOW.1).unwrap();
    shots.take("setup-not-installed-dark", &mut app, |app| {
        // "Checking" gives way to what the daemon found: each card offers Check again, and Codex's says Not installed.
        for provider in ["claude", "codex"] {
            app.wait(
                &format!("setup.{provider}.check"),
                Predicate::Exists,
                60_000,
            )
            .unwrap();
        }
        common::wait_for_text(app, "Not installed", Duration::from_secs(30));
        common::wait_for_text(
            app,
            "Install Claude Code from claude.com/claude-code",
            Duration::from_secs(30),
        );
    });
}

#[test]
#[ignore = "takes the README's pictures; readme-shots.yml runs it on a hosted runner"]
fn e2e_readme_shots() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    assert!(
        daemon_exe().exists(),
        "build the daemon first: cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked"
    );
    let mut shots = Shots::new();

    // One picture with the normal daemon, then the fixtures daemon for the rest. Each part is attempted on its own: a daemon
    // that doesn't start, or an agent that isn't made, ends that part and not the run, and the summary is printed whatever
    // happened.
    match std::env::var_os("TJOMMIE_SHOTS_PLAIN_DAEMON") {
        Some(plain) => {
            shots.attempt("the first run with the normal daemon", |shots| {
                not_installed(shots, Path::new(&plain));
            });
        }
        None => shots.skip(
            "setup-not-installed-dark",
            "TJOMMIE_SHOTS_PLAIN_DAEMON names no normal daemon (a PC with Claude Code or Codex must not run it)",
        ),
    }
    shots.attempt("the tour with the fixtures daemon", tour);
    shots.finish();
}

/// Every picture that uses the fixtures daemon, in order.
fn tour(shots: &mut Shots) {
    let pages = Orders::start();
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(
        root.path(),
        &fixtures(&pages.url())
            .iter()
            .map(|(name, text)| (*name, text.as_str()))
            .collect::<Vec<_>>(),
    );
    app.resize(WINDOW.0, WINDOW.1).unwrap();

    // First run, before any agent: both providers signed in.
    shots.take("setup-signed-in-dark", &mut app, |app| {
        for provider in ["claude", "codex"] {
            app.wait(
                &format!("setup.{provider}.other_account"),
                Predicate::Exists,
                30_000,
            )
            .unwrap();
        }
        common::wait_for_text(app, "fixtures@example.com", Duration::from_secs(30));
    });

    // The team. Forge is made last of the four so that it is the open chat for the shots that follow.
    let mut ids: Vec<(&str, String)> = Vec::new();
    let quill = Spec {
        name: "Quill",
        instructions: "Keeps the notes folder tidy and writes the weekly cleanup report.",
        ask: false,
        browser: true,
        can_message: &[],
    };
    let atlas = Spec {
        name: "Atlas",
        instructions: "Checks repositories and reports what changed. Asks before running anything.",
        ask: true,
        browser: false,
        can_message: &[],
    };
    let scout = Spec {
        name: "Scout",
        instructions: "Reviews changes and runs the test suite. Asks before running anything.",
        ask: true,
        browser: false,
        can_message: &[],
    };
    let forge = Spec {
        name: "Forge",
        instructions: "Builds and fixes the Orders app, and explains what it changed.",
        ask: false,
        browser: false,
        can_message: &["Scout"],
    };
    // The New agent picture is the form for Forge, filled in and not saved.
    for spec in [&quill, &atlas, &scout] {
        let id = new_agent(&mut app, spec, &ids);
        ids.push((spec.name, id));
    }
    shots.take("new-agent-dark", &mut app, |app| {
        open(app, "sidebar.nav.agents", "screen.agents");
        click_until(app, "agents.new", "screen.agents", "screen.new");
        common::tab_to(app, "bot_form.name");
        app.text(forge.name).unwrap();
        common::tab_to(app, "bot_form.instructions");
        app.text(forge.instructions).unwrap();
        choose_always_ask(app);
    });
    // The form in that picture is not saved: Forge is made for real now.
    let forge_id = new_agent(&mut app, &forge, &ids);
    ids.push(("Forge", forge_id.clone()));
    let bots_folder = |id: &str| root.path().join("bots").join(id);
    std::fs::write(
        bots_folder(&forge_id).join("summary.md"),
        "# Orders summary\n\n- Search matches the order number only.\n- Dates print as `11/3/26`.\n- The export skips cancelled orders.\n",
    )
    .unwrap();

    // Forge's chat: a tool line, a reply with a list, and a file card.
    shots.take("chat-dark", &mut app, |app| {
        open_chat(app, "Forge");
        say(app, "Please look at the order code and summarise it");
        until(app, "no file card came", |app| {
            !ids_like(app, "file.", ".preview").is_empty()
        });
    });

    // Everyone: a mention of Forge and one of Scout, each answered.
    shots.take("room-dark", &mut app, |app| {
        common::mention(app, "Forge", "are the builds green?");
        wait_post(app, "Forge: Yes, the last three builds are green.");
        common::mention(app, "Scout", "are the release notes ready?");
        wait_post(
            app,
            "Scout: The release notes are drafted and wait for your review.",
        );
    });

    // The exchange: Forge asks Scout, Scout answers, and the exchange opens beside Everyone.
    shots.take("exchange-dark", &mut app, |app| {
        open_chat(app, "Forge");
        say(
            app,
            "Please ask Scout whether the search change is safe to merge",
        );
        until(app, "no View thread link came", |app| {
            !ids_like(app, "exchange.", ".open").is_empty()
        });
        // Scout's answer is posted in Everyone too: once it is there, the exchange is complete.
        open(app, "sidebar.nav.rooms", "screen.rooms");
        click_until(app, "rooms.room.team.open", "screen.rooms", "screen.room");
        until(app, "Scout's answer isn't in Everyone", |app| {
            app.snapshot().unwrap().nodes.iter().any(|node| {
                node.id.starts_with("room.post.")
                    && node.text.starts_with("Scout: I read the change")
            })
        });
        // Forge's request is a post of its own, with Open exchange under it.
        until(app, "no Open exchange link in Everyone", |app| {
            !ids_like(app, "room.post.", ".exchange").is_empty()
        });
        let open = ids_like(app, "room.post.", ".exchange").pop().unwrap();
        for _ in 0..3 {
            app.click(&open).unwrap();
            sleep(Duration::from_secs(2));
            if !ids_like(app, "exchange.", ".view").is_empty() {
                break;
            }
        }
        until(app, "the exchange didn't open beside Everyone", |app| {
            !ids_like(app, "exchange.", ".view").is_empty()
        });
    });

    // Projects: a team on a small Git repository the test makes, and two saved tasks.
    let repository = root.path().join("sample-project");
    shots.take("projects-dark", &mut app, |app| {
        std::fs::create_dir_all(&repository).unwrap();
        git(&repository, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(repository.join("README.md"), "# Orders app\n").unwrap();
        git(&repository, &["add", "README.md"]);
        git(
            &repository,
            &["commit", "--quiet", "-m", "Start the Orders app"],
        );
        open(app, "sidebar.nav.projects", "screen.projects");
        activate(app, "projects.new");
        app.wait("project_form.name", Predicate::Exists, 10_000)
            .unwrap();
        app.answer_path_prompt(&repository).unwrap();
        activate(app, "project_form.register");
        common::tab_to(app, "project_form.name");
        app.text("Orders app").unwrap();
        activate(app, &format!("project_form.role.{forge_id}.developer"));
        let scout_id = &ids.iter().find(|(name, _)| *name == "Scout").unwrap().1;
        activate(app, &format!("project_form.role.{scout_id}.reviewer"));
        activate(app, "project_form.save");
        wait_gone(app, "project_form.name");
        let team = {
            let mut found = None;
            until(app, "the new team isn't in the list", |app| {
                found = ids_like(app, "projects.team.", "")
                    .into_iter()
                    .find(|id| id.matches('.').count() == 2);
                found.is_some()
            });
            found.unwrap()
        };
        activate(app, &team);
        app.wait(
            "projects.name",
            Predicate::Text("Orders app".into()),
            10_000,
        )
        .unwrap();
        for (title, instructions) in [
            (
                "Add order search",
                "Let the person search orders by customer name as well as by number.",
            ),
            (
                "Fix date format",
                "Print order dates as 3 Nov 2026 everywhere, not 11/3/26.",
            ),
        ] {
            // From the team's screen: a saved task's detail is up after the first, so the team comes back first, and an Enter
            // that goes unheard (a runner showed one) is made again.
            let mut opened = false;
            for _ in 0..4 {
                if app.snapshot().unwrap().node("projects.detail").is_some() {
                    activate(app, &team);
                    sleep(Duration::from_millis(500));
                }
                activate(app, "projects.new_task");
                if app
                    .wait("task_form.title", Predicate::Exists, 6_000)
                    .is_ok()
                {
                    opened = true;
                    break;
                }
            }
            assert!(opened, "the New task dialog never opened");
            common::tab_to(app, "task_form.title");
            app.text(title).unwrap();
            common::tab_to(app, "task_form.instructions");
            app.text(instructions).unwrap();
            activate(app, "task_form.save");
            wait_gone(app, "task_form.title");
        }
        // Saving a task opens it; the picture is the team, with both tasks in its list. The team's own entry brings the team
        // back, and a runner showed that the first try can go unheard: so the screen is asked for until it is the team's
        // (its New task button, and no task's detail), and left for Agents and come back to if it still is not.
        let team_shown = |app: &mut Client| {
            let snapshot = app.snapshot().unwrap();
            snapshot.node("projects.detail").is_none()
                && snapshot.node("projects.new_task").is_some()
                && snapshot
                    .nodes
                    .iter()
                    .filter(|node| node.id.starts_with("projects.task."))
                    .count()
                    >= 2
        };
        for attempt in 0..4 {
            if attempt == 2 {
                open(app, "sidebar.nav.agents", "screen.agents");
                open(app, "sidebar.nav.projects", "screen.projects");
            }
            activate(app, &team);
            let waited = Instant::now();
            while !team_shown(app) && waited.elapsed() < Duration::from_secs(6) {
                sleep(Duration::from_millis(150));
            }
            if team_shown(app) {
                break;
            }
        }
        assert!(team_shown(app), "the team never showed with its two tasks");
    });
    shots.take("project-task-dark", &mut app, |app| {
        let task = app
            .snapshot()
            .unwrap()
            .nodes
            .into_iter()
            .find(|node| {
                node.id.starts_with("projects.task.") && node.text.contains("Add order search")
            })
            .expect("the task Add order search")
            .id;
        activate(app, &task);
        app.wait("projects.detail", Predicate::Exists, 10_000)
            .unwrap();
    });

    // Routines: Forge's Morning summary and Quill's Weekly cleanup, made in the editor.
    shots.take("routines-dark", &mut app, |app| {
        open(app, "sidebar.nav.routines", "screen.routines");
        // A new routine starts on weekdays at 09:00. Days are numbered from Sunday (0), so Monday is 1 and Friday 5.
        for (agent, name, prompt, days, time) in [
            (
                "Forge",
                "Morning summary",
                "Summarise what changed in the Orders app overnight.",
                &[1, 2, 3, 4, 5][..],
                "08:00",
            ),
            (
                "Quill",
                "Weekly cleanup",
                "Tidy the notes folder and write the weekly cleanup report.",
                &[5][..],
                "16:00",
            ),
        ] {
            let id = &ids.iter().find(|(known, _)| *known == agent).unwrap().1;
            activate(app, &format!("routines.{id}.add"));
            app.wait("routine.name", Predicate::Exists, 10_000).unwrap();
            common::tab_to(app, "routine.name");
            app.text(name).unwrap();
            common::tab_to(app, "routine.prompt");
            app.text(prompt).unwrap();
            for day in 0..=6 {
                let id = format!("routine.day.{day}");
                let on = app
                    .snapshot()
                    .unwrap()
                    .node(&id)
                    .is_some_and(|node| node.toggled);
                if on != days.contains(&day) {
                    activate(app, &id);
                }
            }
            // The 09:00 it starts with goes, and the time wanted is typed into the field Add time opens.
            activate(app, "routine.time.0.remove");
            activate(app, "routine.add_time");
            app.wait("routine.new_time", Predicate::Exists, 10_000)
                .unwrap();
            app.text(time).unwrap();
            app.key("enter").unwrap();
            activate(app, "routine.save");
            wait_gone(app, "routine.name");
        }
    });
    shots.take("routine-editor-dark", &mut app, |app| {
        open(app, "sidebar.nav.routines", "screen.routines");
        let forge = ids
            .iter()
            .find(|(known, _)| *known == "Forge")
            .unwrap()
            .1
            .clone();
        let mut row = None;
        until(app, "Forge's routine isn't in the list", |app| {
            row = ids_like(app, &format!("routines.{forge}."), "")
                .into_iter()
                .find(|id| {
                    !id.ends_with(".add") && !id.ends_with(".group") && !id.ends_with(".none")
                });
            row.is_some()
        });
        activate(app, &row.unwrap());
        app.wait("routine.name", Predicate::Exists, 10_000).unwrap();
    });
    let _ = app.key("escape");

    // The agent's browser, and its logins menu.
    shots.take("browser-dark", &mut app, |app| {
        open_chat(app, "Quill");
        say(app, "Please open the orders page");
        until(app, "the browser tool didn't finish", |app| {
            app.snapshot().unwrap().nodes.iter().any(|node| {
                node.id.starts_with("tools.")
                    && node.id.ends_with(".b1")
                    && node.text.starts_with("browser_navigate, Done")
            })
        });
        if app.snapshot().unwrap().node("browser.panel").is_none() {
            app.key("ctrl-b").unwrap();
        }
        app.wait("browser.panel", Predicate::Exists, 10_000)
            .unwrap();
        app.click("browser.pane.browser").unwrap();
        common::wait_for_text(app, "Example Orders", Duration::from_secs(30));
    });
    shots.take("logins-menu-dark", &mut app, |app| {
        app.click("browser.logins").unwrap();
        common::wait_for_text(app, "Fill a saved login", Duration::from_secs(10));
    });
    let _ = app.key("escape");

    // Settings: pairing, accounts, usage.
    //
    // Devices shows the relay's address in a field, seeded with the app's default, which is a person's account name on a
    // workers.dev host. It is replaced before anything of Devices is photographed, as a step of its own (never inside a
    // picture's step, where a failure would photograph the default, into the public artifact): the invented address is
    // typed and saved, and the Use the public relay button, which only shows once the saved address is not the default, is
    // the proof that it is. From then on every render of Devices, behind the pairing dialog and in a failure picture too,
    // shows the invented one. If that step fails, neither Devices picture is taken at all. Nothing dials the relay: with no
    // device and no pairing code shown, the daemon connects to nothing.
    let relay_hidden = shots.attempt("replacing the relay address before Devices", |_| {
        settings_section(&mut app, "devices");
        app.click("settings.devices.relay.address").unwrap();
        app.key("ctrl-a").unwrap();
        app.text("https://relay.example.com").unwrap();
        until(
            &mut app,
            "Save didn't offer to save the invented address",
            |app| {
                app.snapshot()
                    .unwrap()
                    .node("settings.devices.relay.save")
                    .is_some_and(|node| node.enabled)
            },
        );
        app.click("settings.devices.relay.save").unwrap();
        until(&mut app, "the invented relay address wasn't saved", |app| {
            app.snapshot()
                .unwrap()
                .node("settings.devices.relay.public")
                .is_some()
        });
    });
    if relay_hidden {
        shots.take_without_failure_picture("devices-dark", &mut app, |app| {
            settings_section(app, "devices");
        });
        shots.take_without_failure_picture("pair-dialog-dark", &mut app, |app| {
            activate(app, "settings.devices.pair");
            app.wait("dialog.title", Predicate::Exists, 10_000).unwrap();
        });
        let _ = app.key("escape");
    } else {
        for name in ["devices-dark", "pair-dialog-dark"] {
            shots.skip(
                name,
                "the relay address could not be replaced, so nothing of Devices is photographed",
            );
        }
    }
    shots.take("accounts-dark", &mut app, |app| {
        settings_section(app, "accounts");
    });
    shots.take("usage-dark", &mut app, |app| {
        settings_section(app, "usage");
    });

    // Last, because they leave agents busy: Atlas waits on an approval and Scout works. The Agents screen shows both.
    shots.take("agents-dark", &mut app, |app| {
        open_chat(app, "Atlas");
        say(app, "Please check the repository status");
        until(app, "no approval card came", |app| {
            !ids_like(app, "approval.", ".approve").is_empty()
        });
        open_chat(app, "Scout");
        say(app, "Please run the tests");
        open(app, "sidebar.nav.agents", "screen.agents");
        common::wait_for_text(app, "Needs you", Duration::from_secs(30));
        show_every_card(app, &forge_id);
    });
    shots.take("approval-dark", &mut app, |app| {
        open_chat(app, "Atlas");
        until(app, "the approval card is gone", |app| {
            !ids_like(app, "approval.", ".approve").is_empty()
        });
    });
    shots.take("agents-light", &mut app, |app| {
        settings_section(app, "appearance");
        app.wait("settings.appearance.theme.light", Predicate::Exists, 10_000)
            .unwrap();
        app.click("settings.appearance.theme.light").unwrap();
        until(app, "Light wasn't chosen", |app| {
            app.snapshot()
                .unwrap()
                .node("settings.appearance.theme.light")
                .is_some_and(|node| node.toggled)
        });
        open(app, "sidebar.nav.agents", "screen.agents");
        common::wait_for_text(app, "Needs you", Duration::from_secs(30));
        show_every_card(app, &forge_id);
    });
}

/// The Agents screen at 1000 by 700 shows its Needs you group and one row of cards; Tab to the last card's Open chat
/// scrolls the screen far enough that all four cards are in the picture, and the first row still shows Atlas waiting.
fn show_every_card(app: &mut Client, last: &str) {
    common::tab_to(app, &format!("agents.card.{last}.open"));
}

/// Autonomy, on the New agent form: Own folder is the chosen one, a radio group is one Tab stop, and the arrows move the
/// choice, so Always ask is one arrow to the left. The row stays in view (Tab brought it there).
fn choose_always_ask(app: &mut Client) {
    common::tab_to(app, "bot_form.autonomy.levels.folder");
    app.key("left").unwrap();
    until(app, "Always ask wasn't chosen", |app| {
        app.snapshot()
            .unwrap()
            .node("bot_form.autonomy.levels.ask")
            .is_some_and(|node| node.selected || node.toggled)
    });
}

/// A post in the open room that says `text`.
fn wait_post(app: &mut Client, text: &str) {
    until(app, &format!("no post says {text:?}"), |app| {
        let snapshot = app.snapshot().unwrap();
        snapshot
            .nodes
            .iter()
            .any(|node| node.id.starts_with("room.post.") && node.text == text)
    });
}

/// Runs git in `folder`, with an invented identity, and fails if it does. Nothing of the machine's own git setup reaches it:
/// not its global or system configuration (a `commit.gpgsign` there would fail the commit), and not an author or committer
/// named in the environment (which outranks `-c`).
fn git(folder: &Path, args: &[&str]) {
    let out = desktop_host::spawn::command("git")
        .env("GIT_CONFIG_GLOBAL", "nul")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .args([
            "-c",
            "user.name=Sample Author",
            "-c",
            "user.email=sample@example.invalid",
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
