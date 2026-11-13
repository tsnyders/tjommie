//! `vault-host.ts`: jobs the daemon queues for the desktop's vault host, and its replies.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::vault::{VaultHostAction, VaultHostCommand};
use crate::{Invalid, Wire, validate};

pub const VAULT_HOST_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultHostJob {
    pub id: String,
    pub sequence: u64,
    pub command: VaultHostCommand,
}

impl Wire for VaultHostJob {
    const NAME: &'static str = "VaultHostJob";
}

/// `{ id, ok: true, result }` or `{ id, ok: false }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultHostReply {
    pub id: String,
    pub ok: bool,
    /// Unread here; check it with [`vault_host_result`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

impl Wire for VaultHostReply {
    const NAME: &'static str = "VaultHostReply";
}

/// `vaultHostResult`: checks a reply's result against the `vault.*` method the action stands for. `disconnect` answers `{}`.
/// The checked value goes out unchanged as that method's result.
pub fn vault_host_result(action: &VaultHostAction, value: Value) -> Result<Value, Invalid> {
    let method = match action {
        VaultHostAction::Status => "vault.status",
        VaultHostAction::List(_) => "vault.list",
        VaultHostAction::Unlock => "vault.unlock",
        VaultHostAction::Lock => "vault.lock",
        VaultHostAction::Entry(_) => "vault.entry",
        VaultHostAction::Delete(_) => "vault.delete",
        VaultHostAction::Fill(_) => "vault.fill",
        VaultHostAction::Finish(_) => "vault.finish",
        VaultHostAction::Cancel => "vault.cancel",
        VaultHostAction::Disconnect => {
            return match &value {
                Value::Object(fields) if fields.is_empty() => Ok(value),
                _ => Err(Invalid("Expected {}".into())),
            };
        }
    };
    validate(method, &value)?;
    Ok(value)
}
