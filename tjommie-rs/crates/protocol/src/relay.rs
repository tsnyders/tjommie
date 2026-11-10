//! `relay.ts`: the host side of the Cloudflare relay protocol.

use serde::{Deserialize, Serialize};

use crate::Wire;
pub use crate::remote::PushKind;
use crate::remote::RemoteKey;

pub const DEFAULT_RELAY_URL: Option<&str> =
    Some("https://tjommie-relay.tyrellsnyders30.workers.dev");

pub struct RelayLimits {
    pub max_frame: u32,
    pub host_ping_ms: u64,
    pub host_dead_ms: u64,
    pub accept_timeout_ms: u64,
    pub max_channels: u32,
    pub max_pending_channels: u32,
    pub connects_per_ip_per_minute: u32,
    pub max_push_tokens: u32,
}

pub const RELAY: RelayLimits = RelayLimits {
    max_frame: 65_535,
    host_ping_ms: 30_000,
    host_dead_ms: 75_000,
    accept_timeout_ms: 10_000,
    max_channels: 32,
    max_pending_channels: 8,
    connects_per_ip_per_minute: 20,
    max_push_tokens: 16,
};

/// What a device gets when it registers without saying which kinds it wants.
pub const LEGACY_PUSH_KINDS: [PushKind; 3] =
    [PushKind::Approval, PushKind::Limit, PushKind::RoutineFailed];

/// The bytes a host signs to log in to the relay: domain-separated so the signature can't be reused elsewhere.
pub fn login_message(host_id: &str, nonce: &str, relay_host: &str) -> Vec<u8> {
    format!("tjommie-relay-login/v1\n{host_id}\n{nonce}\n{relay_host}").into_bytes()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResult {
    pub ok: bool,
    pub unregistered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum RelayToHost {
    Challenge {
        nonce: RemoteKey,
    },
    Ready {
        push: bool,
        /// At most 16 of at most 32 characters: the manifest schema caps it, so decode with [`crate::parse`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kinds: Option<Vec<String>>,
    },
    Open {
        channel: RemoteKey,
    },
    Pushed {
        id: i64,
        /// At most 16: the manifest schema caps it, so decode with [`crate::parse`].
        results: Vec<PushResult>,
    },
}

impl Wire for RelayToHost {
    const NAME: &'static str = "RelayToHost";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum HostToRelay {
    Login {
        public_key: RemoteKey,
        /// An unpadded base64url Ed25519 signature of [`login_message`].
        signature: String,
    },
    Push {
        id: i64,
        tokens: Vec<String>,
        kind: PushKind,
        bot_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        room_id: Option<String>,
    },
}

impl Wire for HostToRelay {
    const NAME: &'static str = "HostToRelay";
}

/// The literal `"1"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PushDataVersion {
    #[serde(rename = "1")]
    V1,
}

/// What a push carries: ids only, never content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushData {
    pub v: PushDataVersion,
    pub host_id: String,
    pub kind: PushKind,
    pub bot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
}

impl Wire for PushData {
    const NAME: &'static str = "PushData";
}

pub struct RelayCloseCodes {
    pub host_offline: u16,
    pub bad_login: u16,
    pub replaced: u16,
    pub host_dead: u16,
    pub too_large: u16,
    pub limited: u16,
}

pub const RELAY_CLOSE: RelayCloseCodes = RelayCloseCodes {
    host_offline: 4404,
    bad_login: 4401,
    replaced: 4409,
    host_dead: 4408,
    too_large: 1009,
    limited: 4429,
};
