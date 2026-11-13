//! Drives `codex app-server`: one process and one thread per bot session.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use protocol::model::{Autonomy, Provider};
use regex::Regex;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, watch};

use super::codex_parse::{
    map_codex_elicitation, map_codex_notification, map_codex_request, parse_codex_mcp_tool_call,
};
use super::codex_rpc::{CodexServer, JsonRpcLines, RpcId};
use super::{
    ApprovalAnswer, McpServer, ProviderError, ProviderEvent, ProviderSession, StartOptions,
    Subscribers, TurnResult, Usage, js, redact_provider_event,
};
use crate::account_pricing::has_pricing;
use crate::cli::{
    BoxFuture, ChildExit, CliCommand, CliExit, CliHost, Retirement, wait_for_cli_exit,
};

const EXITED_BEFORE_STOP: &str = "Codex exited before its process tree could be stopped.";
static TURN_ALREADY_ENDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?:^|: )no active turn to interrupt$").unwrap());
static UNEXPECTED_TURN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^expected active turn id `[^`\r\n]+` but found `[^`\r\n]+`$").unwrap()
});

fn turn_id(params: &Value) -> Option<String> {
    let id = params
        .get("turnId")
        .and_then(Value::as_str)
        .or_else(|| params.get("turn")?.as_object()?.get("id")?.as_str());
    id.map(str::to_string)
}

fn is_turn_already_ended(error: &ProviderError) -> bool {
    matches!(error, ProviderError::Remote { message, .. } if TURN_ALREADY_ENDED.is_match(message))
}

fn is_turn_not_steerable(error: &ProviderError) -> bool {
    // Codex's turn_steer_inner uses invalid-request errors for these turn-state races.
    let ProviderError::Remote {
        code: -32600,
        message,
    } = error
    else {
        return false;
    };
    matches!(
        message.as_str(),
        "no active turn to steer" | "cannot steer a review turn" | "cannot steer a compact turn"
    ) || UNEXPECTED_TURN.is_match(message)
}

/// `{ approvalPolicy, sandbox }` for a thread.
pub fn codex_policy(autonomy: Autonomy) -> Value {
    let (approval_policy, sandbox) = match autonomy {
        Autonomy::Ask => ("untrusted", "read-only"),
        Autonomy::Folder => ("on-request", "workspace-write"),
        Autonomy::Full => ("never", "danger-full-access"),
    };
    json!({ "approvalPolicy": approval_policy, "sandbox": sandbox })
}

/// Codex features that are on by default and would reach past Tjommie: ChatGPT apps, driving the person's own
/// browser and desktop, plugins, and memories shared by every bot on the account. Bots get Tjommie's MCP servers only.
pub const CODEX_FEATURES_OFF: [&str; 10] = [
    "apps",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "in_app_browser",
    "plugins",
    "remote_plugin",
    "memories",
    "tool_suggest",
    "skill_mcp_dependency_install",
];

/// Per-thread config overrides. The daemon gates Own folder before this session starts. `windows`: the platform is
/// Windows (`platform === "win32"` in TypeScript).
pub fn codex_config(options: &StartOptions, windows: bool) -> Value {
    let elevated = options.autonomy == Autonomy::Folder
        || (options.autonomy == Autonomy::Ask && options.codex_sandbox_ready);
    let mut config = Map::new();
    config.insert(
        "sandbox_workspace_write".into(),
        json!({ "network_access": false, "writable_roots": options.extra_dirs }),
    );
    if windows {
        config.insert(
            "windows".into(),
            json!({ "sandbox": if elevated { "elevated" } else { "unelevated" } }),
        );
    }
    config.insert(
        "features".into(),
        CODEX_FEATURES_OFF
            .iter()
            .map(|feature| (feature.to_string(), json!(false)))
            .collect::<Map<_, _>>()
            .into(),
    );
    let servers = options.mcp_servers.iter().map(|server| {
        let config = match server {
            McpServer::Http { url, headers, .. } => json!({ "url": url, "http_headers": headers }),
            McpServer::Stdio {
                command, args, env, ..
            } => json!({ "command": command, "args": args, "env": env }),
        };
        (server.name().to_string(), config)
    });
    config.insert("mcp_servers".into(), servers.collect::<Map<_, _>>().into());
    config.into()
}

/// What `waitForExit` waits on: the latest server's exit, or its worker's proof that the whole tree has gone.
#[derive(Clone)]
enum ServerExit {
    Server(ChildExit, Option<Retirement>),
    /// A launch whose process tree may still run.
    Unconfirmed,
}

