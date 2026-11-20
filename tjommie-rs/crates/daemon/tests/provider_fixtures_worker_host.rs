//! A fixtures daemon that has the desktop's host token serves `/worker-host` over the lease-only `FixtureBoundary`: its
//! `status` says isolation is available, so the desktop's Logins vault reads `locked` rather than `unavailable`, and a
//! lease can be taken, renewed and released without any process. Feature-gated like the rest of the fixtures tests.
#![cfg(feature = "provider-fixtures")]

mod common;
mod host_support;

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use common::TempRoot;
use daemon::provider_fixtures::Fixtures;
use daemon::startup::{Auth, Launch, Mode, start_in_with_fixtures};
use host_support::post;
use serde_json::json;

#[tokio::test]
async fn worker_host_reports_isolation_available_and_takes_leases_without_a_process() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider");
    let fixtures = Fixtures::open(&dir).unwrap();
    let root = TempRoot::new();
    let launch = Launch {
        mode: Mode::Production,
        port: 0,
        lan_port: 0,
        lan_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        skip_account_check: false,
        skip_folder_protection: false,
        accounts_dir: None,
    };
    let (token, host) = ("t".repeat(43), "h".repeat(43));
    let auth = Auth::Production {
        token,
        desktop_host_token: Some(host.clone()),
    };
    let daemon = start_in_with_fixtures(&launch, &auth, root.0.clone(), Some(fixtures.clone()))
        .await
        .expect("a fixtures daemon with the desktop's host token starts");
    let port = daemon.port();
    let host_auth = format!("Bearer {host}");
    let headers = [("Authorization", host_auth.as_str())];

    let status = post(port, "/worker-host", &headers, br#"{"kind":"status"}"#).await;
    assert_eq!(status.status, 200);
    let status = status.json();
    assert_eq!(status["available"], true, "{status}");
    assert!(status["reason"].is_null(), "{status}");

    let acquired = post(port, "/worker-host", &headers, br#"{"kind":"acquire"}"#).await;
    assert_eq!(acquired.status, 200);
    let lease = acquired.json()["id"]
        .as_str()
        .expect("a lease id")
        .to_owned();
    let renew = json!({ "kind": "renew", "id": lease }).to_string();
    let renewed = post(port, "/worker-host", &headers, renew.as_bytes()).await;
    assert_eq!(renewed.status, 200);
    let release = json!({ "kind": "release", "id": lease }).to_string();
    let released = post(port, "/worker-host", &headers, release.as_bytes()).await;
    assert_eq!(released.status, 200);

    assert_eq!(
        fixtures.host().launch_attempts(),
        0,
        "nothing asked to start a process"
    );
    daemon.close().await;
}
