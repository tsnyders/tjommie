//! T3.6: the browser's side of filling a saved login, on real HTTPS pages (`native-browser-login.ts` and `native-browser.ts`
//! `beginVault`, `writeVault`, `finishVault`). The pages are served over TLS on loopback with a test certificate that the
//! browser is told to accept twice over: `--ignore-certificate-errors` for the process (this binary only), and, before each
//! navigation, `Security.setIgnoreCertificateErrors` on the tab (as `browser_prompts.rs` does). Nothing real is touched: no
//! account, no Credential Manager entry, no network beyond 127.0.0.1.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Once};
use std::time::Duration;

use common::Rig;
use desktop_host::browser::engine::VaultFinish;
use desktop_host::browser::tools::Automation;
use desktop_host::vault::storage::VaultSecret;
use protocol::native_browser::NativeBrowserToolCommand;
use protocol::vault::{LoginOrigin, VaultFillTarget};
use rustls::{ServerConfig, ServerConnection, Stream};
use serde_json::json;

const LOGIN: &str = r#"<!doctype html><title>Sign in</title>
<form method="post" action="/session">
  <input id="user" type="text" autocomplete="username" aria-label="User" style="width:200px;height:24px">
  <input id="pass" type="password" aria-label="Password" style="width:200px;height:24px">
  <button type="submit">Sign in</button>
</form>"#;

fn page_for(path: &str) -> &'static str {
    match path {
        "/two.html" => {
            r#"<title>Two</title><form method="post" action="/s"><input id="user" type="text"><input type="password"><input type="password"><button>Go</button></form>"#
        }
        "/frame.html" => {
            r#"<title>Frame</title><iframe src="about:blank"></iframe><form method="post" action="/s"><input id="user" type="text"><input id="pass" type="password"></form>"#
        }
        "/get.html" => {
            r#"<title>Get</title><form method="get" action="/s"><input id="user" type="text"><input id="pass" type="password"></form>"#
        }
        "/other.html" => "<title>Other</title>nothing here",
        _ => LOGIN,
    }
}

/// An HTTPS server on loopback. Its identity is made for this run (`common::tls_identity`).
fn https_server() -> u16 {
    let (certs, key) = common::tls_identity();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("server certificate"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for tcp in listener.incoming() {
            let Ok(mut tcp) = tcp else { continue };
            let config = config.clone();
            std::thread::spawn(move || {
                let Ok(mut connection) = ServerConnection::new(config) else {
                    return;
                };
                let mut tls = Stream::new(&mut connection, &mut tcp);
                let mut request = [0u8; 4096];
                let read = tls.read(&mut request).unwrap_or(0);
                if read == 0 {
                    return;
                }
                let text = String::from_utf8_lossy(&request[..read]).into_owned();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = page_for(&path);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(reply.as_bytes());
                let _ = tls.flush();
                connection.send_close_notify();
                let _ = connection.complete_io(&mut tcp);
            });
        }
    });
    port
}

/// The browser accepts the test certificate from the start: this binary only, and before any browser starts. WebView2 reads this
/// only when its browser process starts, which a hosted runner's did not honour, so `go` also tells each tab (below).
fn accept_the_test_certificate() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: set once, before this binary's first browser is created; WebView2 reads it when it starts.
        unsafe {
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--ignore-certificate-errors",
            )
        };
    });
}

struct Fixture {
    rig: Rig,
    port: u16,
}

impl Fixture {
    fn new() -> Fixture {
        accept_the_test_certificate();
        Fixture {
            rig: Rig::new(),
            port: https_server(),
        }
    }

    fn origin(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }

    /// The bot's page at `path` on the HTTPS server, with the person holding the browser.
    fn on(&mut self, path: &str) {
        self.rig.open("alpha");
        self.go(path);
        self.rig
            .exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    }

