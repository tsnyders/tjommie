//! `database-upgrade.test.ts` and the fixture database test (spec §5 Phase 1). `fixtures/fixture.db` is a v0.4.2
//! database (seeded as in `database-upgrade.test.ts`, upgraded by the TypeScript `openDb`) and `fixtures/schema.json` is
//! `sqlite_master` of a fresh TypeScript database. Both were generated once from the 0.4.2 daemon (the `legacy` branch)
//! and are now owned here: change them only to follow a deliberate Rust migration. The first case, which ends in the
//! runner's recovery, is in `tests/runner/upgrade.rs`.

use std::fs;

use daemon::db::{self, MIGRATIONS};
use rusqlite::Connection;
use serde::Deserialize;
use tempfile::TempDir;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fixture.db");
const SCHEMA: &str = include_str!("fixtures/schema.json");

#[derive(Debug, PartialEq, Deserialize)]
struct SchemaObject {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    tbl_name: String,
    sql: Option<String>,
}

fn temp() -> TempDir {
    tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap()
}

fn user_version(db: &Connection) -> i64 {
    db.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

/// Same query and order as the one that wrote `fixtures/schema.json`.
fn schema(db: &Connection) -> Vec<SchemaObject> {
    db.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap()
        .query_map([], |row| {
            Ok(SchemaObject {
                kind: row.get(0)?,
                name: row.get(1)?,
                tbl_name: row.get(2)?,
                sql: row.get(3)?,
            })
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn row_counts(db: &Connection) -> Vec<(String, i64)> {
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    tables
        .into_iter()
        .map(|table| {
            let rows = db
                .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                    row.get(0)
                })
                .unwrap();
            (table, rows)
        })
        .collect()
}

/// A copy of the fixture, so a test never writes next to the committed file.
fn fixture_copy(dir: &TempDir) -> std::path::PathBuf {
    let file = dir.path().join("tjommie.db");
    fs::copy(FIXTURE, &file).unwrap();
    file
}

fn typescript_schema() -> Vec<SchemaObject> {
    serde_json::from_str(SCHEMA).unwrap()
}

#[test]
fn opens_a_v0_4_2_database_at_version_12_with_the_same_rows() {
    let dir = temp();
    let file = fixture_copy(&dir);
    let before = row_counts(&Connection::open(&file).unwrap());
    assert!(
        before.contains(&("messages".into(), 4)),
        "the fixture holds the seeded chat: {before:?}"
    );

    let db = db::open(&file).unwrap();
    assert_eq!(user_version(&db), 12);
    assert_eq!(row_counts(&db), before);
    assert_eq!(schema(&db), typescript_schema());
}

#[test]
fn a_fresh_database_has_the_typescript_schema() {
    let dir = temp();
    let db = db::open(&dir.path().join("tjommie.db")).unwrap();
    assert_eq!(user_version(&db), 12);
    assert_eq!(schema(&db), typescript_schema());
}

#[test]
fn creates_the_pending_routine_approval_index_on_an_existing_version_3_database() {
    let dir = temp();
    let file = dir.path().join("existing.db");
    {
        let existing = Connection::open(&file).unwrap();
        for migration in &MIGRATIONS[..3] {
            existing.execute_batch(migration).unwrap();
        }
        existing.execute_batch("PRAGMA user_version = 3;").unwrap();
    }

    let db = db::open(&file).unwrap();
    let plan: Vec<String> = db
        .prepare(
            "EXPLAIN QUERY PLAN SELECT id FROM approvals
      WHERE bot_id = ? AND kind = 'routine' AND status = 'pending' LIMIT 1",
        )
        .unwrap()
        .query_map(["bot"], |row| row.get(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|detail| detail.contains("USING INDEX approvals_pending_routine_by_bot")),
        "{plan:?}"
    );
    assert_eq!(user_version(&db), 12);
}

// T1.3 first-open backup (spec §5, release R1): `data\backup\tjommie-<version>.db`, once per app version.
const BACKUP: &str = concat!("tjommie-", env!("TJOMMIE_VERSION"), ".db");

/// Releases bump the package.json files, not Cargo.toml (AGENTS.md), so the backup takes the root package.json version.
#[test]
fn names_the_backup_after_the_root_package_json_version() {
    #[derive(Deserialize)]
    struct Package {
        version: String,
    }
    let text = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../package.json"
    ))
    .unwrap();
    let package: Package = serde_json::from_str(&text).unwrap();
    assert_eq!(env!("TJOMMIE_VERSION"), package.version);
}

fn backups(dir: &TempDir) -> Vec<String> {
    fs::read_dir(dir.path().join("backup"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

#[test]
fn backs_up_the_database_once_per_version() {
    let dir = temp();
    let file = fixture_copy(&dir);
    let rows = row_counts(&Connection::open(&file).unwrap());

    drop(db::open(&file).unwrap());
    drop(db::open(&file).unwrap());

    assert_eq!(backups(&dir), [BACKUP]);
    let backup = Connection::open(dir.path().join("backup").join(BACKUP)).unwrap();
    assert_eq!(row_counts(&backup), rows);
}

#[test]
fn backs_up_before_the_migrations_run() {
    let dir = temp();
    let file = dir.path().join("tjommie.db");
    {
        let existing = Connection::open(&file).unwrap();
        for migration in &MIGRATIONS[..3] {
            existing.execute_batch(migration).unwrap();
        }
        existing.execute_batch("PRAGMA user_version = 3;").unwrap();
    }

    drop(db::open(&file).unwrap());

    let backup = Connection::open(dir.path().join("backup").join(BACKUP)).unwrap();
    assert_eq!(user_version(&backup), 3);
    // This database was made without WAL, and the copy keeps that: no PRAGMA ran before it.
    let mode: String = backup
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
}

#[test]
fn backs_up_rows_that_are_still_only_in_the_wal() {
    let dir = temp();
    let file = fixture_copy(&dir);
    let writer = Connection::open(&file).unwrap();
    writer
        .execute_batch(
            "PRAGMA wal_autocheckpoint = 0; INSERT INTO settings (key, value) VALUES ('wal-only', 'yes');",
        )
        .unwrap();

    drop(db::open(&file).unwrap());

    // The main file on its own still lacks the row, so the copy had to read it from the WAL.
    let main_only = dir.path().join("main-only.db");
    fs::copy(&file, &main_only).unwrap();
    let in_main: i64 = Connection::open(&main_only)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key = 'wal-only'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(in_main, 0);
    let backup = Connection::open(dir.path().join("backup").join(BACKUP)).unwrap();
    let value: String = backup
        .query_row(
            "SELECT value FROM settings WHERE key = 'wal-only'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, "yes");
    drop(writer);
}

/// Another process can hold the database for a moment (WAL recovery, an exclusive checkpoint); start-up waits for it.
#[test]
fn waits_for_a_brief_lock_before_backing_up() {
    let dir = temp();
    let file = fixture_copy(&dir);
    let holder = Connection::open(&file).unwrap();
    // In exclusive locking mode the lock stays after the write commits, until the connection closes.
    holder
        .execute_batch("PRAGMA locking_mode = EXCLUSIVE; INSERT INTO settings (key, value) VALUES ('held', 'yes');")
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(holder);
    });

    drop(db::open(&file).unwrap());
    release.join().unwrap();
    assert_eq!(backups(&dir), [BACKUP]);
}

#[test]
fn a_new_database_has_nothing_to_back_up() {
    let dir = temp();
    drop(db::open(&dir.path().join("tjommie.db")).unwrap());
    assert!(!dir.path().join("backup").exists());
}
