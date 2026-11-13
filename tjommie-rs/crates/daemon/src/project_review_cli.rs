//! Port of `packages/daemon/src/project-review-cli.ts`: one bounded Claude review turn, and the CLI version probe.
//! Every path that can't prove the reviewer's process tree has stopped reports a `CheckError::Termination`, which keeps
//! its account and scratch profile reserved.

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::cli::{
    ChildExit, CliChild, CliCommand, CliExit, CliHost, KillTree, LaunchOptions, Retirement,
    wait_for_cli_exit,
};
use crate::ids::new_id;
use crate::project_checks::CheckError;
use crate::project_review_policy::{
    REVIEW_OUTPUT_LIMIT, ReviewEffectivePolicy, is_record, review_effective_policy,
};
use crate::providers::claude_parse::{ClaudeLine, ResultLine, parse_claude_line};
use crate::providers::{ApiKey, ProviderEvent, TokenUsage, Usage, redact_provider_event};
use crate::win::boundary::BoundaryError;

const DEFAULT_TIMEOUT_MS: u64 = 180_000;
const RESULT_LIMIT: usize = 262_144;
const READ_CHUNK: usize = 8_192;
const VERSION_OUTPUT_LIMIT: usize = 4_096;
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MINIMUM_PATCH: u64 = 314;
const LAUNCH_RETIREMENT: &str = "Reviewer launch retirement is unconfirmed; its account and scratch \
    profile are retained.";
const PROCESS_TREE: &str = "Reviewer process-tree termination is unconfirmed; its account and scratch \
    profile are retained.";
const JOB_RETIREMENT: &str = "Reviewer job retirement is unconfirmed; its account and scratch profile \
    are retained.";
const SPENDING: &str = "Reviewer spending could not be recorded; its account and scratch profile are \
    retained for host reconciliation.";
const VERSION_LAUNCH: &str = "Reviewer version-probe retirement is unconfirmed.";
const VERSION_JOB: &str = "Reviewer version-probe job retirement is unconfirmed.";
const VERSION_PROCESS: &str = "Reviewer version-probe process exit is unconfirmed.";
static VERSION_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(2\.1\.([0-9]+)) \(Claude Code\)$").expect("the version pattern is valid")
});

/// `ReviewCliResult` (`project-review-cli.ts:9`): the reviewer's text, with the policy it ran under and what it cost.
pub struct ReviewCliResult {
    pub text: String,
    pub policy: ReviewEffectivePolicy,
    pub cost_usd: f64,
    pub model: String,
}

/// `ReviewCliOptions` (`project-review-cli.ts:10-11`). `on_event` is `onEvent`; `host.launch` is `launchCli`.
pub struct ReviewCliOptions<'a> {
    pub host: Arc<dyn CliHost>,
    pub command: CliCommand,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub input: String,
    pub model: String,
    pub signal: CancellationToken,
    pub api_key: Option<String>,
    pub on_event: &'a (dyn Fn(ProviderEvent) -> Result<(), String> + Send + Sync),
    pub timeout_ms: Option<u64>,
}

/// The process-tree stop that `stop()` starts once (`killTreeAndWait(child.pid)`), and its outcome once it is known.
enum Stop {
    NotStarted,
    Running(oneshot::Receiver<Result<(), String>>),
    Finished(Result<(), String>),
}

impl Stop {
    /// `await stopped`: waits for a running stop; every later call sees the same outcome.
    async fn settled(&mut self) -> Result<(), String> {
        if let Stop::Running(receiver) = self {
            let outcome = receiver
                .await
                .unwrap_or_else(|_| Err("The process-tree stop ended without an outcome.".into()));
            *self = Stop::Finished(outcome);
        }
        match self {
            Stop::Finished(outcome) => outcome.clone(),
            Stop::NotStarted | Stop::Running(_) => Ok(()),
        }
    }

    fn is_running(&self) -> bool {
        matches!(self, Stop::Running(_))
    }
}