    fn go(&mut self, path: &str) {
        let url = format!("{}{path}", self.origin());
        let guard = self.rig.begin("alpha");
        let tab = guard.tab().expect("a tab");
        let loaded = self.rig.ui.run(async move {
            // The fixture's certificate is self-signed, so this tab is told to ignore that before it navigates, as
            // browser_prompts.rs does (and it passed on a hosted runner). The process-wide variable alone was not enough there:
            // WebView2 reads it only when its browser process starts, and on a hosted runner every page here came back with the
            // pathname "/" (what Chromium's own error page reports), where this PC's came back as the page. If that is still
            // so, the message below says what the browser is on.
            tab.call("Security.setIgnoreCertificateErrors", r#"{"ignore":true}"#)
                .await
                .expect("ignore the test certificate");
            tab.navigate(&url).await
        });
        guard.end();
        // (This once accepted `Ok(false)` too, "because the certificate error is ignored". It was not the certificate: `navigate` took
        // the cancelled start page's completion for its own, which browser_navigate.rs now pins.)
        assert_eq!(loaded, Ok(true), "the navigation to {path} did not succeed");
        self.rig.eval_until(
            "alpha",
            "document.readyState",
            "complete",
            Duration::from_secs(30),
        );
        // One read, not a wait for the path: a navigation that returned before the page committed must fail here, and say
        // so, not be retried into passing.
        let seen = self.rig.eval("alpha", "location.pathname");
        if seen != path {
            // Where the browser is, and, if that is an error page, the error it names (ERR_...).
            let page = self.rig.eval(
                "alpha",
                "location.href + ' | ' + document.title + ' | ' + document.readyState + ' | ' + (document.body ? document.body.innerText.slice(0, 300) : '')",
            );
            panic!(
                "the page {path} did not load: the browser is on {page} (the navigation said {loaded:?})"
            );
        }
    }

    fn target(&self) -> VaultFillTarget {
        let state = self.rig.state("alpha");
        VaultFillTarget {
            bot_id: "alpha".into(),
            generation: state.generation,
            page_id: state.active_page_id.expect("a page"),
            control_epoch: state.control_epoch.expect("an epoch"),
        }
    }

    fn begin(&mut self, origin: &str) -> Result<u64, String> {
        let (engine, bot, target) = (
            self.rig.engine.clone(),
            self.rig.bot("alpha"),
            self.target(),
        );
        let origin = LoginOrigin::new(origin).expect("an https origin");
        self.rig
            .ui
            .run(async move {
                engine
                    .begin_vault(&bot, &target, "local", &origin, &|| Ok(()))
                    .await
            })
            .map_err(|error| error.to_string())
    }

    fn write(&mut self, epoch: u64, username: &str, password: &str) -> Result<(), String> {
        let (engine, bot) = (self.rig.engine.clone(), self.rig.bot("alpha"));
        let secret = VaultSecret::new(username, password);
        self.rig
            .ui
            .run(async move { engine.write_vault(&bot, epoch, &secret, &|| Ok(())).await })
            .map_err(|error| error.to_string())
    }

    fn finish(&mut self, epoch: u64, mode: VaultFinish) -> Result<(), String> {
        let (engine, bot) = (self.rig.engine.clone(), self.rig.bot("alpha"));
        let generation = self.rig.state("alpha").generation;
        self.rig
            .ui
            .run(async move {
                engine
                    .finish_vault(&bot, &generation, epoch, mode, &|| Ok(()))
                    .await
            })
            .map_err(|error| error.to_string())
    }

    fn values(&mut self) -> String {
        self.rig.eval(
            "alpha",
            "document.getElementById('user').value + '|' + document.getElementById('pass').value",
        )
    }
}

#[test]
fn a_login_is_filled_privately_then_cleared_and_verified() {
    let mut f = Fixture::new();
    f.on("/login.html");
    let origin = f.origin();
    let (engine, bot, target) = (f.rig.engine.clone(), f.rig.bot("alpha"), f.target());
    let found = engine
        .vault_destination(&bot, &target, "local")
        .expect("the page's origin");
    assert_eq!(
        found.as_str(),
        origin,
        "the destination is the page's exact HTTPS origin"
    );

    let epoch = f.begin(&origin).expect("the form is found");
    assert_eq!(
        f.rig.state("alpha").private_active,
        Some(true),
        "a private interaction is on"
    );
    assert_eq!(f.values(), "|", "nothing is typed yet");

    // While it lasts the bot's tools see nothing of the page.
    let tools = Automation::new(f.rig.engine.clone());
    let command: NativeBrowserToolCommand =
        serde_json::from_value(json!({ "name": "browser_snapshot", "args": {} }))
            .expect("a command");
    let (automation, bot_id) = (tools.clone(), f.rig.bot("alpha"));
    let reply = f
        .rig
        .ui
        .run(async move { automation.run(&bot_id, &command).await });
    assert!(
        reply.is_error,
        "no tool runs during a private interaction: {}",
        reply.words()
    );

    f.write(epoch, "alice", "correct horse battery staple")
        .expect("the secret is typed");
    assert_eq!(
        f.values(),
        "alice|correct horse battery staple",
        "the form holds the login"
    );

    f.finish(epoch, VaultFinish::Clear)
        .expect("the fields are cleared and verified");
    assert_eq!(f.values(), "|", "and the secret is gone from the page");
    assert_eq!(
        f.rig.state("alpha").private_active,
        Some(false),
        "the private interaction is over"
    );
    // The person still returns the browser to the bot explicitly, after looking at the cleared page.
    assert_eq!(
        f.rig.state("alpha").control,
        protocol::browser::BrowserControl::Human
    );
}

#[test]
fn a_page_that_is_not_an_ordinary_login_form_receives_nothing() {
    let mut f = Fixture::new();
    f.on("/login.html");
    for (path, why) in [
        ("/two.html", "two password fields"),
        ("/frame.html", "a nested frame"),
        ("/get.html", "a GET form"),
        ("/other.html", "no form at all"),
    ] {
        f.go(path);
        let origin = f.origin();
        let words = f.begin(&origin).expect_err(why);
        assert!(
            words.contains("cannot receive this login safely"),
            "{why}: {words}"
        );
        // A refused fill leaves the interaction private until it is finished: close it as the vault would.
        let epoch = f.rig.state("alpha").private_epoch.expect("an epoch");
        f.finish(epoch, VaultFinish::Close)
            .expect("the private browser closes");
        f.on("/login.html");
    }
}

#[test]
fn a_fill_is_refused_for_another_origin_a_stale_epoch_or_a_page_that_moved() {
    let mut f = Fixture::new();
    f.on("/login.html");
    let origin = f.origin();
    let words = f.begin("https://example.test").expect_err("another origin");
    assert_eq!(words, "This login belongs to a different HTTPS origin.");
    assert_eq!(
        f.rig.state("alpha").private_active,
        Some(false),
        "nothing began"
    );

    let epoch = f.begin(&origin).expect("the form is found");
    assert_eq!(
        f.write(epoch + 1, "alice", "x").expect_err("a stale epoch"),
        "The private login interaction changed."
    );
    f.go("/other.html");
    let words = f.write(epoch, "alice", "x").expect_err("the page moved");
    assert!(
        words.contains("cannot receive this login safely") || words.contains("changed"),
        "{words}"
    );
    assert_eq!(
        f.values_or_none(),
        None,
        "and nothing reached the page that replaced it"
    );
    f.finish(epoch, VaultFinish::Close).expect("close");
}

impl Fixture {
    /// The form's values, or `None` if the page has no form.
    fn values_or_none(&mut self) -> Option<String> {
        let found = self
            .rig
            .eval("alpha", "document.getElementById('user') ? 'form' : 'none'");
        (found == "form").then(|| self.values())
    }
}

#[test]
fn a_page_that_is_not_https_has_no_login_destination() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    rig.exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    let state = rig.state("alpha");
    let target = VaultFillTarget {
        bot_id: "alpha".into(),
        generation: state.generation,
        page_id: state.active_page_id.expect("a page"),
        control_epoch: state.control_epoch.expect("an epoch"),
    };
    let bot = rig.bot("alpha");
    let words = rig
        .engine
        .vault_destination(&bot, &target, "local")
        .expect_err("plain http")
        .to_string();
    assert!(words.contains("exact HTTPS origin"), "{words}");
    // And a destination that has moved on is refused in its own words.
    let stale = VaultFillTarget {
        control_epoch: target.control_epoch + 1,
        ..target.clone()
    };
    assert!(rig.engine.vault_destination(&bot, &stale, "local").is_err());
    let other = VaultFillTarget {
        page_id: "not-a-page".into(),
        ..target
    };
    assert_eq!(
        rig.engine
            .vault_destination(&bot, &other, "local")
            .expect_err("another page")
            .to_string(),
        "The login destination changed. Refresh the browser before filling."
    );
}

