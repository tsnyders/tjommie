//! Local dictation (T6.5; spec §3.10; port of `speech-runtime.ts`, `speech-model.ts`, `speech-resources.ts`).
//!
//! `whisper-cli.exe` (CPU, built in CI from a pinned commit and checked by digest) transcribes one recording at a time
//! from a temporary WAV file; the English model is downloaded once, from a pinned URL, and checked by size and SHA-256.
//! Audio never leaves the PC and never reaches the daemon. Capture is `cpal` (WASAPI) on its own thread; the window
//! losing focus cancels a recording in progress.

mod capture;
mod model;
mod permission;
mod resources;
mod runtime;

use std::io::Read as _;
use std::path::Path;

use sha2::{Digest as _, Sha256};

pub use capture::{
    CaptureHandle, CaptureSource, CpalSource, Dictation, Resampler, Sink, mono_from, pcm16_from,
};
pub use model::{MODEL, ModelSource, SpeechModel, pinned};
pub use permission::microphone_permission;
pub use resources::{
    RESOURCE_NAMES, RUNTIME_COMMIT, packaged_speech_digest, verify_speech_resources,
};
pub use runtime::{
    PcmChunk, RuntimeOptions, SpeechProcess, SpeechRuntime, WhisperProcess, speech_text, wav_header,
};

/// Resolves when `signal` becomes true. Its own function, so a `select!` arm doesn't hold the watch's guard (which
/// isn't `Send`) across an `.await`.
pub(crate) async fn cancelled(signal: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = signal.wait_for(|cancelled| *cancelled).await;
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The SHA-256 of a regular file (not a link to one), as lowercase hex.
pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    let kind = std::fs::symlink_metadata(path)?.file_type();
    if !kind.is_file() {
        return Err(std::io::Error::other(
            "A speech resource is not a regular file.",
        ));
    }
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(buffer.get(..read).unwrap_or_default());
    }
    Ok(hex(&hash.finalize()))
}
