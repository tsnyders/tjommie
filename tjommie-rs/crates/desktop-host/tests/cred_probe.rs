//! Rust and TypeScript read each other's device keys in the real Windows Credential Manager, byte for byte: the TypeScript
//! side is `tools/cred-probe.ts`, which runs `packages/desktop/src/device-keys.ts` (needs `node` on PATH). Everything is
//! under a RANDOM test namespace, derived from a throwaway root in the temp folder, and each entry is deleted by name when
//! the test ends; the owner's `Tjommie/<production namespace>/...` entries are never read, written or listed.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use desktop_host::device_keys::{DeviceKeys, credential_namespace};
use desktop_host::spawn;

const NAMES: [&str; 2] = ["probe-from-typescript", "probe-from-rust"];

struct Root(PathBuf);

impl Root {
    /// A path in the temp folder that nothing else uses. It is never created: only its namespace matters.
    fn random() -> Root {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).unwrap();
        let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let root = std::env::temp_dir().join(format!("tjommie-rs-cred-probe-{name}"));
        let production = Path::new(&std::env::var("LOCALAPPDATA").unwrap()).join("Tjommie");
        assert_ne!(
            credential_namespace(&root).unwrap(),
            credential_namespace(&production).unwrap(),
            "the probe must never share the production namespace"
        );
        Root(root)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        // Whatever happened, the test's entries go: by name, under the test namespace only.
        if let Ok(keys) = DeviceKeys::new(&self.0) {
            for name in NAMES {
                let _ = keys.delete(name);
            }
        }
    }
}

/// `node tools/cred-probe.ts <args>`, run from the tjommie-rs folder; returns what it printed.
fn probe(args: &[&str]) -> String {
    let tool = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tools")
        .join("cred-probe.ts");
    let output = spawn::command("node")
        .arg(&tool)
        .args(args)
        .output()
        .expect("node is on PATH");
    assert!(
        output.status.success(),
        "cred-probe {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

// Real Credential Manager: skipped on hosted CI like the daemon's own Credential Manager tests (ci.yml sets TJOMMIE_CI_HOSTED=1
// because hosted service logons can't reliably reach it). This runs on real PCs.
#[test]
fn typescript_and_rust_read_each_others_device_keys() {
    if std::env::var("TJOMMIE_CI_HOSTED").as_deref() == Ok("1") {
        eprintln!(
            "skipped: hosted Windows service logons cannot reliably reach Credential Manager"
        );
        return;
    }
    let root = Root::random();
    let root_text = root.0.to_str().unwrap();
    let keys = DeviceKeys::new(&root.0).unwrap();

    // TypeScript writes, Rust reads. 1920 bytes is the longest key, and every byte value appears.
    let from_typescript: Vec<u8> = (0..=255u8).cycle().take(1920).collect();
    probe(&[
        "seal",
        root_text,
        NAMES[0],
        &URL_SAFE_NO_PAD.encode(&from_typescript),
    ]);
    let read = keys
        .read(NAMES[0])
        .unwrap()
        .expect("Rust finds the entry TypeScript wrote");
    assert_eq!(read.as_slice(), from_typescript.as_slice());

    // Rust writes, TypeScript reads.
    let from_rust: Vec<u8> = (0..=255u8).rev().cycle().take(32).collect();
    keys.seal(NAMES[1], &from_rust).unwrap();
    assert_eq!(
        probe(&["read", root_text, NAMES[1]]),
        URL_SAFE_NO_PAD.encode(&from_rust)
    );

    // Deleting on one side is deleting on the other.
    keys.delete(NAMES[0]).unwrap();
    assert_eq!(probe(&["read", root_text, NAMES[0]]), "null");
    probe(&["delete", root_text, NAMES[1]]);
    assert!(keys.read(NAMES[1]).unwrap().is_none());
}
