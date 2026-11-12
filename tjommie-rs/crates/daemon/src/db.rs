//! The SQLite database: a port of `packages/daemon/src/db.ts`.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use rusqlite::backup::NO_PROGRESS;
use rusqlite::{Connection, MAIN_DB};

/// `migrations` from `db.ts`, exported byte for byte by `scripts/export-migrations.ts`. Append-only, and no
/// migration 13 until one release after R1, so 0.4.2 can still open the database (spec §5).
pub const MIGRATIONS: [&str; 12] = [
    include_str!("../migrations/01.sql"),
    include_str!("../migrations/02.sql"),
    include_str!("../migrations/03.sql"),
    include_str!("../migrations/04.sql"),
    include_str!("../migrations/05.sql"),
    include_str!("../migrations/06.sql"),
    include_str!("../migrations/07.sql"),
    include_str!("../migrations/08.sql"),
    include_str!("../migrations/09.sql"),
    include_str!("../migrations/10.sql"),
    include_str!("../migrations/11.sql"),
    include_str!("../migrations/12.sql"),
];

/// `openDb`. An existing database is backed up first, once per daemon version. An error names the file.
pub fn open(file: &Path) -> io::Result<Connection> {
    open_named(file).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Could not open the database {}: {error}", file.display()),
        )
    })
}

fn open_named(file: &Path) -> io::Result<Connection> {
    let existed = file.exists();
    let db = Connection::open(file).map_err(io::Error::other)?;
    // The copy runs before the PRAGMAs, so wait out a brief lock here too. rusqlite already defaults to 5 s; this
    // keeps the wait if that default changes. It sets a connection handler and doesn't touch the file.
    db.busy_timeout(Duration::from_secs(5))
        .map_err(io::Error::other)?;
    if existed {
        back_up_once(&db, file)?;
    }
    migrate(&db).map_err(io::Error::other)?;
    Ok(db)
}

/// Before a daemon version first opens the database, the SQLite backup API copies it to `backup\tjommie-<version>.db`
/// beside it, so rolling back to the previous release keeps its data (spec §5, release R1).
fn back_up_once(db: &Connection, file: &Path) -> io::Result<()> {
    let version = env!("TJOMMIE_VERSION");
    let dir = file.with_file_name("backup");
    let backup = dir.join(format!("tjommie-{version}.db"));
    if backup.exists() {
        return Ok(());
    }
    fs::create_dir_all(&dir)?;
    // Copy under a temporary name, so a copy cut short never counts as this version's backup.
    let partial = dir.join(format!("tjommie-{version}.db.partial"));
    let _ = fs::remove_file(&partial); // A leftover from a copy cut short. If it can't go, the backup fails below.
    db.backup(MAIN_DB, &partial, NO_PROGRESS)
        .map_err(io::Error::other)?;
    fs::rename(&partial, &backup)
}

/// The same PRAGMAs as `openDb`, then each missing migration in its own transaction with its `user_version`.
fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
    )?;
    let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = db.unchecked_transaction()?;
        tx.execute_batch(migration)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1))?;
        tx.commit()?;
    }
    // This lookup index also belongs on existing version 3 databases without changing stored data.
    db.execute_batch(
        "CREATE INDEX IF NOT EXISTS approvals_pending_routine_by_bot ON approvals (bot_id) WHERE kind = 'routine' AND status = 'pending';",
    )
}
