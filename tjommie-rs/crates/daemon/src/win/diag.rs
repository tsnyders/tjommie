//! Diagnostics for the Windows worker launch path.
//!
//! The launch path answers every failure with one fixed sentence on purpose (it must not say what it saw to an
//! unprivileged reader), which leaves the owner and the CI log guessing. This records what each step saw, in order and
//! with the time since the first note, so a failure can be explained from its own record:
//! - [`note`] records a line of what a step saw;
//! - [`problem`] records a line about something that went wrong, and keeps it apart so that [`problems_since`] can
//!   hand back a run's problems (the cause first) for an error message;
//! - [`recent`] returns the last lines of everything.
//!
//! Nothing is printed unless `TJOMMIE_WORKER_DIAG=1`, in which case every note and problem is printed to stderr as it
//! happens and `accept-worker` appends the last lines to a failure. Without it the record is silent: it only costs a
//! little memory, and the cause of a failure reaches the owner through the error.
//!
//! Never pass a password, a credential blob, a token or an environment value here. Account names, SIDs, process ids,
//! image names, paths and Win32 error codes are what this is for.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Instant;

/// How many lines (and how many problems) are kept. Older ones are dropped.
const KEPT: usize = 3000;

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Problems in the order they were recorded, each with its sequence number.
static PROBLEMS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static VERBOSE: LazyLock<bool> =
    LazyLock::new(|| std::env::var("TJOMMIE_WORKER_DIAG").is_ok_and(|value| value == "1"));

/// Whether every note and problem is printed as well as recorded (`TJOMMIE_WORKER_DIAG=1`).
pub fn verbose() -> bool {
    *VERBOSE
}

fn record(message: &str) {
    let line = format!(
        "[worker-diag +{:>6}ms {:?}] {message}",
        STARTED.elapsed().as_millis(),
        std::thread::current().id()
    );
    if verbose() {
        eprintln!("{line}");
    }
    let mut lines = LINES.lock().unwrap_or_else(PoisonError::into_inner);
    if lines.len() >= KEPT {
        lines.remove(0);
    }
    lines.push(line);
}

/// Records one line of what a step saw.
pub fn note(message: impl AsRef<str>) {
    record(message.as_ref());
}

/// Records one line about something that went wrong.
pub fn problem(message: impl AsRef<str>) {
    let message = message.as_ref();
    record(&format!("PROBLEM {message}"));
    let sequence = SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    let mut problems = PROBLEMS.lock().unwrap_or_else(PoisonError::into_inner);
    if problems.len() >= KEPT {
        problems.remove(0);
    }
    problems.push((
        sequence,
        format!("[+{}ms] {message}", STARTED.elapsed().as_millis()),
    ));
}

/// A point to ask [`problems_since`] about: take it when a run starts.
pub fn mark() -> u64 {
    SEQUENCE.load(Ordering::SeqCst)
}

/// The first `limit` problems recorded after `mark`, oldest first: the cause comes before what follows from it.
pub fn problems_since(mark: u64, limit: usize) -> Vec<String> {
    PROBLEMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(sequence, _)| *sequence > mark)
        .take(limit)
        .map(|(_, line)| line.clone())
        .collect()
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
