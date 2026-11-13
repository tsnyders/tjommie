//! `paths.test.ts`, and the `dataPaths` cases of `db.test.ts`.

mod common;

use std::fs;
use std::path::Path;

use common::TempRoot;
use daemon::paths::{Paths, data_paths, migrate_legacy_db, read_or_create_token};

fn is_token(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[test]
fn regenerates_an_empty_or_short_token_file() {
    for previous in ["", "  \n", &"x".repeat(42)] {
        let root = TempRoot::new();
        let file = root.0.join("daemon-token");
        std::fs::write(&file, previous).unwrap();
        let token = read_or_create_token(&file).unwrap();
        assert!(is_token(&token), "{token}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), token);
        assert_eq!(read_or_create_token(&file).unwrap(), token);
    }
}

#[test]
fn installs_a_complete_token_with_a_temp_file_and_rename() {
    // The TypeScript test also watches the rename happen; here the observable result is checked: the token file is
    // whole and no temp file is left behind.
    let root = TempRoot::new();
    let file = root.0.join("daemon-token");
    let token = read_or_create_token(&file).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), token);
    let leftovers: Vec<_> = std::fs::read_dir(&root.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("daemon-token."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

fn read(file: impl AsRef<Path>) -> String {
    fs::read_to_string(file).unwrap()
}

/// A root whose path has a space and an accented letter, like many Windows profile paths.
fn accented_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap()
}

#[test]
fn creates_the_folders_under_a_root_with_spaces_and_accents() {
    let root = accented_root();
    let paths = data_paths(root.path()).unwrap();
    for dir in [
        &paths.bots,
        &paths.accounts,
        &paths.logs,
        &paths.data,
        &paths.shared_files,
    ] {
        assert!(dir.is_dir(), "{}", dir.display());
    }
}

#[test]
fn creates_a_token_once_and_returns_the_same_one_afterwards() {
    let root = accented_root();
    let paths = data_paths(root.path()).unwrap();
    let token = read_or_create_token(&paths.token).unwrap();
    assert!(is_token(&token), "{token}");
    assert_eq!(read_or_create_token(&paths.token).unwrap(), token);
}

#[test]
fn moves_a_legacy_database_and_all_sqlite_sidecars_under_data() {
    let root = TempRoot::new();
    let paths = data_paths(&root.0).unwrap();
    assert_eq!(paths.db, paths.data.join("tjommie.db"));
    for suffix in ["", "-wal", "-shm", "-journal"] {
        fs::write(
            root.0.join(format!("tjommie.db{suffix}")),
            format!("old{suffix}"),
        )
        .unwrap();
    }
    migrate_legacy_db(&paths).unwrap();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        assert!(!root.0.join(format!("tjommie.db{suffix}")).exists());
        assert_eq!(
            read(paths.data.join(format!("tjommie.db{suffix}"))),
            format!("old{suffix}")
        );
    }
}

/// The TypeScript test mocks `renameSync`; here a handle that shares nothing makes Windows refuse the real rename.
#[cfg(windows)]
#[test]
fn leaves_every_database_part_in_one_location_after_a_failed_move_and_resumes_safely() {
    use std::os::windows::fs::OpenOptionsExt;

    let root = TempRoot::new();
    let paths = data_paths(&root.0).unwrap();
    fs::write(root.0.join("tjommie.db"), "main").unwrap();
    fs::write(root.0.join("tjommie.db-wal"), "wal").unwrap();
    fs::write(root.0.join("tjommie.db-shm"), "shm").unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(root.0.join("tjommie.db-wal"))
        .unwrap();
    assert!(migrate_legacy_db(&paths).is_err());
    assert_eq!(read(root.0.join("tjommie.db")), "main");
    assert_eq!(read(paths.data.join("tjommie.db-shm")), "shm");
    drop(locked);
    migrate_legacy_db(&paths).unwrap();
    assert_eq!(read(&paths.db), "main");
    assert_eq!(read(paths.data.join("tjommie.db-wal")), "wal");
    migrate_legacy_db(&paths).unwrap();
}

#[test]
fn refuses_to_overwrite_a_database_already_in_data() {
    let root = TempRoot::new();
    let paths = data_paths(&root.0).unwrap();
    fs::write(&paths.db, "new").unwrap();
    fs::write(root.0.join("tjommie.db"), "old").unwrap();
    fs::write(root.0.join("tjommie.db-wal"), "old-wal").unwrap();
    let error = migrate_legacy_db(&paths).unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(read(&paths.db), "new");
    assert_eq!(read(root.0.join("tjommie.db")), "old");
    assert_eq!(read(root.0.join("tjommie.db-wal")), "old-wal");
}

#[test]
fn refuses_sidecars_without_the_legacy_database() {
    let root = TempRoot::new();
    let paths: Paths = data_paths(&root.0).unwrap();
    fs::write(root.0.join("tjommie.db-wal"), "wal").unwrap();
    let error = migrate_legacy_db(&paths).unwrap_err();
    assert!(
        error.to_string().contains("without the legacy database"),
        "{error}"
    );
    assert_eq!(read(root.0.join("tjommie.db-wal")), "wal");
}
