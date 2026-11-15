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
