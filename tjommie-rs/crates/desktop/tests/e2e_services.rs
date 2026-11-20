//! The desktop services through the test driver (Phase 3 M6 follow-up; plan T6.1 to T6.3): closing the window hides it and the
//! app keeps running, the tray's Quit stops everything, a notice becomes a toast and a routine failure is acknowledged only
//! after its toast, and a dev launch never writes the login item. An e2e build swaps the real tray, toasts and registry for
//! in-memory stand-ins the driver reads (`Request::Services`, `Request::Poke`), so nothing here puts an icon in the owner's
//! tray, a toast on their desktop or a value in their registry. Run with the e2e identity, the daemon built with the switch
//! beside the app: `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_services --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use desktop::services::Poke;
use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

/// Whether the window is shown; an app that is gone (it quit) reads as not shown.
fn visible(app: &mut Client) -> bool {
    app.services().is_ok_and(|state| state.visible)
}

fn toasts(app: &mut Client) -> Vec<(String, String, Option<String>)> {
    app.services().unwrap().toasts
}

/// An agent that asks to run a command: the daemon raises an approval notice for it.
const ASKS: &str = r#"{ "version": 1, "turns": [ { "expect": "deploy", "events": [
  { "type": "approval", "id": "ask-1", "tool": "Bash", "summary": "Run the deploy", "then": {} } ] } ] }"#;

#[test]
fn e2e_closing_the_window_hides_it_and_the_app_keeps_running() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(root.path(), &[("default.json", common::QUIET)]);
    assert!(visible(&mut app));
    let daemon = common::daemons(app.pid());
    assert_eq!(daemon.len(), 1, "the app's daemon: {daemon:?}");

    // The close button hides the window; nothing stops.
    common::close_window(app.pid());
    common::eventually("the window to hide", Duration::from_secs(15), || {
        !visible(&mut app)
    });
    std::thread::sleep(Duration::from_secs(2));
    assert!(!app.has_exited().unwrap(), "the app quit with its window");
    assert_eq!(
        common::daemons(app.pid()),
        daemon,
        "the daemon went with the window"
    );
    // The first time, a toast says so (the fake notifier took it).
    let first = toasts(&mut app);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].0, "Tjommie is still running");
    assert_eq!(first[0].2, None, "it only informs");

    // The tray's icon brings it back, and a second close hides it again without saying so again.
    app.poke(Poke::Tray("show".into())).unwrap();
    common::eventually("the window to show", Duration::from_secs(15), || {
        visible(&mut app)
    });
    common::close_window(app.pid());
    common::eventually("the window to hide again", Duration::from_secs(15), || {
        !visible(&mut app)
    });
    assert_eq!(
        toasts(&mut app).len(),
        1,
        "the hint shows only the first time"
    );
    assert!(!app.has_exited().unwrap());
}

#[test]
fn e2e_quit_from_the_tray_hides_the_window_at_once_and_stops_everything() {
    assert_eq!(identity::CURRENT.name, "e2e");
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(root.path(), &[("default.json", common::QUIET)]);
    // The vault is up, so there is something to lock.
    app.click("sidebar.nav.settings").unwrap();
    app.wait("settings.nav.logins", Predicate::Exists, 10_000)
        .unwrap();
    let daemon = common::daemons(app.pid());
    assert_eq!(daemon.len(), 1, "the app's daemon: {daemon:?}");

    app.poke(Poke::Tray("quit".into())).unwrap();
    // The window is gone at once, not for as long as the stop takes.
    common::eventually("the window to hide", Duration::from_secs(5), || {
        app.has_exited().unwrap() || !visible(&mut app)
    });
    common::eventually("the app to exit", Duration::from_secs(60), || {
        app.has_exited().unwrap()
    });
    // It held the close, stopped its clients and the daemon, and only then exited (its log says so, in order).
    let log = std::fs::read_to_string(root.path().join("logs").join("desktop.log")).unwrap();
    let closing =
        log.find("Closing: the browser, Logins and worker clients stop, then the daemon.");
    let closed = log.find("Closed: the clients and the daemon have stopped");
    assert!(
        closing.is_some() && closed > closing,
        "the app did not run its graceful quit: {log}"
    );
    assert!(
        common::daemons(app.pid()).is_empty() && !common::process_alive(daemon[0]),
        "the daemon outlived the app"
    );
}

