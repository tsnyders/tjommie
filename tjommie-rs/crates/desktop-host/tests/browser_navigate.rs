//! `navigate` reports on its own navigation. A bot's browser opens with its start page still loading; a navigation made at once
//! cancels that one, and the cancelled one's completion reaches WebView2's event first.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::time::Duration;

use common::Rig;

/// `open` returns while the start page is still loading, so a navigation made straight after it supersedes that one. Its completion
/// (a failure, "operation canceled") used to be taken for the new navigation's own: over plain HTTP, with no certificate in play, the
/// call said the navigation had failed in 12 of 12 runs, and it could return before the page was there. The address is read in the
/// same task, straight after the call returns, so no wait can hide an early return.
#[test]
fn a_navigation_made_straight_after_open_reports_its_own_page_and_its_own_success() {
    let mut rig = Rig::new();
    rig.open("alpha");
    let url = rig.server.url("tools.html");
    let guard = rig.begin("alpha");
    let tab = guard.tab().expect("a tab");
    let wanted = url.clone();
    let (reported, here) = rig.ui.run(async move {
        let reported = tab.navigate(&wanted).await;
        let here = common::eval(&tab, "location.href").await;
        (reported, here)
    });
    guard.end();
    assert_eq!(reported, Ok(true), "the navigation succeeded");
    assert_eq!(
        here, url,
        "and the page it reported on is the one asked for"
    );
}

/// Navigating to the start page's own address, straight after `open`, joins the navigation already loading it, so WebView2 may raise no
/// start of its own for this call. It must still finish: a lifecycle test (`rig.load` of the start page) once waited the whole 120 s
/// here, under load. Several fresh browsers, because which way it falls depends on timing; each is given 30 s, so a hang fails fast and
/// says which round.
#[test]
fn a_navigation_to_the_start_page_straight_after_open_still_finishes() {
    for round in 0..6 {
        let mut rig = Rig::new();
        rig.open("alpha");
        let url = rig.server.url("blank.html");
        let guard = rig.begin("alpha");
        let tab = guard.tab().expect("a tab");
        let wanted = url.clone();
        let finished = rig.ui.run_within(Duration::from_secs(30), async move {
            let reported = tab.navigate(&wanted).await;
            let here = common::eval(&tab, "location.href").await;
            (reported, here)
        });
        guard.end();
        let (reported, here) =
            finished.unwrap_or_else(|| panic!("round {round}: the navigation never finished"));
        assert_eq!(reported, Ok(true), "round {round}: it succeeded");
        assert_eq!(here, url, "round {round}: and the page is the start page");
    }
}

/// A navigation that really fails still says so: the new rule must not turn a failure into a success.
#[test]
fn a_navigation_that_fails_still_reports_failure() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    // Nothing listens on port 1 of the loopback address: the connection is refused and the browser shows its error page.
    let guard = rig.begin("alpha");
    let tab = guard.tab().expect("a tab");
    let reported = rig
        .ui
        .run(async move { tab.navigate("http://127.0.0.1:1/never.html").await });
    guard.end();
    assert_eq!(
        reported,
        Ok(false),
        "a refused connection is a failed navigation"
    );
}
