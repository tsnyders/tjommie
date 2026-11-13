//! Drives `claude -p` over stream-json: one process per bot session; turns go to stdin as JSON lines.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use protocol::model::Autonomy;
use regex::Regex;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::activity_labels::plain_activity_text;
use super::claude_files::{SessionPhase, record_claude_session_owner};
use super::claude_parse::{ClaudeLine, parse_claude_line, tool_summary};
use super::{
    Approval, ApprovalAnswer, McpServer, ProviderError, ProviderEvent, ProviderSession,
    StartOptions, Subscribers, TurnResult, Usage, js, read_lines, redact_provider_event,
};
use crate::cli::{
    BoxFuture, CliChild, CliCommand, CliExit, CliHost, Env, KillTree, LaunchOptions, Retirement,
    cli_env, process_env,
};

const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
/// The `turn_done` error when a worker could not prove that Claude's process tree has gone.
pub const UNCONFIRMED_EXIT: &str =
    "The isolated provider could not confirm process-tree exit. Its account stays reserved.";
const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// Kept for the exit message: the last 2,000 characters are reported.
const STDERR_TAIL_BYTES: usize = 8_000;

pub fn claude_mode(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::Ask => "manual",
        Autonomy::Folder => "acceptEdits",
        Autonomy::Full => "bypassPermissions",
    }
}

pub struct SessionFiles {
    pub instructions: String,
    pub mcp: String,
}

pub fn claude_args(
    options: &StartOptions,
    files: &SessionFiles,
) -> Result<Vec<String>, ProviderError> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        claude_mode(options.autonomy),
        "--model",
        &options.model,
        "--append-system-prompt-file",
        &files.instructions,
        "--strict-mcp-config",
    ]
    .map(String::from)
    .into();
    if !options.mcp_servers.is_empty() {
        args.extend(["--mcp-config".into(), files.mcp.clone()]);
    }
    for dir in &options.extra_dirs {
        args.extend(["--add-dir".into(), dir.clone()]);
    }
    if let Some(resume_id) = options.resume_id.as_ref().filter(|id| !id.is_empty()) {
        args.extend(["--resume".into(), resume_id.clone()]);
    }
    if let Some(budget) = options.max_budget_usd {
        if !budget.is_finite() || budget <= 0.0 {
            return Err(ProviderError::Failed(
                "The API account has no remaining spending allowance.".into(),
            ));
        }
        args.extend(["--max-budget-usd".into(), budget.to_string()]);
    }
    let auto_allowed = options
        .mcp_servers
        .iter()
        .filter(|server| server.auto_allow())
        .map(|server| format!("mcp__{}", server.name()));
    let allowed: Vec<String> = auto_allowed
        .chain(options.allow_rules.iter().cloned())
        .collect();
    if !allowed.is_empty() {
        args.push("--allowedTools".into()); // takes several values, so it goes last
        args.extend(allowed);
    }
    Ok(args)
}

pub fn claude_mcp_config(servers: &[McpServer]) -> Value {
    let servers = servers.iter().map(|server| {
        let config = match server {
            McpServer::Http { url, headers, .. } => {
                json!({ "type": "http", "url": url, "headers": headers })
            }
            McpServer::Stdio {
                command, args, env, ..
            } => json!({ "type": "stdio", "command": command, "args": args, "env": env }),
        };
        (server.name().to_string(), config)
    });
    json!({ "mcpServers": servers.collect::<Map<_, _>>() })
}

struct PendingApproval {
    input: Map<String, Value>,
    tool_name: String,
    suggestion: Option<Map<String, Value>>,
}

struct Child {
    generation: u64,
    pid: Option<u32>,
    /// `None` once stdin has been ended.
    stdin: Option<mpsc::UnboundedSender<String>>,
    exit: crate::cli::ChildExit,
    kill_tree: KillTree,
    retirement: Option<Retirement>,
}

