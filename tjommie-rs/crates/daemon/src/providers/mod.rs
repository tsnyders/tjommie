//! The CLI adapters: Claude (`claude -p` over stream-json) and Codex (`codex app-server` over JSON-RPC). Port of
//! `packages/daemon/src/providers/types.ts`; each submodule ports the TypeScript file of the same name.
//!
//! JSON from a CLI is decoded into these types at the edge. Values TypeScript keeps as `unknown` or
//! `Record<string, unknown>` (tool inputs, permission suggestions, RPC params) stay `serde_json` values: they are passed
//! back to the CLI unread.

pub mod activity_labels;
pub mod claude;
pub mod claude_files;
pub mod claude_parse;
pub mod codex;
pub mod codex_parse;
pub mod codex_rpc;
pub(crate) mod js;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use protocol::model::Autonomy;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

use crate::cli::BoxFuture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAnswer {
    Once,
    Always,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitState {
    Ok,
    Near,
    Reached,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LimitWindow {
    pub window_key: String,
    pub duration_minutes: Option<f64>,
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub state: LimitState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub state: LimitState,
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub limit_id: Option<String>,
    pub windows: Option<Vec<LimitWindow>>,
}

/// Token counts are JavaScript numbers in TypeScript, so they stay `f64` here.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TokenUsage {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
    pub reasoning: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub model: String,
    pub tokens: TokenUsage,
    pub cost_usd: Option<f64>,
    pub total_cost_usd: Option<f64>,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub id: String,
    pub tool: String,
    pub summary: String,
    pub detail: String,
    pub rule: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnResult {
    pub ok: bool,
    pub error: Option<String>,
}

/// What every CLI adapter reports, whatever its CLI prints.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    Session {
        cli_session_id: String,
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
    Approval(Approval),
    Usage(Usage),
    Limit(Limit),
    TurnDone(TurnResult),
    Exit {
        error: Option<String>,
        completed_turn: Option<TurnResult>,
    },
}

/// Provider errors and echoed approval details must never put the explicit API key on the wire.
pub fn redact_provider_event(mut event: ProviderEvent, key: Option<&str>) -> ProviderEvent {
    let Some(key) = key.filter(|key| !key.is_empty()) else {
        return event;
    };
    let redact = |text: &mut String| {
        if text.contains(key) {
            *text = text.replace(key, "[redacted]");
        }
    };
    let redact_option = |text: &mut Option<String>| text.iter_mut().for_each(redact);
    match &mut event {
        ProviderEvent::Session { cli_session_id } => redact(cli_session_id),
        ProviderEvent::Text { delta } => redact(delta),
        ProviderEvent::TextEnd => {}
        ProviderEvent::Tool { id, name, summary } => {
            [id, name, summary].into_iter().for_each(redact)
        }
        ProviderEvent::ToolDone { id, .. } => redact(id),
        ProviderEvent::Approval(approval) => {
            [
                &mut approval.id,
                &mut approval.tool,
                &mut approval.summary,
                &mut approval.detail,
            ]
            .into_iter()
            .for_each(redact);
            redact_option(&mut approval.rule);
        }
        ProviderEvent::Usage(usage) => {
            redact(&mut usage.model);
            redact_option(&mut usage.event_id);
        }
        ProviderEvent::Limit(limit) => {
            redact_option(&mut limit.resets_at);
            redact_option(&mut limit.limit_id);
            for window in limit.windows.iter_mut().flatten() {
                redact(&mut window.window_key);
                redact_option(&mut window.resets_at);
            }
        }
        ProviderEvent::TurnDone(turn) => redact_option(&mut turn.error),
        ProviderEvent::Exit {
            error,
            completed_turn,
        } => {
            redact_option(error);
            completed_turn
                .iter_mut()
                .for_each(|turn| redact_option(&mut turn.error));
        }
    }
    event
}

/// An API key for the CLI. Its `Debug` output never shows the key.
#[derive(Clone, PartialEq)]
pub struct ApiKey(pub String);

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

/// An MCP server attached to one CLI session: Tjommie's own over HTTP, or a program the CLI starts (the browser).
/// `auto_allow`: its tools run without asking the person.
#[derive(Debug, Clone, PartialEq)]
pub enum McpServer {
    Http {
        name: String,
        url: String,
        headers: BTreeMap<String, String>,
        auto_allow: bool,
    },
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        auto_allow: bool,
    },
}

