//! What the host-wiring e2e tests share (Phase 3 host wiring): the app on a temp root with the provider-fixtures daemon, an
//! agent made through the UI, a mention sent from the Everyone room, and a loopback page server. The agents' turns are
//! recorded fixtures, invented in each test; no provider runs and no account is touched. Needs the daemon built with the
//! switch, beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test <name> --locked`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use desktop::test_driver::{Client, Predicate};

/// Test only: the desktop exe the e2e tests launch in place of the one cargo built. An acceptance run on a runner installs a
/// packaged test-identity build (with the `test-driver` feature) and runs the e2e set against it. Unset, nothing changes.
pub const E2E_EXE_VAR: &str = "TJOMMIE_E2E_EXE";

/// The exe an e2e test launches: `over` (the value of [`E2E_EXE_VAR`]) when it is set, else `built`, the one cargo built. The
/// value must be the absolute path of an existing file, and anything else is an error that says so, never a silent fallback to
/// the built exe: a run that meant to test the installed build must not test another one. An empty value counts as set.
pub fn resolve_desktop_exe(over: Option<&OsStr>, built: &str) -> Result<String, String> {
    let Some(value) = over else {
        return Ok(built.to_owned());
    };
    let path = Path::new(value);
    let shown = path.display();
    if value.is_empty() {
        return Err(format!(
            "{E2E_EXE_VAR} is set but empty. Give the absolute path of the desktop exe to test, or leave it unset to test the one cargo built."
        ));
    }
    if !path.is_absolute() {
        return Err(format!(
            "{E2E_EXE_VAR} must be an absolute path to the desktop exe, but {shown} is not absolute."
        ));
    }
    if !path.exists() {
        return Err(format!(
            "{E2E_EXE_VAR} names {shown}, which does not exist."
        ));
    }
    if !path.is_file() {
        return Err(format!(
            "{E2E_EXE_VAR} must name the desktop exe, a file, but {shown} is not a file."
        ));
    }
    path.to_str().map(str::to_owned).ok_or_else(|| {
        format!("{E2E_EXE_VAR} names {shown}, which is not valid text, so it can't be launched.")
    })
}

/// The desktop exe the e2e tests launch ([`resolve_desktop_exe`] over this process's environment). A bad
/// [`E2E_EXE_VAR`] fails the test with the reason.
pub fn desktop_exe() -> String {
    match resolve_desktop_exe(
        std::env::var_os(E2E_EXE_VAR).as_deref(),
        env!("CARGO_BIN_EXE_tjommie-desktop"),
    ) {
        Ok(exe) => exe,
        Err(reason) => panic!("{reason}"),
    }
}

/// The debug daemon beside the app, built with `--features provider-fixtures`.
pub fn daemon_exe() -> PathBuf {
    PathBuf::from(desktop_exe()).with_file_name("tjommie-daemon.exe")
}

/// A fixture for an agent that is never messaged: its folder needs a file the daemon can read, or it refuses to start.
pub const QUIET: &str = r#"{ "version": 1, "turns": [ { "events": [
  { "type": "text", "delta": "Nothing to do." }, { "type": "text_end" } ] } ] }"#;

/// The app on `root` with the fixtures daemon, up to the Agents screen. `fixtures` are `(file name, contents)`: each file
/// is an agent's recorded turns (`<agent name>.json`, or `default.json`).
pub fn launch(root: &Path, fixtures: &[(&str, &str)]) -> Client {
    let mut app = launch_args(root, fixtures, &[]);
    // A daemon without the switch refuses the variable and never starts: this wait is where that shows.
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
    app
}

/// `launch`, with command-line arguments for the app (`--hidden`), and without waiting for a screen: a window that starts in
/// the tray draws nothing to wait for.
pub fn launch_args(root: &Path, fixtures: &[(&str, &str)], args: &[&str]) -> Client {
    assert!(
        daemon_exe().exists(),
        "build the daemon first: cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked"
    );
    let folder = root.join("provider-fixtures");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, contents) in fixtures {
        std::fs::write(folder.join(name), contents).unwrap();
    }
    Client::launch_args(
        &desktop_exe(),
        root,
        &[
            ("TJOMMIE_DAEMON_EXE", daemon_exe().to_str().unwrap()),
            ("TJOMMIE_PROVIDER_FIXTURES", folder.to_str().unwrap()),
        ],
        args,
    )
    .unwrap()
}

/// The process ids of the app's daemon: `tjommie-daemon.exe` children of the app.
pub fn daemons(app: u32) -> Vec<u32> {
    let filter = format!("ParentProcessId={app} and Name='tjommie-daemon.exe'");
    let command = format!("(Get-CimInstance Win32_Process -Filter \"{filter}\").ProcessId");
    let output = desktop_host::spawn::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// Whether process `pid` is running.
pub fn process_alive(pid: u32) -> bool {
    let command = format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue) -ne $null");
    let output = desktop_host::spawn::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim() == "True"
}

/// Asks the app's window to close as the person does (WM_CLOSE, which `taskkill` without /F sends).
pub fn close_window(app: u32) {
    let closed = desktop_host::spawn::command("taskkill")
        .args(["/PID", &app.to_string()])
        .output()
        .unwrap();
    assert!(closed.status.success(), "the window was not asked to close");
}

/// Polls `condition` until it holds, or fails after `timeout` saying `what`.
pub fn eventually(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < timeout, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The id of the control that has the keyboard focus.
fn focused(app: &mut Client) -> Option<String> {
    app.snapshot()
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.focused)
        .map(|node| node.id)
}

