//! The shared-file half of `team.ts`: which files a bot may share, and the bounded snapshot it copies into the
//! protected `shared-files` folder so that a later swap, deletion or growth of the source can't change what is delivered.
//!
//! Errors are the messages a bot reads, as plain text.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use protocol::model::FileData;

use crate::fsid;
use crate::providers::js;

pub const MAX_SHARE_BYTES: u64 = 200 * 1024 * 1024;
/// Folders left by a crash are removed at start-up once they are older than this.
pub const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const CHANGED: &str = "The file changed while it was being shared. Please try again.";
const TOO_BIG: &str = "That file is too big to share. The limit is 200 MB.";
const STAGING_UNAVAILABLE: &str = "The protected shared-files folder is unavailable.";
const OWN_FOLDERS: &str = "You can only share files from your own folders.";

/// `within`: `path` is `root` or lies below it, by file identity (junctions, `\\?\` and UNC aliases agree). Any doubt
/// answers no.
pub fn within(root: &Path, path: &Path) -> bool {
    matches!(fsid::is_within(path, root), Ok(true))
}

/// `path` lies below `root`, and is not `root` itself. Any doubt answers no.
fn strictly_inside(path: &Path, root: &Path) -> bool {
    within(root, path) && matches!(fsid::same_location(path, root), Ok(false))
}

/// The real path of `target` when it's an existing file inside one of `roots`, with links followed. Anything else is an
/// error the bot can read.
pub fn file_inside(roots: &[PathBuf], target: &Path) -> Result<PathBuf, String> {
    let real = fs::canonicalize(target)
        .map_err(|_| format!("There's no file at {}.", target.display()))?;
    let inside = roots
        .iter()
        .any(|root| fs::canonicalize(root).is_ok() && strictly_inside(&real, root));
    if !inside {
        return Err(OWN_FOLDERS.to_owned());
    }
    let stats = fs::metadata(&real).map_err(|error| error.to_string())?;
    if !stats.is_file() {
        return Err(format!("{} is a folder, not a file.", target.display()));
    }
    if stats.len() > MAX_SHARE_BYTES {
        return Err(TOO_BIG.to_owned());
    }
    Ok(real)
}

fn io_message(error: std::io::Error) -> String {
    error.to_string()
}

/// Opens the file once and copies a bounded snapshot from that handle into a new `share-<bot>-…` folder under
/// `staging`. A later rename, link swap, deletion or growth of the source cannot change the file card or the delivered
/// bytes.
///
/// The handle must still be the file at the path: the same volume serial and 128-bit file id (`fsid::is_open_file`),
/// where TypeScript compares device and inode. A different file copied into its place, even with the same size and
/// times, is refused.
pub fn snapshot_file(
    roots: &[PathBuf],
    target: &Path,
    staging: &Path,
    bot_id: &str,
) -> Result<FileData, String> {
    let file = file_inside(roots, target)?;
    let mut source = File::open(&file).map_err(io_message)?;
    let opened = source.metadata().map_err(io_message)?;
    let checked_path = file_inside(roots, &file)?;
    if !opened.is_file() || !matches!(fsid::is_open_file(&source, &checked_path), Ok(true)) {
        return Err(CHANGED.to_owned());
    }
    if opened.len() > MAX_SHARE_BYTES {
        return Err(TOO_BIG.to_owned());
    }
    // The daemon creates and protects this folder before serving tools. A missing or replaced staging directory must not
    // be silently recreated without its ACL.
    match fs::symlink_metadata(staging) {
        Ok(stats) if !stats.file_type().is_symlink() && stats.is_dir() => {}
        _ => return Err(STAGING_UNAVAILABLE.to_owned()),
    }
    let folder = tempfile::Builder::new()
        .prefix(&format!("share-{bot_id}-"))
        .tempdir_in(staging)
        .map_err(io_message)?
        .keep();
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = folder.join(&name);
    let copied = (|| -> Result<u64, String> {
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_message)?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let count = source.read(&mut buffer).map_err(io_message)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > MAX_SHARE_BYTES {
                return Err(TOO_BIG.to_owned());
            }
            destination
                .write_all(&buffer[..count])
                .map_err(io_message)?;
        }
        let finished = source.metadata().map_err(io_message)?;
        if size != opened.len()
            || finished.len() != opened.len()
            || finished.modified().ok() != opened.modified().ok()
        {
            return Err(CHANGED.to_owned());
        }
        Ok(size)
    })();
    match copied {
        Ok(size) => Ok(FileData {
            path: path.to_string_lossy().into_owned(),
            name,
            size,
            attachment_id: None,
            content_type: None,
            companions: None,
        }),
        Err(error) => {
            let _ = fs::remove_dir_all(&folder);
            Err(error)
        }
    }
}

/// `#snapshotFolder`: the `share-…` folder directly under `staging` that holds `file`, if it is one.
pub fn snapshot_folder(staging: &Path, file: &Path) -> Option<PathBuf> {
    let folder = file.parent()?;
    let named_like_a_snapshot = folder
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("share-"));
    (folder.parent() == Some(staging) && named_like_a_snapshot).then(|| folder.to_path_buf())
}

/// What `Team.start` does: removes `share-…` and `cleanup-…` folders a crash left, once they are a day old.
pub fn sweep_stale(staging: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(staging) else {
        return;
    };
    let cutoff = now.checked_sub(SNAPSHOT_MAX_AGE);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_snapshot = name.starts_with("share-") || name.starts_with("cleanup-");
        if !is_snapshot || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let folder = entry.path();
        let stale = match (fs::symlink_metadata(&folder).and_then(|stats| stats.modified()), cutoff) {
            (Ok(modified), Some(cutoff)) => modified < cutoff,
            _ => false,
        };
        if stale {
            let _ = fs::remove_dir_all(&folder);
        }
    }
}

/// `firstLine`: the first line of `text`, trimmed, cut to `max` UTF-16 units.
pub fn first_line(text: &str, max: usize) -> String {
    let trimmed = js::trim(text);
    let line = trimmed.split('\n').next().unwrap_or("");
    let line = line.strip_suffix('\r').unwrap_or(line);
    js::slice(line, max).to_owned()
}