/// Starts the stop without waiting, as the TypeScript promise does. Only the first call starts it.
fn start_stop(stop: &mut Stop, kill_tree: &KillTree) {
    if matches!(stop, Stop::NotStarted) {
        let (sender, receiver) = oneshot::channel();
        let stopping = kill_tree();
        tokio::spawn(async move {
            let _ = sender.send(stopping.await);
        });
        *stop = Stop::Running(receiver);
    }
}

struct Answer {
    text: String,
    cost_usd: f64,
    model: String,
}

/// The state that `runReviewCli`'s callbacks share (its closure variables).
struct Turn<'a> {
    turn_id: String,
    api_key: Option<&'a str>,
    on_event: &'a (dyn Fn(ProviderEvent) -> Result<(), String> + Send + Sync),
    kill_tree: KillTree,
    stop: Stop,
    usage_reliable: bool,
    bytes: usize,
    pending: Vec<u8>,
    failure: Option<String>,
    policy: Option<ReviewEffectivePolicy>,
    answer: Option<Answer>,
    result_seen: bool,
}

impl Turn<'_> {
    /// `fail(message, terminate)`: the first failure is kept; a terminating one also stops the process tree.
    fn fail(&mut self, message: &str, terminate: bool) {
        self.record_failure(message);
        if terminate {
            self.begin_stop();
        }
    }

    /// `failure ??= new Error(message)`.
    fn record_failure(&mut self, message: &str) {
        if self.failure.is_none() {
            self.failure = Some(message.to_owned());
        }
    }

    fn begin_stop(&mut self) {
        start_stop(&mut self.stop, &self.kill_tree);
    }

    fn stop_running(&self) -> bool {
        self.stop.is_running()
    }

    /// `emit`: records an event with the key redacted. A recorder that fails makes the usage unreliable.
    fn emit(&mut self, event: ProviderEvent) {
        let event = redact_provider_event(event, self.api_key);
        if (self.on_event)(event).is_err() {
            self.usage_reliable = false;
            self.fail("review_usage_record_failed", true);
        }
    }

    /// `line(text)`: checks one stdout line against the reviewer protocol. Like TypeScript, a failure does not skip
    /// the lines after it; only the output limit stops the reader.
    fn line(&mut self, text: &str) {
        let raw = match serde_json::from_str::<Value>(text) {
            Ok(raw) if is_record(&raw) => raw,
            _ => {
                self.fail("review_provider_protocol_invalid", true);
                return;
            }
        };
        let kind = text_field(&raw, "type");
        if kind == Some("system") && text_field(&raw, "subtype") == Some("init") {
            if self.policy.is_some() {
                self.fail("review_provider_policy_changed", true);
                return;
            }
            match review_effective_policy(&raw) {
                Ok(policy) => self.policy = Some(policy),
                Err(_) => self.fail("review_effective_tools_unavailable", true),
            }
        }
        if kind == Some("control_request") || (kind == Some("user") && has_tool_result(&raw)) {
            self.fail("review_tool_attempted", true);
        }
        let parsed = parse_claude_line(text);
        if matches!(&parsed, ClaudeLine::Assistant { tools, .. } if !tools.is_empty())
            || matches!(&parsed, ClaudeLine::Permission(_))
        {
            self.fail("review_tool_attempted", true);
        }
        match parsed {
            ClaudeLine::RateLimit(limit) => self.emit(ProviderEvent::Limit(limit)),
            ClaudeLine::Result(result) => self.result_line(&raw, result),
            _ => {}
        }
    }

    /// The `result` branch of `line`: one final usage record, and the answer when the turn completed.
    fn result_line(&mut self, raw: &Value, parsed: ResultLine) {
        if self.result_seen {
            self.fail("review_provider_result_repeated", true);
            return;
        }
        self.result_seen = true;
        let usage = parsed.usage;
        let counts_safe = [
            usage.input,
            usage.cached_input,
            usage.output,
            usage.reasoning,
        ]
        .into_iter()
        .all(safe_count);
        let cost = parsed
            .total_cost_usd
            .filter(|cost| cost.is_finite() && *cost >= 0.0);
        let model = parsed.model.filter(|model| !model.is_empty());
        let (true, Some(cost), Some(model)) = (counts_safe, cost, model) else {
            self.fail("review_usage_unavailable", false);
            return;
        };
        self.usage_reliable = match self.failure.as_deref() {
            None => true,
            Some(message) => matches!(message, "review_cancelled" | "review_timeout"),
        };
        let event_id = parsed.event_id.as_deref().unwrap_or("result");
        self.emit(ProviderEvent::Usage(Usage {
            model: model.clone(),
            tokens: usage,
            cost_usd: Some(cost),
            total_cost_usd: Some(cost),
            event_id: Some(format!("{}:{event_id}", self.turn_id)),
        }));
        match raw.get("result").and_then(Value::as_str) {
            Some(text) if parsed.ok && self.policy.is_some() && text.len() <= RESULT_LIMIT => {
                self.answer = Some(Answer {
                    text: text.to_owned(),
                    cost_usd: cost,
                    model,
                });
            }
            _ => self.fail("review_provider_turn_incomplete", false),
        }
    }

    /// The stdout `data` listener: counts the bytes against the output limit, then checks each complete line.
    fn stdout_chunk(&mut self, chunk: &[u8]) {
        self.bytes += chunk.len();
        if self.bytes > REVIEW_OUTPUT_LIMIT {
            self.fail("review_output_limit", true);
            return;
        }
        self.pending.extend_from_slice(chunk);
        let mut start = 0;
        while let Some(offset) = self.pending[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + offset;
            let mut line = self.pending[start..end].to_vec();
            start = end + 1;
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.is_empty() {
                self.line(&String::from_utf8_lossy(&line));
            }
        }
        self.pending.drain(..start);
    }

    /// The stderr `data` listener: stderr counts against the same output limit.
    fn stderr_chunk(&mut self, len: usize) {
        self.bytes += len;
        if self.bytes > REVIEW_OUTPUT_LIMIT {
            self.fail("review_output_limit", true);
        }
    }

    /// The stdin `error` listener: a closed input is a failure, but it does not stop the tree by itself.
    fn input_closed(&mut self) {
        self.record_failure("review_provider_input_closed");
    }
}

