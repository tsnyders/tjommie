//! The files a browser tool reads and writes: uploads from the agent's own folders, and saved output in its
//! `browser-files` folder. Every path is canonical, inside the owned folder, and never a link.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::reply::ToolError;
use crate::browser::downloads;

/// All uploads of one call, together.
const UPLOAD_LIMIT: u64 = 20 * 1024 * 1024;

/// Whether `path` is `root` or inside it, as Windows compares paths (components, case-insensitive). Both are canonical.
fn inside(root: &Path, path: &Path) -> bool {
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    parts(path).starts_with(&parts(root))
}

/// The real, plain paths of the files to upload: absolute, regular files inside the bot's home or project folder, at
/// most 20 MiB together. An 8.3 alias or a `..` resolves to the canonical file, and a link out of the folders is refused.
pub fn uploads(
    home: &Path,
    project: Option<&Path>,
    paths: &[String],
) -> Result<Vec<String>, ToolError> {
    let outside = || {
        ToolError::new(
            "Uploads must be regular files inside this agent's folders, at most 20 MiB combined.",
        )
    };
    let mut roots = vec![std::fs::canonicalize(home).map_err(|_| outside())?];
    if let Some(project) = project {
        roots.push(std::fs::canonicalize(project).map_err(|_| outside())?);
    }
    let mut size = 0u64;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        if !Path::new(path).is_absolute() {
            return Err(ToolError::new(
                "Upload paths must be absolute paths inside this agent's folders.",
            ));
        }
        let file = std::fs::canonicalize(path).map_err(|_| outside())?;
        let info = std::fs::metadata(&file).map_err(|_| outside())?;
        size = size.saturating_add(info.len());
        if !roots.iter().any(|root| inside(root, &file)) || !info.is_file() || size > UPLOAD_LIMIT {
            return Err(outside());
        }
        files.push(downloads::plain(file).to_string_lossy().into_owned());
    }
    Ok(files)
}

/// Whether Windows reads `name` as a device, not a file: `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM1`-`COM9` and
/// `LPT1`-`LPT9` (the superscript digits too), in any case, with any extension (`nul.txt`), and with spaces before the dot.
/// Opening one writes to the device and says it saved a file.
fn is_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    ["COM", "LPT"].iter().any(|prefix| {
        upper.strip_prefix(prefix).is_some_and(|digit| {
            let mut chars = digit.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
            )
        })
    })
}

/// A file name the model chose for saved output: one plain name, nothing that is a path or that Windows forbids (a device
/// name, or a name ending in a dot or a space, which Windows drops).
fn plain_filename(name: &str) -> Result<&str, ToolError> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !is_device(name)
        && !name
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\') || c < ' ')
        && Path::new(name)
            .file_name()
            .is_some_and(|only| only == std::ffi::OsStr::new(name));
    if plain {
        Ok(name)
    } else {
        Err(ToolError::new(
            "Use a plain filename inside this agent's browser-files folder.",
        ))
    }
}

