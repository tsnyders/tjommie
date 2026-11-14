//! T3.1: the engine's lazy environment and the visibility state machine, against the real WebView2 runtime, in a
//! plain Win32 parent window, with fixtures on loopback and a temp root. No real site is ever visited.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Rig, webview_processes};
use desktop_host::browser::engine::DeleteOutcome;
use desktop_host::browser::lifecycle::{MemoryLevel, Placement};
use desktop_host::browser::{flags, policy, profiles};
use windows::Win32::Foundation::RECT;

fn parked(rect: Option<RECT>) -> bool {
    rect.is_some_and(|rect| rect.right < -1000)
}

#[test]
fn no_environment_until_open() {
    let mut rig = Rig::new();
    let folder = rig.root.path().to_path_buf();
    // Starting the engine, showing windows and pumping for a while creates no browser process.
    rig.ui.settle(Duration::from_secs(3));
    assert!(
        webview_processes(&folder).is_empty(),
        "a browser started without a bot"
    );
    assert!(!folder.join("data").join("webview2").exists());
    rig.open("alpha");
    assert!(
        !webview_processes(&folder).is_empty(),
        "open starts the browser"
    );
    assert!(folder.join("data").join("webview2").exists());
}

#[test]
fn browser_process_runs_with_the_exact_flags() {
    let mut rig = Rig::new();
    rig.open("alpha");
    let flags = flags::browser_arguments();
    let processes = webview_processes(rig.root.path());
    assert!(
        processes.iter().any(|p| p
            .command_line
            .as_deref()
            .is_some_and(|line| line.contains(&flags))),
        "no process was started with {flags}: {processes:?}"
    );
}

#[test]
fn profiles_isolated() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.open("beta");
    rig.load("alpha", "blank.html");
    rig.load("beta", "blank.html");
    rig.eval(
        "alpha",
        "localStorage.setItem('who', 'alpha'); document.cookie = 'c=alpha; path=/'; 'ok'",
    );
    // The same origin, another bot's profile: nothing crosses.
    assert_eq!(
        rig.eval("beta", "String(localStorage.getItem('who'))"),
        "null"
    );
    assert_eq!(rig.eval("beta", "document.cookie"), "");
    assert_eq!(rig.eval("alpha", "localStorage.getItem('who')"), "alpha");
    assert_eq!(rig.eval("alpha", "document.cookie"), "c=alpha");
    // One folder per bot, named by the bot id's hash.
    let on_disk = rig.engine.profile_folders().expect("profile folders");
    for id in ["alpha", "beta"] {
        let hash = policy::profile_name(id).expect("hash");
        assert!(
            on_disk.contains(&hash),
            "{id}'s profile {hash} is not among {on_disk:?}"
        );
    }
}

#[test]
fn hidden_idle_bot_suspends() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    rig.show("alpha");
    let shown = rig.engine.snapshot("alpha").expect("snapshot");
    assert_eq!(shown.placement, Placement::Panel);
    assert!(shown.visible && !shown.suspended && shown.container_shown);
    rig.hide("alpha");
    rig.wait_suspended("alpha");
    let hidden = rig.engine.snapshot("alpha").expect("snapshot");
    assert_eq!(hidden.placement, Placement::Hidden);
    assert!(!hidden.visible && !hidden.container_shown && hidden.suspended);
    // Showing it again wakes it, in the panel.
    rig.show("alpha");
    let again = rig.engine.snapshot("alpha").expect("snapshot");
    assert!(again.visible && !again.suspended && again.placement == Placement::Panel);
    assert_eq!(rig.eval("alpha", "document.readyState"), "complete");
}

#[test]
fn tool_on_suspended_bot_resumes_first() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    rig.show("alpha");
    rig.hide("alpha");
    rig.wait_suspended("alpha");

    // A tool arrives for a suspended bot: it is awake, visible and parked before any command can reach it.
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    let guard = rig
        .ui
        .run(async move { engine.begin_tool(&bot).await.expect("begin tool") });
    let snap = rig.engine.snapshot("alpha").expect("snapshot");
    assert!(
        !snap.suspended,
        "the page must be resumed before the tool runs"
    );
    assert!(snap.visible, "the page must render while a tool uses it");
    assert_eq!(snap.placement, Placement::Parked);
    assert!(
        parked(snap.container),
        "the page is parked off-screen, not in the panel: {:?}",
        snap.container
    );
    // And its first command is answered, not left to time out.
    let started = Instant::now();
    assert_eq!(rig.eval("alpha", "document.title"), "Blank fixture");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the first command took {:?}",
        started.elapsed()
    );
    guard.end();
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").placement,
        Placement::Hidden
    );
    rig.wait_suspended("alpha");

    // A tool for a bot whose page does not exist yet creates it, parked and awake.
    let (engine, bot) = (rig.engine.clone(), rig.bot("beta"));
    let guard = rig
        .ui
        .run(async move { engine.begin_tool(&bot).await.expect("begin tool") });
    let snap = rig.engine.snapshot("beta").expect("snapshot");
    assert!(snap.open && !snap.suspended && snap.visible && snap.placement == Placement::Parked);
    assert_eq!(rig.eval("beta", "1 + 1"), "2");
    guard.end();
}