/// `runReviewCli` (`project-review-cli.ts:14`): runs one review turn. Output is bounded, each line is checked against the
/// reviewer protocol, and the process tree is stopped before the answer, or the failure, is returned.
pub async fn run_review_cli(options: ReviewCliOptions<'_>) -> Result<ReviewCliResult, CheckError> {
    if options.signal.is_cancelled() {
        return Err(CheckError::Failed("review_cancelled".into()));
    }
    let turn_id = new_id();
    let api_key = options.api_key.clone().filter(|key| !key.is_empty());
    let launch = LaunchOptions {
        cwd: options.cwd.clone(),
        env: options.env.clone(),
        claude_api_key: api_key.clone().map(ApiKey),
    };
    let mut child = match options
        .host
        .launch(&options.command, options.args.clone(), launch)
        .await
    {
        Ok(child) => child,
        Err(error) if is_launch_termination(&error) => {
            // TypeScript's `finally` throws here, so the retirement error replaces any error from recording this.
            let _ = (options.on_event)(unknown_usage(
                &options.model,
                format!("{turn_id}:unknown-launch"),
            ));
            return Err(CheckError::Termination(LAUNCH_RETIREMENT.into()));
        }
        Err(error) => return Err(CheckError::Failed(error.to_string())),
    };
    let mut turn = Turn {
        turn_id,
        api_key: api_key.as_deref(),
        on_event: options.on_event,
        kill_tree: child.kill_tree.clone(),
        stop: Stop::NotStarted,
        usage_reliable: false,
        bytes: 0,
        pending: Vec::new(),
        failure: None,
        policy: None,
        answer: None,
        result_seen: false,
    };
    let timeout = Duration::from_millis(options.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let outcome = review_turn(
        &mut turn,
        &mut child,
        options.input.as_bytes(),
        &options.signal,
        timeout,
    )
    .await;
    // Every path awaits the stop: a stop that failed means the tree may still run, whatever else happened.
    let outcome = match turn.stop.settled().await {
        Ok(()) => outcome,
        Err(_) => Err(CheckError::Termination(PROCESS_TREE.into())),
    };
    // TypeScript's `finally`: spending that was never recorded in a final usage record is recorded as unknown.
    if child.pid.is_some() && !turn.usage_reliable {
        let unknown = unknown_usage(&options.model, format!("{}:unknown", turn.turn_id));
        if (options.on_event)(unknown).is_err() {
            return Err(CheckError::Termination(SPENDING.into()));
        }
    }
    outcome
}

/// The body of `runReviewCli`'s `try`: waits until the turn closes, then returns its answer or its first failure.
async fn review_turn(
    turn: &mut Turn<'_>,
    child: &mut CliChild,
    input: &[u8],
    signal: &CancellationToken,
    timeout: Duration,
) -> Result<ReviewCliResult, CheckError> {
    read_until_closed(turn, child, input, signal, timeout).await?;
    // `closed` rejected: the caller's stop is still awaited (the outcome handling in `run_review_cli`), as in the catch.
    if start_failed(&child.exit) {
        return Err(CheckError::Failed("review_provider_start_failed".into()));
    }
    if !job_retired(&child.exit, child.retirement.as_ref()).await {
        return Err(CheckError::Termination(JOB_RETIREMENT.into()));
    }
    if !turn.pending.is_empty() {
        let rest = String::from_utf8_lossy(&turn.pending).into_owned();
        turn.line(&rest);
    }
    if let Some(message) = &turn.failure {
        return Err(CheckError::Failed(message.clone()));
    }
    match (
        exit_code(&child.exit),
        turn.answer.take(),
        turn.policy.take(),
    ) {
        (Some(0), Some(answer), Some(policy)) => Ok(ReviewCliResult {
            text: answer.text,
            policy,
            cost_usd: answer.cost_usd,
            model: answer.model,
        }),
        _ => Err(CheckError::Failed("review_provider_turn_incomplete".into())),
    }
}

/// The `closed` wait of `runReviewCli`: reads bounded output, writes the prompt, and watches the timer, the caller's
/// cancellation and the stop, until stdout, stderr and the child's exit have all finished. A stop that fails ends the
/// wait at once, as TypeScript's `stopFailure` does.
async fn read_until_closed(
    turn: &mut Turn<'_>,
    child: &mut CliChild,
    input: &[u8],
    signal: &CancellationToken,
    timeout: Duration,
) -> Result<(), CheckError> {
    if signal.is_cancelled() {
        turn.fail("review_cancelled", true);
    }
    let mut out = [0u8; READ_CHUNK];
    let mut err = [0u8; READ_CHUNK];
    let (mut stdout_open, mut stderr_open, mut stdin_open, mut exited) = (true, true, true, false);
    let (mut timer_armed, mut cancel_armed) = (true, true);
    let stdin_written = write_input(child.stdin.as_mut(), input);
    tokio::pin!(stdin_written);
    let timer = tokio::time::sleep(timeout);
    tokio::pin!(timer);
    while stdout_open || stderr_open || !exited {
        tokio::select! {
            read = child.stdout.read(&mut out), if stdout_open => match read {
                Ok(n) if n > 0 => turn.stdout_chunk(&out[..n]),
                _ => stdout_open = false,
            },
            read = child.stderr.read(&mut err), if stderr_open => match read {
                Ok(n) if n > 0 => turn.stderr_chunk(n),
                _ => stderr_open = false,
            },
            written = &mut stdin_written, if stdin_open => {
                stdin_open = false;
                if written.is_err() {
                    turn.input_closed();
                }
            }
            _ = child.exit.wait(), if !exited => exited = true,
            _ = &mut timer, if timer_armed => {
                timer_armed = false;
                turn.fail("review_timeout", true);
            }
            _ = signal.cancelled(), if cancel_armed => {
                cancel_armed = false;
                turn.fail("review_cancelled", true);
            }
            settled = turn.stop.settled(), if turn.stop_running() => {
                if settled.is_err() {
                    return Err(CheckError::Termination(PROCESS_TREE.into()));
                }
            }
        }
    }
    Ok(())
}

/// `reviewCliVersion` (`project-review-cli.ts:90`): runs `claude --version` and accepts only Claude Code 2.1.314 or
/// later. The output is capped and the probe is stopped after 10 seconds or when the caller cancels.
pub async fn review_cli_version(
    host: &Arc<dyn CliHost>,
    command: &CliCommand,
    cwd: &str,
    env: BTreeMap<String, String>,
    signal: &CancellationToken,
) -> Result<String, CheckError> {
    if signal.is_cancelled() {
        return Err(CheckError::Failed("review_cancelled".into()));
    }
    let CliChild {
        stdin,
        mut stdout,
        mut stderr,
        exit,
        kill_tree,
        retirement,
        ..
    } = host
        .launch(
            command,
            vec!["--version".into()],
            LaunchOptions {
                cwd: cwd.to_owned(),
                env,
                claude_api_key: None,
            },
        )
        .await
        .map_err(|error| {
            // TypeScript lets this escape unchanged, and its callers treat it as termination.
            if is_launch_termination(&error) {
                CheckError::Termination(VERSION_LAUNCH.into())
            } else {
                CheckError::Failed(error.to_string())
            }
        })?;
    // `child.stdin.end()`: the probe gets no input.
    drop(stdin);
    let mut stop = Stop::NotStarted;
    if signal.is_cancelled() {
        start_stop(&mut stop, &kill_tree);
    }
    let mut output = Vec::new();
    let mut invalid = false;
    let mut out = [0u8; READ_CHUNK];
    let mut err = [0u8; READ_CHUNK];
    let (mut stdout_open, mut stderr_open, mut exited) = (true, true, false);
    let (mut timer_armed, mut cancel_armed) = (true, true);
    let timer = tokio::time::sleep(VERSION_TIMEOUT);
    tokio::pin!(timer);
    while stdout_open || stderr_open || !exited {
        tokio::select! {
            read = stdout.read(&mut out), if stdout_open => match read {
                Ok(n) if n > 0 => {
                    if !take_bounded(&mut output, &out[..n]) {
                        invalid = true;
                        start_stop(&mut stop, &kill_tree);
                    }
                }
                _ => stdout_open = false,
            },
            read = stderr.read(&mut err), if stderr_open => match read {
                Ok(n) if n > 0 => {
                    if !take_bounded(&mut output, &err[..n]) {
                        invalid = true;
                        start_stop(&mut stop, &kill_tree);
                    }
                }
                _ => stderr_open = false,
            },
            _ = exit.wait(), if !exited => exited = true,
            _ = &mut timer, if timer_armed => {
                timer_armed = false;
                invalid = true;
                start_stop(&mut stop, &kill_tree);
            }
            _ = signal.cancelled(), if cancel_armed => {
                cancel_armed = false;
                start_stop(&mut stop, &kill_tree);
            }
        }
    }
    // Both checks run before either error is returned, so the stop is awaited on every path and nothing is left
    // running. The job check comes first, as in TypeScript.
    let job = job_retired(&exit, retirement.as_ref()).await;
    let stopped = stop.settled().await;
    // `child.once("error")` rejects before either check (project-review-cli.ts:101): a plain error, nothing retained.
    if start_failed(&exit) {
        return Err(CheckError::Failed("review_cli_unavailable".into()));
    }
    if !job {
        return Err(CheckError::Termination(VERSION_JOB.into()));
    }
    if stopped.is_err() {
        return Err(CheckError::Termination(VERSION_PROCESS.into()));
    }
    let code = exit_code(&exit);
    let text = String::from_utf8_lossy(&output);
    match VERSION_LINE.captures(text.trim()) {
        Some(version)
            if !invalid
                && !signal.is_cancelled()
                && code == Some(0)
                && patch_at_least(&version[2], MINIMUM_PATCH) =>
        {
            Ok(version[1].to_owned())
        }
        _ => Err(CheckError::Failed("review_cli_version_unsupported".into())),
    }
}

/// `child.stdin.end(input)`: writes the prompt, then closes the pipe so the CLI sees the end of its input.
async fn write_input<W>(stdin: &mut W, input: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    stdin.write_all(input).await?;
    stdin.shutdown().await
}

/// `waitForCliExit(child, Promise.resolve())`: false when the worker boundary could not confirm that the child's tree
/// has gone, so it may still run. A child that merely failed to start is [`start_failed`], not this.
/// A worker child's `retirement` is the proof (its exit alone doesn't show that the tree is gone); any other child's
/// exit is.
async fn job_retired(exit: &ChildExit, retirement: Option<&Retirement>) -> bool {
    wait_for_cli_exit(exit, retirement).await.is_ok()
}

/// A child `error` event (project-review-cli.ts:64, :101): the process never started. In TypeScript that is a plain
/// `Error`, so nothing is retained. The worker boundary reports its unconfirmed retirement with its own text instead.
fn start_failed(exit: &ChildExit) -> bool {
    matches!(exit.status(), Some(CliExit::Failed(message)) if !is_termination_message(&message))
}

fn is_termination_message(message: &str) -> bool {
    message == BoundaryError::Termination.to_string()
}

/// The exit code of a child that has exited. `None` when it was stopped by a signal or its exit is unknown.
fn exit_code(exit: &ChildExit) -> Option<i32> {
    match exit.status() {
        Some(CliExit::Exited { code, .. }) => code,
        Some(CliExit::Failed(_)) | None => None,
    }
}

/// Whether a launch failed because the worker boundary could not confirm that a tree had stopped
/// (`WorkerLaunchTerminationError`).
// TODO(M5-integration): the boundary returns `io::Error::other(BoundaryError::Termination)`, and Core's `cli.rs`
// checks for its own `WorkerTermination` type. Both carry the same message, so the message is the test here.
fn is_launch_termination(error: &io::Error) -> bool {
    is_termination_message(&error.to_string())
}

/// The `usage` record for spending that could not be measured (`costUsd: null`).
fn unknown_usage(model: &str, event_id: String) -> ProviderEvent {
    ProviderEvent::Usage(Usage {
        model: model.to_owned(),
        tokens: TokenUsage::default(),
        cost_usd: None,
        total_cost_usd: None,
        event_id: Some(event_id),
    })
}

/// `append` in `reviewCliVersion`: keeps a chunk only while the output stays within its limit.
fn take_bounded(output: &mut Vec<u8>, chunk: &[u8]) -> bool {
    if output.len() + chunk.len() > VERSION_OUTPUT_LIMIT {
        return false;
    }
    output.extend_from_slice(chunk);
    true
}

/// `Number.isSafeInteger(value) && value >= 0`.
fn safe_count(value: f64) -> bool {
    value.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&value)
}

/// `Number(digits) >= minimum`. A number too large for u64 is still above any minimum, as `Number()` reads it.
fn patch_at_least(digits: &str, minimum: u64) -> bool {
    match digits.parse::<u64>() {
        Ok(patch) => patch >= minimum,
        Err(_) => true,
    }
}

/// `raw.message.content.some(block => block.type === "tool_result")` for a user line.
fn has_tool_result(raw: &Value) -> bool {
    raw.get("message")
        .filter(|message| is_record(message))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|content| {
            content
                .iter()
                .any(|block| is_record(block) && text_field(block, "type") == Some("tool_result"))
        })
}

/// `value[key]` when it is a string.
fn text_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}
