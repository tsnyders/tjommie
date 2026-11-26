//! The speech runtime shipped beside the app (`resources\speech\`): `whisper-cli.exe`, its licences and the model
//! notice, listed with their sizes and SHA-256 in `manifest.json` (written by `build-speech-runtime.ps1`). Dictation is
//! available only if every file matches. Port of `speech-resources.ts`.

use std::path::Path;

use serde::Deserialize;

use super::sha256_file;

/// The whisper.cpp commit the runtime is built from.
pub const RUNTIME_COMMIT: &str = "d1be6fde11ac6e0407606b4e42fe72d34add8037";
pub const RESOURCE_NAMES: [&str; 5] = [
    "whisper-cli.exe",
    "WHISPER-LICENSE.txt",
    "GGML-LICENSE.txt",
    "OPENAI-WHISPER-LICENSE.txt",
    "MODEL-NOTICE.txt",
];
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct File {
    name: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Manifest {
    version: u32,
    repository: String,
    commit: String,
    platform: String,
    arch: String,
    cpu_only: bool,
    files: Vec<File>,
}

fn invalid() -> String {
    "Invalid packaged speech manifest.".into()
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Checks the manifest and every file it lists. Returns the runtime's SHA-256.
pub fn verify_speech_resources(directory: &Path) -> Result<String, String> {
    let path = directory.join("manifest.json");
    let meta = std::fs::symlink_metadata(&path).map_err(|_| invalid())?;
    if !meta.file_type().is_file() || meta.len() > 16384 {
        return Err(invalid());
    }
    let text = std::fs::read_to_string(&path).map_err(|_| invalid())?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|_| invalid())?;
    if manifest.version != 1
        || manifest.repository != "https://github.com/ggml-org/whisper.cpp"
        || manifest.commit != RUNTIME_COMMIT
        || manifest.platform != "win32"
        || manifest.arch != "x64"
        || !manifest.cpu_only
        || manifest.files.len() != RESOURCE_NAMES.len()
    {
        return Err(invalid());
    }
    let mut names: Vec<&str> = manifest
        .files
        .iter()
        .map(|file| file.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    if names.len() != RESOURCE_NAMES.len()
        || !manifest.files.iter().all(|file| {
            RESOURCE_NAMES.contains(&file.name.as_str())
                && (1..=MAX_FILE_BYTES).contains(&file.bytes)
                && is_sha256(&file.sha256)
        })
    {
        return Err("Missing packaged speech notice or runtime.".into());
    }
    let mut runtime = None;
    for file in &manifest.files {
        let target = directory.join(&file.name);
        let info = std::fs::symlink_metadata(&target)
            .ok()
            .filter(|info| info.file_type().is_file() && info.len() == file.bytes);
        let digest = info.and_then(|_| sha256_file(&target).ok());
        if digest.as_deref() != Some(file.sha256.as_str()) {
            return Err("A packaged speech resource failed its integrity check.".into());
        }
        if file.name == "whisper-cli.exe" {
            runtime = Some(file.sha256.clone());
        }
    }
    runtime.ok_or_else(|| "Missing packaged speech notice or runtime.".into())
}

/// The runtime's SHA-256 if the whole folder verifies; `None` otherwise (dictation then says it is unavailable).
pub fn packaged_speech_digest(directory: &Path) -> Option<String> {
    verify_speech_resources(directory).ok()
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::speech::hex;

    struct Fixture {
        root: tempfile::TempDir,
        manifest: serde_json::Value,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let files: Vec<_> = RESOURCE_NAMES
            .iter()
            .map(|name| {
                let bytes = format!("synthetic resource: {name}").into_bytes();
                std::fs::write(root.path().join(name), &bytes).unwrap();
                serde_json::json!({ "name": name, "bytes": bytes.len(), "sha256": hex(&Sha256::digest(&bytes)) })
            })
            .collect();
        let manifest = serde_json::json!({
            "version": 1, "repository": "https://github.com/ggml-org/whisper.cpp", "commit": RUNTIME_COMMIT,
            "platform": "win32", "arch": "x64", "cpuOnly": true, "files": files,
        });
        std::fs::write(root.path().join("manifest.json"), manifest.to_string()).unwrap();
        Fixture { root, manifest }
    }

    // `speech-resources.test.ts` "accepts only the pinned CPU runtime and exact checked notice/runtime bytes"
    #[test]
    fn accepts_only_the_pinned_cpu_runtime_and_exact_bytes() {
        let f = fixture();
        let expected = f.manifest["files"][0]["sha256"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(verify_speech_resources(f.root.path()).unwrap(), expected);
        assert_eq!(packaged_speech_digest(f.root.path()), Some(expected));
        std::fs::write(f.root.path().join("whisper-cli.exe"), "different runtime").unwrap();
        assert_eq!(packaged_speech_digest(f.root.path()), None);
        assert!(
            verify_speech_resources(f.root.path())
                .unwrap_err()
                .contains("integrity")
        );
    }

    // "fails closed for missing, wrong-commit, missing-notice or path-bearing manifests"
    #[test]
    fn fails_closed_for_missing_wrong_commit_missing_notice_or_path_bearing_manifests() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(packaged_speech_digest(empty.path()), None);
        let path_bearing = serde_json::json!(vec![
            serde_json::json!({ "name": "../outside", "bytes": 1, "sha256": "0".repeat(64) });
            5
        ]);
        for (key, value) in [
            ("commit", serde_json::json!("0".repeat(40))),
            ("cpuOnly", serde_json::json!(false)),
            ("files", serde_json::json!([])),
            ("files", path_bearing),
            ("version", serde_json::json!(2)),
            ("arch", serde_json::json!("arm64")),
            (
                "repository",
                serde_json::json!("https://github.com/evil/whisper.cpp"),
            ),
        ] {
            let f = fixture();
            let mut manifest = f.manifest.clone();
            manifest[key] = value;
            std::fs::write(f.root.path().join("manifest.json"), manifest.to_string()).unwrap();
            assert_eq!(packaged_speech_digest(f.root.path()), None, "{key}");
        }
        // A manifest with a field it shouldn't have, or the same file listed twice, is refused too.
        let f = fixture();
        let mut manifest = f.manifest.clone();
        manifest["extra"] = serde_json::json!(true);
        std::fs::write(f.root.path().join("manifest.json"), manifest.to_string()).unwrap();
        assert_eq!(packaged_speech_digest(f.root.path()), None);
        let f = fixture();
        let mut manifest = f.manifest.clone();
        manifest["files"][1] = manifest["files"][0].clone();
        std::fs::write(f.root.path().join("manifest.json"), manifest.to_string()).unwrap();
        assert_eq!(packaged_speech_digest(f.root.path()), None);
    }
}
