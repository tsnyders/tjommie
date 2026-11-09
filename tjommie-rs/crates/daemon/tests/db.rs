//! `db.test.ts`, the `openDb` cases. Its `dataPaths` cases are ported with `paths.rs` (T1.1).

use std::path::Path;

use daemon::db::{self, MIGRATIONS};
use rusqlite::Connection;
use tempfile::TempDir;

/// A folder whose path has a space and an accented letter, like many Windows profile paths.
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

/// The first `version` migrations in one transaction, like `legacyDb`. The test sets `user_version` itself.
fn legacy_db(file: &Path, version: usize) -> Connection {
    let db = Connection::open(file).unwrap();
    let tx = db.unchecked_transaction().unwrap();
    for migration in &MIGRATIONS[..version] {
        tx.execute_batch(migration).unwrap();
    }
    tx.commit().unwrap();
    db
}

fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn upgrades_a_version_6_database_with_everyone_and_empty_room_memberships() {
    let dir = temp();
    let file = dir.path().join("v6.db");
    legacy_db(&file, 6)
        .execute_batch("PRAGMA user_version = 6")
        .unwrap();
    let db = db::open(&file).unwrap();
    assert_eq!(user_version(&db), 12);
    let team: (Option<String>, Option<String>) = db
        .query_row(
            "SELECT name, created_at FROM conversations WHERE id = 'team'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(team, (Some("Everyone".into()), None));
    assert_eq!(count(&db, "SELECT COUNT(*) FROM room_members"), 0);
}

#[test]
fn adds_a_nullable_push_token_to_the_previous_device_schema_without_changing_devices() {
    let dir = temp();
    let file = dir.path().join("v7.db");
    legacy_db(&file, 7)
        .execute_batch(
            "INSERT INTO devices(id,name,public_key,paired_at) VALUES('phone','Phone','key','2026-10-25T00:00:00.000Z'); PRAGMA user_version = 7;",
        )
        .unwrap();
    let db = db::open(&file).unwrap();
    assert_eq!(user_version(&db), 12);
    let device: (String, Option<String>) = db
        .query_row(
            "SELECT id, push_token FROM devices WHERE id = 'phone'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(device, ("phone".into(), None));
}

#[test]
fn upgrades_a_version_8_device_schema_with_nullable_push_kinds() {
    let dir = temp();
    let file = dir.path().join("v8.db");
    legacy_db(&file, 8)
        .execute_batch(
            "INSERT INTO devices(id,name,public_key,paired_at,push_token) VALUES('phone','Phone','key','now','existing-token'); PRAGMA user_version = 8;",
        )
        .unwrap();
    let db = db::open(&file).unwrap();
    assert_eq!(user_version(&db), 12);
    let device: (Option<String>, Option<String>) = db
        .query_row(
            "SELECT push_token, push_kinds FROM devices WHERE id = 'phone'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(device, (Some("existing-token".into()), None));
}

#[test]
fn migrates_version_4_accounts_pins_sessions_and_historical_usage_without_moving_identities() {
    let dir = temp();
    let file = dir.path().join("v4.db");
    legacy_db(&file, 4)
        .execute_batch(
            "INSERT INTO accounts VALUES ('claude-main', 'claude', 'Original', 'subscription', 'unchanged/main', '2026-01-01');
      INSERT INTO accounts VALUES ('claude-second', 'claude', 'Second', 'subscription', 'unchanged/second', '2026-02-01');
      INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
        VALUES ('saved', 'Saved', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'x', 'then'),
        ('unsaved', 'Unsaved', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'y', 'then');
      INSERT INTO sessions VALUES ('saved', 'claude', 'claude-second', 'retained-session', 'then', 1.25);
      INSERT INTO usage VALUES ('known', 'saved', 'claude-main', 'claude', 'haiku', 10, 2, 3, 0, 0.5, 'reported', '2026-10-22T12:00:00Z');
      INSERT INTO usage VALUES ('unknown', 'deleted-bot', 'missing-account', 'codex', 'm', 1, 0, 2, 1, NULL, NULL, '2026-10-22T13:00:00Z');
      PRAGMA user_version = 4;",
        )
        .unwrap();
    {
        let db = db::open(&file).unwrap();
        let accounts: Vec<(String, String, i64)> = db
            .prepare("SELECT id, folder, is_default FROM accounts ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            accounts,
            [
                ("claude-main".into(), "unchanged/main".into(), 1),
                ("claude-second".into(), "unchanged/second".into(), 0)
            ]
        );
        let bots: Vec<(String, String)> = db
            .prepare("SELECT id, account_id FROM bots ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            bots,
            [
                ("saved".into(), "claude-second".into()),
                ("unsaved".into(), "claude-main".into())
            ]
        );
        type Session = (String, String, String, String, String, Option<f64>, String);
        let session: Session = db
            .query_row("SELECT * FROM sessions", [], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .unwrap();
        assert_eq!(
            session,
            (
                "saved".into(),
                "claude".into(),
                "claude-second".into(),
                "retained-session".into(),
                "then".into(),
                Some(1.25),
                String::new()
            )
        );
        db.execute(
            "INSERT INTO sessions(bot_id,provider,account_id,cli_session_id,updated_at,total_cost_usd) VALUES ('saved', 'claude', 'claude-main', 'other-session', 'then', NULL)",
            [],
        )
        .unwrap();
        assert_eq!(count(&db, "SELECT COUNT(*) FROM sessions"), 2);
        type Usage = (String, String, Option<f64>, String, Option<String>);
        let usage: Vec<Usage> = db
            .prepare("SELECT id, account_kind, cost_usd, cost_source, pricing_revision FROM usage ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            usage,
            [
                (
                    "known".into(),
                    "subscription".into(),
                    Some(0.5),
                    "reported".into(),
                    None
                ),
                (
                    "unknown".into(),
                    "unknown".into(),
                    None,
                    "unknown".into(),
                    None
                )
            ]
        );
        db.execute("DELETE FROM bots WHERE id = 'saved'", [])
            .unwrap();
        assert_eq!(count(&db, "SELECT COUNT(*) FROM sessions"), 0);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM usage"), 2);
    }
    let again = db::open(&file).unwrap();
    assert_eq!(count(&again, "SELECT COUNT(*) FROM accounts"), 2);
}

#[test]
fn upgrades_version_3_without_replaying_historical_failures() {
    let dir = temp();
    let file = dir.path().join("v3.db");
    legacy_db(&file, 3)
        .execute_batch(
            r#"INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
      VALUES ('bot', 'Writer', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'x', 'then');
      INSERT INTO routines (id, bot_id, name, prompt, days, times, time_zone, created_at)
      VALUES ('routine', 'bot', 'Check', 'Check', '[1]', '["08:00"]', 'UTC', 'then');
      INSERT INTO routine_runs (id, routine_id, due_at, ended_at, status, note)
      VALUES ('run', 'routine', 'then', 'then', 'failed', 'Historical failure');
      PRAGMA user_version = 3;"#,
        )
        .unwrap();
    let db = db::open(&file).unwrap();
    assert_eq!(user_version(&db), 12);
    let run: (String, String) = db
        .query_row(
            "SELECT status, note FROM routine_runs WHERE id = 'run'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(run, ("failed".into(), "Historical failure".into()));
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM routine_failure_notifications"),
        0
    );
    let error = db
        .execute(
            "INSERT INTO routine_failure_notifications VALUES ('missing', 'Failure', 'Restarted')",
            [],
        )
        .unwrap_err();
    assert!(error.to_string().contains("FOREIGN KEY"), "{error}");
}

#[test]
fn creates_the_schema_once_and_keeps_data_when_reopened() {
    let dir = temp();
    let file = dir.path().join("t.db");
    db::open(&file)
        .unwrap()
        .execute(
            "INSERT INTO accounts (id, provider, label, kind, folder, created_at) VALUES ('a', 'claude', 'Claude', 'subscription', 'x', 'now')",
            [],
        )
        .unwrap();
    let again = db::open(&file).unwrap();
    let label: String = again
        .query_row("SELECT label FROM accounts WHERE id = 'a'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(label, "Claude");
    assert_eq!(user_version(&again), 12);
}

#[test]
fn has_the_team_room_conversation_and_the_routine_tables_after_migration_2() {
    let dir = temp();
    let db = db::open(&dir.path().join("t.db")).unwrap();
    let kind: String = db
        .query_row(
            "SELECT kind FROM conversations WHERE id = 'team'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "room");
    for table in [
        "settings",
        "chains",
        "exchanges",
        "routines",
        "routine_runs",
    ] {
        let name: String = db
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, table);
    }
}

#[test]
fn enforces_foreign_keys() {
    let dir = temp();
    let db = db::open(&dir.path().join("t.db")).unwrap();
    let error = db
        .execute(
            "INSERT INTO conversations (id, kind, bot_id) VALUES ('c', 'bot', 'missing')",
            [],
        )
        .unwrap_err();
    assert!(error.to_string().contains("FOREIGN KEY"), "{error}");
}

#[test]
fn upgrades_saved_sessions_with_an_unknown_cost_baseline() {
    let dir = temp();
    let file = dir.path().join("t.db");
    legacy_db(&file, 2)
        .execute_batch(
            "PRAGMA foreign_keys = ON;
      INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
        VALUES ('bot', 'Writer', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'x', 'then');
      INSERT INTO sessions VALUES ('bot', 'claude', 'claude-main', 'saved', 'then');
      PRAGMA user_version = 2;",
        )
        .unwrap();
    let db = db::open(&file).unwrap();
    let session: (String, Option<f64>) = db
        .query_row(
            "SELECT cli_session_id, total_cost_usd FROM sessions WHERE bot_id = 'bot'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(session, ("saved".into(), None));
    assert_eq!(user_version(&db), 12);
}
