//! `attachments.ts` and `attachment-handlers.ts`: files shared in chat, copied once into `shared-files\attachments` and
//! served in 256 KiB chunks. A file is reachable only through a message that links it on this host, never by path.
//! A transfer handle belongs to one connection and one principal: `device:<id>` for a paired device, which is
//! re-checked against revocation on every call, or `local:<connection>` for this PC.
//!
//! The methods run on the blocking pool, so this is plain synchronous file I/O. A read uses its own handle to the
//! leased file, so a second read is refused at once and a close stops a long content check between chunks.

use std::collections::HashMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::future::ready;
use std::io::{self, Read, Write};
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use protocol::ErrorCode;
use protocol::attachments::{
    ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentInfo, AttachmentOpen, AttachmentRead,
    AttachmentRef, MAX_ATTACHMENT_BYTES, TransferId,
};
use protocol::wire::{Empty, method};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use crate::context::Db;
use crate::fsid;
use crate::ids::{new_id, now_iso};
use crate::server::{Handlers, Principal, RequestContext, RpcError};

const LEASE: Duration = Duration::from_secs(5 * 60);
const CHUNK: u64 = ATTACHMENT_CHUNK_BYTES as u64;
const MAX_LEASES: usize = 16;
const LEASES_PER_OWNER: usize = 2;
const STALE_PART: Duration = Duration::from_secs(24 * 60 * 60);

fn error(code: ErrorCode, message: &str) -> RpcError {
    RpcError::new(code, message)
}

fn database(_: rusqlite::Error) -> RpcError {
    error(
        ErrorCode::Internal,
        "The shared file records could not be read.",
    )
}

/// The size, write time and creation time a handle reports. They say whether the open file changed; they can't say
/// which file a path names, which is `fsid`'s job (`sameFile` in Node also compares the device and inode).
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len()
        && a.modified().ok() == b.modified().ok()
        && a.created().ok() == b.created().ok()
}

/// The snapshot open as `input` is unmodified since `initial` and is still the file at `source`. An identity that
/// can't be read refuses.
fn unchanged(initial: &Metadata, input: &File, source: &Path) -> bool {
    input.metadata().is_ok_and(|now| same_file(initial, &now))
        && fsid::is_open_file(input, source).unwrap_or(false)
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// A plain directory: not a link or junction.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

/// Node's `path.win32.basename`: the last part after `/` or `\`, without a drive prefix.
fn basename(name: &str) -> &str {
    let trimmed = name.trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    match last.as_bytes() {
        [drive, b':', ..] if drive.is_ascii_alphabetic() && last == trimmed => &last[2..],
        _ => last,
    }
}

fn storage_name_ok(id: &str, storage: &str) -> bool {
    let valid = storage.len() == 40
        && storage.ends_with(".bin")
        && storage[..36]
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'));
    valid && storage == format!("{id}.bin")
}

/// `attachmentContentType`: by content signature only, so a file can't claim an active type by its name.
pub fn attachment_content_type(name: &str, data: &[u8]) -> &'static str {
    let starts = |prefix: &[u8]| data.starts_with(prefix);
    let at = |range: std::ops::Range<usize>, value: &[u8]| {
        data.get(range).is_some_and(|bytes| bytes == value)
    };
    let lower = name.to_ascii_lowercase();
    if starts(&[137, 80, 78, 71, 13, 10, 26, 10]) {
        "image/png"
    } else if starts(&[255, 216, 255]) {
        "image/jpeg"
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        "image/gif"
    } else if starts(b"RIFF") && at(8..12, b"WEBP") {
        "image/webp"
    } else if starts(b"RIFF") && at(8..12, b"WAVE") {
        "audio/wav"
    } else if starts(b"%PDF-") {
        "application/pdf"
    } else if starts(b"ID3")
        || (data.first() == Some(&255)
            && data
                .get(1)
                .is_some_and(|byte| [251, 243, 242].contains(byte)))
    {
        "audio/mpeg"
    } else if at(4..8, b"ftyp") {
        if lower.ends_with(".m4a") {
            "audio/mp4"
        } else {
            "video/mp4"
        }
    } else if starts(&[26, 69, 223, 163]) && data.windows(4).any(|window| window == b"webm") {
        "video/webm"
    } else if (lower.ends_with(".md") || lower.ends_with(".markdown"))
        && !data.contains(&0)
        // A streaming decode: a character cut off at the end of the sample is fine.
        && std::str::from_utf8(data).map_or_else(|error| error.error_len().is_none(), |_| true)
    {
        "text/markdown"
    } else {
        "application/octet-stream"
    }
}

