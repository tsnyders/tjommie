//! The login vault's records and secrets, ported from `vault-storage.ts`. The records (`data/logins.sqlite`, the same
//! schema) hold metadata and opaque references only. A login's username and password live in Credential Manager
//! (`credential.rs`) and pass through [`VaultSecret`], which wipes them on drop and never prints them.

use protocol::vault::LoginEntry;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde::de::IgnoredAny;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// The records' limit on logins, `vault-storage.ts:88`.
const ENTRY_LIMIT: usize = 1000;
/// `Number.MAX_SAFE_INTEGER`, the largest revision zod's `int()` accepts.
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
/// The keys a stored login has. zod's `.strict()` refuses any other.
const STORED_KEYS: [&str; 7] = [
    "id",
    "name",
    "origin",
    "botId",
    "revision",
    "createdAt",
    "updatedAt",
];

/// Every failure the vault reports. The messages are the TypeScript's; none carries a secret or a Windows detail.
#[derive(Debug)]
pub enum VaultError {
    /// `vaultStorageError`: Credential Manager, a stored secret or the records folder is unusable.
    Storage,
    /// `validateVaultSecret`.
    InvalidSecret,
    /// The records already hold 1000 logins.
    EntryLimit,
    /// No login has this id.
    Missing,
    /// The login's revision is not the one the caller saw.
    Changed,
    /// A stored or saved login that the `LoginEntry` schema refuses. zod's own message is not for users.
    InvalidEntry,
    /// SQLite failed. The message stays generic; the cause is kept for logs.
    Records(rusqlite::Error),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Storage => {
                "Logins could not access its protected storage. No secret was returned."
            }
            Self::InvalidSecret => {
                "Enter a username and password without line breaks, within the protected storage limit."
            }
            Self::EntryLimit => "Logins has reached its entry limit.",
            Self::Missing => "This login no longer exists.",
            Self::Changed => "This login changed. Refresh Logins before continuing.",
            Self::InvalidEntry => "This login's details are not valid.",
            Self::Records(_) => "Logins could not read or update its records.",
        })
    }
}

impl std::error::Error for VaultError {}

impl From<rusqlite::Error> for VaultError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Records(error)
    }
}

/// A login's username and password. The fields are wiped when the value drops, and `Debug` shows neither.
pub struct VaultSecret {
    username: String,
    password: String,
}

impl VaultSecret {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl Drop for VaultSecret {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

impl fmt::Debug for VaultSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultSecret { username: <redacted>, password: <redacted> }")
    }
}

/// `JSON.stringify({ username, password })`: the blob Credential Manager holds. The returned copy is wiped on drop.
pub(crate) fn secret_json(secret: &VaultSecret) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    #[derive(Serialize)]
    struct Wire<'a> {
        username: &'a str,
        password: &'a str,
    }
    serde_json::to_vec(&Wire {
        username: secret.username(),
        password: secret.password(),
    })
    .map(Zeroizing::new)
    .map_err(|_| VaultError::Storage)
}

/// `validateVaultSecret`: both values non-empty, no NUL or line break, the username at most 320 bytes and the stored
/// JSON at most 2500.
pub fn validate_vault_secret(secret: &VaultSecret) -> Result<(), VaultError> {
    let (username, password) = (secret.username(), secret.password());
    let json_len = secret_json(secret).map_or(usize::MAX, |json| json.len());
    let line_break = username
        .chars()
        .chain(password.chars())
        .any(|c| matches!(c, '\0' | '\r' | '\n'));
    let valid = !username.is_empty()
        && !password.is_empty()
        && !line_break
        && username.len() <= 320
        && json_len <= 2500;
    if valid {
        Ok(())
    } else {
        Err(VaultError::InvalidSecret)
    }
}

/// A login as the records hold it, with the reference that names its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLogin {
    pub entry: LoginEntry,
    pub reference: String,
}

/// The records: the `logins` and `login_deletions` tables. A deletion stays journalled until its secret is gone, so a
/// crash or a failed Credential Manager write never orphans a secret silently.
#[derive(Debug)]
pub struct VaultRecords {
    db: Connection,
}