impl McpServer {
    pub fn name(&self) -> &str {
        match self {
            McpServer::Http { name, .. } | McpServer::Stdio { name, .. } => name,
        }
    }

    pub fn auto_allow(&self) -> bool {
        match self {
            McpServer::Http { auto_allow, .. } | McpServer::Stdio { auto_allow, .. } => *auto_allow,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub cwd: String,
    pub extra_dirs: Vec<String>,
    pub model: String,
    pub instructions: String,
    pub autonomy: Autonomy,
    /// Always ask uses elevated only after the account setup is ready.
    pub codex_sandbox_ready: bool,
    pub allow_rules: Vec<String>,
    pub resume_id: Option<String>,
    /// The last reported cumulative cost for this saved CLI session.
    pub resume_cost_usd: Option<f64>,
    pub env: BTreeMap<String, String>,
    /// `authentication: { kind: "api_key", key }` in TypeScript. Private daemon -> owned CLI only; never serialize or
    /// broadcast.
    pub api_key: Option<ApiKey>,
    /// An additional provider estimate threshold, not an external billing guarantee.
    pub max_budget_usd: Option<f64>,
    pub mcp_servers: Vec<McpServer>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderError {
    /// An error response from the CLI (`RpcRemoteError` in TypeScript).
    Remote { code: i64, message: String },
    /// Anything else: a transport failure, a timeout or the session's state.
    Failed(String),
}

impl ProviderError {
    /// A worker launch or exit whose process tree may still run (`WorkerLaunchTerminationError`).
    pub fn is_worker_termination(&self) -> bool {
        self.message() == crate::cli::WorkerTermination.to_string()
    }

    pub fn message(&self) -> &str {
        match self {
            ProviderError::Remote { message, .. } | ProviderError::Failed(message) => message,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ProviderError {}

impl From<std::io::Error> for ProviderError {
    fn from(error: std::io::Error) -> Self {
        ProviderError::Failed(error.to_string())
    }
}

/// One running CLI conversation. Claude and Codex each have an adapter that implements this.
///
/// Like the TypeScript async methods, each call does its synchronous part (state changes, writes to the CLI) before
/// it returns; the future only waits for the rest.
pub trait ProviderSession: Send + Sync {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>>;
    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>>;
    /// `Ok(false)`: couldn't add it to the current turn, so the caller queues it.
    fn steer(&self, text: &str) -> BoxFuture<'static, Result<bool, ProviderError>>;
    /// `Ok(true)`: the provider handled this Stop, including a pending send.
    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>>;
    fn answer(&self, approval_id: &str, answer: ApprovalAnswer);
    fn close(&self);
    /// Resolves after the underlying process has exited, including a pending start. An error means a worker could not
    /// confirm that its whole process tree has gone: the account stays reserved.
    fn wait_for_exit(&self) -> BoxFuture<'static, Result<(), ProviderError>>;
    fn pid(&self) -> Option<u32>;
    /// Every event from now on (`onEvent` in TypeScript).
    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent>;
}

/// The listeners of one session.
#[derive(Default)]
struct Subscribers(Mutex<Vec<mpsc::UnboundedSender<ProviderEvent>>>);

impl Subscribers {
    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.0.lock().expect("subscribers lock").push(sender);
        receiver
    }

    fn emit(&self, event: ProviderEvent) {
        self.0
            .lock()
            .expect("subscribers lock")
            .retain(|sender| sender.send(event.clone()).is_ok());
    }
}

/// Calls `on_line` for each line of a CLI's output, like Node's readline: a `\r\n` ending counts as one, the last
/// line needs no ending, and invalid UTF-8 is replaced rather than ending the stream.
async fn read_lines(
    output: Box<dyn AsyncRead + Send + Unpin>,
    mut on_line: impl FnMut(String) + Send + 'static,
) {
    let mut reader = BufReader::new(output);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = buffer.strip_suffix(b"\n").unwrap_or(&buffer);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                on_line(String::from_utf8_lossy(line).into_owned());
            }
        }
    }
}
