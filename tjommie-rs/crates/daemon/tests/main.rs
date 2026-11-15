//! `main.test.ts`, run against `tjommie-daemon.exe`. The account folders arrive with T3.4.
//! "stops cleanly when production stdin errors" is in `startup.rs`: Rust can't inject a stdin error into the exe.

mod common;

use std::path::Path;

use common::{Daemon, LISTENING, TempRoot, connect, launch, refused, token, try_connect};
use serde_json::json;

const MISSING_MODE: &str = "Start Tjommie from the desktop app, or run cargo run -p daemon --bin tjommie-daemon -- --dev in tjommie-rs for development.";
const ACCOUNTS_NOTICE: &str = "Using account folders from TJOMMIE_ACCOUNTS_DIR.";
const STALE_NOTICE: &str = "Removed a leftover development daemon-token file.";

fn token_file(root: &Path) -> std::path::PathBuf {
    root.join("daemon-token")
}

async fn exits_plainly(mut daemon: Daemon, message: &str) {
    assert_eq!(daemon.exited().await.code(), Some(1));
    assert_eq!(daemon.stderr().trim(), message);
    assert_eq!(daemon.stdout(), "");
    assert!(!token_file(&daemon.root.0).exists());
}

async fn piped_production_token(stale: bool) {
    let root = TempRoot::new();
    if stale {
        std::fs::write(token_file(&root.0), token()).unwrap();
    }
    let token = token();
    let mut daemon = launch(&["--token-stdin"], root, &[]);
    daemon
        .write(&format!("{token}\nignored second line\n"))
        .await;
    let port = daemon.ready().await;
    assert_eq!(
        refused(try_connect(port, &[("Origin", "http://localhost:3000")]).await),
        403
    );
    assert!(!token_file(&daemon.root.0).exists());
    if stale {
        assert!((daemon.stdout() + &daemon.stderr()).contains(STALE_NOTICE));
    }
    let mut client = connect(port).await;
    assert_eq!(client.hello(&token).await["type"], "welcome");
    let bots = client.call(1, "bots.list", json!({})).await;
    assert_eq!(
        (&bots["ok"], &bots["result"]),
        (&json!(true), &json!([])),
        "{bots}"
    );
    let mut stranger = connect(port).await;
    stranger
        .send(json!({ "type": "hello", "protocol": 2, "token": common::token() }))
        .await;
    assert_eq!(stranger.closed().await.map(|(code, _)| code), Some(4401));
    assert!(daemon.running());
    assert!(!token_file(&daemon.root.0).exists());
}

#[tokio::test]
async fn authenticates_a_piped_production_token() {
    piped_production_token(false).await;
}

#[tokio::test]
async fn authenticates_a_piped_production_token_and_removes_a_stale_file() {
    piped_production_token(true).await;
}

#[tokio::test]
async fn stops_when_the_desktop_app_closes_its_stdin() {
    let token = token();
    let mut daemon = launch(&["--token-stdin"], TempRoot::new(), &[]);
    daemon.write(&format!("{token}\n")).await;
    let port = daemon.ready().await;
    let mut client = connect(port).await;
    client.hello(&token).await;
    daemon.close_stdin();
    assert_eq!(daemon.exited().await.code(), Some(0));
    client.closed().await;
}

#[tokio::test]
async fn announces_the_account_folder_override_in_production_mode() {
    let root = TempRoot::new();
    let accounts = root.0.join("signed-in-accounts").display().to_string();
    let mut daemon = launch(
        &["--token-stdin"],
        root,
        &[("TJOMMIE_ACCOUNTS_DIR", &accounts)],
    );
    daemon.write(&format!("{}\n", token())).await;
    daemon.ready().await;
    assert_eq!(
        daemon.stderr().trim().lines().collect::<Vec<_>>(),
        [ACCOUNTS_NOTICE]
    );
}

/// The harness sets both test hooks, and the daemon says so, once each, so a log shows a protection is off. (The
/// `stderr()` the other tests compare leaves these two lines out.)
#[tokio::test]
async fn announces_each_test_hook_it_honours() {
    let daemon = launch(&["--dev"], TempRoot::new(), &[]);
    daemon.ready().await;
    let stderr = daemon.stderr_raw();
    let notices: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with(common::HOOK_NOTICE))
        .collect();
    assert_eq!(
        notices,
        [
            "Test hook TJOMMIE_SKIP_ACCOUNT_CHECK is on: the provider CLIs are not probed at start.",
            "Test hook TJOMMIE_SKIP_FOLDER_PROTECTION is on: the Codex data ACL pass is skipped.",
        ]
    );
}

