//! Diagnostics for the Windows worker launch path.
//!
//! The launch path answers every failure with one fixed sentence on purpose (it must not say what it saw to an
//! unprivileged reader), which leaves the owner and the CI log guessing. This records what each step saw, in order and
//! with the time since the first note, so a failure can be explained from its own log:
//! - [`note`] records a line, and prints it to stderr when `TJOMMIE_WORKER_DIAG=1`;
//! - [`problem`] records a line and always prints it: it is for the failure paths, which are rare;
//! - [`recent`] returns the last lines, which `accept-worker` appends to a termination error.
//!
//! Never pass a password, a credential blob, a token or an environment value here. Account names, SIDs, process ids,
//! image names, paths and Win32 error codes are what this is for.

use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Instant;

/// How many lines are kept. Older ones are dropped.
const KEPT: usize = 3000;

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static VERBOSE: LazyLock<bool> =
    LazyLock::new(|| std::env::var("TJOMMIE_WORKER_DIAG").is_ok_and(|value| value == "1"));

/// Whether every note is printed as well as recorded (`TJOMMIE_WORKER_DIAG=1`).
pub fn verbose() -> bool {
    *VERBOSE
}

fn record(message: &str, print: bool) {
    let line = format!(
        "[worker-diag +{:>6}ms {:?}] {message}",
        STARTED.elapsed().as_millis(),
        std::thread::current().id()
    );
    if print {
        eprintln!("{line}");
    }
    let mut lines = LINES.lock().unwrap_or_else(PoisonError::into_inner);
    if lines.len() >= KEPT {
        lines.remove(0);
    }
    lines.push(line);
}

/// Records one line of what a step saw; printed only when `TJOMMIE_WORKER_DIAG=1`.
pub fn note(message: impl AsRef<str>) {
    record(message.as_ref(), verbose());
}

/// Records one line about something that went wrong, and always prints it.
pub fn problem(message: impl AsRef<str>) {
    record(&format!("PROBLEM {}", message.as_ref()), true);
}

/// The last `count` lines recorded, oldest first.
pub fn recent(count: usize) -> Vec<String> {
    let lines = LINES.lock().unwrap_or_else(PoisonError::into_inner);
    lines[lines.len().saturating_sub(count)..].to_vec()
}

/// At most `limit` bytes of `bytes` as text with control characters made visible, for a pipe's tail in a log line.
pub fn excerpt(bytes: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(limit)]);
    let shown: String = text
        .chars()
        .map(|c| if c.is_control() && c != ' ' { '.' } else { c })
        .collect();
    if bytes.len() > limit {
        format!("{shown}... ({} bytes)", bytes.len())
    } else {
        shown
    }
}
