//! The browser panel's state through the test driver (Phase 3 plan T3.7): it opens and closes beside a bot's chat, by
//! Ctrl+B, Escape and its Hide button. Run with the e2e identity:
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_panel --locked`.
//! The store isn't wired yet (T1.4), so the app runs on its e2e fixture: connected, with one bot, whose browser is off
//! (no WebView2 in the run: this is the panel's state only). Under `TJOMMIE_E2E_FIXTURE=panel` the app starts no daemon
//! and keeps that fixture's store (M5 started the live store at launch, which replaced the fixture's bot).
//! TODO(host wiring): when `BrowserData.bots` and the panel's feed are wired, this test moves to the live path.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
    // SAFETY: the only test in this binary sets it before the app starts, which inherits it.
    unsafe { std::env::set_var("TJOMMIE_E2E_FIXTURE", "panel") };
    let root = owner_safety::test_root().unwrap();
    let mut app = Client::launch(env!("CARGO_BIN_EXE_tjommie-desktop"), root.path()).unwrap();

    app.wait("sidebar.recent.bot-1", Predicate::Exists, 30_000)
        .unwrap();
    app.key("ctrl-b").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        app.snapshot().unwrap().node("browser.panel").is_none(),
        "no panel away from a chat"
    );

    app.click("sidebar.recent.bot-1").unwrap();
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