#[test]
fn e2e_a_notice_is_a_toast_and_a_routine_failure_is_acknowledged_only_after_its_toast() {
    assert_eq!(identity::CURRENT.name, "e2e");
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(
        root.path(),
        &[("Forge.json", ASKS), ("default.json", common::QUIET)],
    );

    // A notice from the daemon itself: Forge asks to run a command, and the daemon raises an approval notice.
    common::make_agent(&mut app, "Forge", false);
    common::mention(&mut app, "Forge", "deploy it");
    common::eventually("the approval's toast", Duration::from_secs(90), || {
        toasts(&mut app).iter().any(|(_, _, link)| {
            link.as_deref()
                .is_some_and(|link| link.starts_with("/chat/?bot="))
        })
    });

    // A routine failure is acknowledged only after the OS took its toast: with notifications off the toast is refused and
    // the notice stays pending (no acknowledgement); on again, it is shown, then acknowledged.
    let before = toasts(&mut app).len();
    app.poke(Poke::Notifications(false)).unwrap();
    app.poke(Poke::RoutineFailed {
        run_id: "run-1".into(),
    })
    .unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let off = app.services().unwrap();
    assert_eq!(
        off.toasts.len(),
        before,
        "a toast was shown while notifications were off"
    );
    assert!(
        off.acked.is_empty(),
        "a notice nothing showed was acknowledged: {:?}",
        off.acked
    );
    app.poke(Poke::Notifications(true)).unwrap();
    app.poke(Poke::RoutineFailed {
        run_id: "run-1".into(),
    })
    .unwrap();
    common::eventually("run-1 to be acknowledged", Duration::from_secs(10), || {
        app.services().unwrap().acked == ["run-1"]
    });
    let on = app.services().unwrap();
    assert_eq!(
        on.toasts.len(),
        before + 1,
        "its toast came first: {:?}",
        on.toasts
    );
    assert_eq!(
        on.toasts.last().map(|toast| toast.0.as_str()),
        Some("Forge's routine failed")
    );

    // While the window is shown and focused the notice is already in front of the person: no toast, but it is acknowledged.
    app.poke(Poke::Focus(true)).unwrap();
    app.poke(Poke::RoutineFailed {
        run_id: "run-2".into(),
    })
    .unwrap();
    common::eventually(
        "run-2 to be acknowledged without a toast",
        Duration::from_secs(10),
        || app.services().unwrap().acked == ["run-1", "run-2"],
    );
    assert_eq!(
        app.services().unwrap().toasts.len(),
        before + 1,
        "a toast was shown over a focused window"
    );
    app.poke(Poke::Focus(false)).unwrap();

    // A click on a toast shows the window (hidden to the tray meanwhile) and goes to the notice's chat.
    common::close_window(app.pid());
    common::eventually("the window to hide", Duration::from_secs(15), || {
        !visible(&mut app)
    });
    let approval = toasts(&mut app)
        .iter()
        .position(|(_, _, link)| {
            link.as_deref()
                .is_some_and(|link| link.starts_with("/chat/?bot="))
        })
        .unwrap();
    app.poke(Poke::ClickToast(approval)).unwrap();
    common::eventually("the window to show", Duration::from_secs(15), || {
        visible(&mut app)
    });
    app.wait("screen.chat", Predicate::Exists, 15_000).unwrap();
}

#[test]
fn e2e_a_dev_launch_never_writes_the_login_item() {
    assert_eq!(identity::CURRENT.name, "e2e");
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(root.path(), &[("default.json", common::QUIET)]);
    let state = app.services().unwrap();
    assert_eq!(
        state.registry_writes, 0,
        "the start-at-sign-in setting wrote to the registry"
    );
    assert!(!state.login_item, "start at sign-in is on");
    // Nor does anything after a few seconds of running.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(app.services().unwrap().registry_writes, 0);
}

#[test]
fn e2e_hidden_starts_in_the_tray_and_the_tray_shows_the_window() {
    assert_eq!(identity::CURRENT.name, "e2e");
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch_args(
        root.path(),
        &[("default.json", common::QUIET)],
        &["--hidden"],
    );
    common::eventually("the app's services", Duration::from_secs(60), || {
        app.services().is_ok_and(|state| state.registry_writes == 0)
    });
    assert!(!visible(&mut app), "a start in the tray showed its window");
    app.poke(Poke::Tray("show".into())).unwrap();
    common::eventually("the window to show", Duration::from_secs(15), || {
        visible(&mut app)
    });
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
}