/// What a legacy backfill found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Backfill {
    pub registered: u32,
    pub unavailable: u32,
}

struct Row {
    id: String,
    host_id: String,
    storage_name: String,
    original_name: String,
    byte_length: i64,
    content_type: String,
    sha256: String,
}

fn read_row(row: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get("id")?,
        host_id: row.get("host_id")?,
        storage_name: row.get("storage_name")?,
        original_name: row.get("original_name")?,
        byte_length: row.get("byte_length")?,
        content_type: row.get("content_type")?,
        sha256: row.get("sha256")?,
    })
}

struct LeaseState {
    file: Option<File>,
    stats: Option<Metadata>,
    reading: bool,
    verified: bool,
    expires: Instant,
}

struct Lease {
    id: String,
    connection_id: String,
    owner: String,
    signal: tokio_util::sync::CancellationToken,
    reference: AttachmentRef,
    info: AttachmentInfo,
    closed: AtomicBool,
    state: Mutex<LeaseState>,
}

impl Lease {
    fn state(&self) -> MutexGuard<'_, LeaseState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Idle, closed, or its connection ended: it no longer holds capacity.
    fn finished(&self, now: Instant) -> bool {
        self.is_closed() || self.signal.is_cancelled() || now >= self.state().expires
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.state().file = None;
    }
}

fn owner_of(context: &RequestContext) -> String {
    match &context.principal {
        Principal::Device { device_id } => format!("device:{device_id}"),
        Principal::Local => format!("local:{}", context.connection_id),
    }
}

fn closed_transfer() -> RpcError {
    error(ErrorCode::Forbidden, "This file transfer was closed.")
}

/// The database for one operation: the caller's connection when it already holds the lock (the team's code runs inside
/// the runner's locks, and locking again here would deadlock), else this store's own, locked.
enum Handle<'a> {
    Given(&'a Connection),
    Own(MutexGuard<'a, Connection>),
}

impl std::ops::Deref for Handle<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Handle::Given(db) => db,
            Handle::Own(guard) => guard,
        }
    }
}

pub struct Attachments {
    db: Db,
    host_id: String,
    staging: PathBuf,
    root: PathBuf,
    leases: Mutex<HashMap<String, Arc<Lease>>>,
    closed: AtomicBool,
}

impl Attachments {
    /// `root\shared-files` must already be a plain folder (the daemon makes it at start).
    pub fn new(db: Db, data_root: &Path, host_id: &str) -> Result<Attachments, RpcError> {
        let unavailable = || {
            error(
                ErrorCode::Unavailable,
                "The protected shared-files folder is unavailable.",
            )
        };
        if host_id.is_empty() {
            return Err(error(
                ErrorCode::Internal,
                "Attachments need a host identity.",
            ));
        }
        let staging =
            std::path::absolute(data_root.join("shared-files")).map_err(|_| unavailable())?;
        if !is_real_dir(&staging) {
            return Err(unavailable());
        }
        let staging = fs::canonicalize(&staging).map_err(|_| unavailable())?;
        let root = staging.join("attachments");
        if !root.exists() {
            fs::create_dir(&root).map_err(|_| unavailable())?;
        }
        let attachments = Attachments {
            db,
            host_id: host_id.to_owned(),
            staging,
            root,
            leases: Mutex::default(),
            closed: AtomicBool::new(false),
        };
        attachments.check_root()?;
        Ok(attachments)
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn handle<'a>(&'a self, given: Option<&'a Connection>) -> Handle<'a> {
        match given {
            Some(db) => Handle::Given(db),
            None => Handle::Own(self.db()),
        }
    }
    fn leases(&self) -> MutexGuard<'_, HashMap<String, Arc<Lease>>> {
        self.leases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn check_root(&self) -> Result<(), RpcError> {
        let intact = is_real_dir(&self.root)
            && is_real_dir(&self.staging)
            && fs::canonicalize(&self.root).is_ok_and(|real| real == self.root);
        if intact {
            Ok(())
        } else {
            Err(error(
                ErrorCode::Unavailable,
                "Shared files are unavailable.",
            ))
        }
    }

    /// `#safeFile`: the real path of a plain file inside `base`, reached without any link or junction on the way.
    fn safe_file(base: &Path, candidate: &Path) -> Option<PathBuf> {
        let file = std::path::absolute(candidate).ok()?;
        let canonical = fs::canonicalize(&file).ok()?;
        let base = fs::canonicalize(base).ok()?;
        if canonical == base || !canonical.starts_with(&base) {
            return None;
        }
        if file.ancestors().any(is_link) {
            return None;
        }
        fs::symlink_metadata(&file)
            .ok()?
            .is_file()
            .then_some(canonical)
    }

    /// A file directly inside a `share-*` folder of the staging area.
    fn snapshot_path(&self, candidate: &Path) -> Option<PathBuf> {
        let file = Self::safe_file(&self.staging, candidate)?;
        let folder = file.parent()?;
        let shared = folder.parent() == Some(self.staging.as_path())
            && folder
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("share-"));
        shared.then_some(file)
    }