/// What `waitForExit` waits on.
#[derive(Clone, Copy, PartialEq)]
enum Lifecycle {
    /// No child runs.
    Idle,
    Running,
    /// A worker could not confirm that the tree has gone.
    Unconfirmed,
}

impl Child {
    fn running(&self) -> bool {
        self.exit.status().is_none()
    }
}

struct State {
    child: Option<Child>,
    generation: u64,
    pending: HashMap<String, PendingApproval>,
    last_total_cost: Option<f64>,
    results: HashSet<String>,
    cli_session_id: Option<String>,
    streamed: String,
    in_turn: bool,
    closing: bool,
    interrupt_timer: Option<JoinHandle<()>>,
    close_timer: Option<JoinHandle<()>>,
    control_id: u64,
    files_dir: Option<PathBuf>,
}

struct Inner {
    host: Arc<dyn CliHost>,
    cmd: CliCommand,
    options: StartOptions,
    subscribers: Subscribers,
    state: Mutex<State>,
    lifecycle: watch::Sender<Lifecycle>,
}

/// One `claude -p` process. Cloning shares the session.
#[derive(Clone)]
pub struct ClaudeSession(Arc<Inner>);

impl ClaudeSession {
    pub fn new(host: Arc<dyn CliHost>, cmd: CliCommand, options: StartOptions) -> Self {
        let state = State {
            child: None,
            generation: 0,
            pending: HashMap::new(),
            last_total_cost: if options.resume_id.is_none() {
                Some(0.0)
            } else {
                options.resume_cost_usd
            },
            results: HashSet::new(),
            cli_session_id: options.resume_id.clone(),
            streamed: String::new(),
            in_turn: false,
            closing: false,
            interrupt_timer: None,
            close_timer: None,
            control_id: 0,
            files_dir: None,
        };
        ClaudeSession(Arc::new(Inner {
            host,
            cmd,
            options,
            subscribers: Subscribers::default(),
            state: Mutex::new(state),
            lifecycle: watch::channel(Lifecycle::Idle).0,
        }))
    }
}

fn user_message(text: &str) -> Value {
    json!({ "type": "user", "message": { "role": "user", "content": text } })
}

