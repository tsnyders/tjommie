//! Where a download goes on this PC (`attachment-save.ts` for the desktop): the model's `download_attachment` writes
//! through `AttachmentNative`. The destination is picked before the download starts (GPUI's save dialog lives on the
//! UI thread, and these futures must be `Send`), so the save starts at once. Bytes go to `<name>.partial` beside it, on
//! Tokio's blocking pool, and become the file only when every chunk is in: a cancelled or failed download leaves
//! nothing behind.

use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use protocol::attachments::AttachmentInfo;

use crate::model::attachment_transfer::{
    AttachmentNative, NativePreview, SaveStatus, TransferError,
};

/// A download's destination, picked by the person.
pub struct SaveTo {
    path: PathBuf,
}

impl SaveTo {
    pub fn new(path: PathBuf) -> Self {
        SaveTo { path }
    }

    fn partial(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(".partial");
        self.path.with_file_name(name)
    }
}

fn failed(error: impl std::fmt::Display) -> TransferError {
    TransferError::Failed(format!("The file could not be saved: {error}"))
}

/// Runs file work on Tokio's blocking pool (the download is polled with the live runtime entered).
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, TransferError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(failed(error)),
        Err(error) => Err(failed(error)),
    }
}

fn write_at(partial: &Path, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).open(partial)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(bytes)
}

impl AttachmentNative for SaveTo {
    fn start_save(
        &self,
        _info: AttachmentInfo,
    ) -> impl Future<Output = Result<Option<String>, TransferError>> + Send {
        std::future::ready(Ok(Some(self.path.to_string_lossy().into_owned())))
    }

    fn activate_save(
        &self,
        _save_id: &str,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        let partial = self.partial();
        blocking(move || std::fs::File::create(partial).map(|_| ()))
    }

    fn append_save(
        &self,
        _save_id: &str,
        offset: u64,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        let partial = self.partial();
        blocking(move || write_at(&partial, offset, &bytes))
    }

    fn finish_save(
        &self,
        _save_id: &str,
    ) -> impl Future<Output = Result<SaveStatus, TransferError>> + Send {
        let (partial, path) = (self.partial(), self.path.clone());
        async move {
            blocking(move || {
                // The save dialog asked before replacing a file.
                if path.exists() {
                    std::fs::remove_file(&path)?;
                }
                std::fs::rename(&partial, &path)
            })
            .await?;
            Ok(SaveStatus::Saved)
        }
    }

    fn cancel_save(
        &self,
        _save_id: &str,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        let partial = self.partial();
        blocking(move || match std::fs::remove_file(partial) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
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
    use super::*;

    #[tokio::test]
    async fn writes_the_partial_then_publishes_it_whole_and_a_cancel_leaves_nothing() {
        // A folder of this test's own under the system's temp folder.
        let dir = std::env::temp_dir().join(format!("tjommie-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.txt");
        let save = SaveTo::new(path.clone());
        save.activate_save("s").await.unwrap();
        save.append_save("s", 0, b"Hello, ".to_vec()).await.unwrap();
        save.append_save("s", 7, b"world".to_vec()).await.unwrap();
        assert!(
            !path.exists(),
            "nothing at the destination until it's whole"
        );
        assert_eq!(save.finish_save("s").await.unwrap(), SaveStatus::Saved);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Hello, world");
        save.cancel_save("s").await.unwrap();
        assert!(!save.partial().exists());

        let other = SaveTo::new(dir.join("half.bin"));
        other.activate_save("t").await.unwrap();
        other.append_save("t", 0, vec![1; 10]).await.unwrap();
        other.cancel_save("t").await.unwrap();
        assert!(!other.partial().exists() && !dir.join("half.bin").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
