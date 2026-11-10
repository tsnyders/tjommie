//! `remote-identity.ts`: this PC's Noise key. The private half lives only in Credential Manager (`remote-host`, 32-hex
//! namespace); the database keeps the id, name and public half. A lost key is never silently replaced while devices
//! are paired to the old one.

use std::fmt;
use std::io;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use protocol::remote::RemoteKey;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use transport::{Failed, Keypair};
use zeroize::Zeroizing;

use crate::devices_store::remote_name;
use crate::ids::{new_id, now_iso};
use crate::win::cred::{self, Namespace};

/// Where the remote and relay keys are kept: Credential Manager in the daemon, a map in tests.
pub trait Vault: Send + Sync {
    fn read(&self, target: &str) -> io::Result<Option<String>>;
    fn write(&self, target: &str, value: &str) -> io::Result<()>;
}

/// The real store. Callers derive targets from the data root, so a test root never reaches production entries.
pub struct CredentialManager;

impl Vault for CredentialManager {
    fn read(&self, target: &str) -> io::Result<Option<String>> {
        cred::read(target)
    }
    fn write(&self, target: &str, value: &str) -> io::Result<()> {
        cred::write(target, value)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostIdentity {
    pub id: String,
    pub name: String,
    pub public_key: [u8; 32],
    pub private_key: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for HostIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostIdentity")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// The one error, with no detail: nothing about the vault or its contents may leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityError;

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "Tjommie could not load this PC's connection key from Windows Credential Manager. Existing devices have not been given a replacement identity.",
        )
    }
}

impl std::error::Error for IdentityError {}

/// `StoredIdentity`, in `JSON.stringify` key order so the entry is byte-identical to 0.4.2's.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredIdentity {
    v: u8,
    id: String,
    private_key: RemoteKey,
    public_key: RemoteKey,
}

fn key_bytes(key: &RemoteKey) -> Option<Zeroizing<[u8; 32]>> {
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(key.as_str()).ok()?);
    Some(Zeroizing::new(bytes.as_slice().try_into().ok()?))
}

pub(crate) fn key_text(key: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

pub fn load_remote_identity(
    db: &Connection,
    root: &Path,
    name: &str,
    vault: &dyn Vault,
    generate: impl FnOnce() -> Result<Keypair, Failed>,
) -> Result<HostIdentity, IdentityError> {
    let load = || -> Option<HostIdentity> {
        let target = Namespace::new(root).ok()?.remote_host_target();
        let row: Option<(String, String, String)> = db
            .query_row(
                "SELECT id, name, public_key AS publicKey FROM remote_host WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .ok()?;
        // `HostRow.parse`
        if let Some((id, name, key)) = &row {
            uuid::Uuid::try_parse(id).ok()?;
            remote_name(name).ok()?;
            RemoteKey::new(key).ok()?;
        }
        let identity = match vault.read(&target).ok()? {
            Some(saved) => {
                let identity: StoredIdentity = serde_json::from_str(&saved).ok()?;
                if identity.v != 1 || uuid::Uuid::try_parse(&identity.id).is_err() {
                    return None;
                }
                if row.as_ref().is_some_and(|(id, _, key)| {
                    *id != identity.id || key != identity.public_key.as_str()
                }) {
                    return None;
                }
                identity
            }
            None => {
                let devices: Option<i64> = db
                    .query_row("SELECT 1 FROM devices LIMIT 1", [], |row| row.get(0))
                    .optional()
                    .ok()?;
                if row.is_some() || devices.is_some() {
                    return None;
                }
                let keys = generate().ok()?;
                let identity = StoredIdentity {
                    v: 1,
                    id: new_id(),
                    private_key: RemoteKey::new(key_text(&keys.private_key[..])).ok()?,
                    public_key: RemoteKey::new(key_text(&keys.public_key)).ok()?,
                };
                let text = Zeroizing::new(serde_json::to_string(&identity).ok()?);
                vault.write(&target, &text).ok()?;
                identity
            }
        };
        let name = match &row {
            Some((_, name, _)) => name.clone(),
            None => {
                let name = remote_name(name).ok()?.as_str().to_owned();
                db.execute(
                    "INSERT INTO remote_host(singleton,id,name,public_key,created_at) VALUES(1,?,?,?,?)",
                    [&identity.id, &name, identity.public_key.as_str(), &now_iso()],
                )
                .ok()?;
                name
            }
        };
        Some(HostIdentity {
            public_key: *key_bytes(&identity.public_key)?,
            private_key: key_bytes(&identity.private_key)?,
            id: identity.id,
            name,
        })
    };
    load().ok_or(IdentityError)
}

/// The name a new identity gets: `os.hostname()`, the computer's DNS host name.
pub fn host_name() -> String {
    use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
    use windows::core::PWSTR;
    let mut buffer = [0u16; 256];
    let mut size = buffer.len() as u32;
    // SAFETY: the buffer and its length in UTF-16 units are passed together.
    let named = unsafe {
        GetComputerNameExW(
            ComputerNameDnsHostname,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
    };
    match named {
        Ok(()) => String::from_utf16_lossy(&buffer[..size as usize]),
        Err(_) => "This PC".into(),
    }
}