impl ProviderSession for ClaudeSession {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(Arc::clone(&self.0).start())
    }

    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut state = self.0.lock();
        if state.child.is_some() && !state.closing {
            state.in_turn = true;
            write(&mut state, &user_message(text));
        }
        Box::pin(async { Ok(()) })
    }

    /// Claude reads a message written mid-turn at its next step, inside the same turn.
    fn steer(&self, text: &str) -> BoxFuture<'static, Result<bool, ProviderError>> {
        let mut state = self.0.lock();
        let steered = state.child.is_some() && !state.closing;
        if steered {
            write(&mut state, &user_message(text));
        }
        Box::pin(async move { Ok(steered) })
    }

    /// Always `Ok(false)`: Claude ends the turn itself, with a result line.
    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>> {
        let mut state = self.0.lock();
        let Some(generation) = state.child.as_ref().map(|child| child.generation) else {
            return Box::pin(async { Ok(false) });
        };
        if !state.in_turn || state.closing {
            return Box::pin(async { Ok(false) });
        }
        state.control_id += 1;
        let request = json!({ "type": "control_request", "request_id": format!("tjommie-{}", state.control_id), "request": { "subtype": "interrupt" } });
        write(&mut state, &request);
        let inner = Arc::downgrade(&self.0);
        replace_timer(
            &mut state.interrupt_timer,
            tokio::spawn(async move {
                tokio::time::sleep(INTERRUPT_GRACE).await;
                let Some(inner) = inner.upgrade() else { return };
                let kill = {
                    let mut state = inner.lock();
                    match &state.child {
                        Some(child)
                            if child.generation == generation
                                && state.in_turn
                                && child.running() =>
                        {
                            let kill = Arc::clone(&child.kill_tree);
                            state.closing = true;
                            Some(kill)
                        }
                        _ => None,
                    }
                };
                if let Some(kill) = kill {
                    let _ = kill().await;
                }
            }),
        );
        Box::pin(async { Ok(false) })
    }

    fn answer(&self, approval_id: &str, answer: ApprovalAnswer) {
        let mut state = self.0.lock();
        if state.child.is_none() || state.closing {
            return;
        }
        let Some(pending) = state.pending.remove(approval_id) else {
            return;
        };
        let response = match answer {
            ApprovalAnswer::Deny => {
                json!({ "behavior": "deny", "message": "The person declined this in Tjommie." })
            }
            ApprovalAnswer::Once => json!({ "behavior": "allow", "updatedInput": pending.input }),
            ApprovalAnswer::Always => {
                json!({ "behavior": "allow", "updatedInput": pending.input, "updatedPermissions": [session_rule(&pending)] })
            }
        };
        write(
            &mut state,
            &json!({ "type": "control_response", "response": { "subtype": "success", "request_id": approval_id, "response": response } }),
        );
    }

    fn close(&self) {
        let mut state = self.0.lock();
        remove_files(&mut state);
        if state.closing {
            return;
        }
        let Some(child) = state.child.as_mut() else {
            return;
        };
        child.stdin = None; // ends stdin
        let generation = child.generation;
        state.closing = true;
        let inner = Arc::downgrade(&self.0);
        replace_timer(
            &mut state.close_timer,
            tokio::spawn(async move {
                tokio::time::sleep(CLOSE_GRACE).await;
                let Some(inner) = inner.upgrade() else { return };
                let kill = {
                    let state = inner.lock();
                    state
                        .child
                        .as_ref()
                        .filter(|child| child.generation == generation && child.running())
                        .map(|child| Arc::clone(&child.kill_tree))
                };
                if let Some(kill) = kill {
                    let _ = kill().await;
                }
            }),
        );
    }

    fn wait_for_exit(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut lifecycle = self.0.lifecycle.subscribe();
        Box::pin(async move {
            let settled = lifecycle
                .wait_for(|state| *state != Lifecycle::Running)
                .await
                .map(|state| *state);
            match settled {
                Ok(Lifecycle::Unconfirmed) => Err(ProviderError::WorkerTermination),
                _ => Ok(()),
            }
        })
    }

    fn pid(&self) -> Option<u32> {
        self.0.lock().child.as_ref().and_then(|child| child.pid)
    }

    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        self.0.subscribers.subscribe()
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("Claude session lock")
    }

    fn emit(&self, event: ProviderEvent) {
        let key = self.options.api_key.as_ref().map(|key| key.0.as_str());
        self.subscribers.emit(redact_provider_event(event, key));
    }

    async fn start(self: Arc<Self>) -> Result<(), ProviderError> {
        let base = self
            .options
            .env
            .get("CLAUDE_CONFIG_DIR")
            .unwrap_or(&self.options.cwd);
        let dir = tempfile::Builder::new()
            .prefix(".tjommie-session-")
            .tempdir_in(base)?
            .keep();
        self.lock().files_dir = Some(dir.clone());
        let child = match self.launch(&dir).await {
            Ok(child) => child,
            Err(error) if error.is_worker_termination() => {
                // The launch may have left a running tree: keep its files and report it to waitForExit.
                self.lifecycle.send_replace(Lifecycle::Unconfirmed);
                return Err(error);
            }
            Err(error) => {
                remove_files(&mut self.lock());
                return Err(error);
            }
        };
        let CliChild {
            pid,
            stdin,
            stdout,
            stderr,
            exit,
            kill_tree,
            retirement,
        } = child;
        let (lines, lines_out) = mpsc::unbounded_channel();
        let generation = {
            let mut state = self.lock();
            state.generation += 1;
            let generation = state.generation;
            state.child = Some(Child {
                generation,
                pid,
                stdin: Some(lines),
                exit: exit.clone(),
                kill_tree,
                retirement,
            });
            generation
        };
        self.lifecycle.send_replace(Lifecycle::Running);
        tokio::spawn(write_stdin(stdin, lines_out));
        let stderr_tail: Arc<Mutex<Vec<u8>>> = Arc::default();
        tokio::spawn(collect_tail(stderr, Arc::clone(&stderr_tail)));
        let weak = Arc::downgrade(&self);
        tokio::spawn(read_lines(stdout, move |line| {
            if let Some(inner) = weak.upgrade() {
                inner.on_line(&line);
            }
        }));
        let weak = Arc::downgrade(&self);
        exit.on_exit(move |exit| {
            if let Some(inner) = weak.upgrade() {
                inner.exited(generation, exit, &stderr_tail);
            }
        });
        if let Some(child_pid) = pid
            && record_claude_session_owner(&dir, SessionPhase::Started { child_pid }).is_err()
        {
            // Keep the starting marker. Failing to record the PID must not abandon a live CLI.
            eprintln!(
                "Could not record Claude's session process. Its files will be kept until it closes. {}",
                dir.display()
            );
        }
        Ok(())
    }

    async fn launch(&self, dir: &Path) -> Result<CliChild, ProviderError> {
        record_claude_session_owner(dir, SessionPhase::Preparing)?;
        let folder = dir.to_path_buf();
        tokio::task::spawn_blocking(move || restrict_session_folder(&folder))
            .await
            .map_err(|_| {
                ProviderError::Failed("Could not make Claude's session files private.".into())
            })??;
        let mut folders = vec![dir.to_path_buf(), PathBuf::from(&self.options.cwd)];
        folders.extend(self.options.extra_dirs.iter().map(PathBuf::from));
        self.host.prepare_provider_directories(folders).await?;
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let files = SessionFiles {
            instructions: path("instructions.txt"),
            mcp: path("mcp.json"),
        };
        std::fs::write(&files.instructions, &self.options.instructions)?;
        if !self.options.mcp_servers.is_empty() {
            std::fs::write(
                &files.mcp,
                claude_mcp_config(&self.options.mcp_servers).to_string(),
            )?;
        }
        // A crash during spawn may leave a live CLI whose PID was not recorded.
        record_claude_session_owner(dir, SessionPhase::Starting)?;
        let args = claude_args(&self.options, &files)?;
        let options = LaunchOptions {
            cwd: self.options.cwd.clone(),
            env: self.options.env.clone(),
            claude_api_key: self.options.api_key.clone(),
        };
        Ok(self.host.launch(&self.cmd, args, options).await?)
    }

    fn exited(self: &Arc<Self>, generation: u64, exit: &CliExit, stderr_tail: &Mutex<Vec<u8>>) {
        let state = self.lock();
        let Some(child) = state
            .child
            .as_ref()
            .filter(|child| child.generation == generation)
        else {
            return; // already reported or from a superseded child
        };
        let retirement = child.retirement.clone();
        let error = match exit {
            _ if state.closing => None,
            CliExit::Failed(error) => Some(error.clone()),
            CliExit::Exited { code: Some(0), .. } => None,
            CliExit::Exited { code, signal } => {
                let stderr =
                    String::from_utf8_lossy(&stderr_tail.lock().expect("stderr lock")).into_owned();
                let stderr = js::trim(js::slice_end(&stderr, 2000));
                let status = match code {
                    Some(code) => format!("code {code}"),
                    None => format!("signal {}", signal.as_deref().unwrap_or("unknown")),
                };
                Some(if stderr.is_empty() {
                    format!("Claude exited with {status}")
                } else {
                    stderr.to_string()
                })
            }
        };
        drop(state);
        let Some(proof) = retirement else {
            return self.finish_exited(generation, error);
        };
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            match proof.await {
                Ok(()) => inner.finish_exited(generation, error),
                Err(_) => {
                    let mut state = inner.lock();
                    state.closing = true;
                    state.in_turn = false;
                    inner.lifecycle.send_replace(Lifecycle::Unconfirmed);
                    inner.emit(ProviderEvent::TurnDone(TurnResult {
                        ok: false,
                        error: Some(UNCONFIRMED_EXIT.into()),
                    }));
                }
            }
        });
    }

    fn finish_exited(&self, generation: u64, error: Option<String>) {
        let mut state = self.lock();
        if state.child.as_ref().map(|child| child.generation) != Some(generation) {
            return;
        }
        state.child = None;
        remove_files(&mut state);
        self.lifecycle.send_replace(Lifecycle::Idle);
        state.in_turn = false;
        state
            .interrupt_timer
            .take()
            .into_iter()
            .chain(state.close_timer.take())
            .for_each(|timer| timer.abort());
        self.emit(ProviderEvent::Exit {
            error,
            completed_turn: None,
        });
    }

    fn on_line(&self, line: &str) {
        let mut state = self.lock();
        match parse_claude_line(line) {
            ClaudeLine::Init { session_id } => {
                if state.cli_session_id.as_deref() != Some(session_id.as_str()) {
                    state.last_total_cost = Some(0.0);
                }
                state.cli_session_id = Some(session_id.clone());
                state.in_turn = true;
                self.emit(ProviderEvent::Session {
                    cli_session_id: session_id,
                });
            }
            ClaudeLine::TextDelta { text } => {
                state.streamed.push_str(&text);
                self.emit(ProviderEvent::Text { delta: text });
            }
            ClaudeLine::Assistant { texts, tools } => {
                if !texts.is_empty() {
                    // Text normally arrives as deltas first. If it didn't, send the whole block now.
                    if state.streamed.is_empty() {
                        self.emit(ProviderEvent::Text {
                            delta: texts.concat(),
                        });
                    }
                    state.streamed.clear();
                    self.emit(ProviderEvent::TextEnd);
                }
                for tool in tools {
                    let Some(summary) =
                        tool_summary(&tool.name, &tool.input).filter(|summary| !summary.is_empty())
                    else {
                        continue;
                    };
                    let name = if tool.name.starts_with("mcp__") {
                        summary.clone()
                    } else {
                        plain_activity_text(&tool.name)
                    };
                    self.emit(ProviderEvent::Tool {
                        id: tool.id,
                        name,
                        summary,
                    });
                }
            }
            ClaudeLine::ToolResults { results } => {
                for (id, ok) in results {
                    self.emit(ProviderEvent::ToolDone { id, ok });
                }
            }
            ClaudeLine::Permission(permission) => {
                let detail = js::slice(&js::pretty(&Value::Object(permission.input.clone())), 4000)
                    .to_string();
                let pending = PendingApproval {
                    input: permission.input,
                    tool_name: permission.tool_name.clone(),
                    suggestion: permission.suggestion,
                };
                state.pending.insert(permission.request_id.clone(), pending);
                self.emit(ProviderEvent::Approval(Approval {
                    id: permission.request_id,
                    tool: permission.tool_name,
                    summary: permission.description,
                    detail,
                    rule: Some(permission.rule),
                }));
            }
            ClaudeLine::RateLimit(mut limit) => {
                if limit.windows.is_some() {
                    limit.limit_id = Some("claude".into());
                }
                self.emit(ProviderEvent::Limit(limit));
            }
            ClaudeLine::Result(result) => {
                if let Some(event_id) = &result.event_id
                    && !state.results.insert(event_id.clone())
                {
                    return;
                }
                if let Some(timer) = state.interrupt_timer.take() {
                    timer.abort();
                }
                state.in_turn = false;
                state.streamed.clear();
                // Claude restores cumulative session cost on --resume, including earlier processes.
                // A legacy resumed session has no baseline, so its first turn's cost is unknown.
                let cost = result
                    .total_cost_usd
                    .zip(state.last_total_cost)
                    .map(|(total, last)| (total - last).max(0.0));
                if result.total_cost_usd.is_some() {
                    state.last_total_cost = result.total_cost_usd;
                }
                self.emit(ProviderEvent::Usage(Usage {
                    model: result.model.unwrap_or_else(|| self.options.model.clone()),
                    tokens: result.usage,
                    cost_usd: cost,
                    total_cost_usd: result.total_cost_usd,
                    event_id: result.event_id,
                }));
                self.emit(ProviderEvent::TurnDone(TurnResult {
                    ok: result.ok,
                    error: result.error,
                }));
            }
            ClaudeLine::Other => {}
        }
    }
}

