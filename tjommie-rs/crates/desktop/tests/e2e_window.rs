//! The app skeleton through the test driver (Phase 3 plan T0.5). Run with the e2e identity:
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_window --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use desktop::test_driver::{Client, Predicate, UiBounds, current_user_sid};
use desktop_host::{identity, owner_safety};

fn wordmark(app: &mut Client) -> UiBounds {
    app.snapshot()
        .unwrap()
        .node("sidebar.wordmark")
        .expect("the wordmark")
        .bounds
}

#[test]
fn e2e_window() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    let root = owner_safety::test_root().unwrap();
    let mut app = Client::launch(env!("CARGO_BIN_EXE_tjommie-desktop"), root.path()).unwrap();

    // The first frame: the root holds focus, and the wordmark reads "Tjommie" from the semantic tree.
    app.wait("sidebar.wordmark", Predicate::Exists, 30_000)
        .unwrap();
    let snapshot = app.snapshot().unwrap();
    let root_node = snapshot.node("root").expect("the root");
    assert!(root_node.focused, "the root holds focus: {root_node:?}");
    let mark = snapshot.node("sidebar.wordmark").unwrap();
    assert_eq!(
        (mark.role.as_str(), mark.text.as_str()),
        ("Heading", "Tjommie")
    );

    // A key reaches the app through the pipe: Ctrl+= zooms one step, so the wordmark grows by 1.1.
    let before = wordmark(&mut app);
    app.key("ctrl-=").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut after = wordmark(&mut app);
    while after == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
        after = wordmark(&mut app);
    }
    let ratio = after.height / before.height;
    assert!(
        (ratio - 1.1).abs() < 0.05,
        "zoomed by {ratio}: {before:?} -> {after:?}"
    );

    // Only the current user may open the pipe: a protected DACL with one ACE, full access, for this account. Windows
    // writes some accounts as an alias (the built-in Administrator, a hosted runner's runneradmin, is "LA"), so the
    // trustee is compared as an account, not as text.
    let dacl = app.pipe_dacl().unwrap();
    let trustee = dacl
        .strip_prefix("D:P(A;;FA;;;")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("one protected full-access ACE: {dacl}"));
    assert!(!trustee.contains('('), "exactly one ACE: {dacl}");
    assert!(
        same_account(trustee, &current_user_sid().unwrap()),
        "the ACE is for another account: {dacl}"
    );
}

/// Whether two SID strings, or SDDL aliases such as "LA", name the same account.
fn same_account(a: &str, b: &str) -> bool {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows::Win32::Security::{EqualSid, PSID};
    use windows::core::HSTRING;

    let sid = |text: &str| {
        let mut sid = PSID::default();
        // SAFETY: on success Windows allocates the SID, which is freed below.
        unsafe { ConvertStringSidToSidW(&HSTRING::from(text), &mut sid) }.map(|()| sid)
    };
    let (a, b) = (sid(a), sid(b));
    let same = match (&a, &b) {
        // SAFETY: both are valid SIDs from ConvertStringSidToSidW.
        (Ok(a), Ok(b)) => unsafe { EqualSid(*a, *b) }.is_ok(),
        _ => false,
    };
    for sid in [a, b].into_iter().flatten() {
        // SAFETY: allocated by ConvertStringSidToSidW, freed once.
        unsafe { LocalFree(Some(HLOCAL(sid.0))) };
    }
    same
}