impl ServerExit {
    async fn wait(self) -> Result<(), ProviderError> {
        match self {
            ServerExit::Server(exit, retirement) => {
                wait_for_cli_exit(&exit, retirement.as_ref()).await
            }
            ServerExit::Unconfirmed => Err(ProviderError::WorkerTermination),
        }
    }
}

/// A result several callers can await, settled by a task that runs to the end like a JavaScript promise.
type Shared<T> = watch::Receiver<Option<T>>;

async fn settled<T: Clone>(mut shared: Shared<T>) -> T {
    let result = shared
        .wait_for(Option::is_some)
        .await
        .expect("the task settles before it ends");
    result.clone().expect("waited for a result")
}

#[derive(Clone, Copy)]
enum Answers {
    /// Command and file approvals.
    Decision,
    /// MCP tool calls.
    Action,
}

struct PendingApproval {
    rpc_id: RpcId,
    answers: Answers,
}

struct State {
    /// The running server and its id, which tells a replaced server's callbacks apart.
    server: Option<(u64, Arc<CodexServer>)>,
    servers: u64,
    approvals: HashMap<String, PendingApproval>,
    thread_id: String,
    turn_id: Option<String>,
    completed_turns: HashSet<String>,
    usage_events: HashSet<String>,
    model: String,
    usage_enabled: bool,
    closed: bool,
    starting: Option<Shared<Result<(), ProviderError>>>,
    sending: Option<(u64, Shared<Result<(), ProviderError>>)>,
    interrupting: Option<(u64, Shared<Result<bool, ProviderError>>)>,
    tokens: u64,
    stopping: bool,
    stopped_turn_id: Option<String>,
    stop_completion: Option<TurnResult>,
    /// The latest server's exit; `None` before one starts.
    exited: Option<ServerExit>,
}

impl State {
    fn token(&mut self) -> u64 {
        self.tokens += 1;
        self.tokens
    }

    fn server_id(&self) -> Option<u64> {
        self.server.as_ref().map(|(id, _)| *id)
    }

    fn rpc(&self) -> Result<Arc<JsonRpcLines>, ProviderError> {
        let (_, server) = self
            .server
            .as_ref()
            .ok_or_else(|| failed("The Codex session isn't running"))?;
        Ok(Arc::clone(&server.rpc))
    }
}

struct Inner {
    host: Arc<dyn CliHost>,
    cmd: CliCommand,
    options: StartOptions,
    subscribers: Subscribers,
    state: Mutex<State>,
}

/// One `codex app-server` process and thread. Cloning shares the session.
#[derive(Clone)]
pub struct CodexSession(Arc<Inner>);

impl CodexSession {
    pub fn new(host: Arc<dyn CliHost>, cmd: CliCommand, options: StartOptions) -> Self {
        let state = State {
            server: None,
            servers: 0,
            approvals: HashMap::new(),
            thread_id: String::new(),
            turn_id: None,
            completed_turns: HashSet::new(),
            usage_events: HashSet::new(),
            model: String::new(),
            usage_enabled: false,
            closed: false,
            starting: None,
            sending: None,
            interrupting: None,
            tokens: 0,
            stopping: false,
            stopped_turn_id: None,
            stop_completion: None,
            exited: None,
        };
        CodexSession(Arc::new(Inner {
            host,
            cmd,
            options,
            subscribers: Subscribers::default(),
            state: Mutex::new(state),
        }))
    }
}

fn text_input(text: &str) -> Value {
    json!([{ "type": "text", "text": text }])
}

fn failed(message: &str) -> ProviderError {
    ProviderError::Failed(message.into())
}

