//! F2: a bot's page can raise three native prompts that no one can see, because the page is parked or hidden: a screen
//! picker (`getDisplayMedia`), an HTTP basic-auth credential dialog and a client-certificate picker. Each is cancelled,
//! the page gets the refusal, and the person is told. If one were shown it would block its page, so each test also
//! has a bound: a prompt left open fails it.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::io::{BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use common::Rig;
use rustls::pki_types::CertificateDer;
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, Stream};

fn tls_fixture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("tls")
}

/// An HTTPS server on loopback that asks every client for a certificate (and accepts one without). Test-only
/// certificate and key, committed with the fixtures.
fn start_tls_server() -> u16 {
    let dir = tls_fixture_dir();
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut BufReader::new(
        std::fs::File::open(dir.join("cert.pem")).expect("cert"),
    ))
    .collect::<Result<_, _>>()
    .expect("certificates");
    let key = rustls_pemfile::private_key(&mut BufReader::new(
        std::fs::File::open(dir.join("key.pem")).expect("key"),
    ))
    .expect("key file")
    .expect("a key");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.add(certs[0].clone()).expect("root");
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .allow_unauthenticated()
        .build()
        .expect("a client verifier");
    let config = Arc::new(
        ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_client_cert_verifier(verifier)
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
                let mut request = [0u8; 2048];
                if tls.read(&mut request).unwrap_or(0) == 0 {
                    return;
                }
                let body = "<title>tls ok</title>tls ok";
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

#[test]
fn screen_capture_is_denied_so_a_hidden_page_never_raises_the_screen_picker() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let awake = rig.begin("alpha");
    let tab = awake.tab().expect("active tab");
    // CDP-injected input gives the page user activation, which is all `getDisplayMedia` asks for.
    let params = serde_json::json!({
        "expression": "navigator.mediaDevices.getDisplayMedia({ video: true }).then(() => 'granted', (error) => error.name)",
        "awaitPromise": true, "returnByValue": true, "userGesture": true,
    })
    .to_string();
    let answered = rig.ui.run_within(Duration::from_secs(20), async move {
        tab.call("Runtime.evaluate", &params).await
    });
    let reply = answered
        .expect("a screen picker would hold the page; it must be refused at once")
        .expect("evaluate");
    assert!(
        reply.contains("NotAllowedError"),
        "the page must be refused: {reply}"
    );
    assert_eq!(
        rig.wait_notice("media"),
        "Browser permission 'media' is unavailable in Tjommie."
    );
    awake.end();
}

#[test]
fn a_basic_auth_prompt_is_cancelled_and_the_page_reports_the_401() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let awake = rig.begin("alpha");
    let (tab, url) = (
        awake.tab().expect("active tab"),
        rig.server.url("basic-auth"),
    );
    // A credential dialog would hold this navigation for good.
    let finished = rig.ui.run_within(
        Duration::from_secs(30),
        async move { tab.navigate(&url).await },
    );
    assert!(
        finished.is_some(),
        "the navigation was held by a credential dialog"
    );
    let tab = awake.tab().expect("active tab");
    let body = rig
        .ui
        .run(async move { common::eval(&tab, "document.body.innerText").await });
    assert_eq!(
        body.trim(),
        "unauthorized",
        "the page shows the server's 401 body"
    );
    assert_eq!(
        rig.wait_notice("password"),
        "A page asked for a password, which Tjommie's browser never gives."
    );
    awake.end();
}

#[test]
fn a_client_certificate_request_is_declined_and_the_page_still_loads() {
    let mut rig = Rig::new();
    let port = start_tls_server();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let awake = rig.begin("alpha");
    let tab = awake.tab().expect("active tab");
    // The fixture's certificate is self-signed: this test alone tells its own browser to ignore that.
    let (prepare, url) = (tab.clone(), format!("https://127.0.0.1:{port}/"));
    let finished = rig.ui.run_within(Duration::from_secs(30), async move {
        prepare
            .call("Security.setIgnoreCertificateErrors", r#"{"ignore":true}"#)
            .await
            .expect("ignore the test certificate");
        prepare.navigate(&url).await
    });
    assert!(
        finished.is_some(),
        "the navigation was held by a certificate picker"
    );
    let title = rig
        .ui
        .run(async move { common::eval(&tab, "document.title").await });
    assert_eq!(title, "tls ok");
    assert_eq!(
        rig.wait_notice("certificate"),
        "A page asked for a client certificate, which Tjommie's browser never gives."
    );
    awake.end();
}

#[test]
fn a_link_to_another_app_is_stopped_by_the_address_policy_before_any_launch_prompt() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let awake = rig.begin("alpha");
    let tab = awake.tab().expect("active tab");
    // A click on a mailto: or ms-settings: link, as a person's click would make it.
    for scheme in ["mailto:someone@example.test", "ms-settings:display"] {
        let params = serde_json::json!({
            "expression": format!("location.href = '{scheme}'; 'tried'"), "returnByValue": true, "userGesture": true,
        })
        .to_string();
        let tab = tab.clone();
        let answered = rig.ui.run_within(Duration::from_secs(15), async move {
            tab.call("Runtime.evaluate", &params).await
        });
        assert!(answered.is_some(), "{scheme}: the page was held");
        rig.wait_notice("unsupported browser address");
    }
    // The page is still there and still answers.
    let tab = awake.tab().expect("active tab");
    let title = rig
        .ui
        .run(async move { common::eval(&tab, "document.title").await });
    assert_eq!(title, "Blank fixture");
    awake.end();
}

#[test]
fn notifications_are_denied_like_every_other_permission() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let awake = rig.begin("alpha");
    let tab = awake.tab().expect("active tab");
    let params = serde_json::json!({
        "expression": "Notification.requestPermission()", "awaitPromise": true, "returnByValue": true, "userGesture": true,
    })
    .to_string();
    let answered = rig.ui.run_within(Duration::from_secs(20), async move {
        tab.call("Runtime.evaluate", &params).await
    });
    let reply = answered
        .expect("the request must be answered, not left on a prompt")
        .expect("evaluate");
    assert!(
        reply.contains("denied"),
        "notifications must be denied: {reply}"
    );
    awake.end();
}