// ---- the vault's own thread, through EngineLogin (Phase 3 host wiring) -------------------------------------------------
//
// The vault runs on the `/vault-host` client's thread; the engine and WebView2 live on the UI thread. `EngineLogin` is the
// `LoginBrowser` the vault calls: each call becomes a typed request that `serve` runs on the UI thread, and what comes back
// is plain typed data. These tests are the vault's side: a thread of its own with its own runtime.

use std::cell::RefCell;
use std::rc::Rc;

use desktop_host::browser::BotIdentity;
use desktop_host::browser::login::{BotLookup, EngineLogin, serve};
use desktop_host::vault::login::{Check, LoginBrowser, LoginError};
use futures::future::BoxFuture;
use protocol::vault::FinishMode;

/// The daemon's bot list, with one bot in it.
struct Listed(BotIdentity);

impl BotLookup for Listed {
    fn bot(&self, id: &str) -> BoxFuture<'_, Result<BotIdentity, LoginError>> {
        let found = (id == self.0.id).then(|| self.0.clone());
        Box::pin(async move { found.ok_or(LoginError::Failed) })
    }
}

fn allowed() -> Check {
    Arc::new(|| Ok(()))
}

impl Fixture {
    /// `EngineLogin` for the bot `alpha`, served on this (the UI) thread.
    fn engine_login(&mut self) -> Arc<EngineLogin> {
        let (login, requests) = EngineLogin::new(Arc::new(Listed(self.rig.bot("alpha"))));
        let spawner = self.rig.ui.spawner();
        self.rig
            .ui
            .spawn(serve(self.rig.engine.clone(), requests, move |task| {
                use futures::task::LocalSpawnExt as _;
                let _ = spawner.spawn_local(task);
            }));
        Arc::new(login)
    }

