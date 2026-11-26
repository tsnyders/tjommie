//! Port of `speech-runtime.test.ts`, plus the plan's `wav_header_16k` and `stops_on_blur`. No test records from a real
//! microphone and none runs the real `whisper-cli.exe`: the process is a fake that writes the JSON whisper would, and
//! capture is a fake that hands over fixture PCM.

use std::sync::atomic::AtomicUsize;

use futures_util::FutureExt as _;
use sha2::{Digest as _, Sha256};

use super::*;
use crate::speech::capture::{CaptureHandle, CaptureSource, Dictation, Sink};
use crate::speech::hex;
use crate::speech::model::ModelSource;
use crate::updater::{Http, Progress};

type Behavior = Arc<
    dyn Fn(Vec<String>, watch::Receiver<bool>) -> BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;

struct FakeProcess {
    calls: Mutex<Vec<Vec<String>>>,
    behavior: Behavior,
}

impl SpeechProcess for FakeProcess {
    fn run<'a>(
        &'a self,
        _executable: &'a Path,
        args: &'a [String],
        cancel: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.calls.lock().unwrap().push(args.to_vec());
        (self.behavior)(args.to_vec(), cancel)
    }
}

fn after(args: &[String], flag: &str) -> String {
    let at = args.iter().position(|arg| arg == flag).unwrap();
    args[at + 1].clone()
}

/// What whisper does: reads the WAV (checking it is a 4-byte one), writes `<output>.json`.
fn transcribes(text: &'static str) -> Behavior {
    Arc::new(move |args, _cancel| {
        async move {
            let wav = std::fs::read(after(&args, "-f")).unwrap();
            assert_eq!(&wav[..4], b"RIFF");
            assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
            let json = serde_json::json!({ "transcription": [{ "text": text }] });
            std::fs::write(format!("{}.json", after(&args, "-of")), json.to_string()).unwrap();
            Ok(())
        }
        .boxed()
    })
}

struct NoHttp;

impl Http for NoHttp {
    fn get_text<'a>(&'a self, _: &'a url::Url, _: usize) -> BoxFuture<'a, Result<String, String>> {
        async { Err("the runtime never fetches text".to_string()) }.boxed()
    }
    fn download<'a>(
        &'a self,
        _: &'a url::Url,
        _: &'a Path,
        _: u64,
        _: Progress<'a>,
    ) -> BoxFuture<'a, Result<(), String>> {
        async { Err("a recording never downloads the model".to_string()) }.boxed()
    }
}

struct Fixture {
    path: tempfile::TempDir,
    events: Arc<Mutex<Vec<SpeechEvent>>>,
    process: Arc<FakeProcess>,
    runtime: SpeechRuntime,
}

