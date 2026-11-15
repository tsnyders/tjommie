//! Provider fixtures: the daemon plays recorded conversations instead of starting Claude or Codex, so the end-to-end
//! tests can send messages, mention agents, run routines and approve cards with no account, no paid turn and no
//! process. **For tests only.** Nothing enables it by default, and a shipped daemon cannot contain it.
//!
//! # The switch
//!
//! Two things must both be on:
//!
//! 1. The Cargo feature **`provider-fixtures`**: `cargo build -p daemon --bin tjommie-daemon --features
//!    provider-fixtures`. It is refused (a `compile_error!` in lib.rs) for the production identity
//!    (`TJOMMIE_IDENTITY=production`) and for any build without debug assertions (`--release`), so it can only be built
//!    in the dev profile. The unique marker [`BUILD_MARKER`] is in a binary only when the feature is compiled in, so a
//!    release check can search the shipped daemon for it.
//! 2. The environment variable **`TJOMMIE_PROVIDER_FIXTURES=<absolute folder>`** when the daemon starts. A feature
//!    build without the variable behaves exactly like a normal build (real providers). With it, the daemon refuses to
//!    start unless the value is an absolute path of an existing folder, and prints one line to stderr:
//!    `Tjommie daemon: provider fixtures ON (<folder>): no real provider will be started.` (Fixture contents are never
//!    logged.)
//!
//! A build **without** the feature refuses to start when the variable is set at all, empty included (a build with the
//! feature refuses an empty value too, so every build answers the same way: exit code 1, and the value is never echoed):
//! ignoring it would run the real Claude and Codex in a run that meant to replay recorded conversations.
//!
//! A desktop run uses `TJOMMIE_DAEMON_EXE` to name the feature build's exe, and the variable rides in with the rest of
//! the environment (only `TJOMMIE_SKIP_*` is stripped).
//!
//! While it is on, nothing starts a provider process, structurally: the daemon's [`FixtureHost`] (a `CliHost`) refuses
//! every `launch` and counts the attempt, and no Windows worker is installed. Every provider turn is a
//! [`FixtureSession`] that plays the next scripted turn. A turn with no script **fails with a message**; it never falls
//! through to a real CLI.
//!
//! Setup looks finished without a network or a CLI: both providers read as installed, signed in as
//! `fixtures@example.com` (Codex's sandbox ready), the model lists are canned, and usage reads never touch the network
//! (Claude's usage reads as unavailable, Codex's as a low reading). Sign-in, sign-out and Codex sandbox setup are
//! refused (a client sees the handlers' own sentence, "Sign-in could not start...", and a refused launch is logged to
//! stderr). The accounts are checked before the daemon answers, so a fresh data root accepts `chat.send` at once. The
//! Codex data ACL pass is skipped (it would touch the real `~/.codex`).
//!
//! **Project reviews are refused.** A real review launches the Claude CLI for a version probe and a stream-json turn,
//! with an API-key account's key, so it would be the one way back to a real provider. In fixtures mode a review
//! answers at once that it is unavailable, `review_isolation_unavailable: Project review is not available in
//! provider-fixtures mode. A review.json replay is the documented follow-up.` ([`REVIEW_REFUSED`]), before it reads an
//! account, a credential or the `PATH`, and without asking the host for anything ([`Fixtures::review_options_hook`]).
//! Project checks (a Full-auto tester's configured command) go through the same refusing host, so they fail to start
//! and are counted. The follow-up, not built, is a `review.json` replay: the review runtime's `machine_policy_hash` seam,
//! an in-memory credential store and API-key account, and a host `launch` that answers the reviewer's `--version` and
//! its init and result lines, including the acceptance fixture's nonce reply.
//!
//! `tjommie-daemon accept-worker` (which starts the real provider CLIs to prove the worker accounts) refuses to run while
//! the variable is set, empty included, in every build. The refusal is in the library function that does the work
//! (`win::acceptance::accept_worker_in`), so any caller is covered, and `main.rs` asks first so that it prints nothing.
//! The other subcommands start no provider.
//!
//! # Where files go and what they are called
//!
//! Every file is in the folder `TJOMMIE_PROVIDER_FIXTURES` names, one file per agent (a bot):
//!
//! * `<agent name>.json`, with the name made filename-safe: every character outside `[A-Za-z0-9._ -]` becomes `_`
//!   (so `Ada Lovelace` is `Ada Lovelace.json` and `Q&A bot` is `Q_A bot.json`);
//! * else `default.json`, shared by every agent that has no file of its own (and by its cursor, see below).
//!
//! Matching ignores case (Windows). Neither file present means the turn fails with
//! `Provider fixtures: no fixture for agent "<name>" in <folder> (looked for <name>.json and default.json).`
//! Files are read when an agent first needs them (a file may appear after the daemon starts) and are then kept for the
//! life of the daemon process: to change a script, restart the daemon. A file's UTF-8 byte-order mark is ignored.
//!
//! # The file
//!
//! ```json
//! { "version": 1, "turns": [ { "expect": "hello", "delay_ms": 20, "events": [ ... ], "result": { "ok": true } } ] }
//! ```
//!
//! Unknown fields are an error everywhere. `version` is required and must be `1`.
//!
//! A **turn** is what the agent does for one user message:
//!
//! | field | | meaning |
//! |---|---|---|
//! | `expect` | optional | text the user message must contain (case-sensitive) |
//! | `delay_ms` | optional | pause before each event, 0 to 60000; default 0 (events still arrive in order) |
//! | `events` | optional | the events to play, in order |
//! | `result` | optional | how the turn ends: `{ "ok": true }` (the default) or `{ "ok": false, "error": "..." }` |
//!
//! ## Events
//!
//! Each event is an object with a `type`:
//!
//! * `session`: the provider's session id. `{ "type": "session", "id": "fixture-1" }`. The daemon saves it, as it
//!   does a real one. Put it first in a turn to choose the id; a session's first turn always starts with one (the id
//!   of the first event if that is a `session`, else the saved id the session resumes, else `fixture-session`).
//! * `text`: streamed reply text. `{ "type": "text", "delta": "Hello" }`. Send several to stream.
//! * `text_end`: the reply text is finished. `{ "type": "text_end" }`. A text left open ends with the turn.
//! * `tool`: the agent starts using a tool; it appears as a line of activity in the chat.
//!   `{ "type": "tool", "id": "t1", "name": "Reading a file", "summary": "src/lib.rs" }` (`name` is the short label,
//!   `summary` the detail).
//! * `tool_done`: that tool finished. `{ "type": "tool_done", "id": "t1", "ok": true }` (`ok: false` marks it failed).
//! * `approval`: the agent asks permission, which puts an approval card in the chat, and **waits** for the person's
//!   answer (the turn stays open, even forever, until it is answered, the turn is stopped or the agent is closed).
//!   The matching branch of `then` is then played, and the turn goes on after the approval.
//!   ```json
//!   { "type": "approval", "id": "ask-1", "tool": "Bash", "summary": "Run git status",
//!     "detail": "{ \"command\": \"git status\" }", "rule": "Bash(git status:*)",
//!     "then": { "once": [ { "type": "text", "delta": "Ran it once." } ],
//!               "always": [ { "type": "text", "delta": "Ran it, and will again." } ],
//!               "deny": [ { "type": "text", "delta": "Not running it." } ] } }
//!   ```
//!   `detail` and `rule` are optional (`rule` is what "Always allow" saves; a bot that already allows that rule has the
//!   approval answered `once` for it), and so is each branch (a missing branch plays nothing). Branches hold any
//!   events, even another approval.
//! * `usage`: what the turn cost. `{ "type": "usage", "model": "sonnet", "input": 1200, "cached_input": 800,
//!   "output": 90, "reasoning": 0, "cost_usd": 0.0123 }`. Only `model`, `input` and `output` are required; without
//!   `cost_usd` the daemon prices it from the model when it can.
//! * `limit`: the provider reports its usage limit.
//!   `{ "type": "limit", "state": "near", "used_percent": 92, "resets_at": "2030-01-01T00:00:00.000Z",
//!   "limit_id": "claude", "windows": [ { "window_key": "five_hour", "state": "near", "used_percent": 92,
//!   "duration_minutes": 300, "resets_at": "2030-01-01T00:00:00.000Z" } ] }`. `state` is `ok`, `near` or `reached`;
//!   everything else is optional.
//! * `error`: **ends the turn** with that error. `{ "type": "error", "message": "The provider is down." }`. It must be
//!   the last event of its list, and a turn that ends with one has no `result`.
//!
//! Put `usage` last, as a real provider does, before the turn ends.
//!
//! ## An example
//!
//! ```json
//! { "version": 1, "turns": [
//!   { "expect": "hello",
//!     "events": [ { "type": "text", "delta": "Hello" }, { "type": "text", "delta": " there." }, { "type": "text_end" },
//!                 { "type": "usage", "model": "sonnet", "input": 12, "output": 4 } ],
//!     "result": { "ok": true } },
//!   { "events": [ { "type": "tool", "id": "t1", "name": "Running a command", "summary": "git status" },
//!                 { "type": "tool_done", "id": "t1", "ok": true },
//!                 { "type": "text", "delta": "Clean." }, { "type": "text_end" } ] },
//!   { "events": [ { "type": "error", "message": "The provider is down." } ] } ] }
//! ```
//!
//! `tests/fixtures/provider/` has working files, `default.json` and one named after a bot.
//!
//! # How turns are matched and used up
//!
//! * Turns are played **in order, one per user message**, from a cursor kept per file for the life of the daemon
//!   process. The runner may close an idle session and make a new one; the cursor carries on. Every agent that uses
//!   `default.json` shares its cursor.
//! * A message sent while a turn is playing is not steered into it: it runs as the next turn.
//! * `expect` not found in the message (the whole text the agent was sent) fails the turn:
//!   `Provider fixtures: turn <n> of <file> expects the message to contain "<expect>", but the message was "<text>".`
//!   Never more than 200 characters of the message are shown (its end). The turn is used up, so the next message plays
//!   the next turn.
//! * No turns left fails the turn: `Provider fixtures: <file> has <N> turns; turn <N+1> has no fixture.`
//! * A file that is not valid JSON, has an unknown field, the wrong `version` or breaks a rule above fails the turn
//!   with its path and the reason: `Provider fixtures: <path> is not a valid fixture file: <error>`. It is read again
//!   on the next message. Nothing panics.
//! * A failed turn ends like any provider failure: the chat shows the message as an error. It never starts a provider.
//! * Stopping a turn that is playing ends it as interrupted.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use protocol::model::Provider;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::account_quota::{UsageFetch, UsageResponse};
use crate::bots::StoredBot;
use crate::cli::{
    BoxFuture, ChildExit, CliChild, CliCommand, CliExit, CliHost, CliOutput, Env, KillTree,
    LaunchOptions,
};
use crate::m5::ReviewOptionsHook;
use crate::project_review::ReviewContext;
use crate::project_review_runtime::{
    ProjectReviewOptions, ReviewAccountResolver, ReviewAccountSelection,
};
use crate::providers::codex_rpc::{CodexServer, JsonRpcLines};
use crate::providers::{
    Approval, ApprovalAnswer, Limit, LimitState, LimitWindow, ProviderError, ProviderEvent,
    ProviderSession, StartOptions, Subscribers, TokenUsage, TurnResult, Usage,
};
use crate::runner::SessionFactory;

