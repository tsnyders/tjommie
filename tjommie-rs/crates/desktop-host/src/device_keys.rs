//! This device's keys for the PCs it is paired with, in Windows Credential Manager: port of
//! `packages/desktop/src/device-keys.ts` and `credential-transport.ts`. The target is `Tjommie/<ns24>/device/<name>`,
//! the value is the key as unpadded base64url text, and the credential itself (generic type, local-machine
//! persistence, user name `Tjommie`) is the one the TypeScript script writes and `daemon::win::cred` reads, so
//! what 0.5.x saved is found here byte for byte.

use std::fmt;
use std::io;
use std::path::{Component, Path};
use std::{ptr, slice};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows::core::{HSTRING, PWSTR};
use zeroize::Zeroizing;

/// Credential Manager's limit for a generic blob.
const MAX_BLOB: usize = 2560;
/// 1920 raw bytes are exactly 2560 base64url characters.
const MAX_KEY_BYTES: usize = 1920;
const USER_NAME: &str = "Tjommie";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKeyError {
    /// A name or key the TypeScript would refuse.
    Invalid,
    /// Credential Manager failed, or held something that isn't a device key. Never says more: no key material, no
    /// Windows detail.
    Unavailable,
}

impl fmt::Display for DeviceKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "Invalid device key.",
            Self::Unavailable => "Device key storage is unavailable.",
        })
    }
}

impl std::error::Error for DeviceKeyError {}

/// Where credentials live: Credential Manager in the app, a table in tests.
pub trait CredentialStore {
    /// A missing target is `None`.
    fn read(&self, target: &str) -> io::Result<Option<String>>;
    fn write(&self, target: &str, value: &str) -> io::Result<()>;
    /// A missing target is not an error.
    fn delete(&self, target: &str) -> io::Result<()>;
}

/// Windows Credential Manager through `CredReadW`, `CredWriteW` and `CredDeleteW`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsCredentials;

/// The read itself, for a caller that already holds [`credman::locked`].
fn read_unlocked(target: &str) -> io::Result<Option<String>> {
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

impl CredentialStore for WindowsCredentials {
    fn read(&self, target: &str) -> io::Result<Option<String>> {
        credman::locked(|| read_unlocked(target))
    }

    fn write(&self, target: &str, value: &str) -> io::Result<()> {
        if value.is_empty() || value.len() > MAX_BLOB {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let target = HSTRING::from(target);
        let user = HSTRING::from(USER_NAME);
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_ptr().cast_mut()),
            CredentialBlobSize: value.len() as u32,
            // CredWriteW only reads the blob.
            CredentialBlob: value.as_ptr().cast_mut(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_ptr().cast_mut()),
            ..Default::default()
        };
        // SAFETY: every pointer in `credential` outlives the call.
        credman::locked(|| unsafe { CredWriteW(&credential, 0) }).map_err(io::Error::from)
    }

    fn delete(&self, target: &str) -> io::Result<()> {
        credman::locked(|| {
            credman::delete_gone(
                || delete_unlocked(target),
                || read_unlocked(target).map(|found| found.is_some()),
            )
        })
    }
}

fn delete_unlocked(target: &str) -> io::Result<()> {
    // SAFETY: plain call with a null-terminated target.
    match unsafe { CredDeleteW(&HSTRING::from(target), CRED_TYPE_GENERIC, None) } {
        Err(error) if error.code() != ERROR_NOT_FOUND.to_hresult() => Err(error.into()),
        _ => Ok(()),
    }
}

/// The Credential Manager namespace of one data root: the first 24 hex characters of the sha256 of the lowercased,
/// resolved root. A different root gives different targets, which keeps dev and test installs away from production.
/// The rule of `credentialNamespace` in `credential-transport.ts` and `Namespace::short` in the daemon.
pub fn credential_namespace(root: &Path) -> io::Result<String> {
    let digest = Sha256::digest(resolve(root)?.to_lowercase().as_bytes());
    Ok(format!("{digest:x}").chars().take(24).collect())
}

/// Node's `path.resolve()` on Windows: absolute, `\` separators, no `.` or `..`, and no trailing separator except after
/// a bare drive or share. A last name ending in a dot or a space is refused: Windows drops it and Node keeps it, so it
/// would silently get another namespace than the TypeScript app gives it. (Copy of `daemon::win::cred::resolve`.)
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

/// `^[a-z0-9._-]{1,64}$`
fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

/// This install's device keys. The namespace comes from the data root, so a dev or test root never reaches the
/// production entries.
pub struct DeviceKeys<S: CredentialStore = WindowsCredentials> {
    namespace: String,
    store: S,
}

impl DeviceKeys {
    pub fn new(data_root: &Path) -> io::Result<Self> {
        Self::with_store(data_root, WindowsCredentials)
    }
}

