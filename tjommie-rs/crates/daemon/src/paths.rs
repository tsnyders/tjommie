//! `paths.ts`: the data root and the development token. T1.1 adds the folders and the legacy database move.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

// Spec §4: a debug build is the dev identity, so it can't touch the production install on the same PC. Release builds
// are the production identity. TJOMMIE_HOME, TJOMMIE_PORT and TJOMMIE_LAN_PORT override both.
#[cfg(debug_assertions)]
const APP_FOLDER: &str = "Tjommie-rs";
#[cfg(not(debug_assertions))]
const APP_FOLDER: &str = "Tjommie";

pub const DEFAULT_PORT: u16 = if cfg!(debug_assertions) { 47200 } else { 47100 };

/// The development token's file name in the data root.
pub const TOKEN_FILE: &str = "daemon-token";

/// `TJOMMIE_HOME`, else `%LOCALAPPDATA%\Tjommie` (`Tjommie-rs` in a debug build).
pub fn data_root() -> PathBuf {
    data_root_from(|key| std::env::var_os(key))
}

/// [`data_root`] with `env` looking up the environment.
pub fn data_root_from(env: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(home) = env("TJOMMIE_HOME") {
        return home.into();
    }
    let base = env("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| {
        std::env::home_dir()
            .unwrap_or_default()
            .join(".local")
            .join("share")
    });
    base.join(APP_FOLDER)
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the operating system's random source works");
    bytes
}

/// 32 random bytes as 43 base64url characters.
pub fn new_token() -> String {
    URL_SAFE_NO_PAD.encode(random::<32>())
}

/// `readOrCreateToken`: keeps a token of at least 43 characters, else installs a fresh one through a temp file and a
/// rename, so a reader never sees half a token.
/// ponytail: Windows ignores file modes; the per-user ACL of %LOCALAPPDATA% protects the token.
pub fn read_or_create_token(file: &Path) -> io::Result<String> {
    if let Ok(existing) = fs::read_to_string(file) {
        let existing = existing.trim();
        if existing.len() >= 43 {
            return Ok(existing.to_owned());
        }
    }
    let token = new_token();
    let suffix: String = random::<12>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let temporary = file.with_file_name(format!("{TOKEN_FILE}.{suffix}.tmp"));
    let installed = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut written| written.write_all(token.as_bytes()))
        .and_then(|()| fs::rename(&temporary, file));
    let _ = fs::remove_file(&temporary);
    installed.map(|()| token)
}
