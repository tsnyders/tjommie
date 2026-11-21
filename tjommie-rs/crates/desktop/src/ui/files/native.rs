//! Where a download goes on this PC (`attachment-save.ts` for the desktop): the model's `download_attachment` writes
//! through `AttachmentNative`. The destination is picked before the download starts (GPUI's save dialog lives on the
//! UI thread, and these futures must be `Send`), so the save starts at once. Bytes go to a partial file of the save's own
//! beside it, `.<name>.<random>.part`, on Tokio's blocking pool, and are hashed as they go. The partial becomes the file
//! only when it holds the size and SHA-256 the daemon gave: a cancelled, failed or corrupt download leaves nothing behind.

use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher as _, RandomState};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use desktop_host::sha2::{Digest, Sha256};
use protocol::attachments::AttachmentInfo;

use crate::model::attachment_transfer::{
    AttachmentNative, NativePreview, SaveStatus, TransferError,
};

/// A download's destination, picked by the person.
pub struct SaveTo {
    path: PathBuf,
    /// The save's progress, shared with its steps on the blocking pool.
    state: Arc<Mutex<Progress>>,
}

/// What the save has done so far.
#[derive(Default)]
struct Progress {
    /// The size and SHA-256 (lowercase hex, as the daemon gives it) the file must have, from `start_save`.
    expected: Option<(u64, String)>,
    stage: Stage,
    /// The partial file, from activation until it is published or removed. A removal that failed keeps it, for the next
    /// cancel to try again.
    partial: Option<PathBuf>,
}

/// Where the save is.
#[derive(Default)]
enum Stage {
    /// Not activated yet.
    #[default]
    Ready,
    /// Bytes are going into the partial file.
    Writing(Writing),
    /// Published, cancelled or failed: nothing more is written.
    Closed,
}

/// The open partial file, the hash of what it holds, and how many bytes that is.
struct Writing {
    file: File,
    hash: Sha256,
    written: u64,
}

impl Progress {
    /// Moves the partial file to `target`, replacing what is there (the save dialog asked), if it holds exactly the file
    /// the daemon described.
    fn publish(&mut self, target: &Path) -> Result<SaveStatus, TransferError> {
        let Stage::Writing(writing) = std::mem::replace(&mut self.stage, Stage::Closed) else {
            return Err(gone());
        };
        let whole = self.expected.as_ref().is_some_and(|(size, sha256)| {
            writing.written == *size && format!("{:x}", writing.hash.finalize()) == *sha256
        });
        if !whole {
            return Err(TransferError::Native {
                code: Some("corrupt".to_owned()),
                message: "The download failed its content check.".to_owned(),
            });
        }
        writing.file.sync_all().map_err(failed)?;
        drop(writing.file);
        let partial = self.partial.as_ref().ok_or_else(gone)?;
        std::fs::rename(partial, target).map_err(failed)?;
        self.partial = None;
        Ok(SaveStatus::Saved)
    }

    /// Ends the save without publishing it: the partial file is closed and removed. A removal that fails keeps the path,
    /// for the next cancel to try again.
    fn discard(&mut self) -> std::io::Result<()> {
        self.stage = Stage::Closed;
        if let Some(partial) = &self.partial
            && let Err(error) = std::fs::remove_file(partial)
            && error.kind() != ErrorKind::NotFound
        {
            return Err(error);
        }
        self.partial = None;
        Ok(())
    }
}

impl SaveTo {
    pub fn new(path: PathBuf) -> Self {
        SaveTo {
            path,
            state: Arc::default(),
        }
    }

    /// Runs one step of the save on Tokio's blocking pool (the download is polled with the live runtime entered). A step
    /// holds the progress while it works, so steps never overlap; one whose future was dropped (a deadline, a cancel)
    /// still runs, and finds the save as the steps before it left it.
    async fn step<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Progress) -> Result<T, TransferError> + Send + 'static,
    ) -> Result<T, TransferError> {
        let state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || {
            let mut progress = state.lock().unwrap_or_else(PoisonError::into_inner);
            work(&mut progress)
        })
        .await
        .unwrap_or_else(|error| Err(failed(error)))
    }
}

