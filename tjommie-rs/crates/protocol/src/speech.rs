//! `speech.ts`. A current-device bridge only: speech never travels over the daemon or remote wire. `SpeechPcm` (a
//! `Uint8Array` across the JavaScript bridge) and `SpeechApi` have no port.

use serde::{Deserialize, Serialize};

use crate::Wire;

pub struct SpeechLimits {
    pub sample_rate: u32,
    pub max_seconds: u32,
    pub chunk_bytes: usize,
    pub audio_bytes: usize,
    pub result_bytes: usize,
    pub text_characters: usize,
    pub timeout_ms: u64,
}

pub const SPEECH_LIMITS: SpeechLimits = SpeechLimits {
    sample_rate: 16000,
    max_seconds: 60,
    chunk_bytes: 65536,
    audio_bytes: 1_920_000,
    result_bytes: 131_072,
    text_characters: 8192,
    timeout_ms: 120_000,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechProblem {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpeechEngine {
    #[serde(rename = "whisper.cpp")]
    WhisperCpp,
    #[serde(rename = "android-on-device")]
    AndroidOnDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechCapture {
    Pcm,
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechLanguage {
    pub tag: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechPermission {
    Unknown,
    Prompt,
    Granted,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechModelState {
    Missing,
    Downloading,
    Verifying,
    Ready,
    Error,
    SystemManaged,
}

/// Preparation history for this foreground page; each start still needs a fresh SDK confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechModelConfirmation {
    /// Always `en-US`.
    pub language: String,
    /// Always `android-download-success`.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechModel {
    pub state: SpeechModelState,
    pub bytes: u64,
    pub received: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<SpeechModelConfirmation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechStatus {
    pub engine: SpeechEngine,
    pub capture: SpeechCapture,
    pub available: bool,
    pub languages: Vec<SpeechLanguage>,
    pub permission: SpeechPermission,
    pub model: SpeechModel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<SpeechProblem>,
}

impl Wire for SpeechStatus {
    const NAME: &'static str = "SpeechStatus";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechStart {
    pub session_id: String,
    pub language: String,
}

impl Wire for SpeechStart {
    const NAME: &'static str = "SpeechStart";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechSession {
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechPhase {
    Listening,
    Transcribing,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum SpeechEvent {
    Status {
        status: SpeechStatus,
    },
    Session {
        session_id: String,
        phase: SpeechPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        problem: Option<SpeechProblem>,
    },
    Final {
        session_id: String,
        text: String,
    },
}

impl Wire for SpeechEvent {
    const NAME: &'static str = "SpeechEvent";
}