/// The environment variable that turns fixtures on (when the feature is compiled in).
pub const ENV_VAR: &str = "TJOMMIE_PROVIDER_FIXTURES";

/// In a binary only when the `provider-fixtures` feature is compiled in. The release workflow checks the shipped daemon
/// for it. Nothing else may spell it out outside this feature.
pub const BUILD_MARKER: &str = "TJOMMIE-PROVIDER-FIXTURES-BUILD-7f3a9c41";

/// Keeps the marker in the object code even where nothing reads it.
#[used]
static KEEP_BUILD_MARKER: &str = BUILD_MARKER;

/// Why a project review fails while fixtures are on (it is the reason's text after `review_isolation_unavailable: `).
pub const REVIEW_REFUSED: &str = "Project review is not available in provider-fixtures mode. A review.json replay is the documented follow-up.";

const DEFAULT_FILE: &str = "default.json";
const FILE_VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 4 << 20;
const MAX_DELAY_MS: u64 = 60_000;
/// How much of the user's message an `expect` failure may show.
const ECHO_CHARS: usize = 200;
const INTERRUPTED: &str = "Provider fixtures: the turn was stopped.";
const REFUSED: &str = "this daemon is running provider fixtures and starts no provider";
const NO_NETWORK: &str = "Provider fixtures: the network is not used.";

