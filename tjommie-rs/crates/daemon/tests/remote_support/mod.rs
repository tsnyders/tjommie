//! Shared by the M7 remote tests: a database holding the remote tables and an in-memory credential vault.
#![allow(dead_code)] // Each test binary uses part of this.

pub mod device;

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

use daemon::remote_identity::Vault;
use rusqlite::Connection;

/// The remote tables, exactly as `db.ts` migrations 8, 10 and 11 create them.
/// ponytail: lane A's `db::open` isn't merged yet; switch this to `db::open(":memory:")` at merge.
pub fn remote_db() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "PRAGMA foreign_keys = ON;
  CREATE TABLE devices (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, public_key TEXT NOT NULL UNIQUE,
    paired_at TEXT NOT NULL, last_seen_at TEXT, revoked_at TEXT
  );
  CREATE TABLE remote_host (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1), id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL, public_key TEXT NOT NULL, created_at TEXT NOT NULL
  );
  CREATE TABLE pairing_codes (
    id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL, expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL, used_at TEXT
  );
  ALTER TABLE devices ADD COLUMN push_token TEXT;
  ALTER TABLE devices ADD COLUMN push_kinds TEXT;",
    )
    .unwrap();
    db
}

pub fn shared_db() -> Arc<Mutex<Connection>> {
    Arc::new(Mutex::new(remote_db()))
}

/// Unpadded base64url of 32 copies of `byte`, like `Buffer.alloc(32, byte).toString("base64url")`.
pub fn key_of(byte: u8) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([byte; 32])
}

/// The synthetic `CredentialTransport` of the TypeScript tests: a map, with optional failures.
#[derive(Default)]
pub struct MemoryVault {
    pub values: Mutex<BTreeMap<String, String>>,
    pub writes: Mutex<u32>,
    pub fail_writes: bool,
    /// Reads return nothing, like `credentials: async () => null`.
    pub forget: bool,
}

impl Vault for MemoryVault {
    fn read(&self, target: &str) -> io::Result<Option<String>> {
        if self.forget {
            return Ok(None);
        }
        Ok(self.values.lock().unwrap().get(target).cloned())
    }

    fn write(&self, target: &str, value: &str) -> io::Result<()> {
        if self.fail_writes {
            return Err(io::Error::other("secret-must-not-escape"));
        }
        *self.writes.lock().unwrap() += 1;
        self.values
            .lock()
            .unwrap()
            .insert(target.to_owned(), value.to_owned());
        Ok(())
    }
}
