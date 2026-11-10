//! `paths.ts`: the data root, its folders, the legacy database move and the development token.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub const DEFAULT_PORT: u16 = 47100;

/// The development token's file name in the data root.
pub const TOKEN_FILE: &str = "daemon-token";

/// `TJOMMIE_HOME`, else `%LOCALAPPDATA%\Tjommie`.
pub fn data_root() -> PathBuf {
    if let Some(home) = std::env::var_os("TJOMMIE_HOME") {
        return home.into();
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
        });
    base.join("Tjommie")
}

/// The layout under one data root.
pub struct Paths {
    pub root: PathBuf,
    pub data: PathBuf,
    pub db: PathBuf,
    pub token: PathBuf,
    pub bots: PathBuf,
    pub accounts: PathBuf,
    pub logs: PathBuf,
    pub shared_files: PathBuf,
}

/// `dataPaths`: the layout under `root` (normally `data_root()`), with its folders created.
pub fn data_paths(root: &Path) -> io::Result<Paths> {
    let data = root.join("data");
    let paths = Paths {
        root: root.to_path_buf(),
        db: data.join("tjommie.db"),
        data,
        token: root.join(TOKEN_FILE),
        bots: root.join("bots"),
        accounts: root.join("accounts"),
        logs: root.join("logs"),
        shared_files: root.join("shared-files"),
    };
    for dir in [
        &paths.root,
        &paths.bots,
        &paths.accounts,
        &paths.logs,
        &paths.data,
        &paths.shared_files,
    ] {
        fs::create_dir_all(dir)?;
    }
    Ok(paths)
}

const LEGACY_DB: &str = "tjommie.db";

/// `migrateLegacyDb`: moves `tjommie.db` and its sidecars from the root into `data\`. The port lock is held by the
/// caller. Sidecars move first and the main database last, so a failed rename leaves each whole file at one of the
/// two locations and a later start resumes the move. Never open SQLite until this completes: a split WAL and
/// database must not be used.
pub fn migrate_legacy_db(paths: &Paths) -> io::Result<()> {
    let mut names = Vec::new();
    for entry in fs::read_dir(&paths.root)? {
        let name = entry?.file_name();
        let is_legacy = name
            .to_str()
            .and_then(|name| name.strip_prefix(LEGACY_DB))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '.']));
        if is_legacy {
            names.push(name);
        }
    }
    if !names.is_empty() && !paths.root.join(LEGACY_DB).exists() {
        return Err(io::Error::other(
            "Legacy SQLite sidecars exist without the legacy database. The database was left untouched.",
        ));
    }
    names.sort();
    names.sort_by_key(|name| name == LEGACY_DB); // Stable: sidecars keep their order, the main database goes last.
    for name in &names {
        let from = paths.root.join(name);
        let to = paths.data.join(name);
        if !fs::symlink_metadata(&from)?.is_file() {
            return Err(io::Error::other(format!(
                "Legacy database path is not a regular file: {}",
                from.display()
            )));
        }
        if to.exists() {
            return Err(io::Error::other(format!(
                "Database destination already exists: {}",
                to.display()
            )));
        }
    }
    for name in &names {
        fs::rename(paths.root.join(name), paths.data.join(name))?;
    }
    Ok(())
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
