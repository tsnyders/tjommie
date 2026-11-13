//! Port of `packages/daemon/src/project-checks.ts`: one project check as a bounded, cancellable child process.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use protocol::projects::{CheckRun, ProjectCheck};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::cli::{BoxFuture, CliChild, CliCommand, CliExit, CliHost, LaunchOptions};
use crate::win::boundary::BoundaryError;

/// The most output kept, in bytes (project-checks.ts:22-23).
const OUTPUT_LIMIT: usize = 262_144;
/// The output text is cut to this many UTF-16 code units (project-checks.ts:50).
const OUTPUT_TEXT_LIMIT: usize = 262_000;
/// The only variables a check's environment gets (project-checks.ts:13).
const SCRUBBED_ENVIRONMENT: [&str; 8] = [
    "PATH",
    "SystemRoot",
    "SystemDrive",
    "WINDIR",
    "PATHEXT",
    "TEMP",
    "TMP",
    "COMSPEC",
];
const CANCELLED_BEFORE_START: &str = "Check cancelled.";
const LAUNCH_TERMINATED: &str =
    "Could not confirm that the owned check launch stopped. Its worktree remains reserved.";
const LAUNCH_FAILED: &str = "The configured check could not start. Isolated hosts require an executable in their protected worker runtime.";
const STOP_UNCONFIRMED: &str = "Could not confirm that the owned check process tree stopped.";
const CHILD_FAILED: &str = "The configured check could not start.";

/// `CheckTerminationError` (`Termination`) and every other check error (`Failed`, a plain `Error`). The message is
/// what the RPC layer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// `CheckTerminationError`: a process tree's stop, or a launch's stop, could not be confirmed.
    Termination(String),
    /// Any other check error.
    Failed(String),
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Termination(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CheckError {}

/// `TaskEvaluation["checks"][number]` (project-checks.ts:4).
pub type CheckResult = CheckRun;

/// `CheckRuntime` (project-checks.ts:5).
pub trait CheckRuntime: Send + Sync {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        cwd: &'a str,
        signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>>;
}

/// `HostCheckRuntime` (project-checks.ts:9). Use only after the trusted caller verifies explicit check acceptance and
/// Full auto autonomy.
pub struct HostCheckRuntime {
    host: Arc<dyn CliHost>,
}

impl HostCheckRuntime {
    /// `launchCli` is `host.launch`.
    pub fn new(host: Arc<dyn CliHost>) -> Self {
        Self { host }
    }

    async fn run_check(
        &self,
        check: &ProjectCheck,
        cwd: &str,
        signal: &CancellationToken,
    ) -> Result<CheckResult, CheckError> {
        if signal.is_cancelled() {
            return Err(CheckError::Failed(CANCELLED_BEFORE_START.into()));
        }
        let command = CliCommand {
            file: check.command.clone(),
            args: Vec::new(),
            codex_package: None,
        };
        let options = LaunchOptions {
            cwd: cwd.to_string(),
            env: scrubbed_environment(),
            claude_api_key: None,
        };
        let child = self
            .host
            .launch(&command, check.args.clone(), options)
            .await
            .map_err(|error| {
                if is_worker_termination(&error) {
                    CheckError::Termination(LAUNCH_TERMINATED.into())
                } else {
                    CheckError::Failed(LAUNCH_FAILED.into())
                }
            })?;
        let CliChild {
            stdin,
            stdout,
            stderr,
            exit,
            kill_tree,
            ..
        } = child;

        let capture = Mutex::new(Capture::default());
        // Settled = the streams have ended (Node's 'close') and the process has exited.
        let settled = async {
            let mut stdin = stdin;
            // `child.stdin.end()`: the check gets no input. A failed close is ignored, as in TypeScript.
            let closing = async {
                let _ = stdin.shutdown().await;
            };
            let (_, _, _, status) = tokio::join!(
                closing,
                pump(stdout, &capture),
                pump(stderr, &capture),
                exit.wait()
            );
            status
        };
        tokio::pin!(settled);
        let timer = tokio::time::sleep(Duration::from_millis(check.timeout_ms));
        tokio::pin!(timer);

        // One stop attempt: the first of the timeout or the cancellation starts it, and nothing else does.
        let mut stop: Option<BoxFuture<'static, Result<(), String>>> = None;
        let mut stop_started = false;
        let mut timed_out = false;
        let status = loop {
            tokio::select! {
                biased;
                () = &mut timer, if !timed_out => {
                    timed_out = true;
                    if !stop_started {
                        stop_started = true;
                        stop = Some((kill_tree)());
                    }
                }
                () = signal.cancelled(), if !stop_started => {
                    stop_started = true;
                    stop = Some((kill_tree)());
                }
                result = await_stop(&mut stop) => {
                    stop = None;
                    if result.is_err() {
                        return Err(termination());
                    }
                }
                status = &mut settled => break status,
            }
        };

        // A child `error` event is a plain failure in TypeScript (project-checks.ts:34). Only the worker boundary's
        // unconfirmed retirement (`workerProcessRetired` undefined) is a termination, and it carries its own text.
        let exit_code = match status {
            CliExit::Failed(message) if message == BoundaryError::Termination.to_string() => {
                return Err(termination());
            }
            CliExit::Failed(_) => return Err(CheckError::Failed(CHILD_FAILED.into())),
            CliExit::Exited { code, .. } => code,
        };
        if let Some(pending) = stop.take() {
            pending.await.map_err(|_| termination())?;
        }
        timed_out |= timer.is_elapsed();

        let (bytes, truncated) = {
            let mut captured = capture.lock().unwrap_or_else(PoisonError::into_inner);
            (std::mem::take(&mut captured.output), captured.truncated)
        };
        let cancelled = signal.is_cancelled();
        let mut output = decoded_prefix(&bytes);
        if timed_out {
            output.push_str("\nCheck timed out.");
        } else if cancelled {
            output.push_str("\nCheck cancelled.");
        }
        Ok(CheckRun {
            check_id: check.id.clone(),
            command: check.command.clone(),
            args: check.args.clone(),
            exit_code: if timed_out || cancelled {
                None
            } else {
                exit_code.map(i64::from)
            },
            output,
            truncated,
        })
    }
}

impl CheckRuntime for HostCheckRuntime {
    fn run<'a>(
        &'a self,
        check: &'a ProjectCheck,
        cwd: &'a str,
        signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<CheckResult, CheckError>> {
        Box::pin(self.run_check(check, cwd, signal))
    }
}

fn termination() -> CheckError {
    CheckError::Termination(STOP_UNCONFIRMED.into())
}

/// `error instanceof WorkerLaunchTerminationError`. A launch the worker could not prove stopped arrives in one of two
/// forms. Production checks go through `SystemCli::with_worker`, where the boundary's `Termination` becomes
/// `cli::WorkerTermination` (`win/boundary.rs`, `cli::WorkerBoundary::spawn`). A boundary used directly as the
/// `CliHost` carries its own `BoundaryError::Termination` inside the `io::Error` instead.
fn is_worker_termination(error: &io::Error) -> bool {
    crate::cli::is_worker_termination(error)
        || error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<BoundaryError>())
            .is_some_and(|boundary| matches!(boundary, BoundaryError::Termination))
}