    /// A staged snapshot, or a file directly in a bot's inbox (deliveries from before attachments existed).
    fn legacy_path(&self, candidate: &Path) -> Option<PathBuf> {
        if let Some(staged) = self.snapshot_path(candidate) {
            return Some(staged);
        }
        let homes: Vec<String> = {
            let db = self.db();
            let mut statement = db.prepare("SELECT home_folder FROM bots").ok()?;
            statement
                .query_map([], |row| row.get(0))
                .ok()?
                .filter_map(Result::ok)
                .collect()
        };
        homes.into_iter().find_map(|home| {
            let inbox = std::path::absolute(Path::new(&home).join("inbox")).ok()?;
            let file = Self::safe_file(&inbox, candidate)?;
            (file.parent() == Some(fs::canonicalize(&inbox).ok()?.as_path())).then_some(file)
        })
    }

    pub fn register_snapshot(&self, path: &Path, name: &str) -> Result<AttachmentInfo, RpcError> {
        self.register_snapshot_with(None, path, name)
    }

    /// [`Attachments::register_snapshot`] for a caller that holds the database lock already, and passes its connection.
    pub fn register_snapshot_in(
        &self,
        db: &Connection,
        path: &Path,
        name: &str,
    ) -> Result<AttachmentInfo, RpcError> {
        self.register_snapshot_with(Some(db), path, name)
    }

    fn register_snapshot_with(
        &self,
        db: Option<&Connection>,
        path: &Path,
        name: &str,
    ) -> Result<AttachmentInfo, RpcError> {
        self.check_root()?;
        let safe = self.snapshot_path(path).ok_or_else(|| {
            error(
                ErrorCode::BadParams,
                "Only a protected shared snapshot can be registered.",
            )
        })?;
        self.copy(db, &safe, name)
    }

