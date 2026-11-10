//! `diagnostics.test.ts`. Rust has no package.json: the daemon's version is the crate's, and `read_app_version` reads
//! only the desktop's version from the packaged `build-info.json`.

mod common;

use std::fs;

use common::{TempRoot, test_db};
use daemon::diagnostics::{host_diagnostics, read_app_version};
use protocol::diagnostics::StorageState;
use protocol::model::Provider;
use protocol::remote::RelayState;
use serde_json::json;

#[test]
fn uses_cached_provider_versions_and_keeps_desktop_and_daemon_versions_separate() {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    db.execute_batch("CREATE TABLE fixture (body TEXT); INSERT INTO fixture VALUES ('synthetic private message');").unwrap();
    let setup = [
        (Provider::Claude, Some("2.1.3".to_owned())),
        (Provider::Codex, Some("0.155.1".to_owned())),
    ];
    let result = host_diagnostics(Some(&db), &setup, RelayState::Online, None, Some("0.3.2"));
    let json = serde_json::to_value(&result).unwrap();
    protocol::assert_valid("diagnostics.get", &json);
    assert_eq!(
        json,
        json!({ "appVersion": null, "daemonVersion": "0.3.2", "providers": [{ "provider": "claude", "version": "2.1.3" }, { "provider": "codex", "version": "0.155.1" }],
            "relay": "online", "storage": "ok" })
    );
    let body: String = db
        .query_row("SELECT body FROM fixture", [], |row| row.get(0))
        .unwrap();
    assert_eq!(body, "synthetic private message");
}

#[test]
fn never_exposes_storage_errors_or_untrusted_version_strings() {
    let root = TempRoot::new();
    // Not a database: every query fails, like a corrupt file.
    fs::write(root.0.join("broken.db"), "SYNTHETIC_SECRET C:\\private").unwrap();
    let broken = rusqlite::Connection::open(root.0.join("broken.db")).unwrap();
    let setup = [(Provider::Claude, Some("2.1.3-SYNTHETIC_SECRET".to_owned()))];
    let result = host_diagnostics(
        Some(&broken),
        &setup,
        RelayState::Error,
        Some("C:\\private"),
        Some("https://user:secret@example.test"),
    );
    assert_eq!(result.storage, StorageState::Error);
    assert_eq!(result.providers[0].version, None);
    assert_eq!(
        (
            result.app_version.as_deref(),
            result.daemon_version.as_deref()
        ),
        (None, None)
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("SYNTHETIC_SECRET")
    );
    assert_eq!(
        host_diagnostics(None, &[], RelayState::Off, None, None).storage,
        StorageState::Unavailable
    );
}

#[test]
fn reads_packaged_version_metadata_and_unknowns_honestly() {
    let root = TempRoot::new();
    assert_eq!(read_app_version(&root.0), None);
    let build_info = root.0.join("build-info.json");
    fs::write(
        &build_info,
        json!({ "appVersion": "0.4.0", "daemonVersion": "0.3.2" }).to_string(),
    )
    .unwrap();
    assert_eq!(read_app_version(&root.0).as_deref(), Some("0.4.0"));
    fs::write(
        &build_info,
        r#"{"appVersion":"SYNTHETIC_SECRET","daemonVersion":"bad"}"#,
    )
    .unwrap();
    assert_eq!(read_app_version(&root.0), None);
    fs::write(&build_info, "x".repeat(4097)).unwrap();
    assert_eq!(read_app_version(&root.0), None);
}