    /// Runs `work` on a thread of its own, as the vault does, while this thread pumps the browser's messages.
    fn on_the_vaults_thread<T: Send + 'static>(
        &mut self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> T {
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            let _ = done.send(runtime.block_on(work));
        });
        let answer: Rc<RefCell<Option<T>>> = Rc::default();
        let slot = answer.clone();
        self.rig.ui.wait_until(
            "the vault's call to finish",
            Duration::from_secs(90),
            move || {
                if let Ok(value) = result.try_recv() {
                    *slot.borrow_mut() = Some(value);
                }
                slot.borrow().is_some()
            },
        );
        answer.borrow_mut().take().expect("the vault's answer")
    }
}

#[test]
fn the_vault_fills_and_clears_a_login_through_the_engine_from_its_own_thread() {
    let mut f = Fixture::new();
    f.on("/login.html");
    let (origin, target) = (f.origin(), f.target());
    let login = f.engine_login();

    let (asking, aimed) = (login.clone(), target.clone());
    let found = f
        .on_the_vaults_thread(async move { asking.destination(aimed, "local".into()).await })
        .expect("the page's origin");
    assert_eq!(found.as_str(), origin);

    let (beginning, aimed) = (login.clone(), target.clone());
    let wanted = LoginOrigin::new(&origin).expect("an https origin");
    let epoch = f
        .on_the_vaults_thread(async move {
            beginning
                .begin(aimed, "local".into(), wanted, allowed())
                .await
        })
        .expect("the form is found");
    assert_eq!(f.rig.state("alpha").private_active, Some(true));

    let filling = login.clone();
    f.on_the_vaults_thread(async move {
        filling
            .fill(
                "alpha".into(),
                epoch,
                VaultSecret::new("ada", "correct horse"),
                allowed(),
            )
            .await
    })
    .expect("the fields are filled");
    assert_eq!(f.values(), "ada|correct horse");

    let (finishing, generation) = (login.clone(), target.generation.clone());
    f.on_the_vaults_thread(async move {
        finishing
            .finish(
                "alpha".into(),
                generation,
                epoch,
                FinishMode::Clear,
                allowed(),
            )
            .await
    })
    .expect("the fields are cleared and verified");
    assert_eq!(f.values(), "|", "cleared");
}

