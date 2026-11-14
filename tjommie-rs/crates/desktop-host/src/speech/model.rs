//! The English speech model, `ggml-base.en.bin`: downloaded once from a pinned URL, published only if its size and
//! SHA-256 are exactly right, and revalidated whenever it is asked for. Port of `speech-model.ts`.
//!
//! Production keeps Electron's folder (`%APPDATA%\Tjommie\speech\models\<sha256>.bin`), so there is no second download
//! and a rollback to R1 still finds it; dev builds use `<dev root>\speech\models`. Nothing is published unless it is
//! whole: the download goes to a `.partial` file that is renamed into place after it verifies, and a cancelled or
//! failed download leaves nothing behind.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use protocol::speech::{SpeechModel as ModelInfo, SpeechModelState};
use tokio::sync::{OnceCell, watch};
use url::Url;

use super::sha256_file;
use crate::updater::{Http, is_model_host};

/// A model's size, SHA-256 and the URL it comes from.
#[derive(Debug, Clone)]
pub struct ModelSource {
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
}

/// The pinned model: `ggml-base.en.bin` at an exact Hugging Face commit.
pub fn pinned() -> ModelSource {
    ModelSource {
        bytes: 147_964_211,
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002".into(),
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin".into(),
    }
}

/// The pinned model's facts, for a build to cross-check.
pub const MODEL: (u64, &str) = (
    147_964_211,
    "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
);

const CLEANUP_FAILED: &str =
    "Temporary speech model data could not be cleared. Check storage and restart the app.";

struct State {
    state: SpeechModelState,
    received: u64,
    checked: bool,
    cleanup_failed: bool,
}

struct Download {
    cancel: Arc<watch::Sender<bool>>,
    finished: watch::Receiver<bool>,
}

struct Inner {
    directory: PathBuf,
    source: ModelSource,
    path: PathBuf,
    http: Arc<dyn Http>,
    changed: Arc<dyn Fn() + Send + Sync>,
    state: Mutex<State>,
    initialized: OnceCell<bool>,
    verification: tokio::sync::Mutex<()>,
    download: Mutex<Option<Download>>,
    /// A test can make removing a partial file fail, as a locked file would.
    #[cfg(test)]
    fail_partial_cleanup: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
pub struct SpeechModel {
    inner: Arc<Inner>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `.<sha>.<uuid>.partial`: the only files this model ever makes and removes besides the model itself.
fn is_owned_partial(name: &str, sha256: &str) -> bool {
    let prefix = format!(".{sha256}.");
    name.strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(".partial"))
        .is_some_and(|id| {
            id.len() == 36
                && id
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
        })
}

fn free_bytes(directory: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    let wide: Vec<u16> = directory
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path and one output integer, both alive for the call.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }
        .ok()
        .map(|()| free)
}