impl Fixture {
    fn sessions(&self) -> Vec<String> {
        std::fs::read_dir(self.path.path().join("sessions"))
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn finals(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                SpeechEvent::Final { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
    fn spawned(&self) -> usize {
        self.process.calls.lock().unwrap().len()
    }
}

fn start(id: &str) -> SpeechStart {
    SpeechStart {
        session_id: id.into(),
        language: "en".into(),
    }
}

fn pcm(id: &str, sequence: u32, bytes: Vec<u8>) -> PcmChunk {
    PcmChunk {
        session_id: id.into(),
        sequence,
        pcm: bytes,
    }
}

fn fixture_with(behavior: Behavior) -> Fixture {
    let path = tempfile::tempdir().unwrap();
    let content = b"synthetic runtime and model".to_vec();
    let sha = hex(&Sha256::digest(&content));
    let executable = path.path().join("synthetic.exe");
    std::fs::write(&executable, &content).unwrap();
    let source = ModelSource {
        bytes: content.len() as u64,
        sha256: sha.clone(),
        url: "https://huggingface.co/synthetic".into(),
    };
    let model = SpeechModel::new(
        path.path().join("models"),
        source,
        Arc::new(NoHttp),
        Arc::new(|| {}),
    );
    std::fs::create_dir_all(path.path().join("models")).unwrap();
    std::fs::write(model.path(), &content).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let process = Arc::new(FakeProcess {
        calls: Mutex::new(Vec::new()),
        behavior,
    });
    let sink = Arc::clone(&events);
    let runtime = SpeechRuntime::new(RuntimeOptions {
        runtime_path: executable,
        runtime_sha256: Some(sha),
        data_dir: path.path().to_path_buf(),
        emit: Arc::new(move |event| sink.lock().unwrap().push(event)),
        model,
        process: Arc::clone(&process) as Arc<dyn SpeechProcess>,
        permission: Arc::new(|| SpeechPermission::Granted),
    });
    Fixture {
        path,
        events,
        process,
        runtime,
    }
}

fn fixture() -> Fixture {
    fixture_with(transcribes(" synthetic words"))
}

// "requires verified resources, writes bounded PCM WAV, invokes local CPU transcription and deletes raw files before final"
#[tokio::test]
async fn writes_bounded_pcm_wav_transcribes_on_the_cpu_and_deletes_the_audio_before_final() {
    let f = fixture();
    assert!(f.runtime.status().await.available);
    f.runtime.start(start("one")).await.unwrap();
    assert!(f.runtime.is_capturing());
    assert!(!f.runtime.is_recording_audio(), "no audio yet");
    f.runtime
        .append_pcm(pcm("one", 0, vec![1, 0, 2, 0]))
        .await
        .unwrap();
    assert!(f.runtime.is_recording_audio());
    f.runtime.stop("one").await.unwrap();
    assert_eq!(
        f.events.lock().unwrap().last(),
        Some(&SpeechEvent::Final {
            session_id: "one".into(),
            text: "synthetic words".into()
        })
    );
    let args = f.process.calls.lock().unwrap()[0].clone();
    assert!(args.contains(&"-ng".to_string()), "CPU only");
    assert!(!args.contains(&"--prompt".to_string()));
    assert_eq!(after(&args, "-l"), "en");
    assert_eq!(
        after(&args, "-m"),
        f.runtime.model().path().to_string_lossy()
    );
    assert!(!f.runtime.is_capturing());
    assert!(f.sessions().is_empty(), "the raw audio is gone");
    // The phases in order.
    let phases: Vec<_> = f
        .events
        .lock()
        .unwrap()
        .iter()
        .map(|event| match event {
            SpeechEvent::Session { phase, .. } => format!("{phase:?}"),
            SpeechEvent::Final { .. } => "final".into(),
            SpeechEvent::Status { .. } => "status".into(),
        })
        .collect();
    assert_eq!(phases, ["Listening", "Transcribing", "final"]);
}

// "keeps unavailable runtime/model states honest and never spawns"
#[tokio::test]
async fn keeps_unavailable_runtime_and_model_states_honest_and_never_spawns() {
    let f = fixture();
    f.runtime.remove_model().await.unwrap();
    let error = f.runtime.start(start("one")).await.unwrap_err();
    assert!(error.contains("model"), "{error}");
    assert_eq!(
        f.runtime.status().await.problem.unwrap().code,
        "model_missing"
    );
    let missing = SpeechRuntime::new(RuntimeOptions {
        runtime_path: f.path.path().join("missing.exe"),
        runtime_sha256: None,
        data_dir: f.path.path().to_path_buf(),
        emit: Arc::new(|_| {}),
        model: f.runtime.model().clone(),
        process: Arc::clone(&f.process) as Arc<dyn SpeechProcess>,
        permission: Arc::new(|| SpeechPermission::Granted),
    });
    let status = missing.status().await;
    assert!(!status.available);
    assert_eq!(status.problem.unwrap().code, "runtime_unavailable");
    let error = missing.start(start("two")).await.unwrap_err();
    assert!(error.contains("runtime"), "{error}");
    assert_eq!(f.spawned(), 0);
}

// "rechecks the runtime hash before a new recording even after a successful status check"
#[tokio::test]
async fn rechecks_the_runtime_hash_before_a_new_recording_after_a_successful_status() {
    let f = fixture();
    assert!(f.runtime.status().await.available);
    std::fs::write(f.path.path().join("synthetic.exe"), "changed runtime").unwrap();
    let error = f.runtime.start(start("one")).await.unwrap_err();
    assert!(error.contains("runtime"), "{error}");
    assert_eq!(f.spawned(), 0);
}

// "does not transcribe digital silence and refuses reordered/oversized chunks"
#[tokio::test]
async fn does_not_transcribe_silence_and_refuses_reordered_or_oversized_chunks() {
    let f = fixture();
    f.runtime.start(start("one")).await.unwrap();
    f.runtime
        .append_pcm(pcm("one", 0, vec![0; 10]))
        .await
        .unwrap();
    f.runtime.stop("one").await.unwrap();
    assert!(f.finals().is_empty());
    assert_eq!(f.spawned(), 0);
    assert!(f.sessions().is_empty());

    f.runtime.start(start("two")).await.unwrap();
    let error = f
        .runtime
        .append_pcm(pcm("two", 1, vec![1, 0]))
        .await
        .unwrap_err();
    assert!(error.contains("out of order"), "{error}");
    assert!(!f.runtime.is_capturing());

    f.runtime.start(start("three")).await.unwrap();
    assert!(
        f.runtime
            .append_pcm(pcm("three", 0, vec![1; 65538]))
            .await
            .is_err()
    );
    // An odd-length or empty chunk is not PCM16 either; none of these end the recording.
    assert!(
        f.runtime
            .append_pcm(pcm("three", 0, vec![1; 3]))
            .await
            .is_err()
    );
    assert!(f.runtime.append_pcm(pcm("three", 0, vec![])).await.is_err());
    assert!(f.runtime.is_capturing());
    f.runtime.cancel_all().await.unwrap();
    assert!(f.sessions().is_empty());
}

// "fences cancelled transcription even if a late synthetic process completes"
#[tokio::test]
async fn fences_a_cancelled_transcription_even_if_a_late_process_completes() {
    let started = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let (process_started, process_gate) = (Arc::clone(&started), Arc::clone(&gate));
    let behavior: Behavior = Arc::new(move |args, _cancel| {
        let (started, gate) = (Arc::clone(&process_started), Arc::clone(&process_gate));
        async move {
            started.notify_one();
            gate.notified().await;
            std::fs::write(
                format!("{}.json", after(&args, "-of")),
                serde_json::json!({ "transcription": [{ "text": "late" }] }).to_string(),
            )
            .unwrap();
            Ok(())
        }
        .boxed()
    });
    let f = fixture_with(behavior);
    f.runtime.start(start("one")).await.unwrap();
    f.runtime
        .append_pcm(pcm("one", 0, vec![1, 0]))
        .await
        .unwrap();
    let stopping = tokio::spawn({
        let runtime = f.runtime.clone();
        async move { runtime.stop("one").await }
    });
    started.notified().await;
    let cancelling = tokio::spawn({
        let runtime = f.runtime.clone();
        async move { runtime.cancel("one").await }
    });
    tokio::task::yield_now().await;
    let error = f.runtime.start(start("two")).await.unwrap_err();
    assert!(error.contains("stopping"), "{error}");
    gate.notify_one();
    cancelling.await.unwrap().unwrap();
    stopping.await.unwrap().unwrap();
    assert!(f.finals().is_empty(), "the late result is dropped");
    assert!(f.sessions().is_empty());
    let cancelled = f.events.lock().unwrap().iter().any(|event| {
        matches!(
            event,
            SpeechEvent::Session {
                phase: SpeechPhase::Cancelled,
                ..
            }
        )
    });
    assert!(cancelled);
    // And a new recording can start afterwards.
    f.runtime.start(start("three")).await.unwrap();
    f.runtime.cancel_all().await.unwrap();
}

// "suppresses final text and blocks new recordings when temporary audio cleanup fails"
#[tokio::test]
async fn suppresses_final_text_and_blocks_new_recordings_when_audio_cleanup_fails() {
    let f = fixture();
    f.runtime.inner.fail_cleanup.store(true, Ordering::SeqCst);
    f.runtime.start(start("one")).await.unwrap();
    f.runtime
        .append_pcm(pcm("one", 0, vec![1, 0, 2, 0]))
        .await
        .unwrap();
    f.runtime.stop("one").await.unwrap();
    assert!(f.finals().is_empty());
    let shown = format!("{:?}", f.events.lock().unwrap());
    assert!(
        !shown.contains(&*f.path.path().to_string_lossy()),
        "no path in any event"
    );
    assert_eq!(
        f.runtime.status().await.problem.unwrap().code,
        "cleanup_failed"
    );
    assert!(
        f.runtime
            .cancel_all()
            .await
            .unwrap_err()
            .contains("could not be cleared")
    );
    assert!(
        f.runtime
            .cancel_all()
            .await
            .unwrap_err()
            .contains("could not be cleared")
    );
    assert!(
        f.runtime
            .cancel("one")
            .await
            .unwrap_err()
            .contains("could not be cleared")
    );
    assert!(
        f.runtime
            .start(start("two"))
            .await
            .unwrap_err()
            .contains("cleared")
    );
    f.runtime.inner.fail_cleanup.store(false, Ordering::SeqCst);
}

// "bounds the complete PCM stream, not just each individual chunk"
#[tokio::test]
async fn bounds_the_complete_pcm_stream_not_just_each_chunk() {
    let f = fixture();
    f.runtime.start(start("one")).await.unwrap();
    for sequence in 0..29 {
        f.runtime
            .append_pcm(pcm("one", sequence, vec![1; 65536]))
            .await
            .unwrap();
    }
    let error = f
        .runtime
        .append_pcm(pcm("one", 29, vec![1; 65536]))
        .await
        .unwrap_err();
    assert!(error.contains("limits"), "{error}");
    assert!(!f.runtime.is_capturing());
    assert_eq!(f.spawned(), 0);
    assert!(f.sessions().is_empty());
}

// "cancels a start waiting on model verification without creating orphan audio"
#[tokio::test]
async fn cancels_a_start_still_preparing_without_leaving_orphan_audio() {
    let f = fixture();
    let mut starting = Box::pin(f.runtime.start(start("one")));
    // One poll: the recording is registered and start is waiting on the disk, as it would be on model verification.
    tokio::select! {
        biased;
        result = &mut starting => panic!("start finished at once: {result:?}"),
        () = std::future::ready(()) => {}
    }
    let (started, cancelled) = tokio::join!(starting, f.runtime.cancel("one"));
    assert!(started.unwrap_err().contains("cancelled"));
    cancelled.unwrap();
    assert!(f.sessions().is_empty());
    assert!(!f.runtime.is_capturing());
    f.runtime.start(start("two")).await.unwrap();
}

// "terminates its own synthetic child process on cancellation": a real child, through `spawn::command`.
#[tokio::test]
async fn terminates_its_own_child_process_on_cancellation() {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let ping = Path::new(&windir).join("System32").join("ping.exe");
    let (cancel, signal) = watch::channel(false);
    let began = std::time::Instant::now();
    let args = ["-n".to_string(), "30".into(), "127.0.0.1".into()];
    let running = WhisperProcess.run(&ping, &args, signal);
    let stopper = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.send_replace(true);
    };
    let (result, ()) = tokio::join!(running, stopper);
    assert!(result.unwrap_err().contains("cancelled"));
    assert!(
        began.elapsed() < Duration::from_secs(15),
        "ping -n 30 takes 30 s if it isn't killed"
    );
}

#[tokio::test]
async fn the_real_process_runner_reports_start_and_exit_failures() {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let cmd = Path::new(&windir).join("System32").join("cmd.exe");
    let (_keep, signal) = watch::channel(false);
    let exit = |code: &str| vec!["/c".to_string(), format!("exit {code}")];
    assert_eq!(
        WhisperProcess.run(&cmd, &exit("0"), signal.clone()).await,
        Ok(())
    );
    assert!(
        WhisperProcess
            .run(&cmd, &exit("3"), signal.clone())
            .await
            .unwrap_err()
            .contains("failed")
    );
    let missing = WhisperProcess
        .run(Path::new(r"C:\no\such\whisper.exe"), &[], signal)
        .await;
    assert!(missing.unwrap_err().contains("could not start"));
    // A recording cancelled before whisper started never starts it.
    let (_cancelled_tx, cancelled) = watch::channel(true);
    let never = WhisperProcess
        .run(Path::new(r"C:\no\such\whisper.exe"), &[], cancelled)
        .await;
    assert!(never.unwrap_err().contains("cancelled"));
}

// "validates output schema/size and no-speech markers"
#[test]
fn validates_the_output_schema_its_size_and_the_no_speech_markers() {
    assert_eq!(
        speech_text(r#"{"transcription":[{"text":" hello"},{"text":" world"}]}"#).unwrap(),
        "hello world"
    );
    // whisper adds fields of its own; they are ignored.
    assert_eq!(
        speech_text(r#"{"model":{"x":1},"transcription":[{"text":" hi","offsets":{"from":0}}]}"#)
            .unwrap(),
        "hi"
    );
    for json in [
        "bad".to_string(),
        "{}".into(),
        r#"{"transcription":[]}"#.into(),
        r#"{"transcription":[{"text":"[BLANK_AUDIO]"}]}"#.into(),
        r#"{"transcription":[{"text":" (silence) "}]}"#.into(),
        r#"{"transcription":[{"text":"[Music)"}]}"#.into(),
        r#"{"transcription":[{"text":"   "}]}"#.into(),
        "x".repeat(131_073),
        format!(r#"{{"transcription":[{{"text":"{}"}}]}}"#, "a".repeat(8193)),
    ] {
        assert!(
            speech_text(&json).is_err(),
            "{}",
            json.chars().take(60).collect::<String>()
        );
    }
    // A sentence that merely contains a marker word is speech.
    assert_eq!(
        speech_text(r#"{"transcription":[{"text":"play some music"}]}"#).unwrap(),
        "play some music"
    );
    assert_eq!(
        u32::from_le_bytes(wav_header(1_920_000)[40..44].try_into().unwrap()),
        1_920_000
    );
}

#[test]
fn wav_header_16k() {
    let header = wav_header(4);
    assert_eq!(&header[0..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(header[4..8].try_into().unwrap()), 36 + 4);
    assert_eq!(&header[8..16], b"WAVEfmt ");
    assert_eq!(u32::from_le_bytes(header[16..20].try_into().unwrap()), 16);
    assert_eq!(
        u16::from_le_bytes(header[20..22].try_into().unwrap()),
        1,
        "PCM"
    );
    assert_eq!(
        u16::from_le_bytes(header[22..24].try_into().unwrap()),
        1,
        "mono"
    );
    assert_eq!(
        u32::from_le_bytes(header[24..28].try_into().unwrap()),
        16_000
    );
    assert_eq!(
        u32::from_le_bytes(header[28..32].try_into().unwrap()),
        32_000
    );
    assert_eq!(u16::from_le_bytes(header[32..34].try_into().unwrap()), 2);
    assert_eq!(u16::from_le_bytes(header[34..36].try_into().unwrap()), 16);
    assert_eq!(&header[36..40], b"data");
    assert_eq!(u32::from_le_bytes(header[40..44].try_into().unwrap()), 4);
    assert_eq!(header.len(), 44);
}

#[tokio::test]
async fn refuses_other_languages_bad_ids_and_leftovers_of_an_earlier_run_are_cleared() {
    let f = fixture();
    let french = SpeechStart {
        session_id: "one".into(),
        language: "fr".into(),
    };
    assert!(
        f.runtime
            .start(french)
            .await
            .unwrap_err()
            .contains("English")
    );
    assert!(f.runtime.start(start("a b")).await.is_err());
    assert!(f.runtime.start(start("../up")).await.is_err());
    // A recording an earlier run left behind is removed at the next start; other folders are not touched.
    let sessions = f.path.path().join("sessions");
    std::fs::create_dir_all(sessions.join("session-abc123")).unwrap();
    std::fs::write(
        sessions.join("session-abc123").join("input.wav"),
        b"old audio",
    )
    .unwrap();
    std::fs::create_dir_all(sessions.join("keep-me")).unwrap();
    f.runtime.start(start("one")).await.unwrap();
    let names = f.sessions();
    assert!(!names.contains(&"session-abc123".to_string()));
    assert!(names.contains(&"keep-me".to_string()));
    f.runtime.cancel_all().await.unwrap();
}

#[tokio::test]
async fn never_removes_a_folder_that_is_not_one_of_its_own() {
    let f = fixture();
    let elsewhere = f.path.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    assert!(
        f.runtime
            .remove_session_directory(&elsewhere)
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .remove_session_directory(&f.path.path().join("sessions").join("not-a-session"))
            .await
            .is_err()
    );
    assert!(
        f.runtime
            .remove_session_directory(&f.path.path().join("sessions"))
            .await
            .is_err()
    );
    assert!(elsewhere.exists());
}

#[tokio::test]
async fn status_says_when_windows_blocks_the_microphone_for_desktop_apps() {
    let f = fixture();
    let blocked = SpeechRuntime::new(RuntimeOptions {
        runtime_path: f.path.path().join("synthetic.exe"),
        runtime_sha256: f.runtime.inner.options.runtime_sha256.clone(),
        data_dir: f.path.path().to_path_buf(),
        emit: Arc::new(|_| {}),
        model: f.runtime.model().clone(),
        process: Arc::clone(&f.process) as Arc<dyn SpeechProcess>,
        permission: Arc::new(|| SpeechPermission::Denied),
    });
    let status = blocked.status().await;
    assert!(!status.available);
    assert_eq!(status.permission, SpeechPermission::Denied);
    let problem = status.problem.unwrap();
    assert_eq!(problem.code, "microphone_blocked");
    assert_eq!(
        problem.message,
        "Microphone access is off for desktop apps in Windows Settings."
    );
    // And when it isn't blocked there is no problem.
    assert!(f.runtime.status().await.problem.is_none());
}

// ---- capture and the window losing focus ------------------------------------------------------------------------

#[derive(Default)]
struct FakeSource {
    sink: Mutex<Option<Sink>>,
    stopped: Arc<AtomicUsize>,
}

struct FakeCapture(Arc<AtomicUsize>);

impl CaptureHandle for FakeCapture {
    fn stop(self: Box<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl CaptureSource for FakeSource {
    fn start(&self, sink: Sink) -> Result<Box<dyn CaptureHandle>, String> {
        *self.sink.lock().unwrap() = Some(sink);
        Ok(Box::new(FakeCapture(Arc::clone(&self.stopped))))
    }
}

impl FakeSource {
    /// The microphone delivers a chunk of fixture PCM.
    fn hear(&self, pcm: &[u8]) {
        let sink = self.sink.lock().unwrap().clone().unwrap();
        sink(pcm.to_vec());
    }
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never happened: {what}");
}

/// A microphone that takes its time to open: it waits for the runtime to tell it to go on.
struct SlowSource {
    waiting: Arc<std::sync::atomic::AtomicBool>,
    go: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl CaptureSource for SlowSource {
    fn start(&self, _: Sink) -> Result<Box<dyn CaptureHandle>, String> {
        let go = self.go.lock().unwrap().take().unwrap();
        self.waiting.store(true, Ordering::SeqCst);
        go.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "the runtime never went on".to_owned())?;
        Ok(Box::new(FakeCapture(Arc::new(AtomicUsize::new(0)))))
    }
}

/// M4 re-review R4: the microphone opens on the blocking pool, so the runtime goes on meanwhile. On this one-thread
/// runtime the device waits for a task of the runtime's own, which runs only if opening it doesn't hold the thread.
#[tokio::test(flavor = "current_thread")]
async fn the_microphone_opens_without_holding_the_runtime() {
    let f = fixture();
    let waiting = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (go, gone) = std::sync::mpsc::channel();
    let dictation = Dictation::new(
        f.runtime.clone(),
        Arc::new(SlowSource {
            waiting: Arc::clone(&waiting),
            go: Mutex::new(Some(gone)),
        }) as Arc<dyn CaptureSource>,
    );
    tokio::spawn(async move {
        while !waiting.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let _ = go.send(());
    });
    dictation.begin("slow-one").await.unwrap();
    dictation.cancel().await.unwrap();
}

#[tokio::test]
async fn stops_on_blur() {
    let f = fixture();
    let source = Arc::new(FakeSource::default());
    let dictation = Dictation::new(
        f.runtime.clone(),
        Arc::clone(&source) as Arc<dyn CaptureSource>,
    );
    dictation.begin("blur-one").await.unwrap();
    // Listening but nothing heard yet: losing focus leaves it be (`isRecordingAudio` is false).
    assert!(!dictation.on_window_blur().await);
    assert!(f.runtime.is_capturing());
    // Audio arrives; now losing focus cancels the recording and releases the microphone.
    source.hear(&[1, 0, 2, 0]);
    until("the chunk reaches the recording", || {
        f.runtime.is_recording_audio()
    })
    .await;
    assert!(dictation.on_window_blur().await);
    assert!(!f.runtime.is_capturing());
    assert_eq!(
        source.stopped.load(Ordering::SeqCst),
        1,
        "the microphone is released"
    );
    assert!(f.sessions().is_empty(), "the audio is deleted");
    assert!(f.finals().is_empty());
    let cancelled = f.events.lock().unwrap().iter().any(|event| {
        matches!(
            event,
            SpeechEvent::Session {
                phase: SpeechPhase::Cancelled,
                ..
            }
        )
    });
    assert!(cancelled);
    // Nothing left to cancel.
    assert!(!dictation.on_window_blur().await);
}

#[tokio::test]
async fn a_dictation_from_microphone_to_text_and_back_to_a_new_one() {
    let f = fixture();
    let source = Arc::new(FakeSource::default());
    let dictation = Dictation::new(
        f.runtime.clone(),
        Arc::clone(&source) as Arc<dyn CaptureSource>,
    );
    dictation.begin("flow").await.unwrap();
    source.hear(&[1, 0]);
    source.hear(&[2, 0]);
    // Chunks are numbered in the order they were heard; the fake process checks the WAV holds the 4 bytes.
    dictation.finish().await.unwrap();
    assert_eq!(f.finals(), ["synthetic words"]);
    assert_eq!(source.stopped.load(Ordering::SeqCst), 1);
    assert!(f.sessions().is_empty());
    assert!(dictation.finish().await.is_err(), "nothing is recording");
    dictation.begin("again").await.unwrap();
    dictation.cancel().await.unwrap();
    assert_eq!(source.stopped.load(Ordering::SeqCst), 2);
}

// Review finding 1. When the runtime ends a recording itself, nobody calls `finish` or `cancel`: the microphone must close
// on its own, or its indicator stays lit and chunks pile up. The device keeps delivering chunks until it is closed, so each
// test plays that part (`hear`) and then waits for the close.

#[tokio::test]
async fn the_microphone_closes_when_the_runtime_refuses_a_chunk() {
    let f = fixture();
    let source = Arc::new(FakeSource::default());
    let dictation = Dictation::new(
        f.runtime.clone(),
        Arc::clone(&source) as Arc<dyn CaptureSource>,
    );
    dictation.begin("limit").await.unwrap();
    // 29 chunks fit in the 1,920,000-byte limit; the 30th does not, and the runtime cancels the recording.
    for _ in 0..30 {
        source.hear(&vec![1u8; 65536]);
    }
    until("the microphone closes", || {
        source.stopped.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(!f.runtime.is_capturing());
    assert!(f.sessions().is_empty());
    // The UI cancelling afterwards doesn't close it a second time.
    dictation.cancel().await.unwrap();
    assert_eq!(source.stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_microphone_closes_when_the_time_limit_ends_the_recording() {
    let f = fixture();
    f.runtime
        .inner
        .recording_limit_ms
        .store(50, Ordering::SeqCst);
    let source = Arc::new(FakeSource::default());
    let dictation = Dictation::new(
        f.runtime.clone(),
        Arc::clone(&source) as Arc<dyn CaptureSource>,
    );
    dictation.begin("long").await.unwrap();
    source.hear(&[1, 0, 2, 0]);
    until(
        "the runtime ends the recording itself and transcribes it",
        || !f.finals().is_empty(),
    )
    .await;
    assert_eq!(f.finals(), ["synthetic words"]);
    // The device is still delivering when the transcript appears.
    source.hear(&[1, 0]);
    until("the microphone closes", || {
        source.stopped.load(Ordering::SeqCst) == 1
    })
    .await;
    dictation.cancel().await.unwrap();
    assert_eq!(source.stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_microphone_closes_when_something_else_cancels_the_recording() {
    let f = fixture();
    let source = Arc::new(FakeSource::default());
    let dictation = Dictation::new(
        f.runtime.clone(),
        Arc::clone(&source) as Arc<dyn CaptureSource>,
    );
    dictation.begin("other").await.unwrap();
    source.hear(&[1, 0]);
    until("the chunk arrives", || f.runtime.is_recording_audio()).await;
    // Removing the model, say, cancels whatever is recording.
    f.runtime.cancel_all().await.unwrap();
    source.hear(&[2, 0]);
    until("the microphone closes", || {
        source.stopped.load(Ordering::SeqCst) == 1
    })
    .await;
}

#[tokio::test]
async fn finishing_and_cancelling_close_the_microphone_exactly_once_each() {
    let f = fixture();
    let source = Arc::new(FakeSource::default());
    let dictation = Dictation::new(
        f.runtime.clone(),
        Arc::clone(&source) as Arc<dyn CaptureSource>,
    );
    dictation.begin("one").await.unwrap();
    source.hear(&[1, 0, 2, 0]);
    dictation.finish().await.unwrap();
    assert_eq!(source.stopped.load(Ordering::SeqCst), 1);
    dictation.begin("two").await.unwrap();
    source.hear(&[1, 0]);
    dictation.cancel().await.unwrap();
    assert_eq!(source.stopped.load(Ordering::SeqCst), 2);
    // Nothing is left to close.
    dictation.cancel().await.unwrap();
    assert_eq!(source.stopped.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_microphone_that_cannot_open_ends_the_session_cleanly() {
    struct Broken;
    impl CaptureSource for Broken {
        fn start(&self, _: Sink) -> Result<Box<dyn CaptureHandle>, String> {
            Err("No microphone could be opened.".into())
        }
    }
    let f = fixture();
    let dictation = Dictation::new(f.runtime.clone(), Arc::new(Broken));
    assert_eq!(
        dictation.begin("none").await.unwrap_err(),
        "No microphone could be opened."
    );
    assert!(!f.runtime.is_capturing());
    assert!(f.sessions().is_empty());
    f.runtime.start(start("next")).await.unwrap();
}