impl ProviderSession for CodexSession {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut state = self.0.lock();
        if state.closed {
            return Box::pin(async { Err(failed("The Codex session is closed")) });
        }
        let (sender, starting) = watch::channel(None);
        state.starting = Some(starting.clone());
        let inner = Arc::clone(&self.0);
        tokio::spawn(async move {
            sender.send_replace(Some(inner.start().await));
        });
        Box::pin(settled(starting))
    }

    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        let mut state = self.0.lock();
        if state.model.is_empty() {
            return Box::pin(async { Err(failed("The Codex session has not finished starting.")) });
        }
        // Enable before writing: a provider may report new usage before turn/start replies.
        state.usage_enabled = true;
        let rpc = match state.rpc() {
            Ok(rpc) => rpc,
            Err(error) => return Box::pin(async { Err(error) }),
        };
        let params = json!({ "threadId": state.thread_id, "input": text_input(text) });
        let token = state.token();
        let (sender, sending) = watch::channel(None);
        state.sending = Some((token, sending.clone()));
        drop(state);
        let request = rpc.request("turn/start", params);
        let inner = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let result = match request.await {
                Ok(response) => match response["turn"]["id"].as_str() {
                    Some(id) => {
                        if let Some(inner) = inner.upgrade() {
                            // Notifications can finish this turn (and start another) before its start response.
                            let mut state = inner.lock();
                            if !state.completed_turns.contains(id)
                                && state.turn_id.as_deref().is_none_or(|current| current == id)
                            {
                                state.turn_id = Some(id.into());
                            }
                        }
                        Ok(())
                    }
                    None => Err(failed("Codex did not return the turn's id.")),
                },
                Err(error) => Err(error),
            };
            if let Some(inner) = inner.upgrade() {
                let mut state = inner.lock();
                if state
                    .sending
                    .as_ref()
                    .is_some_and(|(current, _)| *current == token)
                {
                    state.sending = None;
                }
            }
            sender.send_replace(Some(result));
        });
        Box::pin(settled(sending))
    }

    fn steer(&self, text: &str) -> BoxFuture<'static, Result<bool, ProviderError>> {
        let state = self.0.lock();
        let (Some(turn_id), Ok(rpc)) = (&state.turn_id, state.rpc()) else {
            return Box::pin(async { Ok(false) });
        };
        let params = json!({ "threadId": state.thread_id, "expectedTurnId": turn_id, "input": text_input(text) });
        drop(state);
        let request = rpc.request("turn/steer", params);
        Box::pin(async move {
            match request.await {
                Ok(_) => Ok(true),
                Err(error) if is_turn_not_steerable(&error) => Ok(false), // the caller queues work that cannot join this turn
                Err(error) => Err(error), // the runner logs unexpected RPC and transport failures
            }
        })
    }

    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>> {
        let mut state = self.0.lock();
        if let Some((_, interrupting)) = &state.interrupting {
            return Box::pin(settled(interrupting.clone()));
        }
        let owned_server = state.server_id();
        state.stopping = true;
        let sending = state.sending.as_ref().map(|(_, sending)| sending.clone());
        let token = state.token();
        let (sender, interrupting) = watch::channel(None);
        state.interrupting = Some((token, interrupting.clone()));
        drop(state);
        let inner = Arc::clone(&self.0);
        tokio::spawn(async move {
            let result = inner.interrupt(owned_server, sending).await;
            {
                let mut state = inner.lock();
                state.stopping = false;
                if state
                    .interrupting
                    .as_ref()
                    .is_some_and(|(current, _)| *current == token)
                {
                    state.interrupting = None;
                }
            }
            sender.send_replace(Some(result));
        });
        Box::pin(settled(interrupting))
    }

    fn answer(&self, approval_id: &str, answer: ApprovalAnswer) {
        let mut state = self.0.lock();
        let Ok(rpc) = state.rpc() else { return };
        let Some(pending) = state.approvals.remove(approval_id) else {
            return;
        };
        drop(state);
        // The daemon remembers "Always allow" per bot and answers matching requests itself, so Codex only
        // ever gets yes or no.
        let yes = answer != ApprovalAnswer::Deny;
        let result = match pending.answers {
            Answers::Action => json!({ "action": if yes { "accept" } else { "decline" } }),
            Answers::Decision => json!({ "decision": if yes { "accept" } else { "decline" } }),
        };
        rpc.respond(&pending.rpc_id, result);
    }

    fn close(&self) {
        self.0.close();
    }

    fn wait_for_exit(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let starting = self.0.lock().starting.clone();
        let inner = Arc::clone(&self.0);
        Box::pin(async move {
            if let Some(starting) = starting {
                let _ = settled(starting).await; // A failed start still closes its process.
            }
            let exited = inner.lock().exited.clone();
            match exited {
                Some(exited) => exited.wait().await,
                None => Ok(()),
            }
        })
    }

    fn pid(&self) -> Option<u32> {
        self.0
            .lock()
            .server
            .as_ref()
            .and_then(|(_, server)| server.pid)
    }

    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        self.0.subscribers.subscribe()
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("Codex session lock")
    }

    fn emit(&self, state: &mut State, event: ProviderEvent) {
        if let ProviderEvent::Usage(Usage {
            event_id: Some(id), ..
        }) = &event
            && !state.usage_events.insert(id.clone())
        {
            return;
        }
        let key = self.options.api_key.as_ref().map(|key| key.0.as_str());
        self.subscribers.emit(redact_provider_event(event, key));
    }

    /// Not under the lock: closing a server can fire its exit, whose callback locks the state.
    fn close(&self) {
        let server = {
            let mut state = self.lock();
            state.closed = true;
            state.server.take()
        };
        if let Some((_, server)) = server {
            (server.close)();
        }
    }

    async fn start(self: Arc<Self>) -> Result<(), ProviderError> {
        let closed = || failed("The Codex session is closed");
        if self.lock().closed {
            return Err(closed());
        }
        let options = &self.options;
        let mut folders = vec![std::path::PathBuf::from(&options.cwd)];
        folders.extend(options.extra_dirs.iter().map(std::path::PathBuf::from));
        self.host.prepare_provider_directories(folders).await?;
        if self.lock().closed {
            return Err(closed());
        }
        let started = self
            .host
            .start_codex_server(
                &self.cmd,
                &options.env,
                &options.cwd,
                options.api_key.is_some(),
            )
            .await;
        let server = match started {
            Ok(server) => Arc::new(server),
            Err(error) => {
                if error.is_worker_termination() {
                    self.lock().exited = Some(ServerExit::Unconfirmed);
                }
                return Err(error);
            }
        };
        let server_id = {
            let mut state = self.lock();
            state.exited = Some(ServerExit::Server(
                server.exit.clone(),
                server.retirement.clone(),
            ));
            if state.closed {
                drop(state);
                (server.close)();
                return Err(closed());
            }
            state.servers += 1;
            state.server = Some((state.servers, Arc::clone(&server)));
            state.servers
        };
        let weak = Arc::downgrade(&self);
        let retirement = server.retirement.clone();
        server.exit.on_exit(move |exit| {
            let Some(inner) = weak.upgrade() else { return };
            let Some(proof) = retirement else {
                return inner.server_exited(server_id, exit);
            };
            let exit = exit.clone();
            tokio::spawn(async move {
                match proof.await {
                    Ok(()) => inner.server_exited(server_id, &exit),
                    Err(_) => {
                        let mut state = inner.lock();
                        let error = super::claude::UNCONFIRMED_EXIT.to_string();
                        inner.emit(
                            &mut state,
                            ProviderEvent::TurnDone(TurnResult {
                                ok: false,
                                error: Some(error),
                            }),
                        );
                    }
                }
            });
        });
        if let Some(key) = &options.api_key {
            let login = server.rpc.request(
                "account/login/start",
                json!({ "type": "apiKey", "apiKey": key.0 }),
            );
            if login.await.is_err() {
                self.lock().server = None;
                (server.close)();
                ServerExit::Server(server.exit.clone(), server.retirement.clone())
                    .wait()
                    .await?;
                return Err(failed(
                    "Codex could not configure this API key. Check the account in Settings.",
                ));
            }
            if self.lock().closed {
                (server.close)();
                return Err(failed(
                    "The Codex session closed while configuring its account.",
                ));
            }
        }
        server.rpc.set_on_notification({
            let weak = Arc::downgrade(&self);
            move |method, params| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_notification(method, &params);
                }
            }
        });
        server.rpc.set_on_request({
            let (weak, rpc) = (Arc::downgrade(&self), Arc::downgrade(&server.rpc));
            move |id, method, params| {
                if let (Some(inner), Some(rpc)) = (weak.upgrade(), rpc.upgrade()) {
                    inner.on_request(&rpc, id, method, &params);
                }
            }
        });

        let resume_id = options.resume_id.as_ref().filter(|id| !id.is_empty());
        let mut params = Map::new();
        if let Some(resume_id) = resume_id {
            params.insert("threadId".into(), json!(resume_id));
        }
        params.insert("cwd".into(), json!(options.cwd));
        params.insert(
            "model".into(),
            if options.model.is_empty() {
                Value::Null
            } else {
                json!(options.model)
            },
        );
        if let Value::Object(policy) = codex_policy(options.autonomy) {
            params.extend(policy);
        }
        params.insert("developerInstructions".into(), json!(options.instructions));
        params.insert("config".into(), codex_config(options, cfg!(windows)));
        let method = if resume_id.is_some() {
            "thread/resume"
        } else {
            "thread/start"
        };
        let thread = server.rpc.request(method, Value::Object(params)).await.and_then(|response| {
            let model = response["model"].as_str().filter(|model| !js::trim(model).is_empty());
            let model = model.ok_or_else(|| failed("Codex did not identify this session's model. Retry the queued work."))?;
            if options.api_key.is_some() && !has_pricing(Provider::Codex, model) {
                return Err(failed(
                    "Codex resolved this API session to a model without supported local pricing. Choose a priced model before retrying.",
                ));
            }
            let thread_id = response["thread"]["id"].as_str().ok_or_else(|| failed("Codex did not identify this session's thread."))?;
            Ok((thread_id.to_string(), model.to_string()))
        });
        let (thread_id, model) = match thread {
            Ok(thread) => thread,
            Err(error) => {
                self.close();
                return Err(error);
            }
        };
        let mut state = self.lock();
        state.thread_id = thread_id.clone();
        state.model = model;
        if state.closed {
            return Err(closed());
        }
        self.emit(
            &mut state,
            ProviderEvent::Session {
                cli_session_id: thread_id,
            },
        );
        Ok(())
    }

    fn server_exited(&self, server_id: u64, exit: &CliExit) {
        let mut state = self.lock();
        if state.server_id() != Some(server_id) {
            return; // closed on purpose
        }
        state.server = None;
        if state.stopping {
            return; // interrupt publishes exit after the whole tree has stopped
        }
        let error = match exit {
            CliExit::Exited { code, .. } => code
                .filter(|code| *code != 0)
                .map(|code| format!("Codex exited with code {code}")),
            // TypeScript listens for `exit` only; a lost process must still end the session for the runner.
            CliExit::Failed(error) => Some(error.clone()),
        };
        self.emit(
            &mut state,
            ProviderEvent::Exit {
                error,
                completed_turn: None,
            },
        );
    }

    fn on_notification(&self, method: &str, params: &Value) {
        let mut state = self.lock();
        // thread/resume can replay usage before its response supplies the resolved model.
        // Those are historical snapshots, not work started by this session. Publishing them
        // would both charge restored usage again and send an empty model to persistence.
        if method == "thread/tokenUsage/updated" && !state.usage_enabled {
            return;
        }
        let id = turn_id(params);
        // An approved command can emit item/started and item/completed AFTER its interrupted
        // turn/completed. Do not let that old work look like a new background turn to the runner.
        // Usage notifications may still arrive late and must remain billable.
        let completed = id
            .as_ref()
            .is_some_and(|id| state.completed_turns.contains(id));
        if completed && (method.starts_with("item/") || method.starts_with("turn/")) {
            return;
        }
        if method == "turn/started" && id.is_some() {
            state.turn_id = id.clone();
        }
        if method == "turn/completed" {
            let completed_id = id.or_else(|| state.turn_id.clone());
            if let Some(completed_id) = &completed_id {
                state.completed_turns.insert(completed_id.clone());
            }
            if state.turn_id.is_some() && state.turn_id != completed_id {
                return;
            }
            state.turn_id = None;
            if state.stopping {
                state.stopped_turn_id = completed_id;
                let completion = map_codex_notification(method, params, &state.model)
                    .into_iter()
                    .find_map(|event| {
                        if let ProviderEvent::TurnDone(turn) = event {
                            Some(turn)
                        } else {
                            None
                        }
                    });
                if completion.is_some() {
                    state.stop_completion = completion;
                }
                return; // the interrupted command can still start; wait for process-tree termination
            }
        }
        for event in map_codex_notification(method, params, &state.model) {
            self.emit(&mut state, event);
        }
    }

    fn on_request(&self, rpc: &JsonRpcLines, id: RpcId, method: &str, params: &Value) {
        let mut state = self.lock();
        let completed = turn_id(params).is_some_and(|turn| state.completed_turns.contains(&turn));
        if method == "mcpServer/elicitation/request" {
            if completed {
                rpc.respond(&id, json!({ "action": "decline" }));
            } else {
                self.on_elicitation(&mut state, rpc, id, params);
            }
            return;
        }
        let Some(approval) = map_codex_request(&id, method, params) else {
            rpc.respond_error(&id, -32601, "Not supported by Tjommie");
            return;
        };
        if completed {
            rpc.respond(&id, json!({ "decision": "decline" }));
            return;
        }
        state.approvals.insert(
            approval.id.clone(),
            PendingApproval {
                rpc_id: id,
                answers: Answers::Decision,
            },
        );
        self.emit(&mut state, ProviderEvent::Approval(approval));
    }

    /// Codex asks before every MCP tool call. Tjommie's own servers marked autoAllow are answered at once; other
    /// tool calls become approval cards. Any other question from an MCP server is declined.
    fn on_elicitation(&self, state: &mut State, rpc: &JsonRpcLines, id: RpcId, params: &Value) {
        let decline = |id: &RpcId| rpc.respond(id, json!({ "action": "decline" }));
        let Some(call) = parse_codex_mcp_tool_call(params) else {
            return decline(&id);
        };
        let Some(server) = self
            .options
            .mcp_servers
            .iter()
            .find(|server| server.name() == call.server_name)
        else {
            return decline(&id);
        };
        if server.auto_allow() {
            return rpc.respond(&id, json!({ "action": "accept" }));
        }
        let Some(approval) = map_codex_elicitation(&id, params) else {
            return decline(&id);
        };
        state.approvals.insert(
            approval.id.clone(),
            PendingApproval {
                rpc_id: id,
                answers: Answers::Action,
            },
        );
        self.emit(state, ProviderEvent::Approval(approval));
    }

    async fn interrupt(
        &self,
        owned_server: Option<u64>,
        sending: Option<Shared<Result<(), ProviderError>>>,
    ) -> Result<bool, ProviderError> {
        let result = self.stop(owned_server, sending).await;
        if let Err(error) = &result {
            // An exit can reject send/interrupt, or precede a failed taskkill. Its callback was
            // held above, so publish the failure instead of leaving the runner busy forever.
            let mut state = self.lock();
            if owned_server.is_some() && state.server_id() != owned_server && !state.closed {
                state.closed = true;
                self.emit(
                    &mut state,
                    ProviderEvent::Exit {
                        error: Some(error.message().into()),
                        completed_turn: None,
                    },
                );
            }
        }
        result
    }

    async fn stop(
        &self,
        owned_server: Option<u64>,
        sending: Option<Shared<Result<(), ProviderError>>>,
    ) -> Result<bool, ProviderError> {
        if let Some(sending) = sending {
            settled(sending).await?;
        }
        // Err: the server has already gone, with the exit to wait for.
        let stop = {
            let mut state = self.lock();
            let id = state
                .turn_id
                .clone()
                .or_else(|| state.stopped_turn_id.clone());
            match state.server.clone() {
                Some(_) if id.is_none() && state.stop_completion.is_none() => return Ok(false),
                Some((server_id, server)) => {
                    state.stopped_turn_id = id.clone();
                    Ok((server_id, server, id, state.thread_id.clone()))
                }
                None if owned_server.is_none() || state.closed => return Ok(false),
                None => Err(state.exited.clone()),
            }
        };
        let (server_id, server, id, thread_id) = match stop {
            Ok(stop) => stop,
            Err(exited) => {
                if let Some(exited) = exited {
                    exited.wait().await?;
                }
                return Err(failed(EXITED_BEFORE_STOP));
            }
        };
        if let Some(id) = &id
            && let Err(error) = server
                .rpc
                .request(
                    "turn/interrupt",
                    json!({ "threadId": thread_id, "turnId": id }),
                )
                .await
        {
            let state = self.lock();
            if !is_turn_already_ended(&error)
                && !state.completed_turns.contains(id)
                && state.server_id() == Some(server_id)
            {
                return Err(error);
            }
        }
        // Codex can launch an approved command after acknowledging interruption. Retire this
        // owned process tree; the runner resumes the saved thread for its next queued item.
        if self.lock().server_id() != Some(server_id) {
            return Err(failed(EXITED_BEFORE_STOP));
        }
        (server.kill_tree)().await.map_err(ProviderError::Failed)?;
        let exited = self.lock().exited.clone();
        if let Some(exited) = exited {
            exited.wait().await?;
        }
        self.finish_stop();
        Ok(true)
    }

    fn finish_stop(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.stopped_turn_id = None;
        let completed_turn = state
            .stop_completion
            .take()
            .filter(|completion| completion.ok);
        self.emit(
            &mut state,
            ProviderEvent::Exit {
                error: None,
                completed_turn,
            },
        );
    }
}
