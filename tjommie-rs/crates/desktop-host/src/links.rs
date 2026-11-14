//! What the app opens outside itself (T6.6; the link rules of `app-protocol.ts`, `main.ts:567-585`). The native app has no
//! pages of its own to navigate, so only the second half of `navigationFor` survives: a web link goes to the default
//! browser, anything else is blocked. Folders and files open only when they pass the local-path rules.

use std::path::{Path, PathBuf};

use url::Url;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::paths::local_file_path;
use crate::spawn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Navigation {
    /// Hand it to the default browser.
    Browser,
    /// Do nothing with it: a `file:` address, a script, another app's scheme, or nothing that parses.
    Block,
}

/// `navigationFor`, without the in-app pages: only `http` and `https` leave the app.
pub fn navigation_for(url: &str) -> Navigation {
    match Url::parse(url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => {
            Navigation::Browser
        }
        _ => Navigation::Block,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `ShellExecuteW "open"`: the default handler for `target`.
fn shell_open(target: &str) -> Result<(), String> {
    let target = wide(target);
    // SAFETY: NUL-terminated strings that outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success with a value above 32.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err("Windows could not open that.".into())
    }
}

/// Opens a web link in the default browser. The address Windows gets is the parsed, normalized one, never the raw text.
pub fn open_external(url: &str) -> Result<(), String> {
    open_external_with(url, shell_open)
}

/// `open_external` with the opener passed in, so a test can see what would have been opened.
pub fn open_external_with(
    url: &str,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    if navigation_for(url) != Navigation::Browser {
        return Err("Only web links open outside Tjommie.".into());
    }
    let parsed = Url::parse(url).map_err(|_| "Only web links open outside Tjommie.".to_string())?;
    open(parsed.as_str())
}

/// `folder:open`: shows a folder in Explorer. Only a real, local folder.
pub fn open_folder(path: &str) -> Result<(), String> {
    if !local_file_path(path) || !Path::new(path).is_dir() {
        return Err("That folder can't be opened.".into());
    }
    shell_open(path)
}

/// `%SystemRoot%explorer.exe`, by absolute path (the daemon's convention for system tools, `cli::system32`): never a
/// lookup through the PATH or the current folder.
fn explorer_exe() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:Windows"), PathBuf::from)
        .join("explorer.exe")
}

/// Explorer asked to select `path`. The path was checked to be an existing local file, which can't hold a quote, so
/// quoting it by hand (Explorer wants `/select,"path"` as one argument) is safe.
fn explorer_command(path: &str) -> spawn::NoWindowCommand {
    use std::os::windows::process::CommandExt as _;
    let mut command = spawn::command(explorer_exe());
    command.raw_arg(format!("/select,\"{path}\""));
    command
}

/// `file:show`: selects a file in Explorer. Only a real, local file.
pub fn show_in_folder(path: &str) -> Result<(), String> {
    if !local_file_path(path) || !Path::new(path).is_file() {
        return Err("That file can't be shown.".into());
    }
    explorer_command(path)
        .spawn()
        .map(drop)
        .map_err(|_| "Windows could not show that file.".to_string())
}

/// `notes:open`: an agent's notes.md in the default editor, and no other file.
pub fn open_notes(agents_dir: &Path, home_folder: &Path) -> Result<(), String> {
    let file = crate::paths::notes_file(agents_dir, home_folder)
        .ok_or_else(|| "That agent has no notes to open.".to_string())?;
    shell_open(&file.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    // `app-protocol.test.ts` "keeps Tjommie's own pages in the window and sends web links to the browser": the half that
    // still applies. (The `stay` cases, and `uiFile`, served the static export, which the native app doesn't have.)
    #[test]
    fn web_links_go_to_the_browser_and_everything_else_is_blocked() {
        assert_eq!(
            navigation_for("https://pnet.co.za/jobs"),
            Navigation::Browser
        );
        assert_eq!(
            navigation_for("http://localhost:3000/chat/"),
            Navigation::Browser
        );
        for blocked in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "app://evil/",
            "app://tjommie/chat/",
            "ftp://example.com/a",
            "mailto:a@b.c",
            "data:text/html,<b>x</b>",
            "C:\\Windows\\System32\\calc.exe",
            "calc.exe",
            "",
            "not a url",
            "https://",
        ] {
            assert_eq!(navigation_for(blocked), Navigation::Block, "{blocked:?}");
        }
    }

    #[test]
    fn only_a_web_link_reaches_windows_and_as_the_normalized_address() {
        let opened = RefCell::new(Vec::new());
        let open = |target: &str| {
            opened.borrow_mut().push(target.to_string());
            Ok(())
        };
        assert!(open_external_with("https://example.com/a b?x=1&y=\"2\"", open).is_ok());
        assert_eq!(
            opened.borrow().as_slice(),
            ["https://example.com/a%20b?x=1&y=%222%22"]
        );
        let never = |_: &str| -> Result<(), String> { panic!("must not open") };
        for blocked in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "calc.exe",
            "https://",
        ] {
            assert!(open_external_with(blocked, never).is_err(), "{blocked}");
        }
    }

    // Review finding 6: a system tool is started by its absolute path, as the daemon starts taskkill, never found through the
    // PATH or the current folder, where a look-alike could sit.
    #[test]
    fn explorer_is_started_by_its_full_system_path() {
        let explorer = explorer_exe();
        assert!(explorer.is_absolute(), "{}", explorer.display());
        assert!(explorer.is_file(), "{}", explorer.display());
        assert!(explorer.ends_with("explorer.exe"));
        let command = explorer_command(r"C:a bc.txt");
        assert_eq!(command.get_program(), explorer.as_os_str());
    }

    #[test]
    fn folders_and_files_open_only_when_they_are_local_and_real() {
        assert!(open_folder(r"\\attacker\share").is_err());
        assert!(open_folder(r"C:\no\such\folder\anywhere").is_err());
        assert!(open_folder("relative").is_err());
        assert!(show_in_folder(r"\\attacker\share\a.txt").is_err());
        assert!(show_in_folder(r"C:\no\such\file.txt").is_err());
        let dir = tempfile::tempdir().unwrap();
        // A folder is not a file to show, and a file is not a folder to open.
        assert!(show_in_folder(&dir.path().to_string_lossy()).is_err());
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(open_folder(&file.to_string_lossy()).is_err());
        assert!(open_notes(dir.path(), &dir.path().join("b1")).is_err());
    }
}