impl SpeechModel {
    /// `directory` holds the model (`<sha256>.bin`) and nothing else of ours. `changed` is told after every change of
    /// state or progress, so a screen can ask `status()` again.
    pub fn new(
        directory: PathBuf,
        source: ModelSource,
        http: Arc<dyn Http>,
        changed: Arc<dyn Fn() + Send + Sync>,
    ) -> SpeechModel {
        let path = directory.join(format!("{}.bin", source.sha256));
        let state = State {
            state: SpeechModelState::Missing,
            received: 0,
            checked: false,
            cleanup_failed: false,
        };
        SpeechModel {
            inner: Arc::new(Inner {
                directory,
                source,
                path,
                http,
                changed,
                state: Mutex::new(state),
                initialized: OnceCell::new(),
                verification: tokio::sync::Mutex::new(()),
                download: Mutex::new(None),
                #[cfg(test)]
                fail_partial_cleanup: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    fn changed(&self) {
        (self.inner.changed)();
    }

    fn downloading(&self) -> bool {
        lock(&self.inner.download).is_some()
    }

    fn set(&self, state: SpeechModelState, received: u64) {
        let mut current = lock(&self.inner.state);
        current.state = state;
        current.received = received;
    }

    fn snapshot(&self) -> ModelInfo {
        let state = lock(&self.inner.state);
        let (kind, received) = if state.cleanup_failed {
            (SpeechModelState::Error, 0)
        } else {
            (state.state, state.received)
        };
        ModelInfo {
            state: kind,
            bytes: self.inner.source.bytes,
            received,
            confirmation: None,
        }
    }

    pub async fn status(&self) -> ModelInfo {
        let unchecked = !lock(&self.inner.state).checked;
        if unchecked && !self.downloading() {
            self.verify().await;
        }
        self.snapshot()
    }

    /// Makes the folder and removes stale partials from an interrupted run. Once.
    async fn initialize(&self) -> Result<(), String> {
        let ok = self
            .inner
            .initialized
            .get_or_init(|| async {
                let directory = self.inner.directory.clone();
                let sha = self.inner.source.sha256.clone();
                let cleaned = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                    std::fs::create_dir_all(&directory)?;
                    for entry in std::fs::read_dir(&directory)? {
                        let entry = entry?;
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if is_owned_partial(&name, &sha)
                            && std::fs::symlink_metadata(entry.path())?
                                .file_type()
                                .is_file()
                        {
                            std::fs::remove_file(entry.path())?;
                        }
                    }
                    Ok(())
                })
                .await;
                if matches!(cleaned, Ok(Ok(()))) {
                    return true;
                }
                {
                    let mut state = lock(&self.inner.state);
                    state.cleanup_failed = true;
                    state.checked = true;
                    state.state = SpeechModelState::Error;
                    state.received = 0;
                }
                false
            })
            .await;
        if *ok {
            Ok(())
        } else {
            Err(CLEANUP_FAILED.into())
        }
    }

    /// Whether the model on disk is whole. Revalidated on every call, not remembered.
    pub async fn verify(&self) -> bool {
        if lock(&self.inner.state).cleanup_failed || self.downloading() {
            return false;
        }
        let _one_at_a_time = self.inner.verification.lock().await;
        if self.initialize().await.is_err() {
            return false;
        }
        let (path, bytes, sha) = (
            self.inner.path.clone(),
            self.inner.source.bytes,
            self.inner.source.sha256.clone(),
        );
        let valid = tokio::task::spawn_blocking(move || {
            std::fs::symlink_metadata(&path)
                .is_ok_and(|meta| meta.file_type().is_file() && meta.len() == bytes)
                && sha256_file(&path).is_ok_and(|digest| digest == sha)
        })
        .await
        .unwrap_or(false);
        {
            let mut state = lock(&self.inner.state);
            state.checked = true;
            state.state = if valid {
                SpeechModelState::Ready
            } else {
                SpeechModelState::Missing
            };
            state.received = if valid { bytes } else { 0 };
        }
        valid
    }

    pub async fn download_model(&self) -> Result<(), String> {
        if lock(&self.inner.state).cleanup_failed {
            return Err(CLEANUP_FAILED.into());
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (finished_tx, finished_rx) = watch::channel(false);
        {
            let mut current = lock(&self.inner.download);
            if current.is_some() {
                return Err("A speech model download is already running.".into());
            }
            *current = Some(Download {
                cancel: Arc::new(cancel_tx),
                finished: finished_rx,
            });
        }
        let result = self.receive(cancel_rx).await;
        *lock(&self.inner.download) = None;
        let _ = finished_tx.send(true);
        result
    }

    /// Stops a download in progress and waits until its partial file is gone.
    pub async fn cancel_download(&self) -> Result<(), String> {
        let current = lock(&self.inner.download)
            .as_ref()
            .map(|download| (Arc::clone(&download.cancel), download.finished.clone()));
        if let Some((cancel, mut finished)) = current {
            let _ = cancel.send(true);
            let _ = finished.wait_for(|done| *done).await;
        }
        if lock(&self.inner.state).cleanup_failed {
            return Err("Speech model cleanup failed. Check storage and restart the app.".into());
        }
        Ok(())
    }

    /// Deletes the model file, and only that file.
    pub async fn remove(&self) -> Result<(), String> {
        self.cancel_download().await?;
        let _one_at_a_time = self.inner.verification.lock().await;
        self.initialize().await?;
        let path = self.inner.path.clone();
        let removed = tokio::fs::remove_file(&path).await;
        if removed
            .as_ref()
            .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
        {
            return Err("The downloaded speech model could not be removed. Check storage permissions and try again.".into());
        }
        {
            let mut state = lock(&self.inner.state);
            state.checked = true;
            state.state = SpeechModelState::Missing;
            state.received = 0;
        }
        self.changed();
        Ok(())
    }

    async fn receive(&self, mut cancel: watch::Receiver<bool>) -> Result<(), String> {
        let partial = self.inner.directory.join(format!(
            ".{}.{}.partial",
            self.inner.source.sha256,
            uuid::Uuid::new_v4()
        ));
        let mut published = false;
        let outcome = self
            .try_receive(&mut cancel, &partial, &mut published)
            .await;
        let cancelled = *cancel.borrow();
        let mut result = Ok(());
        if outcome.is_err() {
            let mut cleanup_failed = false;
            if published && cancelled && tokio::fs::remove_file(&self.inner.path).await.is_err() {
                cleanup_failed = true;
            }
            {
                let mut state = lock(&self.inner.state);
                state.cleanup_failed |= cleanup_failed;
                state.checked = true;
                state.state = if cancelled && !cleanup_failed {
                    SpeechModelState::Missing
                } else {
                    SpeechModelState::Error
                };
                state.received = 0;
            }
            self.changed();
            result = Err(if cleanup_failed {
                "Cancelled speech model data could not be removed. Check storage and restart the app.".into()
            } else if cancelled {
                "Speech model download cancelled.".into()
            } else {
                "Could not download and verify the speech model. Check storage and network, then try again.".into()
            });
        }
        match self.remove_partial(&partial).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                {
                    let mut state = lock(&self.inner.state);
                    state.cleanup_failed = true;
                    state.state = SpeechModelState::Error;
                    state.received = 0;
                }
                self.changed();
                return Err(CLEANUP_FAILED.into());
            }
        }
        result
    }

    async fn remove_partial(&self, partial: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        {
            if self
                .inner
                .fail_partial_cleanup
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(std::io::Error::other("blocked for a test"));
            }
        }
        tokio::fs::remove_file(partial).await
    }