#[test]
fn panel_closed_no_stream_does_not_park() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    rig.show("alpha");
    rig.hide("alpha");
    let hidden = rig.engine.snapshot("alpha").expect("snapshot");
    assert_eq!(
        hidden.placement,
        Placement::Hidden,
        "an idle hidden bot must not be parked"
    );
    assert!(!hidden.visible && !hidden.container_shown && !parked(hidden.container));
    rig.wait_suspended("alpha");
    // A stream keeps it awake and parked; when the stream ends it goes back to sleep.
    rig.engine.set_streaming("alpha", true).expect("streaming");
    let streaming = rig.engine.snapshot("alpha").expect("snapshot");
    assert_eq!(streaming.placement, Placement::Parked);
    assert!(streaming.visible && !streaming.suspended);
    rig.engine.set_streaming("alpha", false).expect("streaming");
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").placement,
        Placement::Hidden
    );
    rig.wait_suspended("alpha");
    // A window that is hidden to the tray shows no bot either.
    rig.show("alpha");
    rig.engine.set_window_visible(false);
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").placement,
        Placement::Hidden
    );
    rig.engine.set_window_visible(true);
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").placement,
        Placement::Panel
    );
}

#[test]
fn idle_controller_closes_after_timeout() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "controls.html");
    rig.show("alpha");
    rig.hide("alpha");
    let ten_minutes = Duration::from_secs(10 * 60);
    // Nine minutes of idleness keeps the controller; ten close it.
    rig.clock
        .set(rig.clock.get() + ten_minutes - Duration::from_secs(60));
    let (engine, now) = (rig.engine.clone(), rig.clock.get());
    rig.ui.run(async move { engine.tick(now).await });
    assert!(rig.engine.snapshot("alpha").expect("snapshot").open);
    rig.clock.set(rig.clock.get() + Duration::from_secs(60));
    let (engine, now) = (rig.engine.clone(), rig.clock.get());
    rig.ui.run(async move { engine.tick(now).await });
    let closed = rig.engine.snapshot("alpha").expect("snapshot");
    assert!(
        !closed.open && closed.tabs == 0,
        "the controller must be closed: {closed:?}"
    );
    // The profile stays on disk, and the next open comes back at the last URL.
    let hash = policy::profile_name("alpha").expect("hash");
    assert!(
        rig.engine
            .profile_folders()
            .expect("folders")
            .contains(&hash)
    );
    rig.open("alpha");
    let reopened = rig.engine.snapshot("alpha").expect("snapshot");
    assert!(reopened.open && reopened.tabs == 1);
    let expected = rig.server.url("controls.html");
    let guard = rig.begin("alpha");
    let tab = guard.tab().expect("active tab");
    // The page is navigating back to its last address; it gets there on its own.
    rig.ui.wait_until(
        "the page to return to its last address",
        Duration::from_secs(15),
        move || tab.source().is_ok_and(|source| source == expected),
    );
    guard.end();
}

#[test]
fn a_bot_kept_awake_out_of_sight_gets_the_low_memory_target_until_shown() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    let guard = rig
        .ui
        .run(async move { engine.begin_tool(&bot).await.expect("begin tool") });
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").memory,
        MemoryLevel::Normal
    );
    rig.clock.set(rig.clock.get() + Duration::from_secs(31));
    let (engine, now) = (rig.engine.clone(), rig.clock.get());
    rig.ui.run(async move { engine.tick(now).await });
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").memory,
        MemoryLevel::Low
    );
    // It is back to Normal before it is shown.
    rig.show("alpha");
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").memory,
        MemoryLevel::Normal
    );
    guard.end();
}