    fn copy(
        &self,
        given: Option<&Connection>,
        source: &Path,
        name: &str,
    ) -> Result<AttachmentInfo, RpcError> {
        let id = new_id();
        let storage = format!("{id}.bin");
        let destination = self.root.join(&storage);
        let temporary = self.root.join(format!("{storage}.part"));
        let mut input = File::open(source).map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "The shared snapshot could not be opened.",
            )
        })?;
        let published = || {
            error(
                ErrorCode::Unavailable,
                "The shared file could not be published.",
            )
        };
        let attempt = (|| -> Result<AttachmentInfo, RpcError> {
            let initial = input.metadata().map_err(|_| published())?;
            let still = |input: &File| -> bool {
                self.legacy_path(source).as_deref() == Some(source)
                    && unchanged(&initial, input, source)
            };
            if !initial.is_file() || initial.len() > MAX_ATTACHMENT_BYTES || !still(&input) {
                return Err(error(
                    ErrorCode::BadParams,
                    "The shared snapshot is invalid or too large.",
                ));
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| published())?;
            self.check_root()?;
            let (mut hash, mut buffer) = (Sha256::new(), vec![0u8; 64 * 1024]);
            let (mut size, mut sample) = (0u64, Vec::new());
            loop {
                let count = input.read(&mut buffer).map_err(|_| published())?;
                if count == 0 {
                    break;
                }
                if size == 0 {
                    sample = buffer[..count.min(8192)].to_vec();
                }
                size += count as u64;
                if size > MAX_ATTACHMENT_BYTES {
                    return Err(error(
                        ErrorCode::BadParams,
                        "The file exceeds the 200 MB sharing limit.",
                    ));
                }
                hash.update(&buffer[..count]);
                output
                    .write_all(&buffer[..count])
                    .map_err(|_| published())?;
            }
            if size != initial.len() || !still(&input) {
                return Err(error(
                    ErrorCode::Corrupt,
                    "The shared snapshot changed while being registered.",
                ));
            }
            output.sync_all().map_err(|_| published())?;
            drop(output);
            let info = AttachmentInfo {
                id: id.clone(),
                host_id: self.host_id.clone(),
                name: basename(name).to_owned(),
                size,
                content_type: attachment_content_type(name, &sample).to_owned(),
                sha256: format!("{:x}", hash.finalize()),
            };
            // `AttachmentInfo.parse`
            let shown = serde_json::to_value(&info).expect("attachment info serializes");
            protocol::validate("attachments.metadata", &shown).map_err(|_| {
                error(
                    ErrorCode::BadParams,
                    "The shared file's details are invalid.",
                )
            })?;
            fs::rename(&temporary, &destination).map_err(|_| published())?;
            self.handle(given)
                .execute(
                    "INSERT INTO attachments(id,host_id,storage_name,original_name,byte_length,content_type,sha256,created_at) VALUES(?,?,?,?,?,?,?,?)",
                    params![id, info.host_id, storage, info.name, info.size as i64, info.content_type, info.sha256, now_iso()],
                )
                .map_err(|_| published())?;
            Ok(info)
        })();
        attempt.map_err(|failure| {
            let gone = |path: &Path| {
                fs::remove_file(path).or_else(|error| {
                    if error.kind() == io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
            };
            if gone(&temporary).and(gone(&destination)).is_err() {
                return error(
                    ErrorCode::Unavailable,
                    "The shared file could not be published; temporary cleanup will retry later.",
                );
            }
            failure
        })
    }

    pub fn link(&self, message_id: &str, index: u64, attachment_id: &str) -> Result<(), RpcError> {
        self.link_with(None, message_id, index, attachment_id)
    }

    /// [`Attachments::link`] for a caller that holds the database lock already, and passes its connection.
    pub fn link_in(
        &self,
        db: &Connection,
        message_id: &str,
        index: u64,
        attachment_id: &str,
    ) -> Result<(), RpcError> {
        self.link_with(Some(db), message_id, index, attachment_id)
    }

    fn link_with(
        &self,
        given: Option<&Connection>,
        message_id: &str,
        index: u64,
        attachment_id: &str,
    ) -> Result<(), RpcError> {
        // `AttachmentRef.parse`
        let bounded = |text: &str| (1..=128).contains(&text.encode_utf16().count());
        if !bounded(message_id) || !bounded(attachment_id) {
            return Err(error(ErrorCode::BadParams, "Invalid attachment reference."));
        }
        let db = self.handle(given);
        let known: Option<i64> = db
            .query_row(
                "SELECT 1 FROM attachments WHERE id=? AND host_id=?",
                [attachment_id, &self.host_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        if known.is_none() {
            return Err(error(
                ErrorCode::Unavailable,
                "This shared file is unavailable.",
            ));
        }
        db.execute(
            "INSERT INTO message_attachments(message_id,attachment_index,attachment_id) VALUES(?,?,?) ON CONFLICT(message_id,attachment_index) DO UPDATE SET attachment_id=excluded.attachment_id",
            params![message_id, index as i64, attachment_id],
        )
        .map(drop)
        .map_err(database)
    }

    fn backfill_message(&self, message_id: &str) -> Result<Backfill, RpcError> {
        let mut result = Backfill::default();
        let row: Option<(String, Option<String>)> = self
            .db()
            .query_row(
                "SELECT kind,data FROM messages WHERE id=?",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(database)?;
        let Some((kind, Some(data))) = row else {
            return Ok(result);
        };
        let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&data) else {
            return Ok(result);
        };
        let files: Vec<Value> = match (kind.as_str(), data.get("files")) {
            ("file", _) => vec![Value::Object(data.clone())],
            ("text", Some(Value::Array(files))) => files.clone(),
            _ => Vec::new(),
        };
        for (index, file) in files.iter().enumerate() {
            let (Some(path), Some(name)) = (
                file.get("path").and_then(Value::as_str),
                file.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            let linked: Option<i64> = self
                .db()
                .query_row(
                    "SELECT 1 FROM message_attachments WHERE message_id=? AND attachment_index=?",
                    params![message_id, index as i64],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database)?;
            if linked.is_some() {
                continue;
            }
            let Some(source) = self.legacy_path(Path::new(path)) else {
                result.unavailable += 1;
                continue;
            };
            let size = fs::metadata(&source)
                .map(|metadata| metadata.len())
                .unwrap_or(u64::MAX);
            let recorded = file.get("size").and_then(Value::as_u64);
            if size > MAX_ATTACHMENT_BYTES || (kind == "file" && recorded != Some(size)) {
                result.unavailable += 1;
                continue;
            }
            let info = match self.copy(None, &source, name) {
                Ok(info) => info,
                Err(failure)
                    if matches!(
                        failure.code,
                        ErrorCode::BadParams | ErrorCode::Corrupt | ErrorCode::Unavailable
                    ) =>
                {
                    result.unavailable += 1;
                    continue;
                }
                Err(failure) => return Err(failure),
            };
            self.link(message_id, index as u64, &info.id)?;
            result.registered += 1;
        }
        Ok(result)
    }

    /// Registers files that messages from before attachments point at, where they are still safe to adopt.
    pub fn backfill_legacy(&self) -> Result<Backfill, RpcError> {
        self.check_root()?;
        let messages: Vec<String> = {
            let db = self.db();
            let mut statement = db
                .prepare(
                    "SELECT id FROM messages WHERE kind IN ('file','text') AND data IS NOT NULL",
                )
                .map_err(database)?;
            statement
                .query_map([], |row| row.get(0))
                .map_err(database)?
                .filter_map(Result::ok)
                .collect()
        };
        let mut result = Backfill::default();
        for message in messages {
            let next = self.backfill_message(&message)?;
            result.registered += next.registered;
            result.unavailable += next.unavailable;
        }
        Ok(result)
    }

    /// `#context`: a live connection, and for a device, one that is still paired.
    fn authorize(&self, context: Option<&RequestContext>) -> Result<RequestContext, RpcError> {
        let context = match context {
            Some(context)
                if !self.closed.load(Ordering::Acquire)
                    && !context.signal.is_cancelled()
                    && !context.connection_id.is_empty() =>
            {
                context
            }
            _ => {
                return Err(error(
                    ErrorCode::Forbidden,
                    "This file connection is no longer authorized.",
                ));
            }
        };
        if let Principal::Device { device_id } = &context.principal {
            let paired: Option<i64> = self
                .db()
                .query_row(
                    "SELECT 1 FROM devices WHERE id=? AND revoked_at IS NULL",
                    [device_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database)?;
            if paired.is_none() {
                return Err(error(
                    ErrorCode::Forbidden,
                    "This device is no longer authorized.",
                ));
            }
        }
        Ok(context.clone())
    }

    fn row(&self, reference: &AttachmentRef) -> Result<Row, RpcError> {
        let find = |joined: bool| -> Result<Option<Row>, RpcError> {
            let sql = if joined {
                "SELECT a.* FROM attachments a JOIN message_attachments m ON m.attachment_id=a.id JOIN messages msg ON msg.id=m.message_id WHERE m.message_id=? AND m.attachment_index=?"
            } else {
                "SELECT a.* FROM attachments a JOIN message_attachments m ON m.attachment_id=a.id WHERE m.message_id=? AND m.attachment_index=?"
            };
            self.db()
                .query_row(
                    sql,
                    params![reference.message_id, reference.attachment_index as i64],
                    read_row,
                )
                .optional()
                .map_err(database)
        };
        let row = match find(true)? {
            Some(row) => Some(row),
            None => {
                self.backfill_message(&reference.message_id)?;
                find(false)?
            }
        };
        match row {
            Some(row)
                if row.host_id == self.host_id
                    && reference
                        .attachment_id
                        .as_ref()
                        .is_none_or(|id| *id == row.id) =>
            {
                Ok(row)
            }
            _ => Err(error(
                ErrorCode::Unavailable,
                "This shared file is unavailable.",
            )),
        }
    }

    fn info(row: &Row) -> Result<AttachmentInfo, RpcError> {
        let corrupt = || error(ErrorCode::Corrupt, "The shared file record is invalid.");
        let info = AttachmentInfo {
            id: row.id.clone(),
            host_id: row.host_id.clone(),
            name: row.original_name.clone(),
            size: u64::try_from(row.byte_length).map_err(|_| corrupt())?,
            content_type: row.content_type.clone(),
            sha256: row.sha256.clone(),
        };
        let shown = serde_json::to_value(&info).expect("attachment info serializes");
        protocol::validate("attachments.metadata", &shown).map_err(|_| corrupt())?;
        Ok(info)
    }

    fn path(&self, row: &Row) -> Result<PathBuf, RpcError> {
        self.check_root()?;
        if !storage_name_ok(&row.id, &row.storage_name) {
            return Err(error(
                ErrorCode::Corrupt,
                "The shared file record is invalid.",
            ));
        }
        Self::safe_file(&self.root, &self.root.join(&row.storage_name))
            .ok_or_else(|| error(ErrorCode::Unavailable, "This shared file is unavailable."))
    }

    pub fn metadata(
        &self,
        reference: &AttachmentRef,
        context: Option<&RequestContext>,
    ) -> Result<AttachmentInfo, RpcError> {
        self.authorize(context)?;
        let row = self.row(reference)?;
        let info = Self::info(&row)?;
        let size = fs::metadata(self.path(&row)?)
            .map(|metadata| metadata.len())
            .ok();
        if size != Some(info.size) {
            return Err(error(ErrorCode::Corrupt, "The shared file changed."));
        }
        Ok(info)
    }

    pub fn open(
        &self,
        reference: &AttachmentRef,
        context: Option<&RequestContext>,
    ) -> Result<AttachmentOpen, RpcError> {
        let context = self.authorize(context)?;
        let info = self.metadata(reference, Some(&context))?;
        let row = self.row(reference)?;
        let file = File::open(self.path(&row)?).map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "This shared file could not be opened.",
            )
        })?;
        let initial = file.metadata().map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "This shared file could not be opened.",
            )
        })?;
        if initial.len() != info.size {
            return Err(error(ErrorCode::Corrupt, "The shared file changed."));
        }
        self.authorize(Some(&context))?;
        if self.row(reference)?.id != info.id {
            return Err(error(
                ErrorCode::Unavailable,
                "This shared file is no longer available.",
            ));
        }
        let owner = owner_of(&context);
        let now = Instant::now();
        let mut leases = self.leases();
        leases.retain(|_, lease| !lease.finished(now));
        if leases.len() >= MAX_LEASES
            || leases.values().filter(|lease| lease.owner == owner).count() >= LEASES_PER_OWNER
        {
            return Err(error(
                ErrorCode::Busy,
                "Two file transfers are already active. Close one and try again.",
            ));
        }
        let id = new_id();
        leases.insert(
            id.clone(),
            Arc::new(Lease {
                id: id.clone(),
                connection_id: context.connection_id.clone(),
                owner,
                signal: context.signal.clone(),
                reference: reference.clone(),
                info: info.clone(),
                closed: AtomicBool::new(false),
                state: Mutex::new(LeaseState {
                    file: Some(file),
                    stats: Some(initial),
                    reading: false,
                    verified: false,
                    expires: now + LEASE,
                }),
            }),
        );
        let expires = jiff::Timestamp::try_from(SystemTime::now() + LEASE)
            .unwrap_or_else(|_| jiff::Timestamp::now());
        Ok(AttachmentOpen {
            transfer_id: id,
            attachment: info,
            chunk_bytes: ATTACHMENT_CHUNK_BYTES,
            expires_at: expires.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        })
    }

    /// The caller's own lease, or `forbidden`.
    fn lease(
        &self,
        transfer_id: &str,
        context: &RequestContext,
    ) -> Result<Option<Arc<Lease>>, RpcError> {
        let Some(lease) = self.leases().get(transfer_id).cloned() else {
            return Ok(None);
        };
        if lease.connection_id != context.connection_id || lease.owner != owner_of(context) {
            return Err(error(
                ErrorCode::Forbidden,
                "This file transfer is not authorized.",
            ));
        }
        Ok(Some(lease))
    }

    fn close_lease(&self, lease: &Lease) {
        lease.close();
        self.leases().remove(&lease.id);
    }

    pub fn read(
        &self,
        input: &AttachmentRead,
        context: Option<&RequestContext>,
    ) -> Result<AttachmentChunk, RpcError> {
        let context = self.authorize(context)?;
        let not_authorized = || {
            error(
                ErrorCode::Forbidden,
                "This file transfer is not authorized.",
            )
        };
        let lease = self
            .lease(&input.transfer_id, &context)?
            .ok_or_else(not_authorized)?;
        if lease.is_closed() {
            return Err(not_authorized());
        }
        if Instant::now() >= lease.state().expires {
            self.close_lease(&lease);
            return Err(error(
                ErrorCode::Expired,
                "This file transfer expired. Open it again.",
            ));
        }
        let size = lease.info.size;
        if input.offset > size || input.length < 1 || u64::from(input.length) > CHUNK {
            return Err(error(ErrorCode::BadParams, "Invalid file byte range."));
        }
        let busy = || error(ErrorCode::Busy, "A file read is already in progress.");
        {
            let state = lease.state();
            if state.reading || state.file.is_none() || state.stats.is_none() {
                return Err(busy());
            }
        }
        if self.row(&lease.reference)?.id != lease.info.id {
            return Err(error(
                ErrorCode::Unavailable,
                "This shared file is no longer available.",
            ));
        }
        let (file, stats, verified) = {
            let mut state = lease.state();
            let (Some(file), Some(stats)) = (state.file.as_ref(), state.stats.clone()) else {
                return Err(busy());
            };
            if state.reading {
                return Err(busy());
            }
            let file = file
                .try_clone()
                .map_err(|_| error(ErrorCode::Unavailable, "The shared file could not be read."))?;
            state.reading = true;
            (file, stats, state.verified)
        };
        let result = self.read_leased(&lease, &context, &file, &stats, verified, input);
        lease.state().reading = false;
        result.inspect_err(|_| self.close_lease(&lease))
    }

    fn read_leased(
        &self,
        lease: &Lease,
        context: &RequestContext,
        file: &File,
        stats: &Metadata,
        verified: bool,
        input: &AttachmentRead,
    ) -> Result<AttachmentChunk, RpcError> {
        let changed = || error(ErrorCode::Corrupt, "The shared file changed.");
        let incomplete = || error(ErrorCode::Corrupt, "The shared file is incomplete.");
        let unreadable = |_| error(ErrorCode::Unavailable, "The shared file could not be read.");
        if !file.metadata().is_ok_and(|now| same_file(stats, &now)) {
            return Err(changed());
        }
        if !verified {
            // Check the whole content before releasing any byte; a close or a lost connection stops it between chunks.
            let (mut hash, mut buffer) = (Sha256::new(), vec![0u8; 64 * 1024]);
            let mut at = 0u64;
            while at < lease.info.size {
                self.authorize(Some(context))?;
                if lease.is_closed() {
                    return Err(closed_transfer());
                }
                let want = buffer.len().min((lease.info.size - at) as usize);
                let count = file
                    .seek_read(&mut buffer[..want], at)
                    .map_err(unreadable)?;
                if count == 0 {
                    return Err(incomplete());
                }
                hash.update(&buffer[..count]);
                at += count as u64;
            }
            if lease.is_closed() {
                return Err(closed_transfer());
            }
            let intact = file.metadata().is_ok_and(|now| same_file(stats, &now))
                && format!("{:x}", hash.finalize()) == lease.info.sha256;
            if !intact {
                return Err(error(
                    ErrorCode::Corrupt,
                    "The shared file failed its content check.",
                ));
            }
            lease.state().verified = true;
        }
        let mut bytes =
            vec![0u8; u64::from(input.length).min(lease.info.size - input.offset) as usize];
        let mut count = 0;
        while count < bytes.len() {
            if lease.is_closed() {
                return Err(closed_transfer());
            }
            let read = file
                .seek_read(&mut bytes[count..], input.offset + count as u64)
                .map_err(unreadable)?;
            if read == 0 {
                return Err(incomplete());
            }
            count += read;
        }
        self.authorize(Some(context))?;
        if lease.is_closed() {
            return Err(closed_transfer());
        }
        if !file.metadata().is_ok_and(|now| same_file(stats, &now))
            || self.row(&lease.reference).map(|row| row.id).ok().as_deref() != Some(&lease.info.id)
        {
            return Err(changed());
        }
        lease.state().expires = Instant::now() + LEASE;
        Ok(AttachmentChunk {
            transfer_id: lease.id.clone(),
            revision: lease.info.sha256.clone(),
            offset: input.offset,
            bytes: STANDARD.encode(&bytes),
            eof: input.offset + count as u64 == lease.info.size,
        })
    }

    pub fn close_transfer(
        &self,
        transfer_id: &str,
        context: Option<&RequestContext>,
    ) -> Result<(), RpcError> {
        let context = self.authorize(context)?;
        if let Some(lease) = self.lease(transfer_id, &context)? {
            self.close_lease(&lease);
        }
        Ok(())
    }

    /// Removes attachments no message links (unless a transfer still holds them) and stray copies older than a day.
    pub fn cleanup(&self) -> Result<(), RpcError> {
        self.cleanup_with(None)
    }

    /// [`Attachments::cleanup`] for a caller that holds the database lock already, and passes its connection.
    pub fn cleanup_in(&self, db: &Connection) -> Result<(), RpcError> {
        self.cleanup_with(Some(db))
    }

    fn cleanup_with(&self, given: Option<&Connection>) -> Result<(), RpcError> {
        self.check_root()?;
        let now = Instant::now();
        let leased: Vec<String> = {
            let mut leases = self.leases();
            leases.retain(|_, lease| {
                let finished = lease.finished(now);
                if finished {
                    lease.close();
                }
                !finished
            });
            leases.values().map(|lease| lease.info.id.clone()).collect()
        };
        let orphans: Vec<Row> = {
            let db = self.handle(given);
            let mut statement = db
                .prepare("SELECT a.* FROM attachments a WHERE NOT EXISTS(SELECT 1 FROM message_attachments m WHERE m.attachment_id=a.id)")
                .map_err(database)?;
            statement
                .query_map([], read_row)
                .map_err(database)?
                .filter_map(Result::ok)
                .collect()
        };
        for row in orphans {
            if leased.contains(&row.id) || !storage_name_ok(&row.id, &row.storage_name) {
                continue;
            }
            match fs::remove_file(self.root.join(&row.storage_name)) {
                Err(failure) if failure.kind() != io::ErrorKind::NotFound => continue,
                _ => {}
            }
            self.handle(given)
                .execute("DELETE FROM attachments WHERE id=?", [&row.id])
                .map_err(database)?;
        }
        let known: Vec<String> = {
            let db = self.handle(given);
            let mut statement = db
                .prepare("SELECT storage_name FROM attachments")
                .map_err(database)?;
            statement
                .query_map([], |row| row.get(0))
                .map_err(database)?
                .filter_map(Result::ok)
                .collect()
        };
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stray = name.len() >= 40
                && storage_name_ok(&name[..36], &name[..40])
                && matches!(&name[40..], "" | ".part")
                && !known.contains(&name);
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let old = metadata
                .modified()
                .ok()
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age > STALE_PART);
            if stray && metadata.is_file() && !metadata.file_type().is_symlink() && old {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        for (_, lease) in self.leases().drain() {
            lease.close();
        }
    }

    /// `attachment-handlers.ts`. Each runs on the blocking pool and returns a finished future.
    pub fn handlers(self: &Arc<Self>, handlers: Handlers) -> Handlers {
        let (metadata, open, read, close) =
            (self.clone(), self.clone(), self.clone(), self.clone());
        handlers
            .on::<method::AttachmentsMetadata, _>(move |reference, context| {
                ready(metadata.metadata(&reference, context.as_ref()))
            })
            .on::<method::AttachmentsOpen, _>(move |reference, context| {
                ready(open.open(&reference, context.as_ref()))
            })
            .on::<method::AttachmentsRead, _>(move |input, context| {
                ready(read.read(&input, context.as_ref()))
            })
            .on::<method::AttachmentsClose, _>(move |TransferId { transfer_id }, context| {
                ready(
                    close
                        .close_transfer(&transfer_id, context.as_ref())
                        .map(|()| Empty {}),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, FileTimes};
    use std::os::windows::fs::FileTimesExt;

    use super::{same_file, unchanged};

    #[test]
    fn a_copy_swapped_in_with_the_same_size_and_times_is_not_the_opened_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let (source, moved) = (
            dir.path().join("snapshot.bin"),
            dir.path().join("moved.bin"),
        );
        fs::write(&source, b"synthetic snapshot").unwrap();
        let input = File::open(&source).unwrap();
        let initial = input.metadata().unwrap();
        assert!(unchanged(&initial, &input, &source));
        fs::rename(&source, &moved).unwrap();
        fs::write(&source, b"synthetic snapshot").unwrap();
        let times = FileTimes::new()
            .set_modified(initial.modified().unwrap())
            .set_created(initial.created().unwrap());
        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(times)
            .unwrap();
        let copy = fs::metadata(&source).unwrap();
        assert!(
            same_file(&initial, &copy),
            "size and times alone can't tell them apart"
        );
        assert!(
            !unchanged(&initial, &input, &source),
            "only the file identity can"
        );
        assert!(unchanged(&initial, &input, &moved));
    }
}
