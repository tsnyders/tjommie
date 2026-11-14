//! One dictation at a time (port of `speech-runtime.ts`): capture PCM chunks into a temporary WAV file, run
//! `whisper-cli.exe` over it when the user stops, hand back the text, and delete the audio. Every failure path deletes
//! the audio too; if even that fails, dictation stays off ("cleanup_failed") until the app restarts, and no text is
//! returned from a recording whose audio could not be removed.
//!
//! Events and error messages never carry a path or a system error: the audio and the transcript are private.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::BoxFuture;
use protocol::speech::{
    SPEECH_LIMITS, SpeechCapture, SpeechEngine, SpeechEvent, SpeechLanguage, SpeechModelState,
    SpeechPermission, SpeechPhase, SpeechProblem, SpeechStart, SpeechStatus,
};
use serde::Deserialize;
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};
use tokio::sync::{OnceCell, watch};

use super::model::SpeechModel;
use super::{permission, sha256_file};
use crate::spawn;

const CLEANUP_FAILED: &str =
    "Temporary local speech data could not be cleared. Check storage and restart the app.";
const NOT_ACTIVE: &str = "This recording is no longer active.";

/// A chunk of 16 kHz mono signed 16-bit little-endian PCM.
#[derive(Debug, Clone)]
pub struct PcmChunk {
    pub session_id: String,
    pub sequence: u32,
    pub pcm: Vec<u8>,
}

/// The 44-byte RIFF header of a 16 kHz mono 16-bit PCM WAV holding `bytes` bytes of samples (`speech-runtime.ts:38-41`).
pub fn wav_header(bytes: u32) -> [u8; 44] {
    let rate = SPEECH_LIMITS.sample_rate;
    let mut header = [0u8; 44];
    let mut put = |at: usize, data: &[u8]| {
        if let Some(slot) = header.get_mut(at..at + data.len()) {
            slot.copy_from_slice(data);
        }
    };
    put(0, b"RIFF");
    put(4, &(36 + bytes).to_le_bytes());
    put(8, b"WAVEfmt ");
    put(16, &16u32.to_le_bytes());
    put(20, &1u16.to_le_bytes()); // PCM
    put(22, &1u16.to_le_bytes()); // mono
    put(24, &rate.to_le_bytes());
    put(28, &(rate * 2).to_le_bytes()); // bytes per second
    put(32, &2u16.to_le_bytes()); // block align
    put(34, &16u16.to_le_bytes()); // bits per sample
    put(36, b"data");
    put(40, &bytes.to_le_bytes());
    header
}

#[derive(Deserialize)]
struct Transcript {
    transcription: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    text: String,
}

/// The text of whisper's JSON output, or why there is none (`speechText`). A result over its size limit, malformed,
/// empty, or just a "no speech" marker is an error.
pub fn speech_text(json: &str) -> Result<String, String> {
    let limit = SPEECH_LIMITS.result_bytes;
    if json.len() > limit {
        return Err("The speech result exceeded its limit.".into());
    }
    let transcript: Transcript = serde_json::from_str(json)
        .map_err(|_| "The speech result was not understood.".to_string())?;
    if transcript.transcription.len() > 4096
        || transcript
            .transcription
            .iter()
            .any(|part| part.text.chars().count() > SPEECH_LIMITS.text_characters)
    {
        return Err("The speech result exceeded its limit.".into());
    }
    let joined: String = transcript
        .transcription
        .iter()
        .map(|part| part.text.as_str())
        .collect();
    let text = joined.trim();
    let no_speech = {
        let inner = text
            .strip_prefix(['[', '('])
            .and_then(|rest| rest.strip_suffix([']', ')']));
        inner.is_some_and(|word| {
            ["blank_audio", "silence", "music"]
                .iter()
                .any(|marker| word.eq_ignore_ascii_case(marker))
        })
    };
    if text.is_empty() || text.chars().count() > SPEECH_LIMITS.text_characters || no_speech {
        return Err("No speech was detected. Try again in a quieter place.".into());
    }
    Ok(text.to_string())
}

