//! Windows Credential Manager for the login vault's secrets, byte-compatible with the TypeScript desktop
//! (`packages/desktop/src/credential-transport.ts` and `vault-storage.ts`): generic credentials, local-machine
//! persistence, user name `Tjommie`, UTF-8 blobs of 1 to 2560 bytes. Targets are `Tjommie/<ns24>/login/<reference>`,
//! where `<ns24>` is derived from the data root, so a test root gets a namespace of its own.

use crate::vault::storage::{VaultError, VaultSecret, secret_json, validate_vault_secret};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Component, Path};
use std::{io, ptr, slice};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows::core::{HSTRING, PWSTR};
use zeroize::{Zeroize, Zeroizing};

/// The largest blob Credential Manager accepts here, which the TypeScript transport enforces too.
pub const MAX_BLOB: usize = 2560;
const USER_NAME: &str = "Tjommie";

/// `credentialNamespace(dataRoot)`: the first 24 hex characters of the sha256 of the resolved, lowercased root.
pub fn namespace(data_root: &Path) -> io::Result<String> {
    let resolved = resolve(data_root)?;
    let digest = Sha256::digest(resolved.to_lowercase().as_bytes());
    Ok(digest
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// `Tjommie/<namespace>/login/<reference>`. `None` unless the reference is 36 characters of `[0-9a-f-]`.
pub fn login_target(namespace: &str, reference: &str) -> Option<String> {
    let bytes = reference.as_bytes();
    let valid = bytes.len() == 36
        && bytes
            .iter()
            .all(|&byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'));
    valid.then(|| format!("Tjommie/{namespace}/login/{reference}"))
}

/// Reads a generic credential's blob. A missing target is `None`. The copy is wiped on drop.
pub fn read_blob(target: &str) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    credman::locked(|| read_unlocked(target))
}

/// `read_blob`, for a caller that already holds [`credman::locked`].
fn read_unlocked(target: &str) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    let mut pointer: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: on success CredReadW allocates one credential. Its blob is copied out and wiped in place, then the
    // allocation is freed exactly once with CredFree. The size is checked before the blob's memory is viewed, and an
    // empty blob (whose pointer may be null) is never viewed at all.
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
        let blob = if size == 0 {
            Ok(Zeroizing::new(Vec::new()))
        } else if size <= MAX_BLOB {
            let bytes = slice::from_raw_parts_mut(credential.CredentialBlob, size);
            let copy = Zeroizing::new(bytes.to_vec());
            bytes.zeroize();
            Ok(copy)
        } else {
            Err(io::Error::from(io::ErrorKind::InvalidData))
        };
        CredFree(pointer as *const _);
        blob.map(Some)
    }
}

/// Writes a generic, local-machine credential under user name `Tjommie`, as the TypeScript transport does.
pub fn write_blob(target: &str, blob: &[u8]) -> io::Result<()> {
    if blob.is_empty() || blob.len() > MAX_BLOB {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let target = HSTRING::from(target);
    let user = HSTRING::from(USER_NAME);
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_ptr().cast_mut()),
        CredentialBlobSize: blob.len() as u32,
        // CredWriteW only reads the blob, so it points at the caller's bytes; the caller wipes them.
        CredentialBlob: blob.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_ptr().cast_mut()),
        ..Default::default()
    };
    // SAFETY: every pointer in `credential` (and the HSTRINGs they point into) outlives the call, and CredWriteW
    // only reads through them.
    credman::locked(|| unsafe { CredWriteW(&credential, 0) }).map_err(io::Error::from)
}

/// Deletes a generic credential. A missing target is not an error. One Windows keeps all the same is.
pub fn delete_blob(target: &str) -> io::Result<()> {
    credman::locked(|| {
        credman::delete_gone(
            || delete_unlocked(target),
            || read_unlocked(target).map(|found| found.is_some()),
        )
    })
}

fn delete_unlocked(target: &str) -> io::Result<()> {
    let target = HSTRING::from(target);
    // SAFETY: CredDeleteW reads the null-terminated target, which the HSTRING keeps alive for the call.
    match unsafe { CredDeleteW(&target, CRED_TYPE_GENERIC, None) } {
        Err(error) if error.code() != ERROR_NOT_FOUND.to_hresult() => Err(error.into()),
        _ => Ok(()),
    }
}

/// The stored JSON, read back. Extra keys are ignored, as `JSON.parse` followed by the property checks does.
#[derive(Deserialize)]
struct StoredSecret {
    username: String,
    password: String,
}

