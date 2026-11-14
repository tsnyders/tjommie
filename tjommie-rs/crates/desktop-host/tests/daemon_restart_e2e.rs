//! Review focus 2 (Phase 3 plan): the daemon restarts while the panel, an overlay or the vault entry is open.
//!
//! Written in T1.2 as a failing end-to-end test and finished in T3.2, which owns the panel, the shield and the vault. It
//! is `#[ignore]`d until then so a shared branch stays green; T3.2 removes the attribute and replaces the body.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(clippy::panic)]

/// With a panel (a WebView2 over the window), an overlay and the vault entry open, kill the daemon process and expect:
/// 1. the shield comes down;
/// 2. every controller of a deleted or browser-off agent closes;
/// 3. the vault locks;
/// 4. the UI reconnects with the new token (`DaemonState::Running` carries a changed token).
///
/// The restart itself is `daemon_process::Supervisor`, covered by `restart_limit_three_per_minute_when_the_daemon_keeps_stopping`.
#[test]
#[ignore = "T3.2 finishes this: it needs the panel, the shield and the vault entry, which don't exist yet"]
fn e2e_daemon_restart_with_panel_open() {
    panic!(
        "T3.2: drive the panel, overlay and vault entry through a daemon restart and check the four expectations above"
    );
}