impl VaultRecords {
    /// Opens (and creates) the records at `path`, refusing a folder or file that is a link. `vault-storage.ts:57-64`.
    pub fn open(path: &Path) -> Result<Self, VaultError> {
        let absolute = std::path::absolute(path).map_err(|_| VaultError::Storage)?;
        let parent = absolute.parent().ok_or(VaultError::Storage)?;
        std::fs::create_dir_all(parent).map_err(|_| VaultError::Storage)?;
        // No folder on the way may be a link. The TypeScript compares the folder's realpath with its spelling, which
        // is the same test as asking each folder on the way whether it is a link.
        for folder in parent.ancestors() {
            if is_link(folder)? {
                return Err(VaultError::Storage);
            }
        }
        if is_link(path)? {
            return Err(VaultError::Storage);
        }
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA trusted_schema=OFF; \
             CREATE TABLE IF NOT EXISTS logins (id TEXT PRIMARY KEY, data TEXT NOT NULL, reference TEXT NOT NULL UNIQUE); \
             CREATE TABLE IF NOT EXISTS login_deletions (reference TEXT PRIMARY KEY);",
        )?;
        Ok(Self { db })
    }

    /// Every login in insertion order, or only one bot's.
    pub fn list(&self, bot_id: Option<&str>) -> Result<Vec<LoginEntry>, VaultError> {
        read_entries(&self.db, bot_id)
    }

    /// One login. With a revision, it must be the stored one.
    pub fn get(&self, id: &str, revision: Option<u64>) -> Result<StoredLogin, VaultError> {
        fetch_login(&self.db, id, revision)
    }

    /// The references whose secrets still need deleting.
    pub fn pending(&self) -> Result<Vec<String>, VaultError> {
        let mut statement = self.db.prepare("SELECT reference FROM login_deletions")?;
        let references = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(references)
    }

    /// Journals a fresh reference before its secret is written, and returns it.
    pub fn prepare(&self) -> Result<String, VaultError> {
        let reference = Uuid::new_v4().to_string();
        self.db
            .execute("INSERT INTO login_deletions VALUES (?1)", [&reference])?;
        Ok(reference)
    }

    /// Saves a login under `reference`. With `previous_revision` it replaces that revision and journals the old
    /// reference as pending; without it, it adds a login, up to 1000.
    pub fn saved(
        &mut self,
        entry: &LoginEntry,
        reference: &str,
        previous_revision: Option<u64>,
    ) -> Result<(), VaultError> {
        let entry = normalize(entry.clone())?;
        let data = serde_json::to_string(&entry).map_err(|_| VaultError::InvalidEntry)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        match previous_revision {
            Some(revision) => {
                let old = fetch_login(&tx, &entry.id, Some(revision))?;
                tx.execute(
                    "INSERT OR IGNORE INTO login_deletions VALUES (?1)",
                    [&old.reference],
                )?;
                tx.execute(
                    "UPDATE logins SET data=?1,reference=?2 WHERE id=?3",
                    (&data, reference, &entry.id),
                )?;
            }
            None => {
                if read_entries(&tx, None)?.len() >= ENTRY_LIMIT {
                    return Err(VaultError::EntryLimit);
                }
                tx.execute(
                    "INSERT INTO logins VALUES (?1,?2,?3)",
                    (&entry.id, &data, reference),
                )?;
            }
        }
        tx.execute(
            "DELETE FROM login_deletions WHERE reference=?1",
            [reference],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a login and journals its reference as pending. The caller then deletes the secret.
    pub fn deleting(&mut self, id: &str, revision: u64) -> Result<(), VaultError> {
        let stored = fetch_login(&self.db, id, Some(revision))?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO login_deletions VALUES (?1)",
            [&stored.reference],
        )?;
        tx.execute("DELETE FROM logins WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Clears a pending reference once its secret is gone.
    pub fn deleted(&self, reference: &str) -> Result<(), VaultError> {
        self.db.execute(
            "DELETE FROM login_deletions WHERE reference=?1",
            [reference],
        )?;
        Ok(())
    }
}

/// Whether `path` is a link (a symbolic link or a junction). A path that does not exist is not one.
fn is_link(path: &Path) -> Result<bool, VaultError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(VaultError::Storage),
    }
}