/// What setup and the account checks see in place of a provider CLI.
const FIXTURE_EMAIL: &str = "fixtures@example.com";
const CLAUDE_VERSION: &str = "2.1.999 (Claude Code)\n";
/// The version the Codex ACL recipe was measured on (account_handlers.rs), so the daemon logs no drift notice.
const CODEX_VERSION: &str = "codex-cli 0.155.1\n";
/// 2100-01-01: a reading that never runs out.
const FAR_FUTURE_SECONDS: u64 = 4_102_444_800;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------------------------------------------
// The fixture file

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFile {
    version: u32,
    turns: Vec<Turn>,
}

/// One scripted turn.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    #[serde(default)]
    expect: Option<String>,
    #[serde(default)]
    delay_ms: Option<u64>,
    #[serde(default)]
    events: Vec<Event>,
    #[serde(default)]
    result: Option<Outcome>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LimitName {
    Ok,
    Near,
    Reached,
}

impl From<LimitName> for LimitState {
    fn from(name: LimitName) -> Self {
        match name {
            LimitName::Ok => LimitState::Ok,
            LimitName::Near => LimitState::Near,
            LimitName::Reached => LimitState::Reached,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowEvent {
    window_key: String,
    state: LimitName,
    #[serde(default)]
    duration_minutes: Option<f64>,
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Branches {
    #[serde(default)]
    once: Vec<Event>,
    #[serde(default)]
    always: Vec<Event>,
    #[serde(default)]
    deny: Vec<Event>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Session {
        id: String,
    },
    Text {
        delta: String,
    },
    TextEnd,
    Tool {
        id: String,
        name: String,
        summary: String,
    },
    ToolDone {
        id: String,
        ok: bool,
    },
    Approval {
        id: String,
        tool: String,
        summary: String,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        rule: Option<String>,
        #[serde(default)]
        then: Branches,
    },
    Usage {
        model: String,
        input: u64,
        #[serde(default)]
        cached_input: u64,
        output: u64,
        #[serde(default)]
        reasoning: u64,
        #[serde(default)]
        cost_usd: Option<f64>,
    },
    Limit {
        state: LimitName,
        #[serde(default)]
        used_percent: Option<f64>,
        #[serde(default)]
        resets_at: Option<String>,
        #[serde(default)]
        limit_id: Option<String>,
        #[serde(default)]
        windows: Option<Vec<WindowEvent>>,
    },
    Error {
        message: String,
    },
}

impl Event {
    /// The event the runner sees, for every kind that is just an emission.
    fn emitted(self) -> Option<ProviderEvent> {
        Some(match self {
            Event::Session { id } => ProviderEvent::Session { cli_session_id: id },
            Event::Text { delta } => ProviderEvent::Text { delta },
            Event::TextEnd => ProviderEvent::TextEnd,
            Event::Tool { id, name, summary } => ProviderEvent::Tool { id, name, summary },
            Event::ToolDone { id, ok } => ProviderEvent::ToolDone { id, ok },
            Event::Usage {
                model,
                input,
                cached_input,
                output,
                reasoning,
                cost_usd,
            } => ProviderEvent::Usage(Usage {
                model,
                tokens: TokenUsage {
                    input: input as f64,
                    cached_input: cached_input as f64,
                    output: output as f64,
                    reasoning: reasoning as f64,
                },
                cost_usd,
                total_cost_usd: None,
                event_id: None,
            }),
            Event::Limit {
                state,
                used_percent,
                resets_at,
                limit_id,
                windows,
            } => ProviderEvent::Limit(Limit {
                state: state.into(),
                used_percent,
                resets_at,
                limit_id,
                windows: windows.map(|windows| {
                    windows
                        .into_iter()
                        .map(|window| LimitWindow {
                            window_key: window.window_key,
                            duration_minutes: window.duration_minutes,
                            used_percent: window.used_percent,
                            resets_at: window.resets_at,
                            state: window.state.into(),
                        })
                        .collect()
                }),
            }),
            Event::Approval { .. } | Event::Error { .. } => return None,
        })
    }
}

/// The rules a file must keep beyond its shape.
fn validate(turns: &[Turn]) -> Result<(), String> {
    for (index, turn) in turns.iter().enumerate() {
        let problem = |text: &str| format!("turn {}: {text}", index + 1);
        if turn.delay_ms.is_some_and(|delay| delay > MAX_DELAY_MS) {
            return Err(problem(&format!(
                "delay_ms must be at most {MAX_DELAY_MS}."
            )));
        }
        if let Some(result) = &turn.result
            && result.ok
            && result.error.is_some()
        {
            return Err(problem("result has an error but ok is true."));
        }
        validate_events(&turn.events).map_err(|text| problem(&text))?;
        if turn.result.is_some() && matches!(turn.events.last(), Some(Event::Error { .. })) {
            return Err(problem(
                "an error event already ends the turn, so it has no result.",
            ));
        }
    }
    Ok(())
}

fn validate_events(events: &[Event]) -> Result<(), String> {
    for (index, event) in events.iter().enumerate() {
        match event {
            Event::Error { .. } if index + 1 != events.len() => {
                return Err("an error event must be the last event of its list.".into());
            }
            Event::Approval { then, .. } => {
                for branch in [&then.once, &then.always, &then.deny] {
                    validate_events(branch)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// The files, and the cursor through each

/// One fixture file with its cursor.
struct Script {
    file: String,
    turns: Vec<Arc<Turn>>,
    next: Mutex<usize>,
}

impl Script {
    /// `None`: there is no such file (a folder of that name is no file either).
    fn load(path: &Path, file: &str) -> Result<Option<Script>, String> {
        let unreadable = |error: &dyn std::fmt::Display| {
            format!(
                "Provider fixtures: could not read {}: {error}",
                path.display()
            )
        };
        let meta = match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(&error)),
        };
        if meta.len() > MAX_FILE_BYTES {
            return Err(format!(
                "Provider fixtures: {} is larger than {} MiB.",
                path.display(),
                MAX_FILE_BYTES >> 20
            ));
        }
        let text = std::fs::read_to_string(path).map_err(|error| unreadable(&error))?;
        let invalid = |reason: &str| {
            format!(
                "Provider fixtures: {} is not a valid fixture file: {reason}",
                path.display()
            )
        };
        let parsed: FixtureFile =
            serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text))
                .map_err(|error| invalid(&error.to_string()))?;
        if parsed.version != FILE_VERSION {
            return Err(invalid(&format!(
                "its version is {}; this daemon reads version {FILE_VERSION}.",
                parsed.version
            )));
        }
        validate(&parsed.turns).map_err(|reason| invalid(&reason))?;
        Ok(Some(Script {
            file: file.to_owned(),
            turns: parsed.turns.into_iter().map(Arc::new).collect(),
            next: Mutex::new(0),
        }))
    }
}

/// The name of an agent's file without its extension: characters outside `[A-Za-z0-9._ -]` become `_`, and a name
/// Windows reserves for a device gets a leading `_`, so reading it can never open the device.
pub fn file_stem(agent: &str) -> String {
    let name: String = agent
        .chars()
        .take(100)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ' ' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let base = name.split('.').next().unwrap_or_default().trim_end();
    let reserved = matches!(
        base.to_ascii_uppercase().as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
    ) || (base.len() == 4
        && (base[..3].eq_ignore_ascii_case("COM") || base[..3].eq_ignore_ascii_case("LPT"))
        && base.as_bytes()[3].is_ascii_digit()
        && base.as_bytes()[3] != b'0');
    if reserved { format!("_{name}") } else { name }
}

/// The last `ECHO_CHARS` characters of a message.
fn tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= ECHO_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().skip(count - ECHO_CHARS).collect();
    format!("...{kept}")
}

struct Registry {
    dir: PathBuf,
    host: Arc<FixtureHost>,
    /// By lower-case file name. Only files that loaded are kept, so a broken or missing one is read again.
    scripts: Mutex<HashMap<String, Arc<Script>>>,
}

/// The fixtures a daemon plays: the folder, the file cursors and the no-launch host. Cloning shares them.
#[derive(Clone)]
pub struct Fixtures(Arc<Registry>);

impl Fixtures {
    /// The folder `dir` must be absolute and exist. Its files are read later, when an agent needs them.
    pub fn open(dir: &Path) -> Result<Fixtures, String> {
        // Read here so that any binary that can turn fixtures on carries the marker, not only one that is lucky.
        std::hint::black_box(KEEP_BUILD_MARKER);
        if dir.as_os_str().is_empty() {
            return Err(format!(
                "{ENV_VAR} is set but empty. Give the absolute path of a folder of fixture files, or leave it unset to run the real providers."
            ));
        }
        if !dir.is_absolute() {
            return Err(format!(
                "{ENV_VAR} must be an absolute path to a folder of fixture files, but {} is not absolute.",
                dir.display()
            ));
        }
        if !dir.is_dir() {
            return Err(format!(
                "{ENV_VAR} must name an existing folder of fixture files, but {} is not one.",
                dir.display()
            ));
        }
        Ok(Fixtures(Arc::new(Registry {
            dir: dir.to_path_buf(),
            host: Arc::new(FixtureHost::default()),
            scripts: Mutex::default(),
        })))
    }

    /// `None` when the variable is unset: the real providers run. Anything else must be a good folder.
    pub fn from_env(get: impl Fn(&str) -> Option<OsString>) -> Result<Option<Fixtures>, String> {
        match get(ENV_VAR) {
            None => Ok(None),
            Some(value) => Fixtures::open(Path::new(&value)).map(Some),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.0.dir
    }

    /// The host the whole daemon uses in place of the real one: it never starts a process.
    pub fn host(&self) -> Arc<FixtureHost> {
        self.0.host.clone()
    }

    pub fn session_factory(&self) -> SessionFactory {
        fixture_session_factory(self.clone())
    }

    pub fn usage_fetch(&self) -> Arc<dyn UsageFetch> {
        Arc::new(FixtureFetch)
    }

    /// How a fixtures daemon wires project reviews, which are refused. A real review launches the Claude CLI with an
    /// API-key account's key, so every way to get there is closed, each on its own:
    ///
    /// * the account resolver is replaced by one that answers "unavailable" with [`REVIEW_REFUSED`], the first thing a
    ///   review asks, so no account, credential, CLI lookup or probe is touched and nothing is asked of the host;
    /// * the host is this daemon's refusing [`FixtureHost`], and there is no review worker to launch through;
    /// * the reviewer's CLI is the placeholder, which is no path, instead of one looked up on `PATH`.
    pub fn review_options_hook(&self) -> ReviewOptionsHook {
        let host = self.host();
        Arc::new(move |options| {
            let placeholder = host.clone();
            ProjectReviewOptions {
                resolve: Arc::new(RefusedReviews),
                host: host.clone(),
                worker: None,
                cli: Some(Arc::new(move || placeholder.find_cli(Provider::Claude))),
                ..options
            }
        })
    }

    /// The next turn of the agent's file, used up. The messages of every failure are the turn's error.
    pub fn next_turn(&self, agent: &str, message: &str) -> Result<Arc<Turn>, String> {
        let script = self.script_for(agent)?;
        let (index, turn) = {
            let mut next = lock(&script.next);
            let index = *next;
            let turn = script.turns.get(index).cloned();
            if turn.is_some() {
                *next += 1;
            }
            (index, turn)
        };
        let Some(turn) = turn else {
            return Err(format!(
                "Provider fixtures: {} has {} turns; turn {} has no fixture.",
                script.file,
                script.turns.len(),
                script.turns.len() + 1
            ));
        };
        if let Some(expect) = &turn.expect
            && !message.contains(expect.as_str())
        {
            return Err(format!(
                "Provider fixtures: turn {} of {} expects the message to contain \"{}\", but the message was \"{}\".",
                index + 1,
                script.file,
                tail(expect),
                tail(message)
            ));
        }
        Ok(turn)
    }

    fn script_for(&self, agent: &str) -> Result<Arc<Script>, String> {
        let own = format!("{}.json", file_stem(agent));
        let files = if own.eq_ignore_ascii_case(DEFAULT_FILE) {
            vec![DEFAULT_FILE.to_owned()]
        } else {
            vec![own.clone(), DEFAULT_FILE.to_owned()]
        };
        let mut scripts = lock(&self.0.scripts);
        for file in &files {
            let key = file.to_ascii_lowercase();
            if let Some(script) = scripts.get(&key) {
                return Ok(script.clone());
            }
            if let Some(script) = Script::load(&self.0.dir.join(file), file)? {
                let script = Arc::new(script);
                scripts.insert(key, script.clone());
                return Ok(script);
            }
        }
        Err(format!(
            "Provider fixtures: no fixture for agent \"{agent}\" in {} (looked for {own} and {DEFAULT_FILE}).",
            self.0.dir.display()
        ))
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The session

/// A [`SessionFactory`] whose sessions play fixtures. It holds no CLI host: a session can't start a process.
pub fn fixture_session_factory(fixtures: Fixtures) -> SessionFactory {
    Arc::new(move |bot: &StoredBot, options: StartOptions| {
        Ok(Arc::new(FixtureSession::new(
            fixtures.clone(),
            bot.name.clone(),
            options.resume_id,
        )) as Arc<dyn ProviderSession>)
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Lifecycle {
    Idle,
    Running,
}

/// A turn on its way to being played: its script, or why it has none.
struct Planned {
    turn: Result<Arc<Turn>, String>,
    cancel: CancellationToken,
}

struct State {
    started: bool,
    announced: bool,
    /// Where turns go to the player; `None` once closed.
    queue: Option<mpsc::UnboundedSender<Planned>>,
    /// Turns sent and not finished, oldest first. Stop cancels the oldest.
    active: VecDeque<CancellationToken>,
    /// Approvals asked and not answered, by the fixture's id.
    pending: HashMap<String, oneshot::Sender<ApprovalAnswer>>,
}

struct Inner {
    fixtures: Fixtures,
    agent: String,
    resume_id: Option<String>,
    subscribers: Subscribers,
    state: Mutex<State>,
    closed: AtomicBool,
    lifecycle: watch::Sender<Lifecycle>,
}

/// One agent's session, which plays the next scripted turn for each message, as a Claude session would answer it.
#[derive(Clone)]
pub struct FixtureSession(Arc<Inner>);

impl FixtureSession {
    pub fn new(fixtures: Fixtures, agent: String, resume_id: Option<String>) -> Self {
        FixtureSession(Arc::new(Inner {
            fixtures,
            agent,
            resume_id,
            subscribers: Subscribers::default(),
            state: Mutex::new(State {
                started: false,
                announced: false,
                queue: None,
                active: VecDeque::new(),
                pending: HashMap::new(),
            }),
            closed: AtomicBool::new(false),
            lifecycle: watch::channel(Lifecycle::Idle).0,
        }))
    }
}

fn done() -> BoxFuture<'static, Result<(), ProviderError>> {
    Box::pin(async { Ok(()) })
}

impl ProviderSession for FixtureSession {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut state = lock(&self.0.state);
        if !state.started && !self.0.closed.load(Ordering::SeqCst) {
            state.started = true;
            let (queue, turns) = mpsc::unbounded_channel();
            state.queue = Some(queue);
            tokio::spawn(play_turns(Arc::downgrade(&self.0), turns));
            self.0.lifecycle.send_replace(Lifecycle::Running);
        }
        done()
    }

    /// Takes the next turn of the agent's file now, and plays it. A session that was never started is an error (the
    /// message is not dropped silently, which would leave the caller waiting for a turn that never ends, and it uses up no
    /// turn). Like a session whose process has gone, a session that was closed does nothing.
    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut state = lock(&self.0.state);
        if !state.started {
            return Box::pin(async {
                Err(ProviderError::Failed(
                    "Provider fixtures: the session was not started, so the message was not sent."
                        .into(),
                ))
            });
        }
        if let Some(queue) = &state.queue {
            let planned = Planned {
                turn: self.0.fixtures.next_turn(&self.0.agent, text),
                cancel: CancellationToken::new(),
            };
            let cancel = planned.cancel.clone();
            if queue.send(planned).is_ok() {
                state.active.push_back(cancel);
            }
        }
        done()
    }

    /// Always `Ok(false)`: a message sent mid-turn runs as the next turn, so the script stays one turn per message.
    fn steer(&self, _text: &str) -> BoxFuture<'static, Result<bool, ProviderError>> {
        Box::pin(async { Ok(false) })
    }

    /// Always `Ok(false)`, as Claude's: the turn is ended here, with a failed result, like Claude's result line.
    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>> {
        if let Some(current) = lock(&self.0.state).active.front() {
            current.cancel();
        }
        Box::pin(async { Ok(false) })
    }

    fn answer(&self, approval_id: &str, answer: ApprovalAnswer) {
        if self.0.closed.load(Ordering::SeqCst) {
            return;
        }
        if let Some(asked) = lock(&self.0.state).pending.remove(approval_id) {
            let _ = asked.send(answer);
        }
    }

    fn close(&self) {
        if self.0.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let started = {
            let mut state = lock(&self.0.state);
            state.queue = None;
            state.active.drain(..).for_each(|turn| turn.cancel());
            state.pending.clear();
            state.started
        };
        if started {
            self.0.lifecycle.send_replace(Lifecycle::Idle);
            self.0.subscribers.emit(ProviderEvent::Exit {
                error: None,
                completed_turn: None,
            });
        }
    }

    fn wait_for_exit(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut lifecycle = self.0.lifecycle.subscribe();
        Box::pin(async move {
            let _ = lifecycle
                .wait_for(|state| *state != Lifecycle::Running)
                .await;
            Ok(())
        })
    }

    fn pid(&self) -> Option<u32> {
        None
    }

    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        self.0.subscribers.subscribe()
    }
}

/// Plays each sent turn to its end, in order. Ends when the session is closed or dropped.
async fn play_turns(session: Weak<Inner>, mut turns: mpsc::UnboundedReceiver<Planned>) {
    while let Some(planned) = turns.recv().await {
        let Some(session) = session.upgrade() else {
            return;
        };
        session.play(planned).await;
    }
}

impl Inner {
    fn emit(&self, event: ProviderEvent) {
        if !self.closed.load(Ordering::SeqCst) {
            self.subscribers.emit(event);
        }
    }

    async fn play(&self, planned: Planned) {
        let Planned { turn, cancel } = planned;
        let result = match turn {
            Err(message) => TurnResult {
                ok: false,
                error: Some(message),
            },
            Ok(turn) => tokio::select! {
                biased;
                () = cancel.cancelled() => TurnResult { ok: false, error: Some(INTERRUPTED.into()) },
                result = self.run(&turn) => result,
            },
        };
        {
            let mut state = lock(&self.state);
            state.active.pop_front();
            state.pending.clear();
        }
        self.emit(ProviderEvent::TurnDone(result));
    }

    /// The session event a session starts with, and the first turn's own if it begins with one.
    fn announce(&self, events: &mut VecDeque<Event>) {
        let first = match events.front() {
            Some(Event::Session { .. }) => match events.pop_front() {
                Some(Event::Session { id }) => Some(id),
                _ => None,
            },
            _ => None,
        };
        let announced = std::mem::replace(&mut lock(&self.state).announced, true);
        let id = match (announced, first) {
            (false, first) => Some(
                first
                    .or_else(|| self.resume_id.clone())
                    .unwrap_or_else(|| "fixture-session".to_owned()),
            ),
            (true, first) => first,
        };
        if let Some(id) = id {
            self.emit(ProviderEvent::Session { cli_session_id: id });
        }
    }

    async fn run(&self, turn: &Turn) -> TurnResult {
        let pause = Duration::from_millis(turn.delay_ms.unwrap_or(0));
        let mut events: VecDeque<Event> = turn.events.iter().cloned().collect();
        self.announce(&mut events);
        while let Some(event) = events.pop_front() {
            if pause.is_zero() {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(pause).await;
            }
            match event {
                Event::Error { message } => {
                    return TurnResult {
                        ok: false,
                        error: Some(message),
                    };
                }
                Event::Approval {
                    id,
                    tool,
                    summary,
                    detail,
                    rule,
                    then,
                } => {
                    let (answer, answered) = oneshot::channel();
                    lock(&self.state).pending.insert(id.clone(), answer);
                    self.emit(ProviderEvent::Approval(Approval {
                        id,
                        tool,
                        summary,
                        detail,
                        rule,
                    }));
                    let Ok(answer) = answered.await else {
                        return TurnResult {
                            ok: false,
                            error: Some(INTERRUPTED.into()),
                        };
                    };
                    let branch = match answer {
                        ApprovalAnswer::Once => then.once,
                        ApprovalAnswer::Always => then.always,
                        ApprovalAnswer::Deny => then.deny,
                    };
                    branch
                        .into_iter()
                        .rev()
                        .for_each(|event| events.push_front(event));
                }
                other => {
                    if let Some(event) = other.emitted() {
                        self.emit(event);
                    }
                }
            }
        }
        match &turn.result {
            Some(result) => TurnResult {
                ok: result.ok,
                error: result.error.clone(),
            },
            None => TurnResult {
                ok: true,
                error: None,
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The host that starts nothing

/// The daemon's `CliHost` while fixtures are on. It never starts a process: [`CliHost::launch`] always fails, and counts
/// the attempt. Providers look installed, and the CLI checks setup makes (`--version`, `auth status`) are answered from
/// memory, so accounts read as installed and signed in.
#[derive(Default)]
pub struct FixtureHost {
    launch_attempts: AtomicUsize,
}

impl FixtureHost {
    /// How often something asked this host to start a process. Every ask was refused; a normal fixtures run has none.
    pub fn launch_attempts(&self) -> usize {
        self.launch_attempts.load(Ordering::SeqCst)
    }
}

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

impl CliHost for FixtureHost {
    fn launch<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        self.launch_attempts.fetch_add(1, Ordering::SeqCst);
        // Callers may turn the error into something generic, so the log says what happened.
        eprintln!("Provider fixtures: a provider CLI launch was refused.");
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("Provider fixtures: {REFUSED}, so a provider CLI cannot be launched."),
            ))
        })
    }

    /// An in-memory Codex app server that answers what setup asks, and refuses sign-in and sandbox setup.
    fn start_codex_server<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        _env: &'a Env,
        _cwd: &'a str,
        _ephemeral_auth: bool,
    ) -> BoxFuture<'a, Result<CodexServer, ProviderError>> {
        Box::pin(async { Ok(codex_server()) })
    }

    /// A placeholder that is no path, so the providers look installed and nothing could start it.
    fn find_cli(&self, name: Provider) -> Option<CliCommand> {
        Some(CliCommand {
            file: format!("<provider fixtures>/{}", provider_name(name)),
            args: Vec::new(),
            codex_package: None,
        })
    }

    fn run_cli<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: &'a [String],
        _env: &'a Env,
        _timeout: Duration,
    ) -> BoxFuture<'a, io::Result<CliOutput>> {
        let codex = cmd.file.ends_with("/codex");
        let output = |stdout: &str| CliOutput {
            code: Some(0),
            stdout: stdout.to_owned(),
            stderr: String::new(),
        };
        let answer = match (
            codex,
            args.iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice(),
        ) {
            (false, ["--version"]) => output(CLAUDE_VERSION),
            (false, ["auth", "status"]) => output(
                &json!({
                    "loggedIn": true,
                    "authMethod": "claude.ai",
                    "email": FIXTURE_EMAIL,
                    "subscriptionType": "max",
                })
                .to_string(),
            ),
            (true, ["--version"]) => output(CODEX_VERSION),
            _ => CliOutput {
                code: Some(1),
                stdout: String::new(),
                stderr: format!("Provider fixtures: {REFUSED}, so this command did not run."),
            },
        };
        Box::pin(async move { Ok(answer) })
    }
}

