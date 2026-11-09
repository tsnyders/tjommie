//! `vault.ts`. No secret, username, selector, script, credential target or host path crosses this protocol.
//! `VaultNativeApi` has no port: it is the JavaScript bridge.

use serde::{Deserialize, Serialize};

use crate::Wire;

/// An exact https origin such as `https://example.com`: no path, credentials, query or fragment, and in canonical form.
fn login_origin(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let exact = url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.origin().ascii_serialization() == value;
    exact.then(|| value.to_owned())
}

refined_string!(
    LoginOrigin,
    login_origin,
    "Use an exact HTTPS origin, for example https://example.com."
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginEntry {
    pub id: String,
    pub name: String,
    pub origin: LoginOrigin,
    pub bot_id: String,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultState {
    Locked,
    Unlocked,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub state: VaultState,
    pub reason: Option<String>,
    pub pending_deletes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFillTarget {
    pub bot_id: String,
    pub generation: String,
    pub page_id: String,
    pub control_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultEntry {
    pub bot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryStatus {
    Saved,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultEntryResult {
    pub status: EntryStatus,
    pub entry: Option<LoginEntry>,
}

/// An entry at a revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryVersion {
    pub id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeleteStatus {
    Deleted,
    Pending,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultDeleteResult {
    pub status: DeleteStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFill {
    pub bot_id: String,
    pub generation: String,
    pub page_id: String,
    pub control_epoch: u64,
    pub id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FillStatus {
    Filled,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFillResult {
    pub status: FillStatus,
    pub private_epoch: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FinishMode {
    Clear,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFinish {
    pub bot_id: String,
    pub generation: String,
    pub private_epoch: u64,
    pub mode: FinishMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FinishStatus {
    Cleared,
    Closed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultFinishResult {
    pub status: FinishStatus,
}

/// What the daemon asks the desktop's vault host to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VaultHostAction {
    Status,
    List(VaultList),
    Unlock,
    Lock,
    Entry(VaultEntry),
    Delete(EntryVersion),
    Fill(VaultFill),
    Finish(VaultFinish),
    Disconnect,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultHostCommand {
    pub controller_id: String,
    pub action: VaultHostAction,
}

impl Wire for VaultHostCommand {
    const NAME: &'static str = "VaultHostCommand";
}
