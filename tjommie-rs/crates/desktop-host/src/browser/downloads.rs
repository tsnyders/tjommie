//! Downloads (`native-browser.ts:811-822`): what a page downloads is saved in the agent's own `browser-files` folder,
//! never where the page or the system suggests, up to 100 MB, under a name the page cannot make into a path.

use std::path::{Path, PathBuf};

/// A download over this size is refused, and one that grows past it is cancelled.
pub const LIMIT_BYTES: i64 = 100 * 1024 * 1024;

/// The longest file name kept, in characters.
const NAME_CHARS: usize = 160;

fn other(message: &str) -> std::io::Error {
    std::io::Error::other(message.to_string())
}

/// A file name the page cannot turn into a path: the last part only, no character Windows forbids, at most 160
/// characters; `download` if nothing is left.
pub fn safe_name(suggested: &str) -> String {
    let base = Path::new(suggested)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c < ' ' {
                '_'
            } else {
                c
            }
        })
        .take(NAME_CHARS)
        .collect();
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    }
}

/// The verbatim prefix `canonicalize` adds to a Windows path (backslash, backslash, question mark, backslash).
const VERBATIM: &str = "\\\\?\\";

/// A path without that prefix, which WebView2 does not accept as a save path. A network path keeps its form.
pub(crate) fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy().into_owned();
    match text.strip_prefix(VERBATIM) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// `root/leaf`, made if need be: a physical folder directly inside `root`, never a link.
pub fn contained_folder(root: &Path, leaf: &str) -> std::io::Result<PathBuf> {
    const OUTSIDE: &str = "Browser storage must stay inside its owned folder.";
    std::fs::create_dir_all(root)?;
    let base = std::fs::canonicalize(root)?;
    let path = base.join(leaf);
    std::fs::create_dir_all(&path)?;
    if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(other(OUTSIDE));
    }
    let canonical = std::fs::canonicalize(&path)?;
    let inside = canonical.parent().is_some_and(|parent| {
        parent.to_string_lossy().to_lowercase() == base.to_string_lossy().to_lowercase()
    });
    if inside {
        Ok(plain(path))
    } else {
        Err(other(OUTSIDE))
    }
}

/// Where a download of `suggested` is saved for a bot whose home folder is `home`: a unique file in `browser-files`.
pub fn target(home: &Path, suggested: &str) -> std::io::Result<PathBuf> {
    let folder = contained_folder(home, "browser-files")?;
    Ok(folder.join(format!("{}-{}", uuid::Uuid::new_v4(), safe_name(suggested))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_the_page_chooses_cannot_become_a_path_or_a_forbidden_name() {
        assert_eq!(
            safe_name(r"C:\Users\someone\Downloads\report.pdf"),
            "report.pdf"
        );
        assert_eq!(safe_name("../../outside.txt"), "outside.txt");
        assert_eq!(safe_name("a<b>c:d\"e|f?g*h.txt"), "a_b_c_d_e_f_g_h.txt");
        assert_eq!(safe_name("tab\there\u{1}.txt"), "tab_here_.txt");
        assert_eq!(safe_name(""), "download");
        assert_eq!(safe_name("/"), "download");
        assert_eq!(safe_name(&"x".repeat(500)).chars().count(), 160);
        // Characters, not bytes: no name is cut in the middle of one.
        assert_eq!(safe_name(&"é".repeat(500)).chars().count(), 160);
    }

    #[test]
    fn a_download_lands_in_a_unique_file_in_the_bots_own_browser_files_folder() {
        let home = tempfile::tempdir().unwrap();
        let first = target(home.path(), "report.txt").unwrap();
        let second = target(home.path(), "report.txt").unwrap();
        assert_ne!(first, second);
        let folder = std::fs::canonicalize(home.path())
            .unwrap()
            .join("browser-files");
        let parent = first.parent().unwrap().to_string_lossy().to_lowercase();
        assert!(
            !parent.starts_with(super::VERBATIM),
            "WebView2 needs a plain path: {parent}"
        );
        assert!(
            folder.to_string_lossy().to_lowercase().ends_with(&parent),
            "{parent} is the folder"
        );
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-report.txt")
        );
    }

    #[test]
    fn a_browser_files_folder_that_is_a_junction_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = home.path().join("browser-files");
        let made = crate::spawn::command("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink failed");
        let error = target(home.path(), "report.txt").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Browser storage must stay inside its owned folder."
        );
        assert!(
            std::fs::read_dir(outside.path()).unwrap().next().is_none(),
            "nothing was written through the link"
        );
        std::fs::remove_dir(&link).unwrap();
    }
}