#[test]
fn the_vault_is_refused_for_an_unknown_bot_a_stopped_check_and_a_different_origin() {
    let mut f = Fixture::new();
    f.on("/login.html");
    let (origin, target) = (f.origin(), f.target());
    let login = f.engine_login();
    let wanted = LoginOrigin::new(&origin).expect("an https origin");

    // A bot the daemon doesn't list: nothing reaches the engine.
    let (asking, aimed) = (login.clone(), target.clone());
    let stranger = VaultFillTarget {
        bot_id: "nobody".into(),
        ..aimed
    };
    assert_eq!(
        f.on_the_vaults_thread(async move { asking.destination(stranger, "local".into()).await }),
        Err(LoginError::Failed)
    );

    // A check that has stopped the operation stops the begin before the page is touched.
    let (beginning, aimed, origin_a) = (login.clone(), target.clone(), wanted.clone());
    let stopped: Check = Arc::new(|| Err(LoginError::Changed));
    assert!(
        f.on_the_vaults_thread(async move {
            beginning
                .begin(aimed, "local".into(), origin_a, stopped)
                .await
        })
        .is_err()
    );
    assert_ne!(
        f.rig.state("alpha").private_active,
        Some(true),
        "no private interaction began"
    );

    // A login that belongs to a different origin is refused.
    let (beginning, aimed) = (login.clone(), target.clone());
    let elsewhere = LoginOrigin::new("https://example.com").expect("an https origin");
    assert!(
        f.on_the_vaults_thread(async move {
            beginning
                .begin(aimed, "local".into(), elsewhere, allowed())
                .await
        })
        .is_err()
    );
    assert_ne!(f.rig.state("alpha").private_active, Some(true));
}

// ---- the whole host stack: the daemon's routes, the clients, the vault, EngineLogin and the engine ----------------------
//
// A login is saved and filled the way the person does it from Logins: the daemon's `/vault-host` queues the commands (as it
// does for the app's RPCs), the app's `Hosts` serves them with its vault, the vault asks the person's windows (here: always
// yes, and a typed login), and the browser steps run on the real engine through `EngineLogin`, on a real HTTPS page, in
// private mode. Only the Logins windows are stand-ins. The secret goes to the real Credential Manager under the namespace of
// the test's own temp root, and a guard deletes what the test saved (by reference, under that namespace only) even when it
// fails; on a hosted runner, whose logons can't reach Credential Manager, it is kept in memory instead.

use std::collections::HashMap;
use std::sync::Mutex;

use daemon::native_browser_broker::NativeBrowserBroker;
use daemon::server::{Handlers, Routes, Server, ServerOptions};
use daemon::vault_host_broker::{VaultHostBroker, VaultRequestOptions};
use daemon::worker_boundary_host::{WorkerBoundary, WorkerBoundaryHost, WorkerLease};
use desktop_host::browser::engine::Reply;
use desktop_host::browser::host_bridge::{Executor as BridgeExecutor, Job};
use desktop_host::hosts::{BotDirectory, BotInfo, DirectoryError, Hosts, Local, Parts};
use desktop_host::vault::credential::LoginCredentials;
use desktop_host::vault::entry::EntryInput;
use desktop_host::vault::login::{Cancel, Confirmation, EntryWindow, SecretStore};
use desktop_host::vault::storage::VaultError;
use protocol::vault::{
    LoginEntry, VaultEntry, VaultFill, VaultFinish as FinishCommand, VaultHostAction,
    VaultHostCommand,
};
use serde_json::Value;

struct Available;

struct OneLease;

impl WorkerLease for OneLease {
    fn valid(&self) -> bool {
        true
    }

    fn release(&self) {}
}

impl WorkerBoundary for Available {
    fn unavailable_reason(&self) -> futures_util::future::BoxFuture<'static, Option<String>> {
        Box::pin(async { None })
    }

    fn acquire(&self) -> futures_util::future::BoxFuture<'static, Option<Box<dyn WorkerLease>>> {
        Box::pin(async { Some(Box::new(OneLease) as Box<dyn WorkerLease>) })
    }

    fn invalidate(&self) {}
}

/// The person: types one login into the entry window and says yes to every confirmation.
struct Person {
    origin: String,
}

impl EntryWindow for Person {
    fn entry(
        &self,
        _: String,
        _: Option<LoginEntry>,
        _: Cancel,
    ) -> BoxFuture<'_, Result<Option<EntryInput>, LoginError>> {
        Box::pin(async move {
            Ok(Some(EntryInput {
                name: "Work login".to_owned(),
                origin: LoginOrigin::new(&self.origin).map_err(|_| LoginError::Failed)?,
                secret: VaultSecret::new("ada", "correct horse"),
            }))
        })
    }

    fn confirm(&self, _: Confirmation, _: Cancel) -> BoxFuture<'_, Result<bool, LoginError>> {
        Box::pin(async { Ok(true) })
    }

    fn close(&self) {}
}

