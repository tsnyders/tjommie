//! `settings.test.ts`

mod common;

use common::{TempRoot, test_db};
use daemon::settings::{get_settings, set_settings};
use protocol::ErrorCode;
use protocol::model::{Settings, SettingsPatch};
use protocol::relay::DEFAULT_RELAY_URL;
use protocol::remote::RelayUrl;

fn relay(url: &str) -> Option<RelayUrl> {
    Some(RelayUrl::new(url).unwrap())
}

fn settings(chain_limit: u32, relay_url: Option<RelayUrl>) -> Settings {
    Settings {
        chain_limit,
        relay_url,
    }
}

fn default_relay() -> Option<RelayUrl> {
    DEFAULT_RELAY_URL.and_then(relay)
}

fn chain_limit(value: u32) -> SettingsPatch {
    SettingsPatch {
        chain_limit: Some(value),
        relay_url: None,
    }
}

fn relay_patch(url: Option<RelayUrl>) -> SettingsPatch {
    SettingsPatch {
        chain_limit: None,
        relay_url: Some(url),
    }
}

fn rows(connection: &rusqlite::Connection) -> Vec<(String, String)> {
    let mut statement = connection
        .prepare("SELECT key, value FROM settings ORDER BY key")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn defaults_the_chain_limit_to_5_and_remembers_a_change() {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    assert_eq!(get_settings(&db).unwrap(), settings(5, default_relay()));
    assert_eq!(
        set_settings(&db, &chain_limit(2)).unwrap(),
        settings(2, default_relay())
    );
    assert_eq!(get_settings(&db).unwrap(), settings(2, default_relay()));
}

#[test]
fn rejects_an_invalid_change_without_changing_the_stored_settings() {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    set_settings(&db, &chain_limit(2)).unwrap();
    let error = set_settings(&db, &chain_limit(0)).unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (ErrorCode::BadParams, "Invalid settings")
    );
    assert_eq!(rows(&db), [("chainLimit".to_owned(), "2".to_owned())]);
    assert_eq!(get_settings(&db).unwrap(), settings(2, default_relay()));
}

#[test]
fn uses_the_default_when_a_stored_setting_is_corrupt() {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    db.execute(
        "INSERT INTO settings (key, value) VALUES ('chainLimit', '0')",
        [],
    )
    .unwrap();
    assert_eq!(get_settings(&db).unwrap(), settings(5, default_relay()));
    db.execute(
        "UPDATE settings SET value = 'not json' WHERE key = 'chainLimit'",
        [],
    )
    .unwrap();
    assert_eq!(get_settings(&db).unwrap(), settings(5, default_relay()));
}

#[test]
fn stores_only_patched_keys_so_a_later_relay_default_can_take_effect() {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    set_settings(&db, &chain_limit(2)).unwrap();
    assert_eq!(rows(&db), [("chainLimit".to_owned(), "2".to_owned())]);
    set_settings(&db, &relay_patch(relay("https://relay.example/old-base"))).unwrap();
    let keys: Vec<String> = rows(&db).into_iter().map(|(key, _)| key).collect();
    assert_eq!(keys, ["chainLimit", "relayUrl"]);
}

#[test]
fn reads_back_a_stored_opt_out_and_a_custom_relay_over_the_default() {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    assert_eq!(get_settings(&db).unwrap().relay_url, default_relay());
    set_settings(&db, &relay_patch(None)).unwrap();
    assert_eq!(get_settings(&db).unwrap().relay_url, None);
    set_settings(&db, &relay_patch(relay("https://relay.example/old-base"))).unwrap();
    assert_eq!(
        get_settings(&db).unwrap().relay_url,
        relay("https://relay.example/old-base")
    );
}

// Not in settings.test.ts: daemon.ts keeps other values in the settings table too, such as attachmentHostId.
#[test]
fn ignores_settings_rows_that_are_not_settings() {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    db.execute(
        "INSERT INTO settings (key, value) VALUES ('attachmentHostId', '\"host\"')",
        [],
    )
    .unwrap();
    set_settings(&db, &chain_limit(3)).unwrap();
    assert_eq!(get_settings(&db).unwrap(), settings(3, default_relay()));
}