/// The reviewer account resolver of a fixtures daemon: no reviewer is ever chosen.
struct RefusedReviews;

impl ReviewAccountResolver for RefusedReviews {
    fn resolve<'a>(
        &'a self,
        _context: &'a ReviewContext,
        _signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<ReviewAccountSelection, String>> {
        Box::pin(async {
            Ok(ReviewAccountSelection::Unavailable(
                REVIEW_REFUSED.to_owned(),
            ))
        })
    }
}

/// Usage reads never reach the network.
pub struct FixtureFetch;

impl UsageFetch for FixtureFetch {
    fn get<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(&'static str, String)],
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<UsageResponse, String>> {
        Box::pin(async { Err(NO_NETWORK.to_owned()) })
    }
}

/// What Codex's app server answers setup, as JSON, or the text it refuses with.
fn codex_answer(method: &str) -> Result<Value, String> {
    match method {
        "account/read" => Ok(json!({
            "account": { "type": "chatgpt", "email": FIXTURE_EMAIL, "planType": "plus" }
        })),
        "config/read" => Ok(json!({ "config": { "windows": { "sandbox": "elevated" } } })),
        "windowsSandbox/readiness" => Ok(json!({ "status": "ready" })),
        "model/list" => Ok(json!({ "data": [
            { "model": "gpt-5.4", "displayName": "GPT-5.4", "isDefault": true, "hidden": false },
            { "model": "gpt-5.3-codex", "displayName": "GPT-5.3 Codex", "isDefault": false, "hidden": false },
        ] })),
        "account/rateLimits/read" => Ok(json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": {
                "limitId": "codex",
                "planType": "plus",
                "spendControlReached": false,
                "primary": { "usedPercent": 3, "windowDurationMins": 300, "resetsAt": FAR_FUTURE_SECONDS },
                "secondary": { "usedPercent": 1, "windowDurationMins": 10080, "resetsAt": FAR_FUTURE_SECONDS },
            }
        })),
        "account/login/cancel" => Ok(json!({})),
        "account/login/start" | "windowsSandbox/setupStart" => Err(format!(
            "Provider fixtures: {REFUSED}, so signing in and sandbox setup are refused."
        )),
        other => Err(format!(
            "Provider fixtures: {other} is not available in fixtures mode."
        )),
    }
}

