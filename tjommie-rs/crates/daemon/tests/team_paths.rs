//! Ported from `packages/daemon/src/team.paths.test.ts`: `fileInside` and the `share_file` tool on files that exist outside
//! the bot's folders. The TypeScript `skip`s become an early return with a note on stderr. The drive-letter and junction
//! cases need Windows, so they are `#[cfg(windows)]`.

mod team_support;

use team_support::*;

use std::fs;
use std::path::{Path, PathBuf};

use daemon::bots::StoredBot;
use daemon::team::file_inside;
use serde_json::json;
use tempfile::TempDir;

const REJECTION: &str = "You can only share files from your own folders.";

/// `beforeEach`: one bot, Writer, in a fresh fixture.
fn setup() -> (Fixture, StoredBot) {
    let fx = fixture();
    let bot = fx.bot(
        "Writer",
        "blue",
        "circle",
        "Write summaries.",
        "codex",
        "gpt-5.6-sol",
    );
    (fx, bot)
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `expectOutsideFile(path)`: the file exists, and both `fileInside` and the `share_file` tool refuse it.
fn expect_outside_file(fx: &Fixture, bot: &StoredBot, path: &Path) {
    let absolute = Path::new(&bot.home_folder).join(path);
    assert!(absolute.exists());
    assert!(absolute.is_file());
    assert_eq!(
        file_inside(&[PathBuf::from(&bot.home_folder)], &absolute).unwrap_err(),
        REJECTION
    );
    let message = fx
        .run_tool("share_file", &bot.id, json!({ "path": path_arg(path) }))
        .unwrap_err();
    assert_eq!(message, REJECTION);
}

/// `makeJunction` for `symlinkSync(target, link, "junction")`. False when the OS refuses.
#[cfg(windows)]
fn make_junction(link: &Path, target: &Path) -> bool {
    daemon::spawn::command("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn drive_letter(path: &Path) -> Option<char> {
    path.to_string_lossy()
        .chars()
        .next()
        .map(|letter| letter.to_ascii_lowercase())
}

/// `otherDriveFile()`: a new folder with a file on the first other drive that takes one, or else an existing file there.
/// A new folder is kept in `keep` so it is removed when the test ends.
#[cfg(windows)]
fn other_drive_file(root: &Path, keep: &mut Vec<TempDir>) -> Option<PathBuf> {
    let own = drive_letter(root);
    for letter in 'A'..='Z' {
        let drive = format!("{letter}:\\");
        if drive_letter(Path::new(&drive)) == own || !Path::new(&drive).exists() {
            continue;
        }
        if let Ok(folder) = tempfile::Builder::new()
            .prefix("tjommie-team-path-test-")
            .tempdir_in(&drive)
        {
            let file = folder.path().join("outside.txt");
            if fs::write(&file, "outside the bot's folders").is_ok() {
                keep.push(folder);
                return Some(file);
            }
        }
        // An unavailable or inaccessible drive cannot provide a new folder, but may still have a file to check.
        if let Ok(entries) = fs::read_dir(&drive) {
            let file = entries
                .flatten()
                .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()));
            if let Some(file) = file {
                return Some(file.path());
            }
        }
    }
    None
}

mod shared_file_containment_with_existing_outside_files {
    use super::*;

    #[test]
    fn rejects_data_root_file_through_a_legacy_broad_project_folder() {
        let (fx, bot) = setup();
        let root = path_arg(fx.root.path());
        // `it.each` over four names, one loop; each row's name is in the assertion messages.
        for name in [
            "accounts/claude-main/settings.json",
            "daemon-token",
            "tjommie.db-fixture",
            "shared-files/private.txt",
        ] {
            let file = fx.root.path().join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, "synthetic private fixture").unwrap();
            fx.db(|db| {
                db.execute(
                    "UPDATE bots SET project_folder = ?1 WHERE id = ?2",
                    [root.as_str(), bot.id.as_str()],
                )
                .unwrap();
            });
            let message = fx
                .run_tool("share_file", &bot.id, json!({ "path": path_arg(&file) }))
                .unwrap_err();
            assert_eq!(message, REJECTION, "row {name}");
            assert_eq!(
                fx.count("SELECT COUNT(*) FROM messages WHERE kind = 'file'"),
                0,
                "row {name}"
            );
        }
    }

    #[test]
    fn rejects_an_existing_sibling_file_reached_through_parent_traversal() {
        let (fx, bot) = setup();
        let outside = Path::new(&bot.home_folder)
            .parent()
            .unwrap()
            .join("private");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "private contents").unwrap();
        let path = Path::new("..").join("private").join("secret.txt");
        assert!(path.starts_with(".."));
        expect_outside_file(&fx, &bot, &path);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_an_existing_file_on_another_drive() {
        let (fx, bot) = setup();
        let mut keep: Vec<TempDir> = Vec::new();
        let Some(file) = other_drive_file(fx.root.path(), &mut keep) else {
            eprintln!("skipped: No accessible additional drive provides an existing test file.");
            return;
        };
        assert_ne!(
            drive_letter(&file),
            drive_letter(Path::new(&bot.home_folder))
        );
        expect_outside_file(&fx, &bot, &file);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_an_existing_file_reached_through_an_inside_junction_or_symlink_to_an_outside_folder()
    {
        let (fx, bot) = setup();
        let outside = fx.root.path().join("private");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "private contents").unwrap();
        let link = Path::new(&bot.home_folder).join("outside-link");
        if !make_junction(&link, &outside) {
            eprintln!("skipped: The operating system refused to create the directory link.");
            return;
        }
        expect_outside_file(&fx, &bot, &Path::new("outside-link").join("secret.txt"));
    }
}