fn failed(error: impl std::fmt::Display) -> TransferError {
    TransferError::Failed(format!("The file could not be saved: {error}"))
}

/// The save has ended (published, cancelled or failed), or was never activated.
fn gone() -> TransferError {
    TransferError::Failed("This download is no longer available.".to_owned())
}

/// Makes the save's own partial file beside `target`: `.<name>.<random>.part`, with the name's first 64 characters.
/// `create_new` makes it a new file, never one already there nor a link's target; a name that is taken gets another
/// random part.
fn create_partial(target: &Path) -> std::io::Result<(PathBuf, File)> {
    let name: String = target
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .take(64)
        .collect();
    for _ in 0..8 {
        // A fresh `RandomState` has random keys, never the same twice in a process.
        let random = RandomState::new().hash_one((std::process::id(), SystemTime::now()));
        let path = target.with_file_name(format!(".{name}.{random:016x}.part"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            opened => return opened.map(|file| (path, file)),
        }
    }
    Err(ErrorKind::AlreadyExists.into())
}

/// The save dialog's suggested name for a shared file: the name without control characters, bidi controls (U+202E and
/// the like make a name read backwards), path separators or the other characters Windows refuses, without the trailing
/// dots and spaces Windows drops, and with a device name (CON, PRN, AUX, NUL, COM1, LPT1…, with or without an
/// extension) behind an underscore. Never empty: "download" stands in.
pub fn safe_file_name(name: &str) -> String {
    let bidi = |c: char| matches!(u32::from(c), 0x061C | 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069);
    let kept: String = name
        .chars()
        .filter(|&c| {
            !c.is_control()
                && !bidi(c)
                && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let name = kept.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return "download".to_owned();
    }
    let stem = name
        .split_once('.')
        .map_or(name, |(stem, _)| stem)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let port = |kind: &str| {
        let mut number = stem.strip_prefix(kind).unwrap_or_default().chars();
        matches!(
            (number.next(), number.next()),
            (Some('0'..='9' | '¹' | '²' | '³'), None)
        )
    };
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || port("COM") || port("LPT") {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

impl AttachmentNative for SaveTo {
    fn start_save(
        &self,
        info: AttachmentInfo,
    ) -> impl Future<Output = Result<Option<String>, TransferError>> + Send {
        let id = self.path.to_string_lossy().into_owned();
        self.step(move |progress| {
            progress.expected = Some((info.size, info.sha256));
            Ok(Some(id))
        })
    }

    fn activate_save(
        &self,
        _save_id: &str,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        let target = self.path.clone();
        self.step(move |progress| {
            // Once, and never after a cancel: an activation given up on may run after the cancel that followed it.
            if !matches!(progress.stage, Stage::Ready) {
                return Err(gone());
            }
            let (partial, file) = create_partial(&target).map_err(failed)?;
            progress.partial = Some(partial);
            progress.stage = Stage::Writing(Writing {
                file,
                hash: Sha256::new(),
                written: 0,
            });
            Ok(())
        })
    }

    fn append_save(
        &self,
        _save_id: &str,
        offset: u64,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        self.step(move |progress| {
            let size = progress.expected.as_ref().map_or(0, |(size, _)| *size);
            let Stage::Writing(writing) = &mut progress.stage else {
                return Err(gone());
            };
            // Right after what is written, and never past the size.
            let end = u64::try_from(bytes.len())
                .ok()
                .and_then(|length| offset.checked_add(length))
                .filter(|end| offset == writing.written && *end <= size);
            let Some(end) = end else {
                return Err(TransferError::Failed(
                    "The download chunk is out of sequence.".to_owned(),
                ));
            };
            if let Err(error) = writing.file.write_all(&bytes) {
                // Part of the chunk may be in the file: the save cannot go on.
                let _ = progress.discard();
                return Err(failed(error));
            }
            writing.hash.update(&bytes);
            writing.written = end;
            Ok(())
        })
    }

    fn finish_save(
        &self,
        _save_id: &str,
    ) -> impl Future<Output = Result<SaveStatus, TransferError>> + Send {
        let target = self.path.clone();
        self.step(move |progress| {
            let published = progress.publish(&target);
            if published.is_err() {
                // A removal that fails is kept for the model's cancel, which tries again and reports it.
                let _ = progress.discard();
            }
            published
        })
    }

    fn cancel_save(
        &self,
        _save_id: &str,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        self.step(|progress| {
            progress.discard().map_err(|error| TransferError::Native {
                code: Some("cleanup_failed".to_owned()),
                message: format!("The partial download could not be removed: {error}"),
            })
        })
    }

    fn preview(
        &self,
        _info: AttachmentInfo,
    ) -> impl Future<Output = Result<NativePreview, TransferError>> + Send {
        std::future::ready(Err(TransferError::Failed(
            "The desktop draws previews itself.".into(),
        )))
    }

    fn close_preview(
        &self,
        _preview_id: &str,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        std::future::ready(Ok(()))
    }

    fn respond(
        &self,
        _preview_id: &str,
        _request_id: &str,
        _bytes: Option<Vec<u8>>,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        std::future::ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use desktop_host::owner_safety::test_root;

    use super::*;

    /// What the daemon says of `bytes`.
    fn info(bytes: &[u8]) -> AttachmentInfo {
        AttachmentInfo {
            id: "41f8bac7-358c-48cf-9f48-a3a358804da6".to_owned(),
            host_id: "host".to_owned(),
            name: "file.bin".to_owned(),
            size: bytes.len() as u64,
            content_type: "application/octet-stream".to_owned(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }

    /// A save to `target` of the file `info` describes, activated and ready for bytes.
    async fn ready(target: &Path, info: AttachmentInfo) -> SaveTo {
        let save = SaveTo::new(target.to_path_buf());
        save.start_save(info).await.unwrap();
        save.activate_save("s").await.unwrap();
        save
    }

    /// The names in `folder`, sorted.
    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The code of a native failure.
    fn code<T: std::fmt::Debug>(result: Result<T, TransferError>) -> String {
        match result {
            Err(TransferError::Native {
                code: Some(code), ..
            }) => code,
            other => panic!("not a native failure: {other:?}"),
        }
    }

    /// Whether `result` failed with these words.
    fn refused(result: Result<(), TransferError>, words: &str) -> bool {
        result.is_err_and(|error| error.to_string().contains(words))
    }

    /// Appends `bytes` at `offset`.
    async fn put(save: &SaveTo, offset: u64, bytes: &[u8]) -> Result<(), TransferError> {
        save.append_save("s", offset, bytes.to_vec()).await
    }

    /// Legacy "renames only after count/digest verification and preserves a destination on cancellation/corruption".
    #[tokio::test]
    async fn publishes_only_a_verified_file_and_keeps_the_old_one_on_a_cancel_or_failed_check() {
        let root = test_root().unwrap();
        let destination = root.path().join("download.bin");
        let bytes = b"verified bytes";
        std::fs::write(&destination, "existing").unwrap();

        let first = ready(&destination, info(bytes)).await;
        put(&first, 0, b"verified ").await.unwrap();
        put(&first, 9, b"bytes").await.unwrap();
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "existing");
        assert_eq!(first.finish_save("s").await, Ok(SaveStatus::Saved));
        assert_eq!(std::fs::read(&destination).unwrap(), bytes);

        let cancelled = ready(&destination, info(bytes)).await;
        put(&cancelled, 0, b"some").await.unwrap();
        cancelled.cancel_save("s").await.unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), bytes);

        let wrong = AttachmentInfo {
            sha256: "0".repeat(64),
            ..info(bytes)
        };
        let corrupt = ready(&destination, wrong).await;
        put(&corrupt, 0, bytes).await.unwrap();
        assert_eq!(code(corrupt.finish_save("s").await), "corrupt");
        assert_eq!(std::fs::read(&destination).unwrap(), bytes);
        assert_eq!(names(root.path()), ["download.bin"]);
    }

    /// Legacy "rejects a late/wrong-owner/out-of-order/malformed write and never commits an incomplete save". There is
    /// no owner here (one `SaveTo` per download), and the model decodes and checks each chunk before it comes.
    #[tokio::test]
    async fn refuses_an_out_of_order_or_late_write_and_never_publishes_an_incomplete_file() {
        let root = test_root().unwrap();
        let destination = root.path().join("download.bin");
        let save = ready(&destination, info(b"bytes")).await;
        assert!(refused(put(&save, 1, b"bytes").await, "out of sequence"));
        put(&save, 0, b"by").await.unwrap();
        // A repeat, and a chunk past the size.
        assert!(refused(put(&save, 0, b"by").await, "out of sequence"));
        assert!(refused(
            put(&save, 2, b"tes and more").await,
            "out of sequence"
        ));
        assert_eq!(code(save.finish_save("s").await), "corrupt");
        assert!(!destination.exists());
        assert!(refused(put(&save, 2, b"tes").await, "no longer available"));
        assert!(names(root.path()).is_empty());
    }

    /// Legacy "bounds pending opens, releases on host abort, and accepts a verified empty file": the empty file. The
    /// bounds are the daemon's (two transfers) and the card's (one save), and the model cancels on an abort.
    #[tokio::test]
    async fn publishes_a_verified_empty_file() {
        let root = test_root().unwrap();
        let destination = root.path().join("empty");
        let save = ready(&destination, info(b"")).await;
        assert_eq!(save.finish_save("s").await, Ok(SaveStatus::Saved));
        assert_eq!(std::fs::read(&destination).unwrap(), b"");
    }

    /// Legacy "waits for pending native opens to remove their private partials on invalidation": a cancel racing the
    /// partial's creation, or coming after the step making it was given up on, leaves nothing, and nothing more is
    /// written.
    #[tokio::test]
    async fn a_cancel_racing_the_partials_creation_leaves_nothing() {
        let root = test_root().unwrap();
        let raced = SaveTo::new(root.path().join("cancelled"));
        raced.start_save(info(b"abc")).await.unwrap();
        let (_, cancelled) = tokio::join!(raced.activate_save("s"), raced.cancel_save("s"));
        cancelled.unwrap();
        assert!(put(&raced, 0, b"abc").await.is_err());
        assert!(names(root.path()).is_empty());

        // A deadline drops the activation's future once its step has begun: the step runs on.
        let dropped = SaveTo::new(root.path().join("cancelled"));
        dropped.start_save(info(b"abc")).await.unwrap();
        let _ = futures::FutureExt::now_or_never(dropped.activate_save("s"));
        dropped.cancel_save("s").await.unwrap();
        assert!(put(&dropped, 0, b"abc").await.is_err());
        assert!(names(root.path()).is_empty());
    }

    /// Legacy "retains failed cleanup for retry instead of claiming a later owner clear succeeded": a partial that can't
    /// be removed is reported, and each later cancel tries the same path again; nothing else is touched.
    #[tokio::test]
    async fn a_partial_that_cannot_be_removed_is_reported_and_retried_by_the_next_cancel() {
        let root = test_root().unwrap();
        let target = root.path().join("destination");
        let save = ready(&target, info(b"abc")).await;
        let listed = names(root.path());
        assert_eq!(listed.len(), 1, "{listed:?}");
        let partial = root.path().join(&listed[0]);
        let held = root.path().join(format!("{}.held", listed[0]));
        // The partial moves aside, and a folder with a file in it takes its name.
        std::fs::rename(&partial, &held).unwrap();
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join("synthetic-blocker"), "block").unwrap();
        assert_eq!(code(save.cancel_save("s").await), "cleanup_failed");
        assert_eq!(code(save.cancel_save("s").await), "cleanup_failed");
        std::fs::remove_dir_all(&partial).unwrap();
        std::fs::write(&partial, "retry the same path").unwrap();
        save.cancel_save("s").await.unwrap();
        assert!(!partial.exists());
        assert!(held.exists());
    }

    /// Review B2: two downloads saved under one name at once write a partial each, so neither publishes the other's
    /// bytes.
    #[tokio::test]
    async fn two_saves_to_one_name_at_once_write_their_own_partials() {
        let root = test_root().unwrap();
        let target = root.path().join("report.pdf");
        let (one, two) = (b"the first report", b"the second");
        let (first, second) = tokio::join!(ready(&target, info(one)), ready(&target, info(two)));
        let partials = names(root.path());
        assert_eq!(partials.len(), 2, "{partials:?}");
        assert!(
            partials
                .iter()
                .all(|name| name.starts_with(".report.pdf.") && name.ends_with(".part")),
            "{partials:?}"
        );
        put(&first, 0, one).await.unwrap();
        put(&second, 0, two).await.unwrap();
        assert!(
            !target.exists(),
            "nothing at the destination until it's whole"
        );
        assert_eq!(first.finish_save("s").await, Ok(SaveStatus::Saved));
        assert_eq!(std::fs::read(&target).unwrap(), one);
        assert_eq!(second.finish_save("s").await, Ok(SaveStatus::Saved));
        assert_eq!(std::fs::read(&target).unwrap(), two);
        assert_eq!(names(root.path()), ["report.pdf"]);
    }

    /// Review B2: a file whose size or digest is not the daemon's is never published, and its partial goes.
    #[tokio::test]
    async fn a_wrong_size_or_digest_publishes_nothing_and_removes_the_partial() {
        let root = test_root().unwrap();
        let target = root.path().join("report.pdf");
        let bytes = b"the whole file";
        let longer = AttachmentInfo {
            size: 15,
            ..info(bytes)
        };
        let other = AttachmentInfo {
            sha256: info(b"another file").sha256,
            ..info(bytes)
        };
        for expected in [longer, other] {
            let save = ready(&target, expected).await;
            put(&save, 0, bytes).await.unwrap();
            assert_eq!(names(root.path()).len(), 1);
            assert_eq!(code(save.finish_save("s").await), "corrupt");
            assert!(names(root.path()).is_empty());
            // The model's cancel after the failure has nothing left to do.
            save.cancel_save("s").await.unwrap();
        }
    }

    /// Review B-N1.
    #[test]
    fn a_suggested_name_loses_controls_separators_trailing_dots_and_device_names() {
        for (name, safe) in [
            ("report.pdf", "report.pdf"),
            ("invoice\u{202E}fdp.exe", "invoicefdp.exe"),
            ("a\u{0}b\tc\u{7F}d\u{2066}e\u{200F}f", "abcdef"),
            ("..\\up/and:out?<>|*\".txt", "..upandout.txt"),
            ("notes.txt. . ", "notes.txt"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("Nul.tar.gz", "_Nul.tar.gz"),
            ("aux .md", "_aux .md"),
            ("PRN.", "_PRN"),
            ("COM1", "_COM1"),
            ("lpt9.log", "_lpt9.log"),
            ("COM10.txt", "COM10.txt"),
            ("CONSOLE.txt", "CONSOLE.txt"),
            ("Résumé 2026.pdf", "Résumé 2026.pdf"),
            ("", "download"),
            (" .. ", "download"),
            ("\u{202E}", "download"),
        ] {
            assert_eq!(safe_file_name(name), safe, "{name:?}");
        }
    }
}