#[derive(Default)]
struct Kept(Mutex<HashMap<String, (String, String)>>);

impl SecretStore for Kept {
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
            .map(|(user, password)| VaultSecret::new(user, password)))
    }

    fn delete(&self, reference: &str) -> Result<(), VaultError> {
        self.0.lock().expect("secrets").remove(reference);
        Ok(())
    }
}

/// The real Credential Manager, under the test root's namespace, noting each reference the vault writes so that a guard can
/// delete exactly those (the records' own list can be unreadable while the vault still holds its database open).
struct Recording {
    credentials: Arc<LoginCredentials>,
    written: Arc<Mutex<Vec<String>>>,
}

impl SecretStore for Recording {
    fn write(&self, reference: &str, secret: &VaultSecret) -> Result<(), VaultError> {
        // Noted first: a write that fails half way is still deleted.
        self.written
            .lock()
            .expect("references")
            .push(reference.to_owned());
        self.credentials.write(reference, secret)
    }

    fn read(&self, reference: &str) -> Result<Option<VaultSecret>, VaultError> {
        self.credentials.read(reference)
    }

    fn delete(&self, reference: &str) -> Result<(), VaultError> {
        self.credentials.delete(reference)
    }
}

/// Deletes what the test wrote to Credential Manager, by reference and under the test root's namespace only, when the test ends,
/// however it ends.
struct CredentialCleanup {
    credentials: Arc<LoginCredentials>,
    written: Arc<Mutex<Vec<String>>>,
}

impl Drop for CredentialCleanup {
    fn drop(&mut self) {
        let references = self
            .written
            .lock()
            .map(|references| references.clone())
            .unwrap_or_default();
        for reference in references {
            let _ = self.credentials.delete(&reference);
        }
    }
}

struct AlphaIsListed(BotIdentity);

impl BotDirectory for AlphaIsListed {
    fn find(&self, id: &str) -> BoxFuture<'_, Result<Option<BotInfo>, DirectoryError>> {
        let found = (id == self.0.id).then(|| BotInfo {
            identity: self.0.clone(),
            browser: true,
        });
        Box::pin(async move { Ok(found) })
    }
}

/// The browser jobs of the daemon's other route: none come in this test.
struct NoJobs;

impl BridgeExecutor for NoJobs {
    fn execute(&self, _: Job) -> BoxFuture<'static, Result<Reply, String>> {
        Box::pin(async { Ok(Reply::empty()) })
    }
}

fn command(action: VaultHostAction) -> Value {
    serde_json::to_value(VaultHostCommand {
        controller_id: "local".into(),
        action,
    })
    .expect("a command serializes")
}

/// What the person does from Logins, as the daemon queues it: the command goes to the broker, the vault answers on its
/// thread, and this (the UI) thread pumps the browser's messages until the answer comes.
fn ask(
    f: &mut Fixture,
    runtime: &tokio::runtime::Runtime,
    broker: &Arc<VaultHostBroker>,
    action: VaultHostAction,
) -> Value {
    let (broker, command) = (broker.clone(), command(action));
    let request = runtime.spawn(async move {
        broker
            .request(command, VaultRequestOptions::default())
            .await
            .map_err(|error| error.to_string())
    });
    f.rig.ui.wait_until(
        "the daemon's Logins request to be answered",
        Duration::from_secs(60),
        || request.is_finished(),
    );
    runtime
        .block_on(request)
        .expect("the request task")
        .expect("the vault answered")
}

