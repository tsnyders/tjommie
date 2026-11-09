//! Windows Credential Manager through `CredReadW`, `CredWriteW` and `CredDeleteW`, byte-compatible with the 0.4.2
//! TypeScript daemon (`account-credentials.ts:28-51`): generic type, local-machine persistence, user name `Tjommie`,
//! UTF-8 blobs of 1 to 2560 bytes. Target names and namespaces follow spec §3.7. The daemon never enumerates
//! Credential Manager; every target is derived from the data root.

use sha2::{Digest, Sha256};
use std::path::{Component, Path};
use std::{fmt, io, ptr, slice};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows::core::{HSTRING, PWSTR};

/// Credential Manager's limit for a generic blob, which the TypeScript transport also enforces.
pub const MAX_BLOB: usize = 2560;
const USER_NAME: &str = "Tjommie";

/// Errors with the TypeScript daemon's fixed messages. They never carry key material or Windows detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    InvalidKey,
    Unavailable,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidKey => "Enter a single API key of at most 2560 UTF-8 bytes, without whitespace.",
            Self::Unavailable => {
                "Tjommie could not access Windows Credential Manager. The API key was not exposed or saved to a file."
            }
        })
    }
}

impl std::error::Error for CredentialError {}

/// Reads a generic credential's blob as UTF-8. A missing target is `None`.
pub fn read(target: &str) -> io::Result<Option<String>> {
    let mut pointer: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: on success CredReadW hands us one allocation, which is copied from and then freed once with CredFree.
    unsafe {
        if let Err(error) = CredReadW(
            &HSTRING::from(target),
            CRED_TYPE_GENERIC,
            None,
            &mut pointer,
        ) {
            return if error.code() == ERROR_NOT_FOUND.to_hresult() {
                Ok(None)
            } else {
                Err(error.into())
            };
        }
        let credential = &*pointer;
        let size = credential.CredentialBlobSize as usize;
        let value = match size {
            0 => Ok(String::new()),
            // Lossy like the C# transport's Encoding.UTF8.GetString. Copied straight out, so no extra copy lingers.
            1..=MAX_BLOB => Ok(String::from_utf8_lossy(slice::from_raw_parts(
                credential.CredentialBlob,
                size,
            ))
            .into_owned()),
            _ => Err(io::Error::from(io::ErrorKind::InvalidData)),
        };
        CredFree(pointer as *const _);
        value.map(Some)
    }
}

/// Writes a generic, local-machine credential with user name `Tjommie`, exactly as the TypeScript daemon does.
pub fn write(target: &str, value: &str) -> io::Result<()> {
    if value.is_empty() || value.len() > MAX_BLOB {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let target = HSTRING::from(target);
    let user = HSTRING::from(USER_NAME);
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_ptr().cast_mut()),
        CredentialBlobSize: value.len() as u32,
        // CredWriteW only reads the blob, so it points at the caller's bytes and no copy needs wiping.
        CredentialBlob: value.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_ptr().cast_mut()),
        ..Default::default()
    };
    // SAFETY: every pointer in `credential` outlives the call.
    unsafe { CredWriteW(&credential, 0) }.map_err(io::Error::from)
}

/// Deletes a generic credential. A missing target is not an error.
pub fn delete(target: &str) -> io::Result<()> {
    // SAFETY: plain call with a null-terminated target.
    match unsafe { CredDeleteW(&HSTRING::from(target), CRED_TYPE_GENERIC, None) } {
        Err(error) if error.code() != ERROR_NOT_FOUND.to_hresult() => Err(error.into()),
        _ => Ok(()),
    }
}

/// `validateApiKey` from `account-credentials.ts`.
pub fn validate_api_key(key: &str) -> Result<(), CredentialError> {
    let invalid = key.is_empty()
        || key.trim_matches(js_whitespace) != key
        || key.bytes().any(|b| b <= 0x20 || b == 0x7f)
        || key.len() > MAX_BLOB;
    if invalid {
        Err(CredentialError::InvalidKey)
    } else {
        Ok(())
    }
}

