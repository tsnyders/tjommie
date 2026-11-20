//! The desktop's three clients of the daemon (`/browser-host`, `/vault-host`, `/worker-host`), started and stopped as one
//! (`main.ts` `launchDaemon`, `will-quit`; Phase 3 host wiring), against the daemon's real routes in-process on 127.0.0.1.
//! The browser's jobs go to a stand-in for the UI thread (no WebView2 here: `tests/vault_fill.rs` and `e2e_restart.rs`
//! have the engine). Nothing here touches the app's data folder, Credential Manager or the network beyond 127.0.0.1: the
//! vault's records live in a temp folder and its secrets in memory.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::native_browser_broker::{BotIdentity as WireBot, NativeBrowserBroker};
use daemon::server::{Handlers, Routes, Server, ServerOptions};
use daemon::vault_host_broker::{VaultHostBroker, VaultRequestOptions};
use daemon::worker_boundary_host::{WorkerBoundary, WorkerBoundaryHost, WorkerLease};
use desktop_host::browser::BotIdentity;
use desktop_host::browser::engine::Reply;
use desktop_host::browser::host_bridge::{Executor as BrowserExecutor, Job};
use desktop_host::hosts::{BotDirectory, BotInfo, DirectoryError, Hosts, Local, Parts};
use desktop_host::vault::gate::{GateError, VaultReply};
use desktop_host::vault::host::HostToken;
use desktop_host::vault::login::{Cancel, Confirmation, EntryWindow, LoginError, SecretStore};
use desktop_host::vault::storage::{VaultError, VaultSecret};
use futures::future::BoxFuture;
use protocol::vault::{LoginEntry, VaultHostAction, VaultHostCommand, VaultList, VaultState};
use serde_json::{Value, json};

fn secret(letter: char) -> String {
    letter.to_string().repeat(43)
}

// ---- the daemon's side ----------------------------------------------------------------------------------------------

/// A boundary that says isolation is available, and counts how often it is asked.
#[derive(Default)]
struct Boundary {
    asked: AtomicUsize,
}

struct Lease;

impl WorkerLease for Lease {
    fn valid(&self) -> bool {
        true
    }

    fn release(&self) {}
}

impl WorkerBoundary for Boundary {
    fn unavailable_reason(&self) -> futures_util::future::BoxFuture<'static, Option<String>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { None })
    }

    fn acquire(&self) -> futures_util::future::BoxFuture<'static, Option<Box<dyn WorkerLease>>> {
        Box::pin(async { Some(Box::new(Lease) as Box<dyn WorkerLease>) })
    }

    fn invalidate(&self) {}
}

/// One daemon: the three host routes behind one host token, as the real daemon mounts them.
struct Daemon {
    browser: Arc<NativeBrowserBroker>,
    vault: Arc<VaultHostBroker>,
    boundary: Arc<Boundary>,
    server: Server,
    local: Local,
}

impl Daemon {
    async fn start(letter: char) -> Daemon {
        let host_token = secret(letter);
        let browser = NativeBrowserBroker::new(host_token.clone(), Duration::from_secs(3));
        let vault = VaultHostBroker::new(host_token.clone(), Duration::from_secs(3));
        let boundary = Arc::new(Boundary::default());
        let worker = WorkerBoundaryHost::new(boundary.clone(), host_token.clone());
        let renderer = secret(letter.to_ascii_uppercase());
        let server = Server::start(ServerOptions {
            port: 0,
            token: renderer.clone(),
            handlers: Handlers::default(),
            allowed_origins: Vec::new(),
            defer_requests: false,
        })
        .await
        .expect("the daemon's server starts");
        server
            .activate_with(
                &renderer,
                Handlers::default(),
                Routes::default()
                    .route("/browser-host", browser.route())
                    .route("/vault-host", vault.route())
                    .route("/worker-host", worker.route()),
            )
            .expect("the routes mount");
        let local = Local {
            port: server.port(),
            token: renderer,
            host_token: HostToken::new(host_token),
        };
        Daemon {
            browser,
            vault,
            boundary,
            server,
            local,
        }
    }

    /// A job for the bot's browser, as the daemon's MCP proxy or a phone would queue it.
    async fn browser_job(&self) -> Result<Value, String> {
        let bot = WireBot {
            id: "bot-1".into(),
            name: "Forge".into(),
            home_folder: "C:\\bots\\Forge".into(),
            project_folder: None,
        };
        self.browser
            .request(bot, json!({ "kind": "close" }), None)
            .await
            .map_err(|error| error.to_string())
    }

    async fn vault_command(&self, action: VaultHostAction) -> Result<Value, String> {
        let command = serde_json::to_value(VaultHostCommand {
            controller_id: "local".into(),
            action,
        })
        .expect("a command serializes");
        self.vault
            .request(command, VaultRequestOptions::default())
            .await
            .map_err(|error| error.to_string())
    }
}

// ---- the app's side: what only the app has --------------------------------------------------------------------------

/// The UI thread's stand-in: every browser job is answered `{}` and counted.
#[derive(Default)]
struct Jobs {
    seen: AtomicUsize,
    lost: AtomicUsize,
}

