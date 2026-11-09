//! `attachments.ts`. `AttachmentNativeApi` has no port: it is the JavaScript bridge to the Android file APIs.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::Invalid;

pub const MAX_ATTACHMENT_BYTES: u64 = 200 * 1024 * 1024;
pub const ATTACHMENT_CHUNK_BYTES: u32 = 256 * 1024;
pub const MAX_ATTACHMENT_COMPANIONS: usize = 64;

/// `attachmentCompanionName`: a relative name such as `images/chart one.png`, percent-decoded and NFC-normalized, or `None`
/// for anything that could reach outside the attachment: traversal, absolute paths, drive letters, URLs, queries.
pub fn attachment_companion_name(source: &str) -> Option<String> {
    if source.is_empty() || source.encode_utf16().count() > 1024 {
        return None;
    }
    let normalized: String = decode_uri_component(source)?.nfc().collect();
    let mut name = normalized.as_str();
    while let Some(rest) = name.strip_prefix("./") {
        name = rest;
    }
    let forbidden = |c: char| {
        matches!(
            c,
            '\\' | '\u{0}'..='\u{1f}' | '\u{7f}' | '%' | '?' | '#' | ':'
        )
    };
    if name.chars().any(forbidden)
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(name.to_owned())
}

/// `decodeURIComponent`, which fails on a `%` without two hex digits and on bytes that aren't UTF-8.
fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = bytes
                .get(at + 1..at + 3)
                .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))?;
            decoded.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

refined_string!(
    /// A companion file's name, normalized by [`attachment_companion_name`].
    CompanionName, attachment_companion_name, "Use a relative companion name without traversal or a URL."
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentCompanion {
    pub name: CompanionName,
    pub attachment_id: String,
    pub attachment_index: u64,
}

/// Companions with unique names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AttachmentCompanions(Vec<AttachmentCompanion>);

impl AttachmentCompanions {
    pub fn new(companions: Vec<AttachmentCompanion>) -> Result<Self, Invalid> {
        let mut names = HashSet::new();
        if companions
            .iter()
            .all(|companion| names.insert(companion.name.as_str()))
        {
            Ok(Self(companions))
        } else {
            Err(Invalid("Companion names must be unique.".into()))
        }
    }

    pub fn as_slice(&self) -> &[AttachmentCompanion] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AttachmentCompanions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// A message's attachment: never a path or a caller's context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRef {
    pub message_id: String,
    pub attachment_index: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub id: String,
    pub host_id: String,
    pub name: String,
    pub size: u64,
    pub content_type: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentOpen {
    pub transfer_id: String,
    pub attachment: AttachmentInfo,
    /// Always [`ATTACHMENT_CHUNK_BYTES`].
    pub chunk_bytes: u32,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRead {
    pub transfer_id: String,
    pub offset: u64,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentChunk {
    pub transfer_id: String,
    /// The attachment's sha256.
    pub revision: String,
    pub offset: u64,
    /// Standard base64.
    pub bytes: String,
    pub eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferId {
    pub transfer_id: String,
}