/// Tabs until `id` has the keyboard focus. The screen scrolls the focused control into view, so this reaches a control
/// however short the window is, which a click does not: the driver's click is a mouse event at the control's middle, and
/// one below the fold lands outside the window and does nothing, and says nothing.
pub fn tab_to(app: &mut Client, id: &str) {
    let mut passed = Vec::new();
    for _ in 0..80 {
        let before = focused(app);
        app.key("tab").unwrap();
        let started = Instant::now();
        let now = loop {
            let now = focused(app);
            if now != before || started.elapsed() > Duration::from_millis(600) {
                break now;
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        if now.as_deref() == Some(id) {
            // The scroll that brings it into view comes a frame after the focus does.
            std::thread::sleep(Duration::from_millis(150));
            return;
        }
        passed.extend(now);
    }
    panic!("no Tab reaches {id}; it passed {passed:?}");
}

/// Makes an agent through the New agent screen, with its browser on or off, and lands on its chat.
pub fn make_agent(app: &mut Client, name: &str, browser: bool) {
    // The size every e2e runs at, the same on every machine: it fits a hosted runner's 1024 by 768 display. A taller window
    // does not exist there (Windows holds a window to the display), so nothing here may depend on one.
    app.resize(1000., 700.).unwrap();
    app.click("sidebar.nav.agents").unwrap();
    app.wait("agents.new", Predicate::Exists, 10_000).unwrap();
    app.click("agents.new").unwrap();
    app.wait("screen.new", Predicate::Exists, 10_000).unwrap();
    app.click("bot_form.name").unwrap();
    app.text(name).unwrap();
    if browser {
        // By keyboard: the switch is below the fold in a window that fits the display, where a click on it misses silently
        // and the agent is made with its browser off (the fixtures daemon then says "this agent's browser is off").
        tab_to(app, "bot_form.browser");
        app.key("enter").unwrap();
        let started = Instant::now();
        while !app
            .snapshot()
            .unwrap()
            .node("bot_form.browser")
            .is_some_and(|node| node.toggled)
        {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the browser switch did not turn on"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    app.click("new_bot.create").unwrap();
    app.wait("screen.chat", Predicate::Exists, 30_000).unwrap();
}

/// The id of the Recents entry named `name`.
pub fn recent(app: &mut Client, name: &str) -> String {
    let started = Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        let found = snapshot
            .nodes
            .iter()
            .find(|node| node.id.starts_with("sidebar.recent.") && node.text.contains(name));
        if let Some(node) = found {
            return node.id.clone();
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no Recents entry for {name}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits until some node's text contains `needle`; returns that text.
pub fn wait_for_text(app: &mut Client, needle: &str, timeout: Duration) -> String {
    let started = Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        if let Some(node) = snapshot
            .nodes
            .iter()
            .find(|node| node.text.contains(needle))
        {
            return node.text.clone();
        }
        assert!(
            started.elapsed() < timeout,
            "no node says {needle:?}: {:?}",
            snapshot
                .nodes
                .iter()
                .filter(|node| !node.text.is_empty())
                .map(|node| (node.id.as_str(), node.text.as_str()))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Everyone, from the Rooms list, with a message that mentions `agent`: the daemon hands it to the agent, which plays its
/// next recorded turn.
pub fn mention(app: &mut Client, agent: &str, message: &str) {
    app.click("sidebar.nav.rooms").unwrap();
    app.wait("rooms.room.team.open", Predicate::Exists, 10_000)
        .unwrap();
    app.click("rooms.room.team.open").unwrap();
    app.wait("screen.room", Predicate::Text("Everyone".into()), 10_000)
        .unwrap();
    app.click("room.composer.field").unwrap();
    let start: String = agent.chars().take(3).collect();
    app.text(&format!("@{start}")).unwrap();
    app.wait("room.mentions.0", Predicate::Text(agent.into()), 10_000)
        .unwrap();
    app.key("enter").unwrap();
    app.text(message).unwrap();
    app.key("enter").unwrap();
}

/// A page server on loopback, for an agent's browser to open: `/` is "Fixture page", `/two` is "Second page".
pub struct Pages {
    pub port: u16,
}

impl Pages {
    pub fn start() -> Pages {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = [0u8; 2048];
                    let read = stream.read(&mut request).unwrap_or(0);
                    let text = String::from_utf8_lossy(request.get(..read).unwrap_or(&[]));
                    let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let (title, heading) = if path == "/two" {
                        ("Second page", "The second page")
                    } else {
                        ("Fixture page", "The fixture page")
                    };
                    let body = format!("<!doctype html><title>{title}</title><h1>{heading}</h1>");
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes());
                });
            }
        });
        Pages { port }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

/// An agent's recorded turns: one per message, each opening the page at `urls[n]` through the browser tool, then saying
/// `said[n]`. The messages must contain `expect[n]`.
pub fn browsing(turns: &[(&str, &str, &str)]) -> String {
    let turns: Vec<String> = turns
        .iter()
        .enumerate()
        .map(|(at, (expect, url, said))| {
            format!(
                r#"{{ "expect": "{expect}", "events": [
                    {{ "type": "browser", "id": "b{at}", "tool": "browser_navigate", "arguments": {{ "url": "{url}" }} }},
                    {{ "type": "text", "delta": "{said}" }}, {{ "type": "text_end" }} ] }}"#
            )
        })
        .collect();
    format!(r#"{{ "version": 1, "turns": [ {} ] }}"#, turns.join(","))
}