impl BrowserExecutor for Jobs {
    fn execute(&self, _: Job) -> BoxFuture<'static, Result<Reply, String>> {
        self.seen.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Reply::empty()) })
    }

    fn daemon_lost(&self) {
        self.lost.fetch_add(1, Ordering::SeqCst);
    }
}

/// The person at the trusted window says yes to every confirmation.
struct Yes;

impl EntryWindow for Yes {
    fn entry(
        &self,
        _: String,
        _: Option<LoginEntry>,
        _: Cancel,
    ) -> BoxFuture<'_, Result<Option<desktop_host::vault::entry::EntryInput>, LoginError>> {
        Box::pin(async { Ok(None) })
    }

    fn confirm(&self, _: Confirmation, _: Cancel) -> BoxFuture<'_, Result<bool, LoginError>> {
        Box::pin(async { Ok(true) })
    }

    fn close(&self) {}
}

#[derive(Default)]
struct Secrets(Mutex<HashMap<String, (String, String)>>);

impl SecretStore for Secrets {
    fn write(&self, reference: &str, secret: &VaultSecret) -> Result<(), VaultError> {
        self.0.lock().expect("secrets").insert(
            reference.to_owned(),
            (secret.username().to_owned(), secret.password().to_owned()),
        );
        Ok(())
    }

    fn read(&self, reference: &str) -> Result<Option<VaultSecret>, VaultError> {
        Ok(self
            .0
            .lock()
            .expect("secrets")
            .get(reference)
            .map(|(username, password)| VaultSecret::new(username, password)))
    }

    fn delete(&self, reference: &str) -> Result<(), VaultError> {
        self.0.lock().expect("secrets").remove(reference);
        Ok(())
    }
}

/// The daemon's bot list: one bot, with its browser on.
struct Listed;

impl BotDirectory for Listed {
    fn find(&self, id: &str) -> BoxFuture<'_, Result<Option<BotInfo>, DirectoryError>> {
        let found = (id == "bot-1").then(|| BotInfo {
            identity: BotIdentity {
                id: "bot-1".into(),
                name: "Forge".into(),
                home_folder: "C:\\bots\\Forge".into(),
                project_folder: None,
            },
            browser: true,
        });
        Box::pin(async move { Ok(found) })
    }
}

/// A daemon whose bot list cannot be read.
struct Unreadable;

impl BotDirectory for Unreadable {
    fn find(&self, _: &str) -> BoxFuture<'_, Result<Option<BotInfo>, DirectoryError>> {
        Box::pin(async { Err(DirectoryError) })
    }
}

struct App {
    hosts: Hosts,
    _root: tempfile::TempDir,
    // Keeps the engine requests' inbox open: nothing serves them here.
    _requests: async_channel::Receiver<desktop_host::browser::login::Request>,
}

fn app() -> App {
    let root = tempfile::tempdir().expect("a temp root");
    let (hosts, requests) = Hosts::new(Parts {
        root: root.path().to_owned(),
        secrets: Some(Arc::new(Secrets::default())),
        entry: Arc::new(Yes),
        runtime: tokio::runtime::Handle::current(),
    });
    assert!(hosts.has_vault(), "the vault opens in a temp root");
    hosts.window().show();
    App {
        hosts,
        _root: root,
        _requests: requests,
    }
}

async fn status(hosts: &Hosts) -> VaultState {
    match hosts
        .execute_local(VaultHostAction::Status)
        .await
        .expect("a trusted local status")
    {
        VaultReply::Status(status) => status.state,
        other => panic!("not a status: {other:?}"),
    }
}

async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..300 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

// ---- the tests -------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_app_starts_all_three_clients_of_the_daemon() {
    let daemon = Daemon::start('a').await;
    let app = app();
    let jobs = Arc::new(Jobs::default());
    let running = app
        .hosts
        .start(&daemon.local, Arc::new(Listed), jobs.clone())
        .expect("the clients start");

    // /browser-host: the daemon's job reaches the UI thread's executor and its answer comes back.
    assert_eq!(daemon.browser_job().await, Ok(json!({})));
    assert_eq!(jobs.seen.load(Ordering::SeqCst), 1);
    // /vault-host: the daemon's Logins status is answered by the app's own vault.
    assert_eq!(
        daemon.vault_command(VaultHostAction::Status).await,
        Ok(json!({ "state": "locked", "reason": null, "pendingDeletes": 0 }))
    );
    // /worker-host: that status asked the daemon whether isolation is available, so the worker client is wired in.
    assert!(daemon.boundary.asked.load(Ordering::SeqCst) >= 1);

    app.hosts.stop(running).await;
    daemon.server.close().await;
}

