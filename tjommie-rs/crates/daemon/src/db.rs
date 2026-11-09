//! The SQLite database: a port of `packages/daemon/src/db.ts`.

use std::path::Path;

use rusqlite::Connection;

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

/// `openDb`: the same PRAGMAs, then each missing migration in its own transaction with its `user_version`.
pub fn open(file: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(file)?;
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
    )?;
    Ok(db)
}
