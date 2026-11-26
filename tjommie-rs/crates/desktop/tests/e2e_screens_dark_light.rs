//! M5's screens in dark and light (Phase 3 plan M5, the dark-mode pass). A fresh install opens in dark, so this opens
//! each screen and Settings section the Screens lane built and keeps a picture of the window for each, then switches
//! Appearance to Light and does it again. The pictures go to `target/m5-screens/<mode>-<screen>.png` (build output),
//! for an owner's eye; the token pairs' contrast is checked by numbers in the M5 report, not here by pixels. A picture is
//! the app's own window (PrintWindow), never the screen, so nothing else on the desktop can get into one. Clicks are
//! fine here: keyboard-only is `e2e_keyboard_navigation`'s. Needs the daemon beside the app, as that test says:
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_screens_dark_light --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use common::daemon_exe;
use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

const SCREENS: [(&str, &str, &str); 5] = [
    ("sidebar.nav.agents", "screen.agents", "agents"),
    ("sidebar.nav.rooms", "screen.rooms", "rooms"),
    ("sidebar.nav.projects", "screen.projects", "projects"),
    ("sidebar.nav.routines", "screen.routines", "routines"),
    ("sidebar.new", "screen.new", "new-agent"),
];

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

/// Clicks `nav` until `title` shows: a click that lands while the app is busy can go unheard.
fn open(app: &mut Client, nav: &str, title: &str) {
    for _ in 0..3 {
        app.click(nav).unwrap();
        if app.wait(title, Predicate::Exists, 5_000).is_ok() {
            return;
        }
    }
    panic!("{title} never showed after clicking {nav}");
}

/// Every M5 screen and Settings section, a picture of each.
fn tour(app: &mut Client, mode: &str, dir: &Path) {
    let keep = |name: &str| {
        // Past the screens' entrances and the focus ring's settling.
        sleep(Duration::from_millis(500));
        common::picture(&dir.join(format!("{mode}-{name}.png")));
    };
    for (nav, title, name) in SCREENS {
        open(app, nav, title);
        keep(name);
    }
    open(app, "sidebar.nav.settings", "settings.nav.general");
    for section in SECTIONS {
        open(
            app,
            &format!("settings.nav.{section}"),
            &format!("settings.{section}.title"),
        );
        keep(section);
    }
}

#[test]
fn e2e_screens_dark_light() {
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
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/m5-screens");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = Client::launch_with(
        &common::desktop_exe(),
        root.path(),
        &[("TJOMMIE_DAEMON_EXE", daemon_exe().to_str().unwrap())],
    )
    .unwrap();
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
    tour(&mut app, "dark", &dir);
    app.click("settings.nav.appearance").unwrap();
    app.wait("settings.appearance.theme.light", Predicate::Exists, 10_000)
        .unwrap();
    app.click("settings.appearance.theme.light").unwrap();
    // A radio choice reads as toggled when it's the one chosen.
    let started = std::time::Instant::now();
    while !app
        .snapshot()
        .unwrap()
        .node("settings.appearance.theme.light")
        .is_some_and(|node| node.toggled)
    {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Light wasn't chosen"
        );
        sleep(Duration::from_millis(100));
    }
    tour(&mut app, "light", &dir);
    for mode in ["dark", "light"] {
        for name in SCREENS.map(|(_, _, name)| name).into_iter().chain(SECTIONS) {
            let file = dir.join(format!("{mode}-{name}.png"));
            assert!(
                std::fs::metadata(&file).is_ok_and(|meta| meta.len() > 0),
                "{} is empty",
                file.display()
            );
        }
    }
}