/// `WindowsLoginCredentials`: one data root's login secrets in Credential Manager.
#[derive(Debug)]
pub struct LoginCredentials {
    namespace: String,
}

impl LoginCredentials {
    pub fn new(data_root: &Path) -> Result<Self, VaultError> {
        namespace(data_root)
            .map(|namespace| Self { namespace })
            .map_err(|_| VaultError::Storage)
    }

    fn target(&self, reference: &str) -> Result<String, VaultError> {
        login_target(&self.namespace, reference).ok_or(VaultError::Storage)
    }

    /// Validates the secret, then writes it as the JSON the TypeScript vault writes.
    pub fn write(&self, reference: &str, secret: &VaultSecret) -> Result<(), VaultError> {
        validate_vault_secret(secret)?;
        let target = self.target(reference)?;
        let json = secret_json(secret)?;
        write_blob(&target, &json).map_err(|_| VaultError::Storage)
    }

    /// The stored secret, or `None` when there is none. Anything unreadable is `Storage`, as in the TypeScript.
    pub fn read(&self, reference: &str) -> Result<Option<VaultSecret>, VaultError> {
        let target = self.target(reference)?;
        let Some(raw) = read_blob(&target).map_err(|_| VaultError::Storage)? else {
            return Ok(None);
        };
        let stored: StoredSecret = serde_json::from_slice(&raw).map_err(|_| VaultError::Storage)?;
        let secret = VaultSecret::new(stored.username, stored.password);
        validate_vault_secret(&secret).map_err(|_| VaultError::Storage)?;
        Ok(Some(secret))
    }

    pub fn delete(&self, reference: &str) -> Result<(), VaultError> {
        let target = self.target(reference)?;
        delete_blob(&target).map_err(|_| VaultError::Storage)
    }
}

/// Node's `path.resolve()` on Windows: absolute, `\` separators, no `.` or `..`, and no trailing separator except
/// after a bare drive or share. `GetFullPathNameW` drops a trailing dot or space from the last name, which Node keeps,
/// so such a root is refused: it would get another namespace than the TypeScript one. Copied from the daemon's
/// `win/cred.rs`, which the daemon's namespace already relies on.
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
        .replace('/', "\\");
    let text = text.trim_end_matches('\\');
    let bare = Path::new(text)
        .components()
        .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir));
    Ok(if bare {
        format!("{text}\\")
    } else {
        text.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_is_the_one_the_typescript_vault_derives() {
        // node: createHash("sha256").update(resolve("C:\\synthetic-vault").toLowerCase()).digest("hex").slice(0, 24)
        assert_eq!(
            namespace(Path::new("C:\\synthetic-vault")).expect("a namespace"),
            "04d30d450a315626a7fcacd1"
        );
    }

    #[test]
    fn namespace_resolves_the_root_as_node_does() {
        // path.resolve("C:/SYNTHETIC-vault/") is "C:\SYNTHETIC-vault", and the lowercased digest ignores the case.
        let plain = namespace(Path::new("C:\\synthetic-vault")).expect("a namespace");
        assert_eq!(
            namespace(Path::new("C:/SYNTHETIC-vault/")).expect("a namespace"),
            plain
        );
    }

    #[test]
    fn a_root_ending_in_a_dot_or_a_space_is_refused() {
        assert!(namespace(Path::new("C:\\synthetic-vault.")).is_err());
        assert!(namespace(Path::new("C:\\synthetic-vault ")).is_err());
    }

    #[test]
    fn login_targets_take_only_a_lowercase_uuid_shaped_reference() {
        assert_eq!(
            login_target("ns", "00000000-0000-4000-8000-000000000123").as_deref(),
            Some("Tjommie/ns/login/00000000-0000-4000-8000-000000000123")
        );
        for bad in [
            "00000000-0000-4000-8000-00000000012",
            "00000000-0000-4000-8000-00000000012G",
            "00000000-0000-4000-8000-00000000012/",
            "..\\..\\escape",
        ] {
            assert!(login_target("ns", bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_secret_is_stored_as_the_json_the_typescript_vault_writes() {
        let secret = VaultSecret::new("Person With Spaces", "pa\tss\"\\\u{1}é");
        let json = secret_json(&secret).expect("the JSON encodes");
        assert_eq!(
            json.as_slice(),
            r#"{"username":"Person With Spaces","password":"pa\tss\"\\\u0001é"}"#.as_bytes()
        );
    }
}
