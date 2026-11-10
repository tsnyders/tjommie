//! `devices-store.ts`: paired devices and their push registrations, with the TypeScript daemon's SQL.
//! Functions take the connection; lane A's `db::open` returns a `rusqlite::Connection`, so they work on it unchanged.

use protocol::ErrorCode;
use protocol::remote::{Device, PushKind, RemoteKey, RemoteName};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::server::RpcError;

const COLUMNS: &str = "id, name, public_key AS publicKey, paired_at AS pairedAt, last_seen_at AS lastSeenAt, revoked_at AS revokedAt";
const MAX_DEVICES: i64 = 64;

/// A device that can be sent pushes. `kinds: None` is a device that never said, which gets the legacy kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub device_id: String,
    pub token: String,
    pub kinds: Option<Vec<PushKind>>,
}

/// `db.ts` `newId`.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `db.ts` `nowIso`: `Date.prototype.toISOString`, so stored times sort and compare as text.
pub fn now_iso() -> String {
    iso(jiff::Timestamp::now())
}

/// `Date.prototype.toISOString` of a moment: UTC with milliseconds.
pub fn iso(timestamp: jiff::Timestamp) -> String {
    timestamp.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub(crate) fn database(_: rusqlite::Error) -> RpcError {
    RpcError::new(ErrorCode::Internal, "The device list could not be read.")
}

fn not_found() -> RpcError {
    RpcError::new(ErrorCode::NotFound, "This device was not found.")
}

fn revoked() -> RpcError {
    RpcError::new(ErrorCode::BadState, "This device has been revoked.")
}

/// `RemoteName.parse`: trimmed like `String.prototype.trim`, 1 to 80 UTF-16 units, no control characters.
pub fn remote_name(value: &str) -> Result<RemoteName, RpcError> {
    let trimmed =
        value.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}'));
    let units = trimmed.encode_utf16().count();
    if !(1..=80).contains(&units) {
        return Err(RpcError::new(
            ErrorCode::BadParams,
            "Enter a device name of 1 to 80 characters.",
        ));
    }
    RemoteName::new(trimmed).map_err(|error| RpcError::new(ErrorCode::BadParams, error.0))
}

fn device(row: &Row) -> rusqlite::Result<Result<Device, RpcError>> {
    let text = |index: usize| row.get::<_, String>(index);
    let optional = |index: usize| row.get::<_, Option<String>>(index);
    let (name, key) = (text(1)?, text(2)?);
    // `Device.parse` on the row: a row that breaks the schema is an internal error, never sent.
    let corrupt = || RpcError::new(ErrorCode::Internal, "A stored device is invalid.");
    Ok(match (RemoteName::new(&name), RemoteKey::new(&key)) {
        (Ok(name), Ok(public_key)) => Ok(Device {
            id: text(0)?,
            name,
            public_key,
            paired_at: text(3)?,
            last_seen_at: optional(4)?,
            revoked_at: optional(5)?,
        }),
        _ => Err(corrupt()),
    })
}

pub fn list_devices(db: &Connection) -> Result<Vec<Device>, RpcError> {
    let mut statement = db
        .prepare(&format!(
            "SELECT {COLUMNS} FROM devices ORDER BY paired_at, id"
        ))
        .map_err(database)?;
    let rows = statement.query_map([], device).map_err(database)?;
    rows.map(|row| row.map_err(database)?).collect()
}

pub fn get_device(db: &Connection, id: &str) -> Result<Device, RpcError> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM devices WHERE id = ?"),
        [id],
        device,
    )
    .optional()
    .map_err(database)?
    .ok_or_else(not_found)?
}

/// Includes revoked keys, so the host can send an authenticated refusal.
pub fn device_for_key(db: &Connection, key: &str) -> Result<Option<Device>, RpcError> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM devices WHERE public_key = ?"),
        [key],
        device,
    )
    .optional()
    .map_err(database)?
    .transpose()
}

/// Unparseable or non-list kinds count as never set; unknown kinds are dropped, so newer apps keep their pushes.
fn read_kinds(value: Option<String>) -> Option<Vec<PushKind>> {
    let names: Vec<String> = serde_json::from_str(&value?).ok()?;
    Some(
        names
            .into_iter()
            .filter_map(|kind| serde_json::from_value(kind.into()).ok())
            .collect(),
    )
}

