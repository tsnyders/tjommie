//! Paths the window may ask the app to touch (T6.6; ports of `local-file-path.ts` and `bot-files.ts`). The UI hands
//! over a path as text; before anything looks at the disk it must be an ordinary local path, never a network share or a
//! device name, and the only file the app opens on request is an agent's own `notes.md`.

use std::path::{Path, PathBuf};

/// `localFilePath`: absolute, and not UNC (`\\server\share`), a device path (`\\?\`, `\\.\`) or their `//` forms.
/// "Absolute" is Node's Windows meaning: a drive and a separator (`C:\`), or a leading separator.
pub fn local_file_path(path: &str) -> bool {
    let mut chars = path.chars();
    let is_separator = |c: char| c == '\\' || c == '/';
    let (first, second) = (chars.next(), chars.next());
    let drive_rooted = first.is_some_and(|c| c.is_ascii_alphabetic())
        && second == Some(':')
        && chars.next().is_some_and(is_separator);
    let rooted = first.is_some_and(is_separator);
    let network_or_device = rooted && second.is_some_and(is_separator);
    (drive_rooted || rooted) && !network_or_device
}

/// `path.resolve`: absolute and with `.` and `..` folded away, without touching the disk.
fn resolve(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Windows compares paths without regard to case.
fn same_folder(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    key(a) == key(b)
}

/// `notesFile`: an agent's `notes.md`, in a home folder directly inside the agents folder. Anything else is refused.
pub fn notes_file(agents_dir: &Path, home_folder: &Path) -> Option<PathBuf> {
    let home = resolve(home_folder);
    if !home
        .parent()
        .is_some_and(|parent| same_folder(parent, &resolve(agents_dir)))
    {
        return None;
    }
    let file = home.join("notes.md");
    std::fs::metadata(&file)
        .is_ok_and(|meta| meta.is_file())
        .then_some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `local-file-path.test.ts`
    #[test]
    fn rejects_unc_and_device_paths_before_any_filesystem_lookup() {
        assert!(local_file_path(r"C:\Users\Alice\file.txt"));
        for path in [
            r"\\attacker\share\file.txt",
            r"\\?\C:\Windows\file.txt",
            r"\\.\C:\file.txt",
            "//attacker/share/file.txt",
            r"relative\file.txt",
        ] {
            assert!(!local_file_path(path), "{path}");
        }
        // The rest of Node's meaning of absolute: forward slashes, a rooted path, and no drive-relative or empty path.
        assert!(local_file_path("C:/Users/Alice/file.txt"));
        assert!(local_file_path(r"\Users\Alice"));
        for path in ["", "C:file.txt", "C:", "x", "ü:\\a", r"\\", "//"] {
            assert!(!local_file_path(path), "{path:?}");
        }
    }

    // `bot-files.test.ts` "opens only a bot's own notes.md"
    #[test]
    fn opens_only_an_agents_own_notes_file() {
        let root = tempfile::tempdir().unwrap();
        let agents = root.path().join("bots");
        let home = agents.join("b1");
        std::fs::create_dir_all(home.join("inbox")).unwrap();
        std::fs::write(home.join("notes.md"), "# b1's notes\n").unwrap();
        std::fs::write(home.join("inbox").join("notes.md"), "an attachment").unwrap();
        std::fs::create_dir_all(agents.join("b2")).unwrap();
        std::fs::create_dir_all(root.path().join("elsewhere")).unwrap();
        std::fs::write(
            root.path().join("elsewhere").join("notes.md"),
            "not an agent",
        )
        .unwrap();

        assert_eq!(notes_file(&agents, &home), Some(home.join("notes.md")));
        assert_eq!(notes_file(&agents, &home.join("inbox")), None);
        assert_eq!(notes_file(&agents, &root.path().join("elsewhere")), None);
        assert_eq!(
            notes_file(&agents, &agents.join("..").join("elsewhere")),
            None
        );
        assert_eq!(
            notes_file(&agents, &agents.join("b2")),
            None,
            "no notes.md there"
        );
        assert_eq!(notes_file(&agents, &agents), None);
        // The same folder spelled with a `..` or in another case is still that agent's.
        assert_eq!(
            notes_file(&agents, &agents.join("b2").join("..").join("b1")),
            Some(home.join("notes.md"))
        );
        let shouted = PathBuf::from(home.to_string_lossy().to_uppercase());
        assert!(notes_file(&agents, &shouted).is_some());
        // A folder named notes.md is not the notes file.
        std::fs::create_dir_all(agents.join("b3").join("notes.md")).unwrap();
        assert_eq!(notes_file(&agents, &agents.join("b3")), None);
    }
}
