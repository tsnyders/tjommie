//! `relay-identity.ts`: the Ed25519 key this PC logs in to the relay with. It lives only in Credential Manager
//! (`relay-login`, 32-hex namespace) as `{"v":1,"privateKey":<PKCS#8 DER>,"publicKey":<raw>}`, both unpadded base64url,
//! exactly as Node's `KeyObject.export` writes it, so 0.4.2 reads a key Rust created and the reverse.

use std::fmt;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use protocol::remote::RemoteKey;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::remote_identity::Vault;
use crate::win::cred::Namespace;

/// RFC 8410 PKCS#8 v1 for Ed25519 up to the seed: what Node exports for `type: "pkcs8"`.
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

pub struct RelayIdentity {
    pub public_key: [u8; 32],
    key: SigningKey,
}

impl RelayIdentity {
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }
}

impl fmt::Debug for RelayIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayIdentity")
            .field("public_key", &URL_SAFE_NO_PAD.encode(self.public_key))
            .finish_non_exhaustive()
    }
}

/// The one error, with no detail from the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayIdentityError;

impl fmt::Display for RelayIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "Tjommie could not load this PC's relay connection key from Windows Credential Manager. No replacement identity was created.",
        )
    }
}

impl std::error::Error for RelayIdentityError {}

/// `StoredKey`, in `JSON.stringify` key order.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredKey {
    v: u8,
    private_key: Zeroizing<String>,
    public_key: RemoteKey,
}

pub fn load_relay_identity(
    root: &Path,
    vault: &dyn Vault,
) -> Result<RelayIdentity, RelayIdentityError> {
    let load = || -> Option<RelayIdentity> {
        let target = Namespace::new(root).ok()?.relay_login_target();
        let saved = match vault.read(&target).ok()? {
            Some(saved) => Zeroizing::new(saved),
            None => {
                let mut seed = Zeroizing::new([0u8; 32]);
                getrandom::fill(seed.as_mut()).ok()?;
                let key = SigningKey::from_bytes(&seed);
                let der = Zeroizing::new([&PKCS8_PREFIX[..], &seed[..]].concat());
                let record = StoredKey {
                    v: 1,
                    private_key: Zeroizing::new(URL_SAFE_NO_PAD.encode(&der)),
                    public_key: RemoteKey::new(
                        URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()),
                    )
                    .ok()?,
                };
                let text = Zeroizing::new(serde_json::to_string(&record).ok()?);
                vault.write(&target, &text).ok()?;
                text
            }
        };
        let record: StoredKey = serde_json::from_str(&saved).ok()?;
        let length = record.private_key.len();
        let alphabet = record
            .private_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if record.v != 1 || !(64..=512).contains(&length) || !alphabet {
            return None;
        }
        // ponytail: only Node's 48-byte v1 encoding is read; a v2 (with public key) export would need a DER parser.
        let der = Zeroizing::new(URL_SAFE_NO_PAD.decode(record.private_key.as_bytes()).ok()?);
        let seed: &[u8; 32] = der.strip_prefix(&PKCS8_PREFIX[..])?.try_into().ok()?;
        let key = SigningKey::from_bytes(seed);
        let public_key = key.verifying_key().to_bytes();
        let stored = URL_SAFE_NO_PAD.decode(record.public_key.as_str()).ok()?;
        // `timingSafeEqual`
        let same = stored.len() == 32
            && stored
                .iter()
                .zip(public_key)
                .fold(0, |difference, (a, b)| difference | (a ^ b))
                == 0;
        same.then_some(RelayIdentity { public_key, key })
    };
    load().ok_or(RelayIdentityError)
}