/// Runs `whisper-cli.exe`.
pub trait SpeechProcess: Send + Sync {
    /// Runs `executable` with `args` and waits for it. A true on `cancel` kills it and is an error.
    fn run<'a>(
        &'a self,
        executable: &'a Path,
        args: &'a [String],
        cancel: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// The real one, through `spawn::command` so no console window opens.
pub struct WhisperProcess;

impl SpeechProcess for WhisperProcess {
    fn run<'a>(
        &'a self,
        executable: &'a Path,
        args: &'a [String],
        mut cancel: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if *cancel.borrow() {
                return Err("Local transcription cancelled.".into());
            }
            let mut command = spawn::command(executable);
            command.args(args);
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let std_command: std::process::Command = command.into();
            let mut command = tokio::process::Command::from(std_command);
            command.kill_on_drop(true);
            let mut child = command
                .spawn()
                .map_err(|_| "The local speech runtime could not start.".to_string())?;
            tokio::select! {
                biased;
                () = super::cancelled(&mut cancel) => {
                    let _ = child.kill().await;
                    Err("Local transcription cancelled.".into())
                }
                status = child.wait() => match status {
                    Ok(status) if status.success() => Ok(()),
                    _ => Err("Local transcription failed.".into()),
                },
            }
        })
    }
}

pub struct RuntimeOptions {
    pub runtime_path: PathBuf,
    /// The runtime's expected SHA-256 (from the verified resource manifest); `None` means dictation is unavailable.
    pub runtime_sha256: Option<String>,
    /// `%APPDATA%\Tjommie\speech` for a real install, `<dev root>\speech` for dev.
    pub data_dir: PathBuf,
    pub emit: Arc<dyn Fn(SpeechEvent) + Send + Sync>,
    pub model: SpeechModel,
    pub process: Arc<dyn SpeechProcess>,
    pub permission: Arc<dyn Fn() -> SpeechPermission + Send + Sync>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Listening,
    Transcribing,
}

#[derive(Default)]
struct Io {
    file: Option<tokio::fs::File>,
    sequence: u32,
    nonzero: bool,
}

struct Session {
    id: String,
    cancel: watch::Sender<bool>,
    phase: Mutex<Phase>,
    io: tokio::sync::Mutex<Io>,
    bytes: AtomicU64,
    directory: Mutex<Option<PathBuf>>,
    /// True while no `start` or `stop` of this session is running; `cancel` waits for it.
    idle: watch::Sender<bool>,
    timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
    disposed: OnceCell<bool>,
}

/// Marks a session busy until dropped.
struct Busy(Arc<Session>);

impl Busy {
    fn begin(session: &Arc<Session>) -> Busy {
        session.idle.send_replace(false);
        Busy(Arc::clone(session))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.idle.send_replace(true);
    }
}

enum Appended {
    Written,
    OutOfLimits,
    WriteFailed,
}

struct Cancelling {
    id: String,
    done: watch::Receiver<bool>,
}

#[derive(Default)]
struct State {
    active: Option<Arc<Session>>,
    cancellation: Option<Cancelling>,
    cleanup_failed: bool,
    fingerprint: Option<String>,
    verified: bool,
}

struct Inner {
    options: RuntimeOptions,
    sessions: PathBuf,
    state: Mutex<State>,
    initialized: OnceCell<bool>,
    /// A test can make removing a session folder fail, as a locked file would.
    #[cfg(test)]
    fail_cleanup: AtomicBool,
    /// A test can shorten the 60 s recording limit; 0 keeps it.
    #[cfg(test)]
    recording_limit_ms: AtomicU64,
}