/// Review Nit 6: a bot list the app could not read is the daemon being unreachable, not the bot being gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bot_list_that_cannot_be_read_is_not_taken_for_a_bot_that_is_gone() {
    let daemon = Daemon::start('c').await;
    let app = app();
    let list = |bot: &str| {
        VaultHostAction::List(VaultList {
            bot_id: Some(bot.to_owned()),
        })
    };
    let running = app
        .hosts
        .start(
            &daemon.local,
            Arc::new(Unreadable),
            Arc::new(Jobs::default()),
        )
        .expect("the clients start");
    assert_eq!(
        app.hosts.execute_local(list("bot-1")).await,
        Err(GateError::Unavailable)
    );
    app.hosts.stop(running).await;

    // A list that can be read, and does not have the bot, is the bot being gone.
    let running = app
        .hosts
        .start(&daemon.local, Arc::new(Listed), Arc::new(Jobs::default()))
        .expect("the clients start");
    assert!(matches!(
        app.hosts.execute_local(list("nobody")).await,
        Err(GateError::Vault(_))
    ));
    app.hosts.stop(running).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_daemon_with_a_new_token_is_served_and_the_old_one_is_let_go() {
    let first = Daemon::start('a').await;
    let second = Daemon::start('b').await;
    let app = app();
    let jobs = Arc::new(Jobs::default());
    let old = app
        .hosts
        .start(&first.local, Arc::new(Listed), jobs.clone())
        .expect("the first clients start");
    assert_eq!(first.browser_job().await, Ok(json!({})));
    // The person unlocks Logins on the first daemon's vault.
    let unlocked = first.vault_command(VaultHostAction::Unlock).await;
    assert_eq!(
        unlocked.expect("unlocked")["state"],
        "unlocked",
        "the vault unlocks"
    );

    // The daemon restarts: the clients stop (the vault locks), and new ones start with the new port and token.
    app.hosts.stop(old).await;
    let jobs_after = Arc::new(Jobs::default());
    let new = app
        .hosts
        .start(&second.local, Arc::new(Listed), jobs_after.clone())
        .expect("the new clients start");
    assert_eq!(second.browser_job().await, Ok(json!({})));
    assert_eq!(jobs_after.seen.load(Ordering::SeqCst), 1);
    assert_eq!(
        second
            .vault_command(VaultHostAction::Status)
            .await
            .expect("the new daemon's Logins are served")["state"],
        "locked",
        "the restart locked the vault"
    );
    // The old daemon has nobody serving it any more.
    assert!(first.browser_job().await.is_err());
    assert_eq!(
        jobs.seen.load(Ordering::SeqCst),
        1,
        "no job reached the old executor"
    );

    app.hosts.stop(new).await;
    first.server.close().await;
    second.server.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_loses_the_daemon_locks_the_vault() {
    let daemon = Daemon::start('a').await;
    let app = app();
    let jobs = Arc::new(Jobs::default());
    let running = app
        .hosts
        .start(&daemon.local, Arc::new(Listed), jobs.clone())
        .expect("the clients start");
    let unlocked = daemon.vault_command(VaultHostAction::Unlock).await;
    assert_eq!(unlocked.expect("unlocked")["state"], "unlocked");
    assert_eq!(status(&app.hosts).await, VaultState::Unlocked);

    // The daemon goes away under the running clients. The vault locks (its status then reads unavailable, for the
    // worker client has no daemon to ask).
    daemon.browser.close();
    daemon.vault.close();
    daemon.server.close().await;
    let mut seen = VaultState::Unlocked;
    for _ in 0..300 {
        seen = status(&app.hosts).await;
        if seen != VaultState::Unlocked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(seen, VaultState::Unlocked, "the vault locked");
    eventually("the browser to be told the daemon is gone", || {
        jobs.lost.load(Ordering::SeqCst) >= 1
    })
    .await;
    app.hosts.stop(running).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_request_is_trusted_only_while_the_window_is_shown_ready_and_the_app_is_not_quitting()
 {
    let app = app();
    // Shown and ready (the helper shows it).
    assert!(
        app.hosts
            .execute_local(VaultHostAction::Status)
            .await
            .is_ok()
    );
    // Hidden or minimized.
    app.hosts.window().set_visible(false);
    assert_eq!(
        app.hosts.execute_local(VaultHostAction::Status).await,
        Err(GateError::Untrusted)
    );
    // Shown again, and the app is quitting.
    app.hosts.window().set_visible(true);
    app.hosts.begin_quit();
    assert_eq!(
        app.hosts.execute_local(VaultHostAction::Status).await,
        Err(GateError::Untrusted)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quit_that_is_given_up_leaves_the_window_trusted_again() {
    let app = app();
    app.hosts.begin_quit();
    assert_eq!(
        app.hosts.execute_local(VaultHostAction::Status).await,
        Err(GateError::Untrusted)
    );
    // The update was not installed, so the app stays: requests from its window are taken again.
    app.hosts.cancel_quit();
    assert!(
        app.hosts
            .execute_local(VaultHostAction::Status)
            .await
            .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_is_up_front_only_while_it_is_shown_and_focused() {
    let app = app();
    let window = app.hosts.window();
    assert!(
        !window.visible_and_focused(),
        "shown, but nothing focused it yet"
    );
    window.set_focused(true);
    assert!(window.visible_and_focused());
    window.set_visible(false);
    assert!(!window.visible_and_focused(), "hidden to the tray");
}