fn write(state: &mut State, message: &Value) {
    if state.closing {
        return;
    }
    if let Some(stdin) = state.child.as_ref().and_then(|child| child.stdin.as_ref()) {
        let _ = stdin.send(format!("{message}\n"));
    }
}

fn replace_timer(slot: &mut Option<JoinHandle<()>>, timer: JoinHandle<()>) {
    if let Some(old) = slot.replace(timer) {
        old.abort();
    }
}

fn remove_files(state: &mut State) {
    let Some(dir) = &state.files_dir else { return };
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            // A locked file must not prevent the process from closing. Exit retries this cleanup.
            eprintln!("Could not remove Claude's session files: {error}");
        }
        _ => state.files_dir = None,
    }
}

/// "Always allow" also tells this session to stop asking; the daemon saves the rule for later sessions.
fn session_rule(pending: &PendingApproval) -> Value {
    match &pending.suggestion {
        Some(suggestion) => {
            let mut rule = suggestion.clone();
            rule.insert("destination".into(), json!("session"));
            Value::Object(rule)
        }
        None => {
            json!({ "type": "addRules", "rules": [{ "toolName": pending.tool_name }], "behavior": "allow", "destination": "session" })
        }
    }
}

async fn write_stdin(
    mut stdin: Box<dyn AsyncWrite + Send + Unpin>,
    mut lines: mpsc::UnboundedReceiver<String>,
) {
    while let Some(line) = lines.recv().await {
        if stdin.write_all(line.as_bytes()).await.is_err() {
            return; // Claude has gone; its exit reports it
        }
    }
    let _ = stdin.shutdown().await;
}