#[tokio::test]
async fn announces_the_account_folder_override_in_development_mode() {
    let root = TempRoot::new();
    let accounts = root.0.join("signed-in-accounts").display().to_string();
    let daemon = launch(&["--dev"], root, &[("TJOMMIE_ACCOUNTS_DIR", &accounts)]);
    daemon.ready().await;
    assert_eq!(
        daemon.stderr().trim().lines().collect::<Vec<_>>(),
        [ACCOUNTS_NOTICE]
    );
}

#[tokio::test]
async fn exits_plainly_for_a_short_or_invalid_production_token() {
    for line in ["too-short\n".to_owned(), format!("{}=\n", "x".repeat(43))] {
        let mut daemon = launch(&["--token-stdin"], TempRoot::new(), &[]);
        daemon.write(&line).await;
        daemon.close_stdin();
        exits_plainly(
            daemon,
            "Daemon token must be at least 43 base64url characters.",
        )
        .await;
    }
}

#[tokio::test]
async fn exits_plainly_for_a_missing_production_token() {
    let mut daemon = launch(&["--token-stdin"], TempRoot::new(), &[]);
    daemon.close_stdin();
    exits_plainly(daemon, "stdin closed before a daemon token arrived.").await;
}

#[tokio::test]
async fn exits_plainly_when_no_production_token_arrives_in_time() {
    let daemon = launch(&["--token-stdin"], TempRoot::new(), &[]);
    exits_plainly(daemon, "No daemon token arrived on stdin within 5 seconds.").await;
}

/// TypeScript checks for a stack trace; the Rust exe prints the error's details instead.
#[tokio::test]
async fn prints_an_unexpected_startup_error_with_its_details_and_without_the_token() {
    let root = TempRoot::new();
    std::fs::create_dir(root.0.join("tjommie.db")).unwrap();
    let token = token();
    let mut daemon = launch(&["--token-stdin"], root, &[]);
    daemon
        .write(&format!(
            "{token}
"
        ))
        .await;
    daemon.close_stdin();
    assert_eq!(daemon.exited().await.code(), Some(1));
    let stderr = daemon.stderr();
    assert!(
        stderr.contains("Legacy database path is not a regular file"),
        "{stderr}"
    );
    assert!(!stderr.contains(&token));
    assert_eq!(daemon.stdout(), "");
}

#[tokio::test]
async fn names_the_database_file_when_it_cannot_be_opened() {
    let root = TempRoot::new();
    let db = root.0.join("data").join("tjommie.db");
    std::fs::create_dir_all(&db).unwrap();
    let token = token();
    let mut daemon = launch(&["--token-stdin"], root, &[]);
    daemon
        .write(&format!(
            "{token}
"
        ))
        .await;
    daemon.close_stdin();
    assert_eq!(daemon.exited().await.code(), Some(1));
    let stderr = daemon.stderr();
    assert!(stderr.contains(&db.display().to_string()), "{stderr}");
    assert!(
        !stderr.contains("Custom {") && !stderr.contains("kind:"),
        "{stderr}"
    );
    assert!(!stderr.contains(&token));
    assert_eq!(daemon.stdout(), "");
}

/// Only the argument check: `provision` itself creates Windows accounts, so no test runs it.
#[tokio::test]
async fn refuses_arguments_to_provision_before_touching_anything() {
    let root = TempRoot::new();
    let daemon = launch(&["provision", "--root", "elsewhere"], root, &[]);
    exits_plainly(
        daemon,
        "Usage: tjommie-daemon provision (from an elevated window, as the Tjommie owner)",
    )
    .await;
}

/// Only the argument check: `accept-worker` itself creates and removes worker fixtures, so no test runs it here.
#[tokio::test]
async fn refuses_arguments_to_accept_worker_before_touching_anything() {
    let root = TempRoot::new();
    let daemon = launch(&["accept-worker", "--root", "elsewhere"], root, &[]);
    exits_plainly(
        daemon,
        "Usage: tjommie-daemon accept-worker (as the Tjommie owner, not elevated, with Tjommie closed)",
    )
    .await;
}

/// With no worker configuration there is nothing to check: it says so and what to do first, creating nothing. The data
/// root is a throwaway folder, never the owner's.
#[tokio::test]
async fn accept_worker_without_a_provisioned_root_says_to_provision_first() {
    let root = TempRoot::new();
    let folder = root.0.clone();
    let mut daemon = launch(&["accept-worker"], root, &[]);
    assert_eq!(daemon.exited().await.code(), Some(1));
    assert_eq!(
        daemon.stderr().trim(),
        format!(
            "There is no worker configuration in {}. Run `tjommie-daemon provision` first (from an elevated window), then restart Windows.",
            folder.display()
        )
    );
    // Like `provision`, it says which install it is looking at before it does anything, then that the run is slow.
    assert_eq!(
        daemon.stdout().trim(),
        format!(
            "Checking the Windows worker accounts for {}\nChecking both worker accounts. This can take a few minutes; leave this window open.",
            folder.display()
        )
    );
    assert_eq!(
        std::fs::read_dir(&folder).unwrap().count(),
        0,
        "nothing was created"
    );
}