impl<S: CredentialStore> DeviceKeys<S> {
    pub fn with_store(data_root: &Path, store: S) -> io::Result<Self> {
        Ok(Self {
            namespace: credential_namespace(data_root)?,
            store,
        })
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    fn target(&self, name: &str) -> Result<String, DeviceKeyError> {
        if valid_name(name) {
            Ok(format!("Tjommie/{}/device/{name}", self.namespace))
        } else {
            Err(DeviceKeyError::Invalid)
        }
    }

    /// Saves `key` (1 to 1920 bytes) under `name`, replacing what was there.
    pub fn seal(&self, name: &str, key: &[u8]) -> Result<(), DeviceKeyError> {
        let target = self.target(name)?;
        if key.is_empty() || key.len() > MAX_KEY_BYTES {
            return Err(DeviceKeyError::Invalid);
        }
        let text = Zeroizing::new(URL_SAFE_NO_PAD.encode(key));
        self.store
            .write(&target, &text)
            .map_err(|_| DeviceKeyError::Unavailable)
    }

    /// The key under `name`, or `None`. What is stored must be exactly what `seal` writes: canonical unpadded base64url
    /// of 1 to 1920 bytes.
    pub fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, DeviceKeyError> {
        let target = self.target(name)?;
        let Some(saved) = self
            .store
            .read(&target)
            .map_err(|_| DeviceKeyError::Unavailable)?
            .map(Zeroizing::new)
        else {
            return Ok(None);
        };
        let characters_ok = (1..=MAX_BLOB).contains(&saved.len())
            && saved
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        let bytes = characters_ok
            .then(|| URL_SAFE_NO_PAD.decode(saved.as_bytes()).ok())
            .flatten()
            .map(Zeroizing::new)
            .filter(|bytes| {
                (1..=MAX_KEY_BYTES).contains(&bytes.len())
                    && Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice())).as_str()
                        == saved.as_str()
            })
            .ok_or(DeviceKeyError::Unavailable)?;
        Ok(Some(bytes))
    }

    pub fn delete(&self, name: &str) -> Result<(), DeviceKeyError> {
        self.store
            .delete(&self.target(name)?)
            .map_err(|_| DeviceKeyError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io;
    use std::path::Path;

    use super::*;

    /// An in-memory Credential Manager that remembers what it was asked, like the TypeScript test's transport.
    #[derive(Default)]
    struct Fake {
        records: RefCell<HashMap<String, String>>,
        operations: RefCell<Vec<String>>,
        fail: bool,
    }

    impl CredentialStore for Fake {
        fn read(&self, target: &str) -> io::Result<Option<String>> {
            self.operations.borrow_mut().push(format!("read:{target}"));
            if self.fail {
                return Err(io::ErrorKind::Other.into());
            }
            Ok(self.records.borrow().get(target).cloned())
        }
        fn write(&self, target: &str, value: &str) -> io::Result<()> {
            self.operations.borrow_mut().push(format!("write:{target}"));
            if self.fail {
                return Err(io::ErrorKind::Other.into());
            }
            self.records
                .borrow_mut()
                .insert(target.to_owned(), value.to_owned());
            Ok(())
        }
        fn delete(&self, target: &str) -> io::Result<()> {
            self.operations
                .borrow_mut()
                .push(format!("delete:{target}"));
            self.records.borrow_mut().remove(target);
            Ok(())
        }
    }

    // sha256 of the lowercased, resolved root, first 24 hex: what Node's credentialNamespace gives these roots
    // (computed with createHash("sha256").update(resolve(root).toLowerCase())).
    const ISOLATED: &str = "c61db3b0384f6c785b097754";
    const NAME: &str = "identity-5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11.pending";

    fn keys(store: Fake) -> DeviceKeys<Fake> {
        DeviceKeys::with_store(Path::new("C:/isolated/tjommie-test"), store).unwrap()
    }

    /// The real store's seal, read and delete, each under the shared lock (one nested in another would hang here), in a
    /// namespace of this test's own. Skipped on hosted runners, which cannot reliably reach Credential Manager.
    #[test]
    fn the_real_store_keeps_and_forgets_a_device_key_under_the_lock() {
        if std::env::var("TJOMMIE_CI_HOSTED").as_deref() == Ok("1") {
            return;
        }
        let root = crate::owner_safety::test_root().unwrap();
        let keys = DeviceKeys::new(root.path()).unwrap();

        keys.seal(NAME, &[1, 2, 3]).unwrap();
        let read = keys.read(NAME);
        keys.delete(NAME).unwrap();
        assert_eq!(read.unwrap().unwrap().as_slice(), [1, 2, 3]);
        assert_eq!(keys.read(NAME).unwrap(), None);
    }

    #[test]
    fn namespaces_match_the_typescript_ones() {
        let namespace = |root: &str| credential_namespace(Path::new(root)).unwrap();
        assert_eq!(namespace("C:/isolated/tjommie-test"), ISOLATED);
        // Case and separators don't matter, like path.resolve + toLowerCase.
        assert_eq!(namespace("c:\\ISOLATED\\Tjommie-Test"), ISOLATED);
        assert_eq!(
            namespace("C:\\Users\\owner\\AppData\\Local\\Tjommie-rs"),
            "2d068a9436c1011964fd7acc"
        );
        assert_eq!(namespace("C:\\"), "3dc8472eaebe1a9d728dd16a");
        // A trailing separator is dropped, as path.resolve drops it.
        assert_eq!(namespace("C:\\isolated\\tjommie-test\\"), ISOLATED);
    }

    #[test]
    fn seals_reads_and_deletes_a_device_key_with_the_typescripts_target_and_bytes() {
        let keys = keys(Fake::default());
        let target = format!("Tjommie/{ISOLATED}/device/{NAME}");
        assert_eq!(keys.read(NAME).unwrap(), None);
        keys.seal(NAME, &[0, 1, 127, 255]).unwrap();
        // base64url without padding, as Buffer.toString("base64url").
        assert_eq!(
            keys.store()
                .records
                .borrow()
                .get(&target)
                .map(String::as_str),
            Some("AAF__w")
        );
        assert_eq!(
            keys.read(NAME).unwrap().as_deref().map(Vec::as_slice),
            Some(&[0u8, 1, 127, 255][..])
        );
        keys.delete(NAME).unwrap();
        assert_eq!(keys.read(NAME).unwrap(), None);
        let expected: Vec<String> = ["read", "write", "read", "delete", "read"]
            .iter()
            .map(|operation| format!("{operation}:{target}"))
            .collect();
        assert_eq!(*keys.store().operations.borrow(), expected);
    }

    #[test]
    fn refuses_names_and_values_the_typescript_refuses() {
        let keys = keys(Fake::default());
        for name in [
            "",
            "UPPER",
            "has space",
            "a/b",
            "a\\b",
            &"a".repeat(65),
            "é",
        ] {
            assert_eq!(
                keys.seal(name, &[1]),
                Err(DeviceKeyError::Invalid),
                "{name:?}"
            );
            assert_eq!(
                keys.read(name).unwrap_err(),
                DeviceKeyError::Invalid,
                "{name:?}"
            );
            assert_eq!(keys.delete(name), Err(DeviceKeyError::Invalid), "{name:?}");
        }
        for name in ["a", "a.b_c-d", &"a".repeat(64), "0"] {
            keys.seal(name, &[1]).unwrap();
        }
        assert_eq!(keys.seal(NAME, &[]), Err(DeviceKeyError::Invalid));
        assert_eq!(
            keys.seal(NAME, &vec![7; 1921]),
            Err(DeviceKeyError::Invalid)
        );
        keys.seal(NAME, &vec![7; 1920]).unwrap(); // 1920 bytes is exactly the 2560-character limit
        assert_eq!(
            keys.read(NAME).unwrap().map(|bytes| bytes.len()),
            Some(1920)
        );
        // Nothing reached the store for a refused call.
        assert!(
            !keys
                .store()
                .operations
                .borrow()
                .iter()
                .any(|operation| operation.contains("UPPER"))
        );
    }

    #[test]
    fn a_stored_value_that_is_not_a_canonical_device_key_is_unavailable() {
        let target = format!("Tjommie/{ISOLATED}/device/{NAME}");
        for stored in [
            "AAF__w=",         // padding
            "AAF__x", // non-canonical trailing bits: decodes, but doesn't re-encode to itself
            "AAF/+w", // standard alphabet
            "",       // empty
            &"A".repeat(2561), // too long to be a device key
            &"A".repeat(2560), // decodes to 1920 bytes + 0: 2560 chars is 1920 bytes, valid length; see below
            "AA AA",           // whitespace
        ] {
            let fake = Fake::default();
            fake.records
                .borrow_mut()
                .insert(target.clone(), stored.to_owned());
            let result = keys(fake).read(NAME);
            // 2560 characters of "A" is a canonical encoding of 1920 zero bytes: the one valid entry in this list.
            if stored.len() == 2560 {
                assert_eq!(result.unwrap().map(|bytes| bytes.len()), Some(1920));
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    DeviceKeyError::Unavailable,
                    "{stored:?}"
                );
            }
        }
    }

    #[test]
    fn a_store_that_fails_is_unavailable_and_says_nothing_more() {
        let keys = keys(Fake {
            fail: true,
            ..Fake::default()
        });
        assert_eq!(keys.seal(NAME, &[1]), Err(DeviceKeyError::Unavailable));
        assert_eq!(keys.read(NAME).unwrap_err(), DeviceKeyError::Unavailable);
        assert_eq!(
            DeviceKeyError::Unavailable.to_string(),
            "Device key storage is unavailable."
        );
        assert_eq!(DeviceKeyError::Invalid.to_string(), "Invalid device key.");
    }
}
