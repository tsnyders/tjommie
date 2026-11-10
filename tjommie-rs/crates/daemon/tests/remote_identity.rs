//! Port of `packages/daemon/src/remote-identity.test.ts`.

mod remote_support;

use std::cell::Cell;
use std::path::Path;

use daemon::remote_identity::{IdentityError, load_remote_identity};
use regex::Regex;
use remote_support::{MemoryVault, remote_db};
use transport::Keypair;
use zeroize::Zeroizing;

fn keys(private: u8, public: u8) -> Keypair {
    Keypair {
        private_key: Zeroizing::new([private; 32]),
        public_key: [public; 32],
    }
}

#[test]
fn keeps_the_private_identity_in_the_injected_vault_and_preserves_it_across_loads() {
    let db = remote_db();
    let vault = MemoryVault::default();
    let generated = Cell::new(0);
    let generate = || {
        generated.set(generated.get() + 1);
        Ok(keys(7, 8))
    };
    let root = Path::new("synthetic-root");
    let first = load_remote_identity(&db, root, "Synthetic PC", &vault, generate).unwrap();
    let second = load_remote_identity(&db, root, "Synthetic PC", &vault, generate).unwrap();
    assert_eq!(second, first);
    assert_eq!(generated.get(), 1);
    assert_eq!(first.name, "Synthetic PC");
    assert_eq!(*first.private_key, [7; 32]);
    assert_eq!(first.public_key, [8; 32]);
    let rows: String = db
        .query_row(
            "SELECT id || name || public_key || created_at FROM remote_host",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!rows.contains(&remote_support::key_of(7)));
    let values = vault.values.lock().unwrap();
    let target = values.keys().next().unwrap();
    assert!(
        Regex::new("^Tjommie/[a-f0-9]{32}/remote-host$")
            .unwrap()
            .is_match(target)
    );
    let forgetful = MemoryVault {
        forget: true,
        ..MemoryVault::default()
    };
    let error = load_remote_identity(&db, root, "Synthetic PC", &forgetful, generate).unwrap_err();
    assert!(error.to_string().contains("replacement identity"));
}

/// The vault entry is what 0.4.2 reads after a rollback, so it is `JSON.stringify` of the same object.
#[test]
fn stores_the_same_json_as_the_typescript_daemon() {
    let db = remote_db();
    let vault = MemoryVault::default();
    let identity = load_remote_identity(&db, Path::new("synthetic-root"), "PC", &vault, || {
        Ok(keys(7, 8))
    })
    .unwrap();
    let values = vault.values.lock().unwrap();
    assert_eq!(
        values.values().next().unwrap(),
        &format!(
            r#"{{"v":1,"id":"{}","privateKey":"{}","publicKey":"{}"}}"#,
            identity.id,
            remote_support::key_of(7),
            remote_support::key_of(8)
        )
    );
}

#[test]
fn does_not_write_database_identity_when_the_vault_write_fails() {
    let db = remote_db();
    let vault = MemoryVault {
        fail_writes: true,
        ..MemoryVault::default()
    };
    let error = load_remote_identity(&db, Path::new("synthetic-root"), "PC", &vault, || {
        Ok(keys(9, 8))
    })
    .unwrap_err();
    assert_eq!(error, IdentityError);
    assert!(error.to_string().contains("Windows Credential Manager"));
    assert!(!format!("{error:?}").contains("secret-must-not-escape"));
    let rows: i64 = db
        .query_row("SELECT count(*) FROM remote_host", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn refuses_to_mint_a_new_identity_while_devices_or_a_host_row_exist() {
    let db = remote_db();
    db.execute(
        "INSERT INTO devices(id,name,public_key,paired_at) VALUES('13653af3-8ca6-4e95-bc95-0949edc28715','Phone',?,'2026-10-22T12:00:00.000Z')",
        [remote_support::key_of(1)],
    )
    .unwrap();
    let vault = MemoryVault::default();
    assert!(
        load_remote_identity(&db, Path::new("synthetic-root"), "PC", &vault, || Ok(keys(
            7, 8
        )))
        .is_err()
    );
    assert!(vault.values.lock().unwrap().is_empty());
}

#[test]
fn refuses_a_vault_identity_that_disagrees_with_the_database() {
    let db = remote_db();
    let vault = MemoryVault::default();
    let root = Path::new("synthetic-root");
    load_remote_identity(&db, root, "PC", &vault, || Ok(keys(7, 8))).unwrap();
    db.execute(
        "UPDATE remote_host SET public_key = ?",
        [remote_support::key_of(9)],
    )
    .unwrap();
    assert!(load_remote_identity(&db, root, "PC", &vault, || Ok(keys(7, 8))).is_err());
}