    async fn try_receive(
        &self,
        cancel: &mut watch::Receiver<bool>,
        partial: &Path,
        published: &mut bool,
    ) -> Result<(), ()> {
        let source = &self.inner.source;
        self.initialize().await.map_err(drop)?;
        if free_bytes(&self.inner.directory).is_some_and(|free| free < source.bytes + 1_048_576) {
            return Err(()); // not enough free storage
        }
        if *cancel.borrow() {
            return Err(());
        }
        self.set(SpeechModelState::Downloading, 0);
        self.changed();
        // Only Hugging Face, over HTTPS; the real client re-checks every redirect hop against the same list.
        let url = Url::parse(&source.url)
            .ok()
            .filter(is_model_host)
            .ok_or(())?;
        let last = std::sync::atomic::AtomicU64::new(0);
        let model = self.clone();
        let total = source.bytes;
        let report = move |received: u64, _total: Option<u64>| {
            lock(&model.inner.state).received = received.min(total);
            let percent = received.saturating_mul(100) / total.max(1);
            if last.swap(percent, std::sync::atomic::Ordering::Relaxed) != percent {
                model.changed();
            }
        };
        let download = self
            .inner
            .http
            .download(&url, partial, source.bytes, &report);
        tokio::select! {
            biased;
            () = super::cancelled(cancel) => return Err(()),
            result = download => result.map_err(drop)?,
        }
        self.set(SpeechModelState::Verifying, source.bytes);
        self.changed();
        let (path, bytes, sha) = (partial.to_path_buf(), source.bytes, source.sha256.clone());
        let whole = tokio::task::spawn_blocking(move || {
            std::fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes)
                && sha256_file(&path).is_ok_and(|digest| digest == sha)
        })
        .await
        .unwrap_or(false);
        if !whole || *cancel.borrow() {
            return Err(());
        }
        tokio::fs::rename(partial, &self.inner.path)
            .await
            .map_err(drop)?;
        *published = true;
        if *cancel.borrow() {
            return Err(());
        }
        {
            let mut state = lock(&self.inner.state);
            state.checked = true;
            state.state = SpeechModelState::Ready;
            state.received = source.bytes;
        }
        self.changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::FutureExt as _;
    use futures_util::future::BoxFuture;
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::speech::hex;
    use crate::updater::Progress;

    const BYTES: &[u8] = b"synthetic speech model";

    fn source() -> ModelSource {
        ModelSource {
            bytes: BYTES.len() as u64,
            sha256: hex(&Sha256::digest(BYTES)),
            url: "https://huggingface.co/synthetic/model".into(),
        }
    }

    enum Serve {
        Bytes(Vec<u8>),
        /// Writes two bytes, reports them, then never finishes (until the download is cancelled).
        Stall,
    }