#[derive(Clone)]
pub struct SpeechRuntime {
    inner: Arc<Inner>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn session_id_ok(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_session_folder(name: &str) -> bool {
    name.strip_prefix("session-")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn problem(code: &str, message: &str) -> SpeechProblem {
    SpeechProblem {
        code: code.into(),
        message: message.into(),
    }
}

impl SpeechRuntime {
    pub fn new(options: RuntimeOptions) -> SpeechRuntime {
        let sessions = options.data_dir.join("sessions");
        SpeechRuntime {
            inner: Arc::new(Inner {
                options,
                sessions,
                state: Mutex::new(State::default()),
                initialized: OnceCell::new(),
                #[cfg(test)]
                fail_cleanup: AtomicBool::new(false),
                #[cfg(test)]
                recording_limit_ms: AtomicU64::new(0),
            }),
        }
    }

    pub fn model(&self) -> &SpeechModel {
        &self.inner.options.model
    }

    /// How long a recording may run before the runtime ends it and transcribes what it has.
    fn recording_limit(&self) -> Duration {
        #[cfg(test)]
        {
            let ms = self.inner.recording_limit_ms.load(Ordering::SeqCst);
            if ms > 0 {
                return Duration::from_millis(ms);
            }
        }
        Duration::from_secs(u64::from(SPEECH_LIMITS.max_seconds))
    }

    fn emit(&self, event: SpeechEvent) {
        (self.inner.options.emit)(event);
    }

    fn session_event(&self, id: &str, phase: SpeechPhase, problem: Option<SpeechProblem>) {
        self.emit(SpeechEvent::Session {
            session_id: id.into(),
            phase,
            problem,
        });
    }

    fn cleanup_failed(&self) -> bool {
        lock(&self.inner.state).cleanup_failed
    }

    fn mark_cleanup_failed(&self) {
        lock(&self.inner.state).cleanup_failed = true;
    }

    async fn remove_session_directory(&self, path: &Path) -> Result<(), ()> {
        #[cfg(test)]
        if self.inner.fail_cleanup.load(Ordering::SeqCst) {
            return Err(());
        }
        let owned = path.parent() == Some(self.inner.sessions.as_path())
            && path
                .file_name()
                .is_some_and(|name| is_session_folder(&name.to_string_lossy()));
        if !owned {
            return Err(()); // refuses to remove a folder that isn't one of ours
        }
        match tokio::fs::remove_dir_all(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(()),
        }
    }

    /// Clears the recordings an earlier run left behind. Once.
    async fn initialize(&self) -> Result<(), String> {
        let ok = self
            .inner
            .initialized
            .get_or_init(|| async {
                let cleared = async {
                    tokio::fs::create_dir_all(&self.inner.sessions)
                        .await
                        .map_err(drop)?;
                    let mut entries = tokio::fs::read_dir(&self.inner.sessions)
                        .await
                        .map_err(drop)?;
                    while let Some(entry) = entries.next_entry().await.map_err(drop)? {
                        if !is_session_folder(&entry.file_name().to_string_lossy()) {
                            continue;
                        }
                        let kind = tokio::fs::symlink_metadata(entry.path())
                            .await
                            .map_err(drop)?
                            .file_type();
                        if kind.is_dir() && !kind.is_symlink() {
                            self.remove_session_directory(&entry.path()).await?;
                        }
                    }
                    Ok::<(), ()>(())
                }
                .await;
                if cleared.is_err() {
                    self.mark_cleanup_failed();
                }
                cleared.is_ok()
            })
            .await;
        if *ok {
            Ok(())
        } else {
            Err(CLEANUP_FAILED.into())
        }
    }

    /// Whether `whisper-cli.exe` is there, is the verified one and still is. `force` re-hashes it even if its
    /// size and times are unchanged.
    async fn runtime_ready(&self, force: bool) -> bool {
        let Some(expected) = self.inner.options.runtime_sha256.clone().filter(|sha| {
            sha.len() == 64 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }) else {
            return false;
        };
        let path = self.inner.options.runtime_path.clone();
        let runtime = self.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                return false;
            };
            if !meta.file_type().is_file() || meta.len() == 0 || meta.len() > 128 * 1024 * 1024 {
                return false;
            }
            let fingerprint = format!(
                "{}:{:?}:{:?}",
                meta.len(),
                meta.modified().ok(),
                meta.created().ok()
            );
            let state = lock(&runtime.inner.state);
            if !force && state.fingerprint.as_deref() == Some(fingerprint.as_str()) {
                return state.verified;
            }
            drop(state);
            let verified = sha256_file(&path).is_ok_and(|digest| digest == expected);
            let mut state = lock(&runtime.inner.state);
            state.verified = verified;
            state.fingerprint = Some(fingerprint);
            verified
        })
        .await
        .unwrap_or(false)
    }

