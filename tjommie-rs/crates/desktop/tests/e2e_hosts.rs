//! The app's three clients of the daemon, end to end through the test driver (Phase 3 host wiring; plan T3.2, T3.6, review
//! focus 2): the app starts `/browser-host`, `/vault-host` and `/worker-host` for the daemon it started, an agent's browser
//! tool reaches the app's own WebView2 and the panel shows the page, Logins is no longer unavailable, and every new
//! daemon (a new port and token) gets new clients while the panel stays open. The agents' turns are recorded fixtures
//! (`common`); no provider runs. Run with the e2e identity, the daemon built with the switch beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_hosts --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

/// Settings, Logins: the vault says what state it is in. While no vault host answers, or the daemon has no worker host,
/// that is "Logins unavailable".
fn logins_state(app: &mut Client) -> String {
    app.click("sidebar.nav.settings").unwrap();
    app.wait("settings.nav.logins", Predicate::Exists, 10_000)
        .unwrap();
    app.click("settings.nav.logins").unwrap();
    app.wait("settings.logins.state", Predicate::Exists, 10_000)
        .unwrap();
    // "Checking Logins…" gives way to what the vault answered.
    let started = std::time::Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        let said = snapshot
            .node("settings.logins.state")
            .map(|node| node.text.clone())
            .unwrap_or_default();
        if !said.is_empty() && !said.starts_with("Checking Logins") {
            return said;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "Logins never answered: {said:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The agent's chat with its browser panel open on the Browser pane.
fn open_panel(app: &mut Client, agent: &str) {
    let recent = common::recent(app, agent);
    app.click(&recent).unwrap();
    app.wait("screen.chat", Predicate::Exists, 10_000).unwrap();
    if app.snapshot().unwrap().node("browser.panel").is_none() {
        app.key("ctrl-b").unwrap();
    }
    app.wait("browser.panel", Predicate::Exists, 10_000)
        .unwrap();
    app.click("browser.pane.browser").unwrap();
}

#[test]
fn e2e_the_app_starts_all_three_hosts() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    let pages = common::Pages::start();
    let forge = common::browsing(&[("open the page", &pages.url("/"), "Opened it.")]);
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(
        root.path(),
        &[("Forge.json", &forge), ("default.json", common::QUIET)],
    );

    // /vault-host and /worker-host: the vault is asked through the app's own gate, which asks the daemon whether isolation
    // is available. Without the hosts this reads "Logins unavailable".
    let said = logins_state(&mut app);
    assert!(said.starts_with("Logins locked."), "Logins said {said:?}");

    // /browser-host: an agent's browser tool call (the recorded turn calls browser_navigate through the daemon's proxy)
    // reaches the app's WebView2, and the panel shows the page it opened.
    common::make_agent(&mut app, "Forge", true);
    common::mention(&mut app, "Forge", "open the page");
    common::wait_for_text(&mut app, "Forge: Opened it.", Duration::from_secs(90));
    open_panel(&mut app, "Forge");
    common::wait_for_text(&mut app, "Fixture page", Duration::from_secs(30));
}

/// The process ids of the app's daemon: `tjommie-daemon.exe` children of the app.
fn daemons(app: u32) -> Vec<u32> {
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

#[test]
fn e2e_daemon_restart_with_panel_open() {
    assert_eq!(identity::CURRENT.name, "e2e");
    let pages = common::Pages::start();
    // Two agents, each with one turn that opens a page. A fixtures daemon plays each file's turns from the start, so
    // the agent used after the restart is a second one: its page is a different one.
    let forge = common::browsing(&[("open the page", &pages.url("/"), "Opened the first.")]);
    let gale = common::browsing(&[("open the page", &pages.url("/two"), "Opened the second.")]);
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(
        root.path(),
        &[
            ("Forge.json", &forge),
            ("Gale.json", &gale),
            ("default.json", common::QUIET),
        ],
    );
    common::make_agent(&mut app, "Forge", true);
    common::make_agent(&mut app, "Gale", true);
    common::mention(&mut app, "Forge", "open the page");
    common::wait_for_text(
        &mut app,
        "Forge: Opened the first.",
        Duration::from_secs(90),
    );
    assert!(logins_state(&mut app).starts_with("Logins locked."));

    // The panel is open on the page the agent opened.
    open_panel(&mut app, "Forge");
    common::wait_for_text(&mut app, "Fixture page", Duration::from_secs(30));

    // The daemon dies under the open panel. The app starts another with new tokens, and its three clients start with them.
    let old = daemons(app.pid());
    assert_eq!(old.len(), 1, "the app's daemon: {old:?}");
    let killed = desktop_host::spawn::command("taskkill")
        .args(["/F", "/T", "/PID", &old[0].to_string()])
        .output()
        .unwrap();
    assert!(killed.status.success(), "the daemon was not stopped");
    let started = std::time::Instant::now();
    let new = loop {
        let now = daemons(app.pid());
        if let Some(pid) = now.iter().find(|pid| !old.contains(pid)) {
            break *pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the app did not restart its daemon"
        );
        std::thread::sleep(Duration::from_millis(300));
    };
    assert_ne!(new, old[0]);
    // The panel stays open through it.
    assert!(
        app.snapshot().unwrap().node("browser.panel").is_some(),
        "the panel closed when the daemon restarted"
    );

    // The new daemon's vault host and worker host are served: Logins reads locked again (not unavailable), and the vault
    // locked with the restart.
    let said = logins_state(&mut app);
    assert!(
        said.starts_with("Logins locked."),
        "Logins after the restart said {said:?}"
    );

    // The new daemon's browser host is served: the second agent's browser tool call reaches the app's browser.
    common::mention(&mut app, "Gale", "open the page");
    common::wait_for_text(
        &mut app,
        "Gale: Opened the second.",
        Duration::from_secs(90),
    );
    open_panel(&mut app, "Gale");
    common::wait_for_text(&mut app, "Second page", Duration::from_secs(30));
}

#[test]
fn e2e_closing_the_window_stops_the_clients_and_the_daemon_before_the_app_exits() {
    assert_eq!(identity::CURRENT.name, "e2e");
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(root.path(), &[("default.json", common::QUIET)]);
    // The vault is up, so there is something to lock.
    assert!(logins_state(&mut app).starts_with("Logins locked."));
    let daemon = daemons(app.pid());
    assert_eq!(daemon.len(), 1, "the app's daemon: {daemon:?}");

    // Close the window as the person does (WM_CLOSE, which taskkill without /F sends). The app holds the close while it
    // stops its clients and the daemon, then exits.
    let closed = desktop_host::spawn::command("taskkill")
        .args(["/PID", &app.pid().to_string()])
        .output()
        .unwrap();
    assert!(closed.status.success(), "the window was not asked to close");
    let started = std::time::Instant::now();
    while !app.has_exited().unwrap() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the app did not exit after its window closed"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    // The app held the close and stopped everything first (its log says so, in order), and the daemon is gone.
    let log = std::fs::read_to_string(root.path().join("logs").join("desktop.log")).unwrap();
    let closing =
        log.find("Closing: the browser, Logins and worker clients stop, then the daemon.");
    let closed = log.find("Closed: the clients and the daemon have stopped.");
    assert!(
        closing.is_some() && closed > closing,
        "the app did not run its graceful quit: {log}"
    );
    assert!(
        daemons(app.pid()).is_empty() && !process_alive(daemon[0]),
        "the daemon outlived the app"
    );
}

fn process_alive(pid: u32) -> bool {
    let command = format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue) -ne $null");
    let output = desktop_host::spawn::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim() == "True"
}
