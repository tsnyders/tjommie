//! The development-token cases of `paths.test.ts`. The folder and legacy-database cases come with T1.1.

mod common;

use common::TempRoot;
use daemon::paths::read_or_create_token;

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