/// `{ ...process.env }` for the scrubbed keys (project-checks.ts:12-15).
fn scrubbed_environment() -> BTreeMap<String, String> {
    SCRUBBED_ENVIRONMENT
        .iter()
        .filter_map(|key| {
            std::env::var_os(key)
                .map(|value| ((*key).to_string(), value.to_string_lossy().into_owned()))
        })
        .collect()
}

/// The output kept so far: at most `OUTPUT_LIMIT` bytes, and whether anything beyond it arrived.
#[derive(Default)]
struct Capture {
    output: Vec<u8>,
    truncated: bool,
}

impl Capture {
    /// The `append` listener (project-checks.ts:21-24). `truncated` is judged against the kept length, as in TypeScript.
    fn append(&mut self, chunk: &[u8]) {
        self.truncated |= self.output.len() + chunk.len() > OUTPUT_LIMIT;
        let room = OUTPUT_LIMIT.saturating_sub(self.output.len());
        self.output
            .extend_from_slice(&chunk[..chunk.len().min(room)]);
    }
}

/// One `data` listener: reads a stream to its end. Bytes past the limit are read and dropped, so the child never
/// blocks on a full pipe.
async fn pump(mut stream: Box<dyn AsyncRead + Send + Unpin>, capture: &Mutex<Capture>) {
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => capture
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .append(&buffer[..read]),
        }
    }
}

/// Awaits the stop attempt once it exists. Before it starts, and after it has finished, this never completes.
async fn await_stop(
    stop: &mut Option<BoxFuture<'static, Result<(), String>>>,
) -> Result<(), String> {
    match stop {
        Some(pending) => pending.await,
        None => std::future::pending().await,
    }
}

/// `Buffer.toString("utf8").slice(0, 262000)`. Invalid bytes become U+FFFD. The slice counts UTF-16 code units; a cut
/// inside a surrogate pair leaves a lone surrogate in JavaScript, which Rust strings can't hold, so it becomes U+FFFD.
fn decoded_prefix(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= OUTPUT_TEXT_LIMIT {
        return text.into_owned();
    }
    let units: Vec<u16> = text.encode_utf16().take(OUTPUT_TEXT_LIMIT).collect();
    String::from_utf16_lossy(&units)
}