/// Saves `data` as a new file `filename` in the bot's `browser-files` folder and returns its path. An existing file is
/// never overwritten.
pub fn write_output(home: &Path, filename: &str, data: &[u8]) -> Result<PathBuf, ToolError> {
    let name = plain_filename(filename)?;
    let directory = downloads::contained_folder(home, "browser-files")
        .map_err(|_| ToolError::new("Browser output must remain inside the agent's folder."))?;
    let path = directory.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| ToolError(format!("Could not save {name}: {error}")))?;
    file.write_all(data)
        .map_err(|error| ToolError(format!("Could not save {name}: {error}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn uploads_only_real_files_inside_the_agents_folders() {
        let home = root();
        let project = root();
        let outside = root();
        let allowed = home.path().join("upload.txt");
        let in_project = project.path().join("brief.txt");
        let denied = outside.path().join("secret.txt");
        for file in [&allowed, &in_project, &denied] {
            std::fs::write(file, "local").unwrap();
        }
        let ok = |paths: &[&Path]| {
            let paths: Vec<String> = paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            uploads(home.path(), Some(project.path()), &paths)
        };
        let both = ok(&[&allowed, &in_project]).unwrap();
        assert_eq!(both.len(), 2);
        assert!(
            !both[0].starts_with(r"\\?\"),
            "WebView2 needs plain paths: {}",
            both[0]
        );
        assert!(ok(&[&denied]).is_err());
        assert!(ok(&[home.path()]).is_err(), "a folder is not a file");
        assert!(ok(&[&home.path().join("missing.txt")]).is_err());
        assert_eq!(
            uploads(home.path(), None, &["relative.txt".into()])
                .unwrap_err()
                .0,
            "Upload paths must be absolute paths inside this agent's folders."
        );
        // `..` out of the folder resolves to the real file and is refused there.
        let sneaky = home
            .path()
            .join("..")
            .join(outside.path().file_name().unwrap())
            .join("secret.txt");
        assert!(ok(&[&sneaky]).is_err());
    }

    #[test]
    fn uploads_are_limited_to_20_mib_together() {
        let home = root();
        let big = |name: &str| {
            let path = home.path().join(name);
            std::fs::write(&path, vec![0u8; 11 * 1024 * 1024]).unwrap();
            path.to_string_lossy().into_owned()
        };
        let (one, two) = (big("a.bin"), big("b.bin"));
        assert!(uploads(home.path(), None, std::slice::from_ref(&one)).is_ok());
        assert!(uploads(home.path(), None, &[one, two]).is_err());
    }

    /// The 8.3 spelling of an existing path, if it has one that differs from the path as given.
    fn short_name(path: &Path) -> Option<PathBuf> {
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        use windows::core::HSTRING;
        let mut buffer = [0u16; 1024];
        // SAFETY: the call writes at most the buffer's length into it.
        let length =
            unsafe { GetShortPathNameW(&HSTRING::from(path.as_os_str()), Some(&mut buffer)) }
                as usize;
        if length == 0 || length >= buffer.len() {
            return None;
        }
        let short = PathBuf::from(String::from_utf16(&buffer[..length]).ok()?);
        (short != path).then_some(short)
    }

    // The alias comes from the API that makes them. It once came from a `cmd /c for %~sI` whose quotes Rust escaped for a command
    // line cmd does not read that way, so it printed the path back, garbled and with no 8.3 name made; the test took its "no 8.3
    // names here" exit on a PC whose own path has no `~` and checked nothing, and failed on a hosted runner, where the profile
    // folder's own `~` got past that exit and the garbled alias was refused.
    #[test]
    fn an_8_3_alias_uploads_the_canonical_file() {
        let home = root();
        let file = home.path().join("upload-with-a-long-name.txt");
        std::fs::write(&file, "local").unwrap();
        let Some(alias) = short_name(&file) else {
            eprintln!("8.3 names are switched off on this volume: nothing to check");
            return;
        };
        assert!(
            alias.to_string_lossy().contains('~'),
            "an 8.3 spelling has a tilde: {}",
            alias.display()
        );
        let canonical = downloads::plain(std::fs::canonicalize(&file).unwrap());
        let sent = uploads(home.path(), None, &[alias.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(sent, vec![canonical.to_string_lossy().into_owned()]);
    }

    #[test]
    fn output_is_a_new_plain_file_in_browser_files() {
        let home = root();
        let saved = write_output(home.path(), "page.jpg", b"synthetic-image").unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), b"synthetic-image");
        assert_eq!(
            saved.parent().unwrap().file_name().unwrap(),
            "browser-files"
        );
        for refused in [
            "../escape.png",
            "a/b.png",
            r"a\b.png",
            "a:b.png",
            "x*.png",
            "",
            ".",
            "..",
            "tab\t.png",
        ] {
            assert_eq!(
                write_output(home.path(), refused, b"x").unwrap_err().0,
                "Use a plain filename inside this agent's browser-files folder.",
                "{refused:?}"
            );
        }
        assert!(
            write_output(home.path(), "page.jpg", b"again").is_err(),
            "an existing file is never overwritten"
        );
        assert_eq!(std::fs::read(&saved).unwrap(), b"synthetic-image");
    }

    #[test]
    fn output_is_never_named_for_a_windows_device() {
        let home = root();
        // Opening one of these writes to the device (or fails), whatever the extension, in any case.
        for refused in [
            "NUL",
            "nul",
            "NUL.txt",
            "nul.tar.gz",
            "con",
            "Aux.log",
            "PRN",
            "COM1",
            "com9.png",
            "LPT3",
            "lpt1.txt",
            "CONIN$",
            "conout$.txt",
            "NUL .txt",
            "COM\u{b9}",
            "LPT\u{b2}.txt",
            // Windows drops a trailing dot or space, so the file would not be the one named.
            "report.",
            "report ",
        ] {
            assert_eq!(
                write_output(home.path(), refused, b"x").unwrap_err().0,
                "Use a plain filename inside this agent's browser-files folder.",
                "{refused:?}"
            );
        }
        // Names that only look like devices are fine.
        for fine in [
            "console.png",
            "nullable.txt",
            "com10.txt",
            "comma.csv",
            "aux-1.txt",
            "lpt.txt",
            "a.nul",
        ] {
            assert!(write_output(home.path(), fine, b"x").is_ok(), "{fine:?}");
        }
    }
}
