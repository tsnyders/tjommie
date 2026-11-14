//! `<root>/logs/desktop.log`, the file Electron's main process writes (`main.ts:96-103`): one line per message,
//! here `LEVEL target: message`, from our crates and GPUI alike. Logging never fails the caller.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// Past this size a log moves to `<name>.1`, replacing the one before, so each log keeps under twice this (review F7).
const CAP: u64 = 4 << 20;

/// One append at a time, so two threads can't both move a full log aside.
static APPENDING: Mutex<()> = Mutex::new(());

/// Sends `log` records at Info and above to desktop.log. Call once, at start.
pub fn init(root: &Path) {
    let logger = FileLog {
        file: root.join("logs").join("desktop.log"),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// Appends `text` to `file`, making its folder and moving a full file aside first; a failure is dropped.
pub(crate) fn append(file: &Path, text: &str) {
    let _appending = APPENDING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(file).is_ok_and(|meta| meta.len() >= CAP) {
        let mut aside = file.as_os_str().to_owned();
        aside.push(".1");
        let _ = std::fs::rename(file, aside);
    }
    if let Ok(mut out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = out.write_all(text.as_bytes());
    }
}

struct FileLog {
    file: PathBuf,
}

impl log::Log for FileLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            append(
                &self.file,
                &format!(
                    "{} {}: {}\n",
                    record.level(),
                    record.target(),
                    record.args()
                ),
            );
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log as _;

    fn info(log: &FileLog, text: &str) {
        log.log(
            &log::Record::builder()
                .level(log::Level::Info)
                .target("desktop")
                .args(format_args!("{text}"))
                .build(),
        );
    }

    #[test]
    fn appends_lines_and_never_fails() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("logs").join("desktop.log");
        let log = FileLog { file: file.clone() };
        info(&log, "first");
        info(&log, "second");
        log.log(
            &log::Record::builder()
                .level(log::Level::Debug)
                .target("gpui")
                .args(format_args!("noise"))
                .build(),
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "INFO desktop: first\nINFO desktop: second\n"
        );
        // A logs folder that can't be made (a file is in the way): the line is lost, the caller carries on.
        let blocked = FileLog {
            file: file.join("logs").join("desktop.log"),
        };
        info(&blocked, "lost");
    }

    #[test]
    fn moves_a_full_log_aside() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("desktop.log");
        std::fs::File::create(&file).unwrap().set_len(CAP).unwrap();
        append(&file, "next\n");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "next\n");
        let aside = root.path().join("desktop.log.1");
        assert_eq!(std::fs::metadata(&aside).unwrap().len(), CAP);
        // Under the cap, lines stay in the same file.
        append(&file, "more\n");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "next\nmore\n");
    }
}