    struct FakeHttp {
        serve: Serve,
        calls: AtomicUsize,
        progressed: tokio::sync::Notify,
    }

    impl Http for FakeHttp {
        fn get_text<'a>(&'a self, _: &'a Url, _: usize) -> BoxFuture<'a, Result<String, String>> {
            async { Err("no text".to_string()) }.boxed()
        }
        fn download<'a>(
            &'a self,
            _url: &'a Url,
            destination: &'a Path,
            _max: u64,
            progress: Progress<'a>,
        ) -> BoxFuture<'a, Result<(), String>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            async move {
                match &self.serve {
                    Serve::Bytes(bytes) => {
                        std::fs::write(destination, bytes).unwrap();
                        progress(bytes.len() as u64, Some(bytes.len() as u64));
                        Ok(())
                    }
                    Serve::Stall => {
                        std::fs::write(destination, &BYTES[..2]).unwrap();
                        progress(2, None);
                        self.progressed.notify_one();
                        std::future::pending().await
                    }
                }
            }
            .boxed()
        }
    }

    fn model(dir: &Path, serve: Serve, source: ModelSource) -> (SpeechModel, Arc<FakeHttp>) {
        let http = Arc::new(FakeHttp {
            serve,
            calls: AtomicUsize::new(0),
            progressed: tokio::sync::Notify::new(),
        });
        let model = SpeechModel::new(
            dir.to_path_buf(),
            source,
            Arc::clone(&http) as Arc<dyn Http>,
            Arc::new(|| {}),
        );
        (model, http)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // `speech-model.test.ts` "publishes only exact bytes/hash, revalidates an existing model, and removes only its file"
    #[tokio::test]
    async fn publishes_only_exact_bytes_revalidates_and_removes_only_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let (model, _) = model(dir.path(), Serve::Bytes(BYTES.to_vec()), source());
        assert_eq!(model.status().await.state, SpeechModelState::Missing);
        model.download_model().await.unwrap();
        assert_eq!(model.status().await.state, SpeechModelState::Ready);
        assert!(model.verify().await);
        std::fs::write(dir.path().join("unrelated.txt"), "keep").unwrap();
        // Revalidated on every ask: damage to the file is noticed.
        std::fs::write(model.path(), b"synthetic speech modeX").unwrap();
        assert!(!model.verify().await);
        assert_eq!(model.status().await.state, SpeechModelState::Missing);
        model.download_model().await.unwrap();
        model.remove().await.unwrap();
        assert_eq!(names(dir.path()), ["unrelated.txt"]);
        assert_eq!(model.status().await.state, SpeechModelState::Missing);
    }

    // "never publishes truncated / corrupt / oversized data"
    #[tokio::test]
    async fn never_publishes_truncated_corrupt_or_oversized_data() {
        let truncated = BYTES[1..].to_vec();
        let corrupt = vec![1u8; BYTES.len()];
        let oversized = [BYTES, BYTES].concat();
        for data in [truncated, corrupt, oversized] {
            let dir = tempfile::tempdir().unwrap();
            let (model, _) = model(dir.path(), Serve::Bytes(data), source());
            let error = model.download_model().await.unwrap_err();
            assert!(error.contains("verify"), "{error}");
            assert_eq!(model.status().await.state, SpeechModelState::Error);
            assert!(
                names(dir.path()).is_empty(),
                "nothing published, no partial left"
            );
        }
    }

    // "cancels a pending download and deletes the partial instead of publishing a late result"
    #[tokio::test]
    async fn cancels_a_pending_download_and_deletes_the_partial() {
        let dir = tempfile::tempdir().unwrap();
        let (model, http) = model(dir.path(), Serve::Stall, source());
        let downloading = tokio::spawn({
            let model = model.clone();
            async move { model.download_model().await }
        });
        http.progressed.notified().await;
        assert_eq!(model.status().await.state, SpeechModelState::Downloading);
        model.cancel_download().await.unwrap();
        let error = downloading.await.unwrap().unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert_eq!(model.status().await.state, SpeechModelState::Missing);
        assert!(names(dir.path()).is_empty());
        // A second download may start once the first is over.
        assert!(model.cancel_download().await.is_ok());
    }

    // "rejects redirects outside upstream HTTPS model hosts without requesting them": the address is checked before
    // anything is asked for (the real client checks every redirect hop against the same list).
    #[tokio::test]
    async fn refuses_an_address_that_is_not_an_upstream_https_model_host() {
        for bad in [
            "http://huggingface.co/model",
            "https://127.0.0.1/private",
            "https://huggingface.co.evil.example/model",
            "https://evilhuggingface.co/model",
            "https://user{AT}huggingface.co/model",
            "https://huggingface.co:8443/model",
        ] {
            // Credentials in the address: the at sign is put in here so no address-like text is in the source.
            let bad = bad.replace("{AT}", "@");
            let dir = tempfile::tempdir().unwrap();
            let (model, http) = model(
                dir.path(),
                Serve::Bytes(BYTES.to_vec()),
                ModelSource {
                    url: bad.clone(),
                    ..source()
                },
            );
            assert!(model.download_model().await.is_err(), "{bad}");
            assert_eq!(http.calls.load(Ordering::SeqCst), 0, "{bad} was requested");
            assert_eq!(model.status().await.state, SpeechModelState::Error);
        }
        for good in [
            "https://huggingface.co/a/b",
            "https://cdn-lfs.hf.co/repos/x",
            "https://cas-bridge.xethub.hf.co/y",
        ] {
            assert!(is_model_host(&Url::parse(good).unwrap()), "{good}");
        }
    }

    // `speech-model-cleanup.test.ts` "keeps repeated model cancellation failed until restart verifies the retained partial
    // was removed"
    #[tokio::test]
    async fn keeps_repeated_model_cancellation_failed_until_restart_verifies_the_partial_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        // The right number of wrong bytes: the download fails its check, and then removing its partial file is blocked.
        let (blocked, _) = model(dir.path(), Serve::Bytes(vec![0; BYTES.len()]), source());
        blocked
            .inner
            .fail_partial_cleanup
            .store(true, Ordering::SeqCst);
        let error = blocked.download_model().await.unwrap_err();
        assert!(error.contains("could not be cleared"), "{error}");
        assert_eq!(names(dir.path()).len(), 1, "the partial is still there");
        for _ in 0..2 {
            let error = blocked.cancel_download().await.unwrap_err();
            assert!(error.contains("cleanup failed"), "{error}");
        }
        assert!(!blocked.verify().await);
        assert_eq!(blocked.status().await.state, SpeechModelState::Error);
        assert!(
            blocked.download_model().await.is_err(),
            "and no new download starts"
        );
        // Even once removing it would work, this run stays failed: only a restart can say the partial is gone.
        blocked
            .inner
            .fail_partial_cleanup
            .store(false, Ordering::SeqCst);
        assert!(
            blocked
                .cancel_download()
                .await
                .unwrap_err()
                .contains("cleanup failed")
        );
        let (restarted, _) = model(dir.path(), Serve::Bytes(vec![]), source());
        assert_eq!(restarted.status().await.state, SpeechModelState::Missing);
        restarted.cancel_download().await.unwrap();
        assert!(names(dir.path()).is_empty());
    }

    // "cleans only stale owned partial model files after an interrupted app run"
    #[tokio::test]
    async fn cleans_only_stale_owned_partial_files_after_an_interrupted_run() {
        let dir = tempfile::tempdir().unwrap();
        let source = source();
        let stale = format!(
            ".{}.00000000-0000-0000-0000-000000000000.partial",
            source.sha256
        );
        std::fs::write(dir.path().join(stale), "incomplete synthetic data").unwrap();
        std::fs::write(dir.path().join("unrelated.partial"), "keep").unwrap();
        std::fs::write(
            dir.path()
                .join(format!(".{}.not-a-uuid.partial", source.sha256)),
            "keep",
        )
        .unwrap();
        let (model, _) = model(dir.path(), Serve::Bytes(vec![]), source.clone());
        assert_eq!(model.status().await.state, SpeechModelState::Missing);
        assert_eq!(
            names(dir.path()),
            [
                format!(".{}.not-a-uuid.partial", source.sha256),
                "unrelated.partial".to_string()
            ]
        );
    }

    #[test]
    fn the_pinned_model_is_the_one_electron_downloads() {
        let pinned = pinned();
        assert_eq!((pinned.bytes, pinned.sha256.as_str()), MODEL);
        assert_eq!(pinned.bytes, 147_964_211);
        assert!(is_model_host(&Url::parse(&pinned.url).unwrap()));
        assert!(pinned.url.ends_with("/ggml-base.en.bin"));
    }
}