fn fetch_login(
    db: &Connection,
    id: &str,
    revision: Option<u64>,
) -> Result<StoredLogin, VaultError> {
    let row = db
        .query_row(
            "SELECT data,reference FROM logins WHERE id=?1",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((data, reference)) = row else {
        return Err(VaultError::Missing);
    };
    let entry = decode(&data)?;
    if revision.is_some_and(|expected| entry.revision != expected) {
        return Err(VaultError::Changed);
    }
    Ok(StoredLogin { entry, reference })
}

/// Every stored login, each one checked, so a single bad row fails the read as `JSON.parse` plus zod does.
fn read_entries(db: &Connection, bot_id: Option<&str>) -> Result<Vec<LoginEntry>, VaultError> {
    let mut statement = db.prepare("SELECT data FROM logins ORDER BY rowid")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut entries = Vec::new();
    for data in rows {
        let entry = decode(&data?)?;
        if bot_id.is_none_or(|bot| entry.bot_id == bot) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// `LoginEntry.parse(JSON.parse(data))` for a stored row, strict about its keys.
fn decode(data: &str) -> Result<LoginEntry, VaultError> {
    let keys: BTreeMap<String, IgnoredAny> =
        serde_json::from_str(data).map_err(|_| VaultError::InvalidEntry)?;
    if keys.keys().any(|key| !STORED_KEYS.contains(&key.as_str())) {
        return Err(VaultError::InvalidEntry);
    }
    let entry: LoginEntry = serde_json::from_str(data).map_err(|_| VaultError::InvalidEntry)?;
    normalize(entry)
}

/// `LoginEntry.parse` as zod runs it: the name is trimmed first, then every field is checked. Returns the entry
/// with its trimmed name.
fn normalize(mut entry: LoginEntry) -> Result<LoginEntry, VaultError> {
    let name = js_trim(&entry.name).to_owned();
    let valid = valid_uuid(&entry.id)
        && (1..=100).contains(&name.chars().count())
        && (1..=256).contains(&entry.bot_id.chars().count())
        && entry.origin.as_str().chars().count() <= 2048
        && (1..=MAX_SAFE_INTEGER).contains(&entry.revision)
        && valid_timestamp(&entry.created_at)
        && valid_timestamp(&entry.updated_at);
    if !valid {
        return Err(VaultError::InvalidEntry);
    }
    entry.name = name;
    Ok(entry)
}

/// zod's `z.string().uuid()`: 8-4-4-4-12 hex, version 1 to 8, variant 8, 9, a, or b, plus the nil and max UUIDs.
fn valid_uuid(text: &str) -> bool {
    const NIL: &str = "00000000-0000-0000-0000-000000000000";
    const MAX: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    if text == NIL || text == MAX {
        return true;
    }
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
        && matches!(bytes.get(14), Some(b'1'..=b'8'))
        && matches!(bytes.get(19), Some(b'8' | b'9' | b'a' | b'b' | b'A' | b'B'))
}

/// zod's `z.string().datetime()`: `YYYY-MM-DDTHH:mm:ss`, optional fraction of digits, then a `Z`. No offsets.
fn valid_timestamp(text: &str) -> bool {
    let Some(rest) = text.strip_suffix('Z') else {
        return false;
    };
    let stamp = match rest.split_once('.') {
        Some((stamp, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return false;
            }
            stamp
        }
        None => rest,
    };
    let shaped = stamp.len() == 19
        && stamp.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            10 => byte == b'T',
            13 | 16 => byte == b':',
            _ => byte.is_ascii_digit(),
        });
    if !shaped {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        stamp
            .get(range)
            .and_then(|digits| digits.parse::<i16>().ok())
    };
    match (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) {
        (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) => {
            hour <= 23
                && minute <= 59
                && second <= 59
                && jiff::civil::Date::new(year, month as i8, day as i8).is_ok()
        }
        _ => false,
    }
}

/// What JavaScript's `String.prototype.trim()` removes (zod's `.trim()`). Rust's `trim()` differs at U+0085 and U+FEFF.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| {
        matches!(
            c,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::vault::LoginOrigin;

    fn entry() -> LoginEntry {
        LoginEntry {
            id: "00000000-0000-4000-8000-000000000123".to_owned(),
            name: "Synthetic account".to_owned(),
            origin: LoginOrigin::new("https://login.invalid").expect("a valid origin"),
            bot_id: "bot".to_owned(),
            revision: 1,
            created_at: "2026-11-15T10:00:00.000Z".to_owned(),
            updated_at: "2026-11-15T10:00:00.000Z".to_owned(),
        }
    }

    fn valid(username: &str, password: &str) -> bool {
        validate_vault_secret(&VaultSecret::new(username, password)).is_ok()
    }

    #[test]
    fn a_secret_follows_the_typescript_rules() {
        assert!(valid("alice", "pw"));
        assert!(valid("alice", "tab\tis fine"));
        assert!(!valid("", "pw"));
        assert!(!valid("alice", ""));
        assert!(!valid("line\nbreak", "pw"));
        assert!(!valid("alice", "carriage\rreturn"));
        assert!(!valid("alice", "nul\0byte"));
        assert!(valid(&"a".repeat(320), "pw"));
        assert!(!valid(&"a".repeat(321), "pw"));
        // The stored JSON is 29 bytes of fixed text plus the two values: 2500 is the limit.
        assert!(valid(&"a".repeat(300), &"b".repeat(2171)));
        assert!(!valid(&"a".repeat(300), &"b".repeat(2172)));
    }

    #[test]
    fn a_secret_is_redacted_when_debugged() {
        let shown = format!("{:?}", VaultSecret::new("alice", "hunter2-secret"));
        assert!(!shown.contains("alice"));
        assert!(!shown.contains("hunter2-secret"));
    }

    #[test]
    fn names_are_trimmed_as_javascript_trims_them_and_counted_in_code_points() {
        let named = |name: &str| {
            normalize(LoginEntry {
                name: name.to_owned(),
                ..entry()
            })
        };
        assert_eq!(named("  Synthetic  ").expect("trimmed").name, "Synthetic");
        // JavaScript's trim removes U+FEFF but not U+0085, which Rust's trim does the other way round.
        assert_eq!(
            named("\u{feff}Synthetic").expect("trimmed").name,
            "Synthetic"
        );
        assert_eq!(named("\u{85}").expect("kept").name, "\u{85}");
        assert!(named("\u{feff} ").is_err());
        assert!(named(&"\u{1F600}".repeat(100)).is_ok());
        assert!(named(&"a".repeat(101)).is_err());
    }

    #[test]
    fn bot_ids_origins_and_revisions_are_bounded_like_zod() {
        assert!(
            normalize(LoginEntry {
                bot_id: "b".repeat(256),
                ..entry()
            })
            .is_ok()
        );
        assert!(
            normalize(LoginEntry {
                bot_id: "b".repeat(257),
                ..entry()
            })
            .is_err()
        );
        assert!(
            normalize(LoginEntry {
                bot_id: String::new(),
                ..entry()
            })
            .is_err()
        );
        let long = LoginOrigin::new(format!("https://{}.invalid", "a".repeat(2100)))
            .expect("the protocol accepts the shape");
        assert!(
            normalize(LoginEntry {
                origin: long,
                ..entry()
            })
            .is_err()
        );
        assert!(
            normalize(LoginEntry {
                revision: 9_007_199_254_740_991,
                ..entry()
            })
            .is_ok()
        );
        assert!(
            normalize(LoginEntry {
                revision: 9_007_199_254_740_992,
                ..entry()
            })
            .is_err()
        );
        assert!(
            normalize(LoginEntry {
                revision: 0,
                ..entry()
            })
            .is_err()
        );
    }

    #[test]
    fn ids_are_uuids_as_zod_reads_them() {
        let with_id = |id: &str| {
            normalize(LoginEntry {
                id: id.to_owned(),
                ..entry()
            })
        };
        for id in [
            "00000000-0000-4000-8000-000000000123",
            "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
            "00000000-0000-0000-0000-000000000000",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
        ] {
            assert!(with_id(id).is_ok(), "{id}");
        }
        for id in [
            "00000000-0000-9000-8000-000000000000",
            "00000000-0000-4000-0000-000000000000",
            "00000000-0000-4000-8000-00000000000g",
            "00000000-0000-4000-8000-0000000001234",
        ] {
            assert!(with_id(id).is_err(), "{id}");
        }
    }

    #[test]
    fn timestamps_are_utc_iso_dates_as_zod_reads_them() {
        let with_time = |time: &str| {
            normalize(LoginEntry {
                created_at: time.to_owned(),
                ..entry()
            })
        };
        for time in [
            "2026-11-15T10:00:00Z",
            "2026-11-15T10:00:00.123Z",
            "2026-11-15T10:00:00.1234567891Z",
            "2024-02-29T00:00:00Z",
        ] {
            assert!(with_time(time).is_ok(), "{time}");
        }
        for time in [
            "2026-11-15T10:00:00+01:00",
            "2026-11-15T10:00:00z",
            "2026-02-30T00:00:00Z",
            "2026-11-15T24:00:00Z",
            "2026-11-15T10:00:60Z",
            "2026-11-15 10:00:00Z",
            "2026-11-15T10:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-15T10:00:00.Z",
        ] {
            assert!(with_time(time).is_err(), "{time}");
        }
    }
}