async fn collect_tail(mut stderr: Box<dyn AsyncRead + Send + Unpin>, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0; 4096];
    while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
        let mut tail = tail.lock().expect("stderr lock");
        tail.extend_from_slice(&chunk[..read]);
        let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
        tail.drain(..excess);
    }
}

static USER_SID: Mutex<Option<String>> = Mutex::new(None);
static SID: LazyLock<Regex> = LazyLock::new(|| Regex::new("S-1-[0-9-]+").unwrap());

/// Windows ignores POSIX file modes. Remove inherited ACL entries before placing secrets in the folder.
fn restrict_session_folder(folder: &Path) -> Result<(), ProviderError> {
    let env = cli_env(&Env::new(), &process_env());
    let sid = {
        let mut sid = USER_SID.lock().expect("SID lock");
        if sid.is_none() {
            let identity = crate::spawn::command(crate::cli::system32("whoami.exe"))
                .args(["/user", "/fo", "csv", "/nh"])
                .env_clear()
                .envs(&env)
                .stdin(Stdio::null())
                .output();
            *sid = identity.ok().and_then(|output| {
                SID.find(&String::from_utf8_lossy(&output.stdout))
                    .map(|sid| sid.as_str().to_string())
            });
        }
        sid.clone().ok_or_else(|| {
            ProviderError::Failed("Could not identify the user for Claude's session files.".into())
        })?
    };
    let result = crate::spawn::command(crate::cli::system32("icacls.exe"))
        .arg(folder)
        .args(["/inheritance:r", "/grant:r", &format!("*{sid}:(OI)(CI)F")])
        .env_clear()
        .envs(&env)
        .stdin(Stdio::null())
        .output();
    match result {
        Ok(output) if output.status.success() => Ok(()),
        _ => Err(ProviderError::Failed(
            "Could not make Claude's session files private.".into(),
        )),
    }
}
