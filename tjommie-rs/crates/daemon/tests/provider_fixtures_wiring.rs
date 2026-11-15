//! Startup wires project reviews to be refused whenever fixtures are on, and checks that it did, on the very value it
//! installs: a daemon whose review hook was dropped on the way (or that has one without fixtures) refuses to start
//! instead of running with a live review path. These tests prove the check itself, both ways it can fail, and that a
//! fixtures daemon starts through it.
#![cfg(feature = "provider-fixtures")]

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use common::{TempRoot, connect};
use daemon::paths::data_paths;
use daemon::provider_fixtures::Fixtures;
use daemon::startup::{Auth, Launch, Mode, check_review_wiring, start_in_with_fixtures};
use serde_json::json;

#[test]
fn the_check_refuses_a_missing_hook_and_a_stray_one_and_passes_the_two_matches() {
    let missing = check_review_wiring(true, false).expect_err("fixtures on without the hook");
    assert_eq!(
        missing.to_string(),
        "Provider fixtures are on, but project reviews are not set to be refused, so a review could start the real Claude. The daemon did not start."
    );
    let stray = check_review_wiring(false, true).expect_err("the hook without fixtures");
    assert_eq!(
        stray.to_string(),
        "Project reviews are set up for provider fixtures, but fixtures are not on. The daemon did not start."
    );
    assert!(check_review_wiring(true, true).is_ok());
    assert!(check_review_wiring(false, false).is_ok());
}

#[tokio::test]
async fn a_fixtures_daemon_starts_through_the_review_wiring_check() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider");
    let fixtures = Fixtures::open(&dir).unwrap();
    let root = TempRoot::new();
    let launch = Launch {
        mode: Mode::Development,
        port: 0,
        lan_port: 0,
        lan_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        skip_account_check: false,
        skip_folder_protection: false,
        accounts_dir: None,
    };
    let daemon = start_in_with_fixtures(
        &launch,
        &Auth::Development,
        root.0.clone(),
        Some(fixtures.clone()),
    )
    .await
    .expect("a fixtures daemon passes the review wiring check and starts");

    // It answers, so startup went all the way through the check and installed the project services.
    let token = std::fs::read_to_string(data_paths(&root.0).unwrap().token).unwrap();
    let mut client = connect(daemon.port()).await;
    client.hello(token.trim()).await;
    let projects = client.call(1, "projects.list", json!({})).await;
    assert_eq!(projects["ok"], true, "{projects}");
    client.close().await;
    daemon.close().await;
    assert_eq!(fixtures.host().launch_attempts(), 0);
}
