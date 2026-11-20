//! The daemon process, against real processes: a stand-in daemon (`examples/fake_daemon.rs`, which can crash, hang, stay
//! silent or leave a grandchild holding its pipes) and, when it is built, the real `tjommie-daemon.exe`.
//! Port of `daemon-process.test.ts` (the process tests) and `daemon-process-tree-kill.test.ts`.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use client::{Client, Endpoint};
use desktop_host::daemon_process::{
    DaemonError, DaemonLaunch, DaemonState, Env, KEEPS_STOPPING, Output, StopOutcome, Supervisor,
    source_daemon_path, start, stop_before_quit,
};
use protocol::wire::{Empty, method};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// `examples/fake_daemon.exe`, built beside the test executables by `cargo test`.
fn fake_daemon() -> PathBuf {
    let tests = std::env::current_exe().unwrap();
    let exe = tests
        .parent()
        .and_then(|deps| deps.parent())
        .unwrap()
        .join("examples")
        .join("fake_daemon.exe");
    assert!(exe.exists(), "cargo test builds {}", exe.display());
    exe
}

fn inherited() -> Env {
    std::env::vars().collect()
}

fn with(mut env: Env, extra: &[(&str, &str)]) -> Env {
    for (name, value) in extra {
        env.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
        env.push(((*name).to_owned(), (*value).to_owned()));
    }
    env
}

fn fake(mode: &str, extra: &[(&str, &str)]) -> DaemonLaunch {
    let mut launch = DaemonLaunch::new(
        fake_daemon(),
        with(
            inherited(),
            &[&[("FAKE_DAEMON_MODE", mode)], extra].concat(),
        ),
    );
    launch.start_timeout = Duration::from_secs(20);
    launch.stop_grace = Duration::from_millis(300);
    launch
}

/// An output sink that keeps everything it is given.
fn collecting() -> (Output, Arc<Mutex<String>>) {
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = seen.clone();
    let output: Output = Arc::new(move |text| sink.lock().unwrap().push_str(text));
    (output, seen)
}

fn silent() -> Output {
    Arc::new(|_| {})
}

fn alive(pid: u32) -> bool {
    // SAFETY: plain Win32 calls on a handle this function opens and closes.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == 259 // STILL_ACTIVE
    }
}