#[test]
fn delete_profile_removes_folder_after_close() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    rig.eval(
        "alpha",
        "localStorage.setItem('secret', 'x'); document.cookie = 'session=1; path=/'; 'ok'",
    );
    let hash = policy::profile_name("alpha").expect("hash");
    let folder: PathBuf = rig.engine.profile_root().join(profiles::folder_name(&hash));
    assert!(
        folder.is_dir(),
        "the profile folder exists while the bot is open: {folder:?}"
    );
    let (engine, id) = (rig.engine.clone(), "alpha".to_string());
    let outcome = rig
        .ui
        .run(async move { engine.delete_profile(&id).await.expect("delete") });
    assert_eq!(outcome, DeleteOutcome::Removed);
    assert!(!folder.exists(), "the profile folder must be gone");
    assert!(
        rig.engine.snapshot("alpha").is_none(),
        "the bot's browser is closed"
    );
    // Opening the bot again gives it a new, empty profile.
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    assert_eq!(
        rig.eval(
            "alpha",
            "String(localStorage.getItem('secret')) + '|' + document.cookie"
        ),
        "null|"
    );
}

#[test]
fn pruning_removes_orphans_only_and_electron_profiles_are_left_untouched() {
    let mut rig = Rig::new();
    // What Electron left behind: never touched, so a rollback to R1 keeps the bots' site sessions.
    let electron = rig
        .root
        .path()
        .join("data")
        .join("native-browser")
        .join(policy::profile_name("alpha").expect("hash"));
    std::fs::create_dir_all(&electron).expect("electron profile");
    std::fs::write(electron.join("Cookies"), "synthetic").expect("marker");
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    rig.eval("alpha", "localStorage.setItem('a', '1'); 'ok'");
    // A profile for a bot that no longer exists.
    let orphan = policy::profile_name("gone").expect("hash");
    let orphan_folder = rig
        .engine
        .profile_root()
        .join(profiles::folder_name(&orphan));
    std::fs::create_dir_all(&orphan_folder).expect("orphan");
    std::fs::write(orphan_folder.join("Cookies"), "synthetic").expect("marker");
    let (engine, valid) = (rig.engine.clone(), vec!["alpha".to_string()]);
    let report = rig
        .ui
        .run(async move { engine.prune_profiles(&valid).await.expect("prune") });
    assert_eq!(report.removed, vec![orphan]);
    assert!(!orphan_folder.exists());
    assert!(
        rig.engine
            .profile_root()
            .join(profiles::folder_name(
                &policy::profile_name("alpha").expect("hash")
            ))
            .is_dir()
    );
    assert!(
        electron.join("Cookies").is_file(),
        "Electron's profile must be left alone"
    );
}

#[test]
fn a_dropped_tool_guard_still_ends_its_need() {
    let mut rig = Rig::new();
    rig.open("alpha");
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    let guard = rig
        .ui
        .run(async move { engine.begin_tool(&bot).await.expect("begin tool") });
    assert_eq!(
        rig.engine.snapshot("alpha").expect("snapshot").placement,
        Placement::Parked
    );
    // A cancelled task drops its guard without calling `end`: the host's event loop puts the page back to sleep.
    drop(guard);
    assert!(!rig.engine.snapshot("alpha").expect("snapshot").need.tool);
    let engine = rig.engine.clone();
    rig.ui.wait_until(
        "the dropped guard's page to hide",
        Duration::from_secs(5),
        move || {
            engine
                .snapshot("alpha")
                .is_some_and(|s| s.placement == Placement::Hidden)
        },
    );
}

#[test]
fn close_all_ends_every_browser_process_and_the_engine_does_not_open_again() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.open("beta");
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    rig.ui.run(async move {
        engine.close_all().await.expect("close all");
        assert!(matches!(
            engine.open(&bot).await,
            Err(desktop_host::browser::engine::EngineError::Closed)
        ));
    });
    assert!(rig.engine.snapshot("alpha").is_none() && rig.engine.snapshot("beta").is_none());
    // The browser process tree ends on its own once nothing holds the environment.
    let folder = rig.root.path().to_path_buf();
    // Windows takes its time to end a browser's processes when the PC is busy.
    let deadline = Instant::now() + Duration::from_secs(120);
    while !webview_processes(&folder).is_empty() {
        assert!(
            Instant::now() < deadline,
            "browser processes still running after close_all"
        );
        rig.ui.settle(Duration::from_secs(1));
    }
}

#[test]
fn a_tool_arriving_while_a_suspend_is_in_flight_still_finds_the_page_awake() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    // Hide (a suspend starts) and start a tool at once, over and over: whichever lands first, the page answers.
    for round in 0..12 {
        rig.show("alpha");
        rig.hide("alpha");
        let guard = rig.begin("alpha");
        let snap = rig.engine.snapshot("alpha").expect("snapshot");
        assert!(
            snap.visible && snap.placement == Placement::Parked,
            "round {round}: {snap:?}"
        );
        assert_eq!(rig.eval("alpha", "6 * 7"), "42", "round {round}");
        guard.end();
    }
}