/// What JavaScript's `String.prototype.trim()` removes. Rust's `trim()` differs at U+0085 and U+FEFF.
fn js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// The Credential Manager namespace of one data root: the sha256 of the lowercased, resolved root. A different
/// root gives different targets, which keeps dev and test installs away from production.
#[derive(Clone, Debug)]
pub struct Namespace {
    digest: String,
}

impl Namespace {
    pub fn new(root: &Path) -> io::Result<Self> {
        Ok(Self {
            digest: sha256_hex(resolve(root)?.to_lowercase().as_bytes()),
        })
    }

    /// 24 hex: `account/`, `login/` and the isolated worker targets.
    pub fn short(&self) -> &str {
        &self.digest[..24]
    }

    /// 32 hex: the `remote-host` and `relay-login` keys.
    pub fn long(&self) -> &str {
        &self.digest[..32]
    }

    /// `account-credentials.ts:92-97`. The id is bounded in UTF-16 units, like JavaScript's `length`.
    pub fn account_target(&self, account_id: &str) -> Result<String, CredentialError> {
        let units = account_id.encode_utf16().count();
        if units == 0 || units > 256 {
            return Err(CredentialError::Unavailable);
        }
        Ok(format!(
            "Tjommie/{}/account/{}",
            self.short(),
            sha256_hex(account_id.as_bytes())
        ))
    }

    /// `vault-storage.ts:27-32`. `None` unless the reference is 36 of `[0-9a-f-]`.
    pub fn login_target(&self, reference: &str) -> Option<String> {
        let valid = reference.len() == 36
            && reference
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'));
        valid.then(|| format!("Tjommie/{}/login/{reference}", self.short()))
    }

    /// `remote-identity.ts:17-20`.
    pub fn remote_host_target(&self) -> String {
        format!("Tjommie/{}/remote-host", self.long())
    }

    /// `relay-identity.ts:10-13`.
    pub fn relay_login_target(&self) -> String {
        format!("Tjommie/{}/relay-login", self.long())
    }

    /// `WindowsAccountCredentials.read`: a stored value that is not a valid key is an error.
    pub fn read_account_key(&self, account_id: &str) -> Result<Option<String>, CredentialError> {
        let key =
            read(&self.account_target(account_id)?).map_err(|_| CredentialError::Unavailable)?;
        if let Some(key) = &key {
            validate_api_key(key).map_err(|_| CredentialError::Unavailable)?;
        }
        Ok(key)
    }

    /// `WindowsAccountCredentials.write`: the key is checked before Credential Manager is touched.
    pub fn write_account_key(&self, account_id: &str, key: &str) -> Result<(), CredentialError> {
        validate_api_key(key)?;
        write(&self.account_target(account_id)?, key).map_err(|_| CredentialError::Unavailable)
    }

    pub fn delete_account_key(&self, account_id: &str) -> Result<(), CredentialError> {
        delete(&self.account_target(account_id)?).map_err(|_| CredentialError::Unavailable)
    }
}

/// Node's `path.resolve()` on Windows: absolute, `\` separators, no `.` or `..`, and no trailing separator except
/// after a bare drive or share. GetFullPathNameW drops a trailing dot or space from the last name and Node keeps
/// it, so such a root is refused: it would silently get a different namespace than the TypeScript daemon gives it.
fn resolve(root: &Path) -> io::Result<String> {
    if let Some(Component::Normal(name)) = root.components().next_back()
        && name.to_str().is_some_and(|name| name.ends_with(['.', ' ']))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Tjommie can't use a data folder whose name ends in a dot or a space.",
        ));
    }
    let absolute = std::path::absolute(root)?;
    let text = absolute
        .to_str()
        .ok_or(io::ErrorKind::InvalidInput)?
        .trim_end_matches('\\');
    let bare = Path::new(text)
        .components()
        .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir));
    Ok(if bare {
        format!("{text}\\")
    } else {
        text.to_owned()
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