#[test]
fn a_saved_login_is_filled_and_cleared_privately_through_the_daemons_vault_route() {
    let mut f = Fixture::new();
    f.on("/login.html");
    let (origin, target) = (f.origin(), f.target());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");

    // The daemon: the three host routes behind one host token.
    let host_token = "h".repeat(43);
    let renderer_token = "r".repeat(43);
    let browser_broker = NativeBrowserBroker::new(host_token.clone(), Duration::from_secs(20));
    let vault_broker = VaultHostBroker::new(host_token.clone(), Duration::from_secs(60));
    let worker = WorkerBoundaryHost::new(Arc::new(Available), host_token.clone());
    let server = runtime
        .block_on(Server::start(ServerOptions {
            port: 0,
            token: renderer_token.clone(),
            handlers: Handlers::default(),
            allowed_origins: Vec::new(),
            defer_requests: false,
        }))
        .expect("the daemon's server starts");
    server
        .activate_with(
            &renderer_token,
            Handlers::default(),
            Routes::default()
                .route("/browser-host", browser_broker.route())
                .route("/vault-host", vault_broker.route())
                .route("/worker-host", worker.route()),
        )
        .expect("the routes mount");

    // The app: its vault and its clients, with the engine's browser steps served on this (the UI) thread.
    let root = tempfile::tempdir().expect("a temp root");
    // Declared before `hosts`, so it drops after them: the vault is closed before its secrets are deleted.
    let (secrets, _cleanup): (Arc<dyn SecretStore>, Option<CredentialCleanup>) =
        if std::env::var_os("TJOMMIE_CI_HOSTED").is_some() {
            (Arc::new(Kept::default()), None)
        } else {
            let credentials = Arc::new(
                LoginCredentials::new(root.path()).expect("a namespace for the temp root"),
            );
            let written = Arc::new(Mutex::new(Vec::new()));
            (
                Arc::new(Recording {
                    credentials: credentials.clone(),
                    written: written.clone(),
                }),
                Some(CredentialCleanup {
                    credentials,
                    written,
                }),
            )
        };
    let (hosts, requests) = Hosts::new(Parts {
        root: root.path().to_owned(),
        secrets: Some(secrets),
        entry: Arc::new(Person {
            origin: origin.clone(),
        }),
        runtime: runtime.handle().clone(),
    });
    assert!(hosts.has_vault());
    let spawner = f.rig.ui.spawner();
    f.rig.ui.spawn(desktop_host::browser::login::serve(
        f.rig.engine.clone(),
        requests,
        move |task| {
            use futures::task::LocalSpawnExt as _;
            let _ = spawner.spawn_local(task);
        },
    ));
    let running = hosts
        .start(
            &Local {
                port: server.port(),
                token: renderer_token,
                host_token: desktop_host::vault::host::HostToken::new(host_token),
            },
            Arc::new(AlphaIsListed(f.rig.bot("alpha"))),
            Arc::new(NoJobs),
        )
        .expect("the clients start");

    // What the person does from Logins, as the daemon queues it.
    assert_eq!(
        ask(&mut f, &runtime, &vault_broker, VaultHostAction::Unlock)["state"],
        "unlocked"
    );
    let saved = ask(
        &mut f,
        &runtime,
        &vault_broker,
        VaultHostAction::Entry(VaultEntry {
            bot_id: "alpha".into(),
            id: None,
            revision: None,
        }),
    );
    assert_eq!(saved["status"], "saved", "{saved}");
    let id = saved["entry"]["id"].as_str().expect("an id").to_owned();
    let revision = saved["entry"]["revision"].as_u64().expect("a revision");

    let filled = ask(
        &mut f,
        &runtime,
        &vault_broker,
        VaultHostAction::Fill(VaultFill {
            bot_id: "alpha".into(),
            generation: target.generation.clone(),
            page_id: target.page_id.clone(),
            control_epoch: target.control_epoch,
            id,
            revision,
        }),
    );
    assert_eq!(filled["status"], "filled", "{filled}");
    let private_epoch = filled["privateEpoch"].as_u64().expect("a private epoch");
    assert_eq!(
        f.rig.state("alpha").private_active,
        Some(true),
        "the page is private while it is filled"
    );
    assert_eq!(f.values(), "ada|correct horse", "the login is on the page");

    let finished = ask(
        &mut f,
        &runtime,
        &vault_broker,
        VaultHostAction::Finish(FinishCommand {
            bot_id: "alpha".into(),
            generation: target.generation,
            private_epoch,
            mode: FinishMode::Clear,
        }),
    );
    assert_eq!(finished["status"], "cleared", "{finished}");
    assert_eq!(f.values(), "|", "the fields are cleared and verified");

    runtime.block_on(hosts.stop(running));
    runtime.block_on(server.close());
}
