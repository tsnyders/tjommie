//! `remote.ts`

use std::sync::LazyLock;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::js_trim;
use crate::{Invalid, Wire, parse};

fn remote_name(value: &str) -> Option<String> {
    (!value.chars().any(|c| c < ' ' || c == '\x7f')).then(|| value.to_owned())
}

refined_string!(
    /// A device or host name without control characters.
    RemoteName, remote_name, "Enter a device name without control characters."
);

/// A canonical unpadded base64url encoding of 32 bytes.
fn remote_key(value: &str) -> Option<String> {
    let alphabet = value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    // The engine rejects nonzero trailing bits, so only the canonical spelling of each key decodes.
    (alphabet && URL_SAFE_NO_PAD.decode(value).is_ok()).then(|| value.to_owned())
}

refined_string!(
    /// A public key, nonce or channel id.
    RemoteKey, remote_key, "Invalid public key."
);

/// zod's URL check (`$ZodURL` in zod 4.6.5): trims with `String.prototype.trim`, parses as a WHATWG URL, and keeps the
/// trimmed text without tabs or newlines. Returns the parsed URL and the text to store.
pub(crate) fn zod_url(value: &str) -> Option<(url::Url, String)> {
    let trimmed = js_trim(value);
    let url = url::Url::parse(trimmed).ok()?;
    Some((url, trimmed.replace(['\t', '\n', '\r'], "")))
}

/// `z.url({ protocol: /^https$/ })`
fn relay_url(value: &str) -> Option<String> {
    zod_url(value)
        .filter(|(url, _)| url.scheme() == "https")
        .map(|(_, text)| text)
}

refined_string!(
    /// An https relay URL.
    RelayUrl, relay_url, "Invalid URL"
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanEndpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    /// Always 2.
    pub v: u8,
    pub relay: Option<RelayUrl>,
    pub pairing_id: String,
    pub host_id: String,
    pub host_name: RemoteName,
    pub host_key: RemoteKey,
    pub secret: RemoteKey,
    pub lan: Vec<LanEndpoint>,
    pub expires: String,
}

impl Wire for PairingCode {
    const NAME: &'static str = "PairingCode";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: RemoteName,
    pub public_key: RemoteKey,
    pub paired_at: String,
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayState {
    Off,
    Connecting,
    Online,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayStatus {
    pub url: Option<RelayUrl>,
    pub state: RelayState,
    pub push: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub host_id: Option<String>,
    pub host_name: RemoteName,
    pub listening: bool,
    pub port: Option<u16>,
    pub connected_device_ids: Vec<String>,
    pub relay: RelayStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrefaceMode {
    Pair,
    Resume,
}

/// The first thing a remote client sends. A struct rather than a tagged enum: its JSON is the Noise prologue, so the keys
/// must come out in the schema's order (`v`, `hostId`, `mode`, `pairingId`) to match the TypeScript peer byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePreface {
    /// Always 1.
    pub v: u8,
    pub host_id: String,
    pub mode: PrefaceMode,
    /// Present exactly when `mode` is `Pair`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_id: Option<String>,
}

impl RemotePreface {
    pub fn pair(host_id: impl Into<String>, pairing_id: impl Into<String>) -> Self {
        Self {
            v: 1,
            host_id: host_id.into(),
            mode: PrefaceMode::Pair,
            pairing_id: Some(pairing_id.into()),
        }
    }

    pub fn resume(host_id: impl Into<String>) -> Self {
        Self {
            v: 1,
            host_id: host_id.into(),
            mode: PrefaceMode::Resume,
            pairing_id: None,
        }
    }
}

impl Wire for RemotePreface {
    const NAME: &'static str = "RemotePreface";
}

/// `remotePrologue`: the bytes both sides mix into the Noise handshake.
pub fn remote_prologue(preface: &RemotePreface) -> Result<Vec<u8>, Invalid> {
    let valid: RemotePreface = parse(serde_json::to_value(preface).expect("a preface serializes"))?;
    Ok(format!(
        "Tjommie remote v1\n{}",
        serde_json::to_string(&valid).expect("a preface serializes")
    )
    .into_bytes())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "confirm")]
pub struct RemoteConfirm {
    /// Always 1.
    pub protocol: u8,
    pub name: RemoteName,
}

impl Wire for RemoteConfirm {
    const NAME: &'static str = "RemoteConfirm";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "ready", rename_all = "camelCase")]
pub struct RemoteReady {
    pub device_id: String,
    pub host_id: String,
    pub host_name: RemoteName,
    pub relay: Option<RelayUrl>,
}

impl Wire for RemoteReady {
    const NAME: &'static str = "RemoteReady";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefusedReason {
    Revoked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "refused")]
pub struct RemoteRefused {
    pub reason: RefusedReason,
}

impl Wire for RemoteRefused {
    const NAME: &'static str = "RemoteRefused";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    Approval,
    Limit,
    RoutineFailed,
    Done,
    Failed,
}

impl PushKind {
    pub const ALL: [PushKind; 5] = [
        PushKind::Approval,
        PushKind::Limit,
        PushKind::RoutineFailed,
        PushKind::Done,
        PushKind::Failed,
    ];
}

const INVALID_PAIRING: &str = "This pairing code is invalid or expired.";
static PAIRING_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{16,8192}$").unwrap());

/// `encodePairingCode`: unpadded base64url of the code's JSON, keys in schema order.
pub fn encode_pairing_code(code: &PairingCode) -> Result<String, Invalid> {
    let valid: PairingCode = parse(serde_json::to_value(code).expect("a pairing code serializes"))?;
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_string(&valid).expect("a pairing code serializes")))
}

/// `parsePairingCode`: accepts the bare code or a `tjommie://pair#` URI that expires within the next 11 minutes. `now` is
/// milliseconds since the Unix epoch. Every failure gives the same message, so the input is never echoed.
pub fn parse_pairing_code(input: &str, now: i64) -> Result<PairingCode, Invalid> {
    let decode = || -> Option<PairingCode> {
        let code = input.strip_prefix("tjommie://pair#").unwrap_or(input);
        if !PAIRING_CODE.is_match(code) {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(code).ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        let parsed: PairingCode = parse(value).ok()?;
        if encode_pairing_code(&parsed).ok()? != code {
            return None;
        }
        let remaining = parsed
            .expires
            .parse::<jiff::Timestamp>()
            .ok()?
            .as_millisecond()
            - now;
        (remaining > 0 && remaining <= 660_000).then_some(parsed)
    };
    decode().ok_or_else(|| Invalid(INVALID_PAIRING.into()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesRename {
    pub id: String,
    pub name: RemoteName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRegisterPush {
    /// `None` unregisters.
    pub token: Option<String>,
    /// Push kinds this device wants. Unknown kinds are kept, so newer apps can ask for kinds this host doesn't send yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingStart {
    pub id: String,
    pub code: String,
    pub uri: String,
    pub expires: String,
}
