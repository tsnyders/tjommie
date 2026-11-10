//! Shared by the M7 remote tests: a database holding the remote tables and an in-memory credential vault.
#![allow(dead_code)] // Each test binary uses part of this.

pub mod device;
pub mod harness;
pub mod probe;

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

use daemon::remote_identity::Vault;
use rusqlite::Connection;

/// A fresh database with every migration, like `openDb(":memory:")`.
pub fn remote_db() -> Connection {
    daemon::db::open(std::path::Path::new(":memory:")).unwrap()
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