#[tokio::test]
async fn requires_an_explicit_mode() {
    exits_plainly(launch(&[], TempRoot::new(), &[]), MISSING_MODE).await;
}

#[tokio::test]
async fn rejects_an_unknown_argument_with_its_name() {
    exits_plainly(
        launch(&["--dev", "--port", "1"], TempRoot::new(), &[]),
        "Unknown argument: --port",
    )
    .await;
}

async fn writes_and_reuses_the_development_token_file(args: &[&str], dev: &str) {
    let mut first = launch(args, TempRoot::new(), &[("TJOMMIE_DEV", dev)]);
    first.ready().await;
    let token = std::fs::read_to_string(token_file(&first.root.0)).unwrap();
    assert!(
        token.len() == 43
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "{token}"
    );
    first.kill().await;
    let root = std::mem::replace(&mut first.root, TempRoot::new());
    let second = launch(args, root, &[("TJOMMIE_DEV", dev)]);
    let port = second.ready().await;
    try_connect(port, &[("Origin", "http://localhost:3000")])
        .await
        .expect("development allows the dev UI origin");
    assert_eq!(
        std::fs::read_to_string(token_file(&second.root.0)).unwrap(),
        token
    );
    assert_eq!(connect(port).await.hello(&token).await["type"], "welcome");
}

#[tokio::test]
async fn writes_and_reuses_the_development_token_file_with_the_dev_flag() {
    writes_and_reuses_the_development_token_file(&["--dev"], "").await;
}

#[tokio::test]
async fn writes_and_reuses_the_development_token_file_with_tjommie_dev() {
    writes_and_reuses_the_development_token_file(&[], "1").await;
}

#[tokio::test]
async fn rejects_conflicting_production_and_development_modes_before_reading_stdin() {
    let daemon = launch(&["--token-stdin"], TempRoot::new(), &[("TJOMMIE_DEV", "1")]);
    exits_plainly(daemon, "Choose either --dev or --token-stdin.").await;
}

// Not in main.test.ts.

#[tokio::test]
async fn prints_exactly_the_listening_line_the_desktop_reads() {
    // updater.real.test.ts:20 and the desktop's daemon-process.ts match this line with a regex. The resolved data root
    // comes first, so a log shows which install a daemon used.
    let daemon = launch(&["--dev"], TempRoot::new(), &[]);
    let port = daemon.ready().await;
    assert_eq!(
        daemon.stdout(),
        format!(
            "Tjommie data folder: {}\n{LISTENING}{port}\n",
            daemon.root.0.display()
        )
    );
}

#[tokio::test]
async fn reports_a_port_in_use_as_tjommie_already_running() {
    // daemon-process.test.ts: "reports the daemon's own reason when it can't start".
    let first = launch(&["--dev"], TempRoot::new(), &[]);
    let port = first.ready().await.to_string();
    let second = launch(&["--dev"], TempRoot::new(), &[("TJOMMIE_PORT", &port)]);
    exits_plainly(
        second,
        &format!("Tjommie is already running: port {port} is in use."),
    )
    .await;
}

#[tokio::test]
async fn accepts_the_desktop_json_credentials() {
    let (token, desktop_host_token) = (token(), token());
    let mut daemon = launch(&["--token-stdin"], TempRoot::new(), &[]);
    daemon
        .write(&format!(
            "{}\n",
            json!({ "token": token, "desktopHostToken": desktop_host_token })
        ))
        .await;
    let port = daemon.ready().await;
    assert_eq!(connect(port).await.hello(&token).await["type"], "welcome");
}

#[tokio::test]
async fn rejects_desktop_credentials_that_are_malformed_or_not_independent() {
    let token = token();
    for (line, message) in [
        ("{oops".to_owned(), "Invalid desktop startup credentials."),
        (
            json!({ "token": token, "desktopHostToken": "short" }).to_string(),
            "Invalid desktop startup credentials.",
        ),
        (
            json!({ "token": token }).to_string(),
            "Invalid desktop startup credentials.",
        ),
        (
            json!({ "token": token, "desktopHostToken": token }).to_string(),
            "The native browser host requires an independent private capability.",
        ),
    ] {
        let mut daemon = launch(&["--token-stdin"], TempRoot::new(), &[]);
        daemon.write(&format!("{line}\n")).await;
        exits_plainly(daemon, message).await;
    }
}
