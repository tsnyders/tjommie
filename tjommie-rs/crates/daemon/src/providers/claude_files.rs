//! `claude-files.ts`: the owner marker in each Claude session folder, and the sweep of stale session folders.

use std::io;
use std::path::Path;

use serde_json::{Value, json};

pub enum SessionPhase {
    Preparing,
    Starting,
    Started { child_pid: u32 },
}

/// Writes `owner.json` atomically: a live owner or CLI keeps the folder from being swept.
pub fn record_claude_session_owner(folder: &Path, phase: SessionPhase) -> io::Result<()> {
    let (phase, child_pid) = match phase {
        SessionPhase::Preparing => ("preparing", Value::Null),
        SessionPhase::Starting => ("starting", Value::Null),
        SessionPhase::Started { child_pid } => ("started", child_pid.into()),
    };
    let file = folder.join("owner.json");
    let temporary = folder.join("owner.json.tmp");
    let owner = json!({ "version": 1, "ownerPid": std::process::id(), "phase": phase, "childPid": child_pid });
    std::fs::write(&temporary, owner.to_string())?;
    std::fs::rename(&temporary, &file)
}

enum Owner {
    /// `preparing`: nothing launched yet.
    Preparing {
        owner_pid: u32,
    },
    /// `starting`: a CLI may be running whose PID was never recorded.
    Starting,
    Started {
        owner_pid: u32,
        child_pid: u32,
    },
}

fn pid(value: Option<&Value>) -> Option<u32> {
    value?
        .as_u64()
        .filter(|pid| *pid > 0)
        .and_then(|pid| u32::try_from(pid).ok())
}

/// `None` for a marker that can't be trusted. Filesystem errors other than a missing marker are errors.
fn read_owner(folder: &Path) -> io::Result<Option<Owner>> {
    let file = folder.join("owner.json");
    let stat = std::fs::symlink_metadata(&file)?;
    if !stat.is_file() || stat.file_type().is_symlink() || stat.len() > 1024 {
        return Ok(None);
    }
    let bytes = std::fs::read(&file)?;
    let Ok(Value::Object(owner)) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&bytes))
    else {
        return Ok(None);
    };
    let Some(owner_pid) = pid(owner
        .get("version")
        .filter(|version| version.as_u64() == Some(1))
        .and(owner.get("ownerPid")))
    else {
        return Ok(None);
    };
    Ok(
        match (
            owner.get("phase").and_then(Value::as_str),
            owner.get("childPid"),
        ) {
            (Some("preparing"), Some(Value::Null)) => Some(Owner::Preparing { owner_pid }),
            (Some("starting"), Some(Value::Null)) => Some(Owner::Starting),
            (Some("started"), child) => pid(child).map(|child_pid| Owner::Started {
                owner_pid,
                child_pid,
            }),
            _ => None,
        },
    )
}

/// `sweepClaudeSessionFiles`. Account folders are shared by daemon processes and real tests: a live owner or CLI, an
/// unknown process state, or an incomplete launch marker keeps its files.
pub fn sweep_claude_session_files(account_folder: &Path) {
    sweep_claude_session_files_with(account_folder, crate::cli::process_gone);
}

/// The sweep with `gone(pid)` as the process probe. A process this user may not open counts as alive.
pub fn sweep_claude_session_files_with(account_folder: &Path, gone: impl Fn(u32) -> bool) {
    let entries = match std::fs::read_dir(account_folder) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!(
                    "Could not check Claude's session files. {}",
                    account_folder.display()
                );
            }
            return;
        }
    };
    for entry in entries.flatten() {
        let named = entry
            .file_name()
            .to_string_lossy()
            .starts_with(".tjommie-session-");
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !named || !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let folder = entry.path();
        let swept = (|| -> io::Result<()> {
            let stale = match read_owner(&folder)? {
                Some(Owner::Preparing { owner_pid }) => gone(owner_pid),
                Some(Owner::Started {
                    owner_pid,
                    child_pid,
                }) => gone(owner_pid) && gone(child_pid),
                Some(Owner::Starting) | None => false,
            };
            let stat = std::fs::symlink_metadata(&folder)?;
            if stale && stat.is_dir() && !stat.file_type().is_symlink() {
                std::fs::remove_dir_all(&folder)?;
            }
            Ok(())
        })();
        if let Err(error) = swept
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "Could not remove stale Claude session files. {}",
                folder.display()
            );
        }
    }
}