    pub async fn status(&self) -> SpeechStatus {
        let (runtime, model) =
            tokio::join!(self.runtime_ready(false), self.inner.options.model.status());
        let cleanup_failed = self.cleanup_failed();
        let permission = (self.inner.options.permission)();
        let ready = model.state == SpeechModelState::Ready;
        let problem = if cleanup_failed {
            Some(problem(
                "cleanup_failed",
                "Temporary speech data could not be cleared. Check storage and restart the app before dictating again.",
            ))
        } else if !runtime {
            Some(problem(
                "runtime_unavailable",
                "Local speech is not included in this app build, or its integrity check failed. Install a build with verified local speech support.",
            ))
        } else if model.state == SpeechModelState::Error {
            Some(problem(
                "model_error",
                "Local speech model storage or verification failed. Check storage and network, restart the app, then try again.",
            ))
        } else if !ready {
            Some(problem(
                "model_missing",
                "Download the verified English speech model to dictate locally.",
            ))
        } else if permission == SpeechPermission::Denied {
            Some(problem("microphone_blocked", permission::BLOCKED))
        } else {
            None
        };
        SpeechStatus {
            engine: SpeechEngine::WhisperCpp,
            capture: SpeechCapture::Pcm,
            available: runtime
                && ready
                && !cleanup_failed
                && permission != SpeechPermission::Denied,
            languages: vec![SpeechLanguage {
                tag: "en".into(),
                label: "English".into(),
            }],
            permission,
            model,
            problem,
        }
    }

    /// Listening, and not cancelled.
    pub fn is_capturing(&self) -> bool {
        lock(&self.inner.state)
            .active
            .as_ref()
            .is_some_and(|session| {
                *lock(&session.phase) == Phase::Listening && !*session.cancel.borrow()
            })
    }

    /// Capturing, and at least one chunk has arrived: what the window losing focus cancels.
    pub fn is_recording_audio(&self) -> bool {
        self.is_capturing()
            && lock(&self.inner.state)
                .active
                .as_ref()
                .is_some_and(|session| session.bytes.load(Ordering::SeqCst) > 0)
    }

    pub async fn download_model(&self) -> Result<(), String> {
        self.inner.options.model.download_model().await
    }

    pub async fn cancel_model_download(&self) -> Result<(), String> {
        self.inner.options.model.cancel_download().await
    }

    pub async fn remove_model(&self) -> Result<(), String> {
        self.cancel_all().await.ok();
        self.inner.options.model.remove().await
    }

    pub async fn start(&self, input: SpeechStart) -> Result<(), String> {
        if !session_id_ok(&input.session_id)
            || input.language.is_empty()
            || input.language.len() > 40
        {
            return Err("Invalid dictation request.".into());
        }
        let session = {
            let mut state = lock(&self.inner.state);
            if state.active.is_some() || state.cancellation.is_some() {
                return Err("Another dictation is still running or stopping.".into());
            }
            if state.cleanup_failed {
                return Err(CLEANUP_FAILED.into());
            }
            if input.language != "en" {
                return Err("This local speech model supports English only.".into());
            }
            let session = Arc::new(Session {
                id: input.session_id.clone(),
                cancel: watch::channel(false).0,
                phase: Mutex::new(Phase::Starting),
                io: tokio::sync::Mutex::new(Io::default()),
                bytes: AtomicU64::new(0),
                directory: Mutex::new(None),
                idle: watch::channel(true).0,
                timer: Mutex::new(None),
                disposed: OnceCell::new(),
            });
            state.active = Some(Arc::clone(&session));
            session
        };
        let busy = Busy::begin(&session);
        let prepared = self.prepare(&session).await;
        let outcome = match prepared {
            Ok(()) => Ok(()),
            Err(()) => {
                {
                    let mut state = lock(&self.inner.state);
                    if state
                        .active
                        .as_ref()
                        .is_some_and(|active| Arc::ptr_eq(active, &session))
                    {
                        state.active = None;
                    }
                }
                if self.dispose(&session).await.is_err() {
                    self.mark_cleanup_failed();
                }
                Err(if *session.cancel.borrow() {
                    "Dictation cancelled."
                } else {
                    "Could not start dictation. Check the verified local runtime, model and temporary storage."
                }
                .to_string())
            }
        };
        drop(busy);
        outcome
    }