async fn gone(pid: u32) -> bool {
    for _ in 0..40 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// The number after `label ` in the output, e.g. `pid 123`.
fn number_after(output: &str, label: &str) -> u32 {
    output
        .lines()
        .find_map(|line| line.strip_prefix(label)?.trim().parse().ok())
        .unwrap_or_else(|| panic!("no `{label}` line in {output:?}"))
}

#[tokio::test]
async fn pipes_the_tokens_in_as_one_json_line_keeps_them_out_of_the_output_and_stops_by_closing_stdin()
 {
    let record = tempfile::tempdir().unwrap();
    let file = record.path().join("credentials.txt");
    let (output, seen) = collecting();
    let launch = fake("serve", &[("FAKE_DAEMON_RECORD", &file.to_string_lossy())]);
    let daemon = start(&launch, output).await.unwrap();
    assert_eq!(daemon.connection.port, 47999);
    let (token, host_token) = (
        daemon.connection.token.expose(),
        daemon.desktop_host_token.expose(),
    );
    assert_eq!((token.len(), host_token.len()), (43, 43));
    assert_ne!(token, host_token);
    let line = std::fs::read_to_string(&file).unwrap();
    let credentials: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(credentials["token"], token);
    assert_eq!(credentials["desktopHostToken"], host_token);
    assert!(
        line.ends_with('\n'),
        "one line, so the daemon's reader finishes it"
    );
    let heard = seen.lock().unwrap().clone();
    assert!(
        heard.contains("listening on http://127.0.0.1:47999"),
        "{heard}"
    );
    assert!(!heard.contains(token) && !heard.contains(host_token));
    daemon.stop().await;
    assert_eq!(daemon.exited().await, Some(0));
}

#[tokio::test]
async fn reports_the_daemons_own_reason_when_it_cant_start() {
    let kills = Arc::new(AtomicUsize::new(0));
    let mut launch = fake("exit1", &[]);
    launch.kill_probe = Some(kills.clone());
    let failed = start(&launch, silent()).await.err().unwrap();
    assert_eq!(
        failed,
        DaemonError::Exited("The fake daemon refuses to start.".to_owned())
    );
    // It had exited already, so there was nothing to kill.
    assert_eq!(kills.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn without_a_reason_on_stderr_the_exit_code_says_what_happened_and_nothing_is_killed() {
    let kills = Arc::new(AtomicUsize::new(0));
    let mut launch = fake("exit1-quiet", &[]);
    launch.kill_probe = Some(kills.clone());
    let failed = start(&launch, silent()).await.err().unwrap();
    assert_eq!(
        failed.to_string(),
        "Tjommie's background service stopped (exit code 1)."
    );
    assert_eq!(
        kills.load(Ordering::SeqCst),
        0,
        "it exited before the failure, so no taskkill"
    );
}

#[tokio::test]
async fn kills_the_daemon_tree_when_startup_times_out() {
    let kills = Arc::new(AtomicUsize::new(0));
    let (output, seen) = collecting();
    let mut launch = fake("silent", &[]);
    launch.start_timeout = Duration::from_millis(600);
    launch.kill_probe = Some(kills.clone());
    let failed = start(&launch, output).await.err().unwrap();
    assert_eq!(failed, DaemonError::StartTimedOut);
    assert_eq!(
        failed.to_string(),
        "Tjommie's background service didn't start in time."
    );
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    let pid = number_after(&seen.lock().unwrap(), "pid ");
    assert!(gone(pid).await, "the silent daemon was killed");
}

#[tokio::test]
async fn kills_the_daemon_tree_when_graceful_shutdown_reaches_its_timeout() {
    let kills = Arc::new(AtomicUsize::new(0));
    let (output, seen) = collecting();
    let mut launch = fake("hang", &[]);
    launch.kill_probe = Some(kills.clone());
    let daemon = start(&launch, output).await.unwrap();
    let pid = number_after(&seen.lock().unwrap(), "pid ");
    daemon.stop().await; // its stdin closes, it ignores that, and after the grace its tree is killed
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    assert!(gone(pid).await);
    assert!(daemon.exited().await.is_none() || !alive(pid));
}

#[tokio::test]
async fn confirms_an_unresponsive_daemon_and_its_child_exited_before_recovery_even_when_the_pipes_stay_open()
 {
    let kills = Arc::new(AtomicUsize::new(0));
    let (output, seen) = collecting();
    let mut launch = fake("hang-with-child", &[]);
    launch.kill_probe = Some(kills.clone());
    // The daemon ignores its stdin closing, so the graceful stop only ends after the grace. A grace far longer than the window
    // below makes that stop time out however fast this machine is, so the test always takes the forced path it is about.
    launch.stop_grace = Duration::from_secs(30);
    let daemon = start(&launch, output).await.unwrap();
    let (daemon_pid, child_pid) = {
        let heard = seen.lock().unwrap();
        (
            number_after(&heard, "pid "),
            number_after(&heard, "child-pid "),
        )
    };
    assert!(alive(daemon_pid) && alive(child_pid));
    let outcome = stop_before_quit(&daemon, Duration::from_millis(400))
        .await
        .unwrap();
    assert_eq!(outcome, StopOutcome::Forced);
    assert!(kills.load(Ordering::SeqCst) >= 1);
    // Confirmed by the time stop_before_quit returns, not eventually.
    assert!(!alive(daemon_pid), "the daemon is confirmed gone");
    assert!(
        gone(child_pid).await,
        "and so is the child that held its pipes"
    );
}

#[tokio::test]
async fn a_daemon_that_already_exited_is_never_killed_by_pid() {
    // Its pid could belong to another process by now, so a late force_stop must not taskkill it.
    let kills = Arc::new(AtomicUsize::new(0));
    let mut launch = fake("crash", &[("FAKE_DAEMON_LIFETIME_MS", "100")]);
    launch.kill_probe = Some(kills.clone());
    let daemon = start(&launch, silent()).await.unwrap();
    assert_eq!(daemon.exited().await, Some(7));
    daemon.force_stop().await;
    daemon.stop().await;
    assert_eq!(kills.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_daemon_that_stops_when_asked_is_stopped_not_forced() {
    let daemon = start(&fake("serve", &[]), silent()).await.unwrap();
    assert_eq!(
        stop_before_quit(&daemon, Duration::from_secs(10))
            .await
            .unwrap(),
        StopOutcome::Stopped
    );
}

/// A daemon that stops on its own is restarted with a new token, at most three times in a minute; the fourth stop is the end.
#[tokio::test]
async fn restart_limit_three_per_minute_when_the_daemon_keeps_stopping() {
    let launch = fake("crash", &[("FAKE_DAEMON_LIFETIME_MS", "300")]);
    let supervisor = Supervisor::start(launch, silent());
    let mut state = supervisor.state();
    let mut tokens = Vec::new();
    let failure = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let current = state.borrow_and_update().clone();
            match current {
                DaemonState::Running { connection, .. } => {
                    let token = connection.token.expose().to_owned();
                    if !tokens.contains(&token) {
                        tokens.push(token);
                    }
                }
                DaemonState::Failed(reason) => return reason,
                DaemonState::Starting => {}
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .expect("the supervisor gives up");
    assert_eq!(failure, KEEPS_STOPPING);
    assert_eq!(
        tokens.len(),
        4,
        "the first start and three restarts, each with a new token"
    );
    assert!(
        supervisor
            .stop_before_quit(Duration::from_secs(5))
            .await
            .is_none(),
        "nothing left to stop"
    );
}

#[tokio::test]
async fn a_start_that_fails_is_reported_and_not_retried() {
    let supervisor = Supervisor::start(fake("exit1", &[]), silent());
    let mut state = supervisor.state();
    let reason = loop {
        if let DaemonState::Failed(reason) = state.borrow_and_update().clone() {
            break reason;
        }
        state.changed().await.unwrap();
    };
    assert_eq!(reason, "The fake daemon refuses to start.");
}

#[tokio::test]
async fn quitting_stops_the_running_daemon_through_the_supervisor() {
    let supervisor = Supervisor::start(fake("serve", &[]), silent());
    let mut state = supervisor.state();
    while !matches!(*state.borrow_and_update(), DaemonState::Running { .. }) {
        state.changed().await.unwrap();
    }
    let outcome = supervisor.stop_before_quit(Duration::from_secs(10)).await;
    assert_eq!(outcome.unwrap().unwrap(), StopOutcome::Stopped);
}

// ---- the real daemon, when it is built ----

/// The Rust daemon these tests start: `TJOMMIE_DAEMON_EXE`, else the debug build. Build it with
/// `cargo build -p daemon --bin tjommie-daemon` inside tjommie-rs; without it those tests say so and do nothing.
fn real_daemon() -> Option<PathBuf> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    let exe = source_daemon_path(&inherited(), &repo);
    if !exe.exists() {
        eprintln!("skipped: {} is not built", exe.display());
        return None;
    }
    Some(exe)
}

/// The real daemon on a disposable data root and a free port, on loopback only, with no provider CLIs on PATH. TJOMMIE_DEV
/// is set on purpose: the app's environment filter must remove it, or the daemon refuses to mix development and production.
fn real_launch(exe: PathBuf, root: &std::path::Path, extra: &[(&str, &str)]) -> DaemonLaunch {
    let env = inherited()
        .into_iter()
        .filter(|(key, _)| !key.eq_ignore_ascii_case("PATH"))
        .collect();
    let env = with(
        env,
        &[
            &[
                ("PATH", ""),
                ("TJOMMIE_HOME", &root.to_string_lossy()),
                ("TJOMMIE_PORT", "0"),
                ("TJOMMIE_LAN_BIND", "127.0.0.1"),
                ("TJOMMIE_LAN_PORT", "0"),
                ("TJOMMIE_DEV", "1"),
            ][..],
            extra,
        ]
        .concat(),
    );
    DaemonLaunch::new(exe, env)
}

#[tokio::test]
async fn the_real_daemon_takes_its_token_on_stdin_answers_with_it_and_stops_when_stdin_closes() {
    let Some(exe) = real_daemon() else { return };
    let root = tempfile::tempdir().unwrap();
    let (output, seen) = collecting();
    let daemon = start(&real_launch(exe, root.path(), &[]), output)
        .await
        .unwrap();
    let (port, token) = (
        daemon.connection.port,
        daemon.connection.token.expose().to_owned(),
    );
    let client = Client::connect(Endpoint::Local { port }, &token)
        .await
        .unwrap();
    assert!(
        client
            .call::<method::BotsList>(Empty {})
            .await
            .unwrap()
            .is_empty()
    );
    let heard = seen.lock().unwrap().clone();
    assert!(
        heard.contains(&format!("listening on http://127.0.0.1:{port}")),
        "{heard}"
    );
    assert!(!heard.contains(&token) && !heard.contains(daemon.desktop_host_token.expose()));
    daemon.stop().await;
    assert_eq!(daemon.exited().await, Some(0));
    let closed = tokio::time::timeout(Duration::from_secs(10), client.closed())
        .await
        .unwrap();
    assert!(closed.opened);
}

#[tokio::test]
async fn the_real_daemon_reports_its_own_reason_when_it_cant_start() {
    let Some(exe) = real_daemon() else { return };
    let (first_root, second_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let first = start(&real_launch(exe.clone(), first_root.path(), &[]), silent())
        .await
        .unwrap();
    let port = first.connection.port;
    let second = real_launch(
        exe,
        second_root.path(),
        &[("TJOMMIE_PORT", &port.to_string())],
    );
    let failed = start(&second, silent()).await.err().unwrap();
    assert_eq!(
        failed.to_string(),
        format!("Tjommie is already running: port {port} is in use.")
    );
    first.stop().await;
}
