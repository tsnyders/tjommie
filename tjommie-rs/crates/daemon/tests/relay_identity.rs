//! Port of `packages/daemon/src/relay-identity.test.ts`, plus a check against an entry the 0.4.2 module minted
//! (`fixtures/compat/relay-identity.json`). Every vault here is in memory; nothing touches Credential Manager.

mod remote_support;

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daemon::relay_identity::load_relay_identity;
use daemon::remote_identity::load_remote_identity;
use daemon::win::cred::Namespace;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use remote_support::{MemoryVault, remote_db};
use serde_json::{Value, json};
use transport::Keypair;
use zeroize::Zeroizing;

const ROOT: &str = "synthetic-relay-root";
/// `privateKey.export({ format: "der", type: "pkcs8" })` of an Ed25519 key in Node: RFC 8410 v1, then the 32-byte seed.
const PKCS8_ED25519: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    VerifyingKey::from_bytes(public_key)
        .unwrap()
        .verify(message, &Signature::from_bytes(signature))
        .is_ok()
}

fn saved(vault: &MemoryVault) -> String {
    vault
        .values
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone()
}

#[test]
fn round_trips_an_ed25519_login_key_through_a_synthetic_credential_vault() {
    let vault = MemoryVault::default();
    let root = Path::new(ROOT);
    let first = load_relay_identity(root, &vault).unwrap();
    let second = load_relay_identity(root, &vault).unwrap();
    let message = b"synthetic login challenge";
    assert_eq!(second.public_key, first.public_key);
    assert!(verify(&first.public_key, message, &first.sign(message)));
    assert!(verify(&first.public_key, message, &second.sign(message)));
    assert_eq!(*vault.writes.lock().unwrap(), 1);
    let db = remote_db();
    load_remote_identity(&db, root, "Synthetic PC", &vault, || {
        Ok(Keypair {
            private_key: Zeroizing::new([3; 32]),
            public_key: [4; 32],
        })
    })
    .unwrap();
    let namespace = Namespace::new(root).unwrap();
    let targets: Vec<String> = vault.values.lock().unwrap().keys().cloned().collect();
    assert_eq!(
        targets,
        [
            namespace.relay_login_target(),
            namespace.remote_host_target()
        ]
    );
    assert!(
        regex::Regex::new("^Tjommie/[a-f0-9]{32}/relay-login$")
            .unwrap()
            .is_match(&targets[0])
    );
}

#[test]
fn stores_the_same_pkcs8_shape_as_node() {
    let vault = MemoryVault::default();
    let identity = load_relay_identity(Path::new(ROOT), &vault).unwrap();
    let record: Value = serde_json::from_str(&saved(&vault)).unwrap();
    // `JSON.stringify` key order; `Value` would sort the keys, so check the text.
    assert!(saved(&vault).starts_with(r#"{"v":1,"privateKey":""#));
    assert!(saved(&vault).contains(r#"","publicKey":""#));
    let der = URL_SAFE_NO_PAD
        .decode(record["privateKey"].as_str().unwrap())
        .unwrap();
    assert_eq!(der.len(), 48);
    assert_eq!(der[..16], PKCS8_ED25519);
    assert_eq!(
        record["publicKey"],
        URL_SAFE_NO_PAD.encode(identity.public_key)
    );
}

#[test]
fn rejects_a_mismatched_saved_key_without_minting_a_replacement() {
    let vault = MemoryVault::default();
    load_relay_identity(Path::new(ROOT), &vault).unwrap();
    let mut record: Value = serde_json::from_str(&saved(&vault)).unwrap();
    record["publicKey"] = json!(URL_SAFE_NO_PAD.encode([9; 32]));
    let target = vault.values.lock().unwrap().keys().next().unwrap().clone();
    vault
        .values
        .lock()
        .unwrap()
        .insert(target, record.to_string());
    let error = load_relay_identity(Path::new(ROOT), &vault).unwrap_err();
    assert!(error.to_string().contains("connection key"));
    assert_eq!(*vault.writes.lock().unwrap(), 1);
}

#[test]
fn keeps_corrupt_and_non_ed25519_vault_contents_out_of_errors_without_minting() {
    let target = Namespace::new(Path::new(ROOT))
        .unwrap()
        .relay_login_target();
    // An X25519 PKCS#8 key (OID 1.3.101.110) is well-formed but the wrong kind.
    let mut x25519 = PKCS8_ED25519;
    x25519[11] = 0x6e;
    let x25519_der = [&x25519[..], &[5; 32]].concat();
    for stored in [
        r#"{"privateKey":"synthetic-sensitive-key","#.to_owned(),
        json!({ "v": 1, "privateKey": URL_SAFE_NO_PAD.encode(&x25519_der), "publicKey": URL_SAFE_NO_PAD.encode([6; 32]) })
            .to_string(),
    ] {
        let vault = MemoryVault::default();
        vault.values.lock().unwrap().insert(target.clone(), stored);
        let error = load_relay_identity(Path::new(ROOT), &vault).unwrap_err();
        assert!(error.to_string().contains("connection key"));
        assert!(!format!("{error} {error:?}").contains("synthetic-sensitive-key"));
        assert_eq!(*vault.writes.lock().unwrap(), 0);
    }
}

/// `fixtures/compat/relay-identity.json` is an entry the 0.4.2 TypeScript module minted, with the public key and the
/// signature it gives a fixed message. Ed25519 signatures are deterministic, so equal ones prove Rust holds the same key.
/// The other way (a rollback to 0.4.2 reads what Rust wrote) rests on `stores_the_same_pkcs8_shape_as_node`: the same
/// record layout, whose key 0.4.2 parses with Node's `createPrivateKey`.
#[test]
fn a_key_written_by_typescript_0_4_2_signs_identically_in_rust() {
    let captured: Value =
        serde_json::from_str(include_str!("fixtures/compat/relay-identity.json")).unwrap();
    let message = URL_SAFE_NO_PAD
        .decode(captured["message"].as_str().unwrap())
        .unwrap();
    let vault = MemoryVault::default();
    let target = Namespace::new(Path::new("remote-probe-root"))
        .unwrap()
        .relay_login_target();
    vault
        .values
        .lock()
        .unwrap()
        .insert(target, captured["saved"].as_str().unwrap().to_owned());
    let rust = load_relay_identity(Path::new("remote-probe-root"), &vault).unwrap();
    assert_eq!(
        URL_SAFE_NO_PAD.encode(rust.public_key),
        captured["publicKey"].as_str().unwrap()
    );
    assert_eq!(
        URL_SAFE_NO_PAD.encode(rust.sign(&message)),
        captured["signature"].as_str().unwrap()
    );
    assert_eq!(*vault.writes.lock().unwrap(), 0, "the saved key is kept");
}
