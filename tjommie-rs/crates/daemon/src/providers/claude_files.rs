//! The owner marker in each Claude session folder. `sweepClaudeSessionFiles`, its reader, comes with `setup.ts`
//! in T3.4.

use std::path::Path;

use serde_json::{Value, json};

pub enum SessionPhase {
    Preparing,
    Starting,
    Started { child_pid: u32 },
}

/// Writes `owner.json` atomically: a live owner or CLI keeps the folder from being swept.
pub fn record_claude_session_owner(folder: &Path, phase: SessionPhase) -> std::io::Result<()> {
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