pub fn push_targets(db: &Connection) -> Result<Vec<PushTarget>, RpcError> {
    let mut statement = db
        .prepare(
            "SELECT id AS deviceId, push_token AS token, push_kinds AS kinds FROM devices WHERE revoked_at IS NULL AND push_token IS NOT NULL",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(database)?;
    let mut targets = Vec::new();
    for row in rows {
        let (device_id, token, kinds) = row.map_err(database)?;
        // `PushTarget.safeParse`: a row whose id or token breaks the schema is skipped.
        let units = token.encode_utf16().count();
        if uuid::Uuid::try_parse(&device_id).is_err() || !(1..=4096).contains(&units) {
            continue;
        }
        targets.push(PushTarget {
            device_id,
            token,
            kinds: read_kinds(kinds),
        });
    }
    Ok(targets)
}

pub fn set_push_token(
    db: &Connection,
    id: &str,
    token: Option<&str>,
    kinds: Option<&[PushKind]>,
) -> Result<(), RpcError> {
    let transaction = db.unchecked_transaction().map_err(database)?;
    if get_device(&transaction, id)?.revoked_at.is_some() {
        return Err(revoked());
    }
    if let Some(token) = token {
        transaction
            .execute(
                "UPDATE devices SET push_token = NULL WHERE push_token = ? AND id <> ?",
                params![token, id],
            )
            .map_err(database)?;
    }
    let kinds = kinds.map(|kinds| serde_json::to_string(kinds).expect("push kinds serialize"));
    transaction
        .execute(
            "UPDATE devices SET push_token = ?, push_kinds = ? WHERE id = ? AND revoked_at IS NULL",
            params![token, kinds, id],
        )
        .map_err(database)?;
    transaction.commit().map_err(database)
}

pub fn clear_push_token(db: &Connection, id: &str, token: &str) -> Result<(), RpcError> {
    db.execute(
        "UPDATE devices SET push_token = NULL WHERE id = ? AND push_token = ?",
        [id, token],
    )
    .map(drop)
    .map_err(database)
}

pub fn add_device(db: &Connection, public_key: &str, name: &str) -> Result<Device, RpcError> {
    let public_key =
        RemoteKey::new(public_key).map_err(|error| RpcError::new(ErrorCode::BadParams, error.0))?;
    let name = remote_name(name)?;
    let paired: Option<String> = db
        .query_row(
            "SELECT id FROM devices WHERE public_key = ?",
            [public_key.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    if paired.is_some() {
        return Err(RpcError::new(
            ErrorCode::BadState,
            "This device key was already paired. Generate a new device identity before pairing again.",
        ));
    }
    let total: i64 = db
        .query_row("SELECT count(*) AS total FROM devices", [], |row| {
            row.get(0)
        })
        .map_err(database)?;
    if total >= MAX_DEVICES {
        return Err(RpcError::new(ErrorCode::Busy, "The device list is full."));
    }
    let id = new_id();
    db.execute(
        "INSERT INTO devices(id,name,public_key,paired_at) VALUES(?,?,?,?)",
        [&id, name.as_str(), public_key.as_str(), &now_iso()],
    )
    .map_err(database)?;
    get_device(db, &id)
}

pub fn rename_device(db: &Connection, id: &str, value: &str) -> Result<Device, RpcError> {
    let name = remote_name(value)?;
    get_device(db, id)?;
    db.execute(
        "UPDATE devices SET name = ? WHERE id = ?",
        [name.as_str(), id],
    )
    .map_err(database)?;
    get_device(db, id)
}

pub fn revoke_device(db: &Connection, id: &str) -> Result<Device, RpcError> {
    get_device(db, id)?;
    db.execute(
        "UPDATE devices SET revoked_at = COALESCE(revoked_at, ?), push_token = NULL, push_kinds = NULL WHERE id = ?",
        [&now_iso(), id],
    )
    .map_err(database)?;
    get_device(db, id)
}

pub fn touch_device(db: &Connection, id: &str, at: Option<&str>) -> Result<(), RpcError> {
    if get_device(db, id)?.revoked_at.is_some() {
        return Err(revoked());
    }
    let at = at.map_or_else(now_iso, str::to_owned);
    db.execute(
        "UPDATE devices SET last_seen_at = ? WHERE id = ? AND revoked_at IS NULL",
        [&at, id],
    )
    .map(drop)
    .map_err(database)
}
