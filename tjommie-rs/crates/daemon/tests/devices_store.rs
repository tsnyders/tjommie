//! Port of `packages/daemon/src/devices-store.test.ts`. The tail of the last case, which sends through `startPush`, is in
//! `push.rs`.

mod remote_support;

use daemon::devices_store::{
    PushTarget, add_device, clear_push_token, get_device, list_devices, push_targets,
    rename_device, revoke_device, set_push_token, touch_device,
};
use protocol::ErrorCode;
use protocol::remote::PushKind;
use remote_support::{key_of, remote_db};
use rusqlite::Connection;

fn key() -> String {
    format!("AQ{}", "A".repeat(41))
}

fn tokens(db: &Connection, id: &str) -> (Option<String>, Option<String>) {
    db.query_row(
        "SELECT push_token, push_kinds FROM devices WHERE id = ?",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

fn target(device_id: &str, token: &str, kinds: Option<Vec<PushKind>>) -> PushTarget {
    PushTarget {
        device_id: device_id.into(),
        token: token.into(),
        kinds,
    }
}

#[test]
fn persists_paired_device_identity_rename_and_last_seen_while_revocation_is_irreversible() {
    let db = remote_db();
    let device = add_device(&db, &key(), "My phone").unwrap();
    assert_eq!(get_device(&db, &device.id).unwrap(), device);
    assert_eq!(list_devices(&db).unwrap().len(), 1);
    rename_device(&db, &device.id, "Phone renamed").unwrap();
    touch_device(&db, &device.id, Some("2026-10-22T12:00:00.000Z")).unwrap();
    let renamed = get_device(&db, &device.id).unwrap();
    assert_eq!(renamed.name.as_str(), "Phone renamed");
    assert_eq!(
        renamed.last_seen_at.as_deref(),
        Some("2026-10-22T12:00:00.000Z")
    );
    assert_eq!(renamed.revoked_at, None);
    revoke_device(&db, &device.id).unwrap();
    assert!(get_device(&db, &device.id).unwrap().revoked_at.is_some());
    assert_eq!(
        add_device(&db, &key(), "Same key").unwrap_err().code,
        ErrorCode::BadState
    );
    assert_eq!(
        touch_device(&db, &device.id, None).unwrap_err().code,
        ErrorCode::BadState
    );
}

#[test]
fn rejects_invalid_names_and_keys_before_writing() {
    let db = remote_db();
    assert!(add_device(&db, "bad", "Phone").is_err());
    assert!(add_device(&db, &key(), "\u{0}").is_err());
    assert!(add_device(&db, &key(), "   ").is_err());
    assert!(add_device(&db, &key(), &"n".repeat(81)).is_err());
    assert_eq!(list_devices(&db).unwrap(), []);
}

#[test]
fn trims_names_like_zod() {
    let db = remote_db();
    let device = add_device(&db, &key(), "  Phone \u{feff}").unwrap();
    assert_eq!(device.name.as_str(), "Phone");
    let renamed = rename_device(&db, &device.id, &format!(" {} ", "n".repeat(80))).unwrap();
    assert_eq!(renamed.name.as_str(), "n".repeat(80));
}

#[test]
fn unknown_ids_are_not_found() {
    let db = remote_db();
    let missing = "13653af3-8ca6-4e95-bc95-0949edc28715";
    for error in [
        get_device(&db, missing).unwrap_err(),
        rename_device(&db, missing, "x").unwrap_err(),
        revoke_device(&db, missing).unwrap_err(),
    ] {
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.message, "This device was not found.");
    }
}

#[test]
fn stores_push_tokens_separately_from_device_details_and_clears_them_on_revoke() {
    let db = remote_db();
    let device = add_device(&db, &key(), "Phone").unwrap();
    set_push_token(&db, &device.id, Some("synthetic-token"), None).unwrap();
    assert_eq!(get_device(&db, &device.id).unwrap(), device);
    assert_eq!(
        push_targets(&db).unwrap(),
        [target(&device.id, "synthetic-token", None)]
    );
    clear_push_token(&db, &device.id, "synthetic-token").unwrap();
    assert_eq!(push_targets(&db).unwrap(), []);
    set_push_token(&db, &device.id, Some("synthetic-token"), None).unwrap();
    revoke_device(&db, &device.id).unwrap();
    assert_eq!(push_targets(&db).unwrap(), []);
    assert_eq!(tokens(&db, &device.id), (None, None));
    db.execute(
        "UPDATE devices SET push_token = ? WHERE id = ?",
        ["stale-token", &device.id],
    )
    .unwrap();
    assert_eq!(push_targets(&db).unwrap(), []);
    assert!(set_push_token(&db, &device.id, Some("new-token"), None).is_err());
}

#[test]
fn keeps_a_newly_registered_token_when_an_older_push_is_reported_unregistered() {
    let db = remote_db();
    let device = add_device(&db, &key(), "Phone").unwrap();
    set_push_token(&db, &device.id, Some("old-token"), None).unwrap();
    set_push_token(&db, &device.id, Some("new-token"), None).unwrap();
    clear_push_token(&db, &device.id, "old-token").unwrap();
    assert_eq!(
        push_targets(&db).unwrap(),
        [target(&device.id, "new-token", None)]
    );
    clear_push_token(&db, &device.id, "new-token").unwrap();
    assert_eq!(push_targets(&db).unwrap(), []);
}

#[test]
fn moves_a_token_to_the_latest_active_device_registration() {
    let db = remote_db();
    let first = add_device(&db, &key(), "Old pairing").unwrap();
    let second = add_device(&db, &key_of(2), "New pairing").unwrap();
    set_push_token(&db, &first.id, Some("shared-install-token"), None).unwrap();
    set_push_token(&db, &second.id, Some("shared-install-token"), None).unwrap();
    assert_eq!(tokens(&db, &first.id).0, None);
    assert_eq!(
        push_targets(&db).unwrap(),
        [target(&second.id, "shared-install-token", None)]
    );
}

#[test]
fn stores_explicit_push_kinds_per_device_and_clears_them_on_revoke() {
    let db = remote_db();
    let device = add_device(&db, &key(), "Phone").unwrap();
    set_push_token(
        &db,
        &device.id,
        Some("synthetic-token"),
        Some(&[PushKind::Done]),
    )
    .unwrap();
    assert_eq!(
        push_targets(&db).unwrap(),
        [target(
            &device.id,
            "synthetic-token",
            Some(vec![PushKind::Done])
        )]
    );
    assert_eq!(tokens(&db, &device.id).1.as_deref(), Some(r#"["done"]"#));
    set_push_token(&db, &device.id, Some("synthetic-token"), Some(&[])).unwrap();
    assert_eq!(push_targets(&db).unwrap()[0].kinds, Some(vec![]));
    revoke_device(&db, &device.id).unwrap();
    assert_eq!(tokens(&db, &device.id), (None, None));
}

#[test]
fn keeps_every_push_target_available_when_stored_kinds_are_malformed_or_newer() {
    let db = remote_db();
    let malformed = add_device(&db, &key(), "Malformed").unwrap();
    let unknown = add_device(&db, &key_of(2), "Unknown").unwrap();
    let mixed = add_device(&db, &key_of(3), "Mixed").unwrap();
    let valid = add_device(&db, &key_of(4), "Valid").unwrap();
    for device in [&malformed, &unknown, &mixed, &valid] {
        set_push_token(
            &db,
            &device.id,
            Some(&format!("token-{}", device.name.as_str())),
            None,
        )
        .unwrap();
    }
    for (kinds, id) in [
        ("not json", &malformed.id),
        (r#"["mention"]"#, &unknown.id),
        (r#"["done","mention"]"#, &mixed.id),
    ] {
        db.execute(
            "UPDATE devices SET push_kinds = ? WHERE id = ?",
            [kinds, id],
        )
        .unwrap();
    }
    let targets = push_targets(&db).unwrap();
    assert_eq!(targets.len(), 4);
    assert!(targets.contains(&target(&malformed.id, "token-Malformed", None)));
    assert!(targets.contains(&target(&unknown.id, "token-Unknown", Some(vec![]))));
    assert!(targets.contains(&target(
        &mixed.id,
        "token-Mixed",
        Some(vec![PushKind::Done])
    )));
}

#[test]
fn skips_rows_whose_stored_token_breaks_the_target_schema() {
    let db = remote_db();
    let device = add_device(&db, &key(), "Phone").unwrap();
    db.execute(
        "UPDATE devices SET push_token = ? WHERE id = ?",
        [&"t".repeat(4097), &device.id],
    )
    .unwrap();
    assert_eq!(push_targets(&db).unwrap(), []);
    db.execute(
        "UPDATE devices SET push_token = '' WHERE id = ?",
        [&device.id],
    )
    .unwrap();
    assert_eq!(push_targets(&db).unwrap(), []);
}

#[test]
fn lists_devices_in_pairing_order() {
    let db = remote_db();
    let first = add_device(&db, &key(), "First").unwrap();
    let second = add_device(&db, &key_of(2), "Second").unwrap();
    let listed: Vec<String> = list_devices(&db)
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();
    let mut expected = vec![first.id, second.id];
    if first.paired_at == second.paired_at {
        expected.sort();
    }
    assert_eq!(listed, expected);
}

#[test]
fn refuses_a_65th_device() {
    let db = remote_db();
    for byte in 0..64u8 {
        add_device(&db, &key_of(byte), "Phone").unwrap();
    }
    let full = add_device(&db, &key_of(64), "Phone").unwrap_err();
    assert_eq!(full.code, ErrorCode::Busy);
    assert_eq!(full.message, "The device list is full.");
}

#[test]
fn stores_times_like_date_to_iso_string() {
    let db = remote_db();
    let device = add_device(&db, &key(), "Phone").unwrap();
    touch_device(&db, &device.id, None).unwrap();
    let pattern = regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$").unwrap();
    let seen = get_device(&db, &device.id).unwrap();
    assert!(pattern.is_match(&seen.paired_at), "{}", seen.paired_at);
    assert!(pattern.is_match(seen.last_seen_at.as_deref().unwrap()));
}
