//! The browser panel's state through the test driver (Phase 3 plan T3.7), on the live path: it opens and closes beside a
//! bot's chat, by Ctrl+B, Escape and its Hide button. The app starts its own daemon, which plays recorded turns in place of a
//! provider; the agent is made through the UI, and the panel reads its agent from the app's state like any screen. Its
//! browser is off (this is the panel's state only: no page, so no WebView2 in the run). Run with the e2e identity, the daemon
//! built with the switch beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_panel --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::{Duration, Instant};

use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

fn wait_gone(app: &mut Client, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.snapshot().unwrap().node(id).is_some() {
        assert!(Instant::now() < deadline, "{id} stayed");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn e2e_panel_open_close() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    let root = owner_safety::test_root().unwrap();
    let mut app = common::launch(root.path(), &[("default.json", common::QUIET)]);
    common::make_agent(&mut app, "Forge", false);
    let forge = common::recent(&mut app, "Forge");

    // Away from a chat there is no panel to open.
    app.click("sidebar.nav.agents").unwrap();
    app.wait("screen.agents", Predicate::Exists, 10_000)
        .unwrap();
    app.key("ctrl-b").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        app.snapshot().unwrap().node("browser.panel").is_none(),
        "no panel away from a chat"
    );

    app.click(&forge).unwrap();
    app.wait("screen.chat", Predicate::Exists, 10_000).unwrap();
    assert!(app.snapshot().unwrap().node("browser.panel").is_none());

    // Ctrl+B opens it beside Forge's chat, on Details, and again closes it.
    app.key("ctrl-b").unwrap();
    app.wait("browser.panel", Predicate::Exists, 10_000)
        .unwrap();
    let snapshot = app.snapshot().unwrap();
    assert_eq!(snapshot.node("browser.panel").unwrap().text, "About Forge");
    assert!(snapshot.node("browser.details.name").is_some());
    app.key("ctrl-b").unwrap();
    wait_gone(&mut app, "browser.panel");

    // Escape closes it.
    app.key("ctrl-b").unwrap();
    app.wait("browser.panel", Predicate::Exists, 10_000)
        .unwrap();
    app.key("escape").unwrap();
    wait_gone(&mut app, "browser.panel");

    // So does its Hide button, on the spring.
    app.key("ctrl-b").unwrap();
    app.wait("browser.hide", Predicate::Exists, 10_000).unwrap();
    app.click("browser.hide").unwrap();
    wait_gone(&mut app, "browser.panel");
    assert!(
        app.snapshot().unwrap().node("screen.chat").is_some(),
        "the chat stays"
    );
}