/// A [`CodexServer`] with nobody on the other end: each request is answered at once, from [`codex_answer`].
fn codex_server() -> CodexServer {
    let slot: Arc<OnceLock<Weak<JsonRpcLines>>> = Arc::default();
    let rpc = Arc::new(JsonRpcLines::new({
        let slot = slot.clone();
        move |line| {
            let Ok(request) = serde_json::from_str::<Value>(&line) else {
                return Ok(());
            };
            // Notifications (`initialized`) have no id and get no answer.
            let (Some(id), Some(method)) = (
                request.get("id"),
                request.get("method").and_then(Value::as_str),
            ) else {
                return Ok(());
            };
            let reply = match codex_answer(method) {
                Ok(result) => json!({ "id": id, "result": result }),
                Err(message) => {
                    json!({ "id": id, "error": { "code": -32000, "message": message } })
                }
            };
            if let Some(rpc) = slot.get().and_then(Weak::upgrade) {
                rpc.handle_line(&reply.to_string());
            }
            Ok(())
        }
    }));
    let _ = slot.set(Arc::downgrade(&rpc));
    let exit = ChildExit::new();
    let kill_tree: KillTree = Arc::new(|| Box::pin(async { Ok(()) }));
    let close: Arc<dyn Fn() + Send + Sync> = {
        let exit = exit.clone();
        Arc::new(move || {
            exit.fire(CliExit::Exited {
                code: Some(0),
                signal: None,
            });
        })
    };
    CodexServer {
        rpc,
        pid: None,
        exit,
        kill_tree,
        close,
        retirement: None,
    }
}