    async fn prepare(&self, session: &Arc<Session>) -> Result<(), ()> {
        let mut cancel = session.cancel.subscribe();
        tokio::select! {
            biased;
            () = super::cancelled(&mut cancel) => Err(()),
            result = self.prepare_steps(session) => result,
        }
    }

    async fn prepare_steps(&self, session: &Arc<Session>) -> Result<(), ()> {
        self.initialize().await.map_err(drop)?;
        if !self.runtime_ready(true).await || !self.inner.options.model.verify().await {
            return Err(());
        }
        let directory = self
            .inner
            .sessions
            .join(format!("session-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::create_dir(&directory).await.map_err(drop)?;
        *lock(&session.directory) = Some(directory.clone());
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("input.wav"))
            .await
            .map_err(drop)?;
        file.write_all(&wav_header(0)).await.map_err(drop)?;
        session.io.lock().await.file = Some(file);
        *lock(&session.phase) = Phase::Listening;
        let runtime = self.clone();
        let id = session.id.clone();
        *lock(&session.timer) = Some(tokio::spawn(async move {
            tokio::time::sleep(runtime.recording_limit()).await;
            // Its own task: `stop` aborts this timer, which must not abort the transcription it starts.
            tokio::spawn(async move {
                let _ = runtime.stop(&id).await;
            });
        }));
        self.session_event(&session.id, SpeechPhase::Listening, None);
        Ok(())
    }

    fn active_listening(&self, id: &str) -> Option<Arc<Session>> {
        let state = lock(&self.inner.state);
        let session = state.active.as_ref().filter(|session| session.id == id)?;
        (*lock(&session.phase) == Phase::Listening && !*session.cancel.borrow())
            .then(|| Arc::clone(session))
    }

    pub async fn append_pcm(&self, chunk: PcmChunk) -> Result<(), String> {
        if !session_id_ok(&chunk.session_id)
            || chunk.pcm.is_empty()
            || chunk.pcm.len() > SPEECH_LIMITS.chunk_bytes
            || !chunk.pcm.len().is_multiple_of(2)
        {
            return Err("Invalid PCM chunk.".into());
        }
        let session = self.active_listening(&chunk.session_id).ok_or(NOT_ACTIVE)?;
        let outcome = {
            let mut io = session.io.lock().await;
            if io.file.is_none() {
                return Err(NOT_ACTIVE.into());
            }
            let so_far = session.bytes.load(Ordering::SeqCst);
            if chunk.sequence != io.sequence
                || so_far + chunk.pcm.len() as u64 > SPEECH_LIMITS.audio_bytes as u64
            {
                Appended::OutOfLimits
            } else {
                io.sequence += 1;
                session
                    .bytes
                    .fetch_add(chunk.pcm.len() as u64, Ordering::SeqCst);
                io.nonzero |= chunk.pcm.iter().any(|byte| *byte != 0);
                match io.file.as_mut() {
                    Some(file) => {
                        if file.write_all(&chunk.pcm).await.is_ok() {
                            Appended::Written
                        } else {
                            Appended::WriteFailed
                        }
                    }
                    None => Appended::WriteFailed,
                }
            }
        };
        match outcome {
            Appended::Written => Ok(()),
            Appended::WriteFailed => {
                let _ = self.cancel(&chunk.session_id).await;
                Err(
                    "Microphone data could not be saved temporarily. Check storage and try again."
                        .into(),
                )
            }
            Appended::OutOfLimits => {
                let _ = self.cancel(&chunk.session_id).await;
                Err("The recording exceeded its limits or arrived out of order.".into())
            }
        }
    }

    pub async fn stop(&self, session_id: &str) -> Result<(), String> {
        if !session_id_ok(session_id) {
            return Err("Invalid dictation request.".into());
        }
        let session = self.active_listening(session_id).ok_or(NOT_ACTIVE)?;
        if let Some(timer) = lock(&session.timer).take() {
            timer.abort();
        }
        *lock(&session.phase) = Phase::Transcribing;
        self.session_event(session_id, SpeechPhase::Transcribing, None);
        let busy = Busy::begin(&session);
        self.transcribe(&session).await;
        drop(busy);
        Ok(())
    }

    async fn transcribe(&self, session: &Arc<Session>) {
        let timed_out = Arc::new(AtomicBool::new(false));
        let timeout = {
            let session = Arc::clone(session);
            let timed_out = Arc::clone(&timed_out);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(SPEECH_LIMITS.timeout_ms)).await;
                timed_out.store(true, Ordering::SeqCst);
                session.cancel.send_replace(true);
            })
        };
        let mut text = self.recognize(session).await.ok();
        timeout.abort();
        let mut failed = text.is_none();
        if self.dispose(session).await.is_err() {
            self.mark_cleanup_failed();
            failed = true;
            text = None;
        }
        // A recording the user cancelled has already left `active`; only a live one reports its own end.
        let still_active = {
            let mut state = lock(&self.inner.state);
            let active = state
                .active
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, session));
            if active {
                state.active = None;
            }
            active
        };
        if !still_active {
            return;
        }
        let id = session.id.clone();
        if *session.cancel.borrow() {
            self.session_event(
                &id,
                SpeechPhase::Error,
                Some(problem(
                    "transcription_timeout",
                    "Local transcription took too long. Try a shorter recording.",
                )),
            );
        } else if let (Some(text), false) = (text, failed) {
            self.emit(SpeechEvent::Final {
                session_id: id,
                text,
            });
        } else {
            self.session_event(
                &id,
                SpeechPhase::Error,
                Some(problem("transcription_failed", "No usable local transcript was produced, or temporary audio cleanup failed. Check storage and try again.")),
            );
        }
    }

    /// Finishes the WAV, runs whisper over it and reads what it wrote.
    async fn recognize(&self, session: &Arc<Session>) -> Result<String, ()> {
        let cancel = session.cancel.subscribe();
        let cancelled = |cancel: &watch::Receiver<bool>| *cancel.borrow();
        let directory = lock(&session.directory).clone().ok_or(())?;
        let bytes = session.bytes.load(Ordering::SeqCst);
        {
            let mut io = session.io.lock().await;
            if bytes == 0 || !io.nonzero || cancelled(&cancel) {
                return Err(()); // digital silence is not transcribed
            }
            let mut file = io.file.take().ok_or(())?;
            file.seek(std::io::SeekFrom::Start(0)).await.map_err(drop)?;
            file.write_all(&wav_header(u32::try_from(bytes).map_err(drop)?))
                .await
                .map_err(drop)?;
            file.flush().await.map_err(drop)?;
        }
        let output = directory.join("result");
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(4);
        let args: Vec<String> = [
            "-m".to_string(),
            self.inner
                .options
                .model
                .path()
                .to_string_lossy()
                .into_owned(),
            "-f".into(),
            directory.join("input.wav").to_string_lossy().into_owned(),
            "-l".into(),
            "en".into(),
            "-ng".into(),
            "-t".into(),
            threads.to_string(),
            "-p".into(),
            "1".into(),
            "-np".into(),
            "-oj".into(),
            "-of".into(),
            output.to_string_lossy().into_owned(),
            "-nth".into(),
            "0.6".into(),
            "-lpt".into(),
            "-1.0".into(),
            "-sns".into(),
        ]
        .into();
        self.inner
            .options
            .process
            .run(&self.inner.options.runtime_path, &args, cancel.clone())
            .await
            .map_err(drop)?;
        if cancelled(&cancel) {
            return Err(());
        }
        let result = directory.join("result.json");
        let meta = tokio::fs::symlink_metadata(&result).await.map_err(drop)?;
        if !meta.file_type().is_file() || meta.len() > SPEECH_LIMITS.result_bytes as u64 {
            return Err(());
        }
        let json = tokio::fs::read_to_string(&result).await.map_err(drop)?;
        speech_text(&json).map_err(drop)
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), String> {
        if !session_id_ok(session_id) {
            return Err("Invalid dictation request.".into());
        }
        let session = {
            let mut state = lock(&self.inner.state);
            match state
                .active
                .as_ref()
                .filter(|session| session.id == session_id)
            {
                Some(_) => state.active.take(),
                None => None,
            }
        };
        let Some(session) = session else {
            // Not the active recording: if it is being cancelled, wait for that to finish.
            let waiting = lock(&self.inner.state)
                .cancellation
                .as_ref()
                .filter(|cancelling| cancelling.id == session_id)
                .map(|cancelling| cancelling.done.clone());
            if let Some(mut done) = waiting {
                let _ = done.wait_for(|finished| *finished).await;
            }
            return if self.cleanup_failed() {
                Err(CLEANUP_FAILED.into())
            } else {
                Ok(())
            };
        };
        if let Some(timer) = lock(&session.timer).take() {
            timer.abort();
        }
        session.cancel.send_replace(true);
        let (finished_tx, finished_rx) = watch::channel(false);
        lock(&self.inner.state).cancellation = Some(Cancelling {
            id: session_id.into(),
            done: finished_rx,
        });
        // The start or transcription in flight sees the abort and ends; then the audio goes.
        let _ = session.idle.subscribe().wait_for(|idle| *idle).await;
        let disposed = self.dispose(&session).await;
        let result = if disposed.is_err() {
            self.mark_cleanup_failed();
            Err(CLEANUP_FAILED.to_string())
        } else {
            self.session_event(session_id, SpeechPhase::Cancelled, None);
            Ok(())
        };
        lock(&self.inner.state).cancellation = None;
        let _ = finished_tx.send(true);
        result
    }

    pub async fn cancel_all(&self) -> Result<(), String> {
        let active = lock(&self.inner.state)
            .active
            .as_ref()
            .map(|session| session.id.clone());
        let mut result = Ok(());
        if let Some(id) = active {
            result = self.cancel(&id).await;
        }
        let waiting = lock(&self.inner.state)
            .cancellation
            .as_ref()
            .map(|cancelling| cancelling.done.clone());
        if let Some(mut done) = waiting {
            let _ = done.wait_for(|finished| *finished).await;
        }
        if result.is_ok() && self.cleanup_failed() {
            result = Err(CLEANUP_FAILED.into());
        }
        let model = self.inner.options.model.cancel_download().await;
        result.and(model)
    }

    /// Closes the audio file and removes the session folder. Once per session; a second call returns the same result.
    async fn dispose(&self, session: &Arc<Session>) -> Result<(), String> {
        let ok = session
            .disposed
            .get_or_init(|| async {
                // Taking the lock waits for a write in flight.
                drop(session.io.lock().await.file.take());
                let directory = lock(&session.directory).take();
                match directory {
                    Some(directory) => self.remove_session_directory(&directory).await.is_ok(),
                    None => true,
                }
            })
            .await;
        if *ok {
            Ok(())
        } else {
            Err(CLEANUP_FAILED.into())
        }
    }
}

#[cfg(test)]
mod tests;
