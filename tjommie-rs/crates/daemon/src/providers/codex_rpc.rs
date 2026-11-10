//! JSON-RPC over newline-delimited JSON, as spoken by `codex app-server` (it omits the "jsonrpc" field).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{Number, Value, json};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{ProviderError, js};
use crate::cli::{ChildExit, CliChild, CliCommand, CliExit, CliHost, KillTree, LaunchOptions};

/// A request id: a number or a string. Tjommie's own requests use numbers.
#[derive(Debug, Clone, PartialEq)]
pub enum RpcId {
    Number(Number),
    String(String),
}

impl RpcId {
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => Some(RpcId::Number(number.clone())),
            Value::String(text) => Some(RpcId::String(text.clone())),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            RpcId::Number(number) => Value::Number(number.clone()),
            RpcId::String(text) => Value::String(text.clone()),
        }
    }
}

impl From<u64> for RpcId {
    fn from(id: u64) -> Self {
        RpcId::Number(id.into())
    }
}

impl From<&str> for RpcId {
    fn from(id: &str) -> Self {
        RpcId::String(id.into())
    }
}

/// `String(id)`.
impl fmt::Display for RpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&js::string(&self.to_value()))
    }
}

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

type Write = Box<dyn Fn(String) -> Result<(), ProviderError> + Send + Sync>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, ProviderError>>>>>;
type OnNotification = Arc<dyn Fn(&str, Value) + Send + Sync>;
type OnRequest = Arc<dyn Fn(RpcId, &str, Value) + Send + Sync>;

/// `params` stay JSON (`unknown` in TypeScript): the session's mappers decode them.
pub struct JsonRpcLines {
    write: Write,
    next_id: AtomicU64,
    pending: Pending,
    on_notification: Mutex<Option<OnNotification>>,
    on_request: Mutex<Option<OnRequest>>,
}

impl JsonRpcLines {
    /// `write` sends one line, without its newline.
    pub fn new(
        write: impl Fn(String) -> Result<(), ProviderError> + Send + Sync + 'static,
    ) -> Self {
        JsonRpcLines {
            write: Box::new(write),
            next_id: AtomicU64::new(1),
            pending: Arc::default(),
            on_notification: Mutex::new(None),
            on_request: Mutex::new(None),
        }
    }

    pub fn set_on_notification(&self, handler: impl Fn(&str, Value) + Send + Sync + 'static) {
        *self.on_notification.lock().expect("handler lock") = Some(Arc::new(handler));
    }

    pub fn set_on_request(&self, handler: impl Fn(RpcId, &str, Value) + Send + Sync + 'static) {
        *self.on_request.lock().expect("handler lock") = Some(Arc::new(handler));
    }

    /// Writes the request now; the future waits for its response.
    pub fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, ProviderError>> + Send + 'static {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
    }

    pub fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> impl Future<Output = Result<Value, ProviderError>> + Send + 'static {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, response) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending lock")
            .insert(id, sender);
        // Not under the lock: a test's write can answer at once.
        let written =
            (self.write)(json!({ "id": id, "method": method, "params": params }).to_string());
        if written.is_err() {
            self.pending.lock().expect("pending lock").remove(&id);
        }
        let pending = Arc::clone(&self.pending);
        let method = method.to_string();
        async move {
            written?;
            match tokio::time::timeout(timeout, response).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(ProviderError::Failed(format!("{method} was abandoned"))),
                Err(_) => {
                    pending.lock().expect("pending lock").remove(&id);
                    Err(ProviderError::Failed(format!("{method} timed out")))
                }
            }
        }
    }

    pub fn notify(&self, method: &str, params: Option<Value>) {
        let message = match params {
            None => json!({ "method": method }),
            Some(params) => json!({ "method": method, "params": params }),
        };
        let _ = (self.write)(message.to_string()); // a closed stdin already failed every pending request
    }

    pub fn respond(&self, id: &RpcId, result: Value) {
        let _ = (self.write)(json!({ "id": id.to_value(), "result": result }).to_string());
    }

    pub fn respond_error(&self, id: &RpcId, code: i64, message: &str) {
        let _ = (self.write)(
            json!({ "id": id.to_value(), "error": { "code": code, "message": message } })
                .to_string(),
        );
    }

    pub fn handle_line(&self, line: &str) {
        let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(line) else {
            return;
        }; // stray log output
        let params = message.remove("params").unwrap_or(Value::Null);

        if let Some(method) = message.get("method").and_then(Value::as_str) {
            let id = message.get("id").and_then(RpcId::from_value);
            match id {
                Some(id) => {
                    let handler = self.on_request.lock().expect("handler lock").clone();
                    if let Some(handler) = handler {
                        handler(id, method, params);
                    }
                }
                None => {
                    let handler = self.on_notification.lock().expect("handler lock").clone();
                    if let Some(handler) = handler {
                        handler(method, params);
                    }
                }
            }
            return;
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(pending) = self.pending.lock().expect("pending lock").remove(&id) else {
            return;
        };
        let result = match message.get("error").filter(|error| !error.is_null()) {
            Some(error) => {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| error.to_string(), str::to_string);
                Err(ProviderError::Remote {
                    code,
                    message: text,
                })
            }
            None => Ok(message.remove("result").unwrap_or(Value::Null)),
        };
        let _ = pending.send(result);
    }

    pub fn fail_all(&self, error: ProviderError) {
        let pending = std::mem::take(&mut *self.pending.lock().expect("pending lock"));
        for sender in pending.into_values() {
            let _ = sender.send(Err(error.clone()));
        }
    }
}

pub struct CodexServer {
    pub rpc: Arc<JsonRpcLines>,
    pub pid: Option<u32>,
    pub exit: ChildExit,
    pub kill_tree: KillTree,
    /// Ends stdin, then stops the process tree if it is still running 3 seconds later.
    pub close: Arc<dyn Fn() + Send + Sync>,
}

/// Starts `codex app-server` with the given environment (CODEX_HOME = the account folder) and completes
/// the initialize handshake.
pub async fn start_codex_server<H: CliHost + ?Sized>(
    host: &H,
    cmd: &CliCommand,
    env: &BTreeMap<String, String>,
    cwd: &str,
    ephemeral_auth: bool,
) -> Result<CodexServer, ProviderError> {
    let mut args: Vec<String> = Vec::new();
    if ephemeral_auth {
        args.extend([
            "-c".into(),
            r#"cli_auth_credentials_store="ephemeral""#.into(),
        ]);
    }
    args.push("app-server".into());
    let options = LaunchOptions {
        cwd: cwd.into(),
        env: env.clone(),
        claude_api_key: None,
    };
    let CliChild {
        pid,
        stdin,
        stdout,
        stderr,
        exit,
        kill_tree,
    } = host.launch(cmd, args, options).await?;

    let (lines, lines_out) = mpsc::unbounded_channel::<String>();
    let input = Arc::new(Mutex::new(Some(lines)));
    let rpc = Arc::new(JsonRpcLines::new({
        let input = Arc::clone(&input);
        move |line| {
            let input = input.lock().expect("stdin lock");
            let sent = input
                .as_ref()
                .is_some_and(|lines| lines.send(format!("{line}\n")).is_ok());
            if sent {
                Ok(())
            } else {
                Err(ProviderError::Failed(
                    "codex app-server's input is closed".into(),
                ))
            }
        }
    }));
    tokio::spawn(write_lines(stdin, lines_out, Arc::downgrade(&rpc)));
    tokio::spawn(super::read_lines(stdout, {
        let rpc = Arc::clone(&rpc);
        move |line| rpc.handle_line(&line)
    }));
    // Codex logs to stderr; drain it so a full pipe can never block the process.
    tokio::spawn(drain(stderr));

    let kill_timer: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::default();
    exit.on_exit({
        let (rpc, kill_timer, input) = (
            Arc::clone(&rpc),
            Arc::clone(&kill_timer),
            Arc::clone(&input),
        );
        move |exit| {
            if let Some(timer) = kill_timer.lock().expect("kill timer lock").take() {
                timer.abort();
            }
            // Later writes fail at once, as Node's EPIPE does, instead of waiting out their timeout.
            input.lock().expect("stdin lock").take();
            rpc.fail_all(ProviderError::Failed(match exit {
                CliExit::Exited { .. } => "codex app-server exited".into(),
                CliExit::Failed(error) => error.clone(),
            }));
        }
    });
    let close: Arc<dyn Fn() + Send + Sync> = Arc::new({
        let (exit, kill_tree, kill_timer) = (
            exit.clone(),
            Arc::clone(&kill_tree),
            Arc::clone(&kill_timer),
        );
        move || {
            let mut timer = kill_timer.lock().expect("kill timer lock");
            if exit.status().is_some() || timer.is_some() {
                return;
            }
            input.lock().expect("stdin lock").take(); // child.stdin.end()
            let (exit, kill_tree, kill_timer) = (
                exit.clone(),
                Arc::clone(&kill_tree),
                Arc::clone(&kill_timer),
            );
            *timer = Some(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                kill_timer.lock().expect("kill timer lock").take();
                if exit.status().is_none() {
                    let _ = kill_tree().await;
                }
            }));
        }
    });

    let client_info =
        json!({ "clientInfo": { "name": "tjommie", "title": "Tjommie", "version": "0.1.0" } });
    if let Err(error) = rpc
        .request_with_timeout("initialize", client_info, Duration::from_secs(30))
        .await
    {
        close();
        exit.wait().await;
        return Err(error);
    }
    rpc.notify("initialized", None);
    Ok(CodexServer {
        rpc,
        pid,
        exit,
        kill_tree,
        close,
    })
}

async fn write_lines(
    mut stdin: Box<dyn AsyncWrite + Send + Unpin>,
    mut lines: mpsc::UnboundedReceiver<String>,
    rpc: Weak<JsonRpcLines>,
) {
    while let Some(line) = lines.recv().await {
        if let Err(error) = stdin.write_all(line.as_bytes()).await {
            if let Some(rpc) = rpc.upgrade() {
                rpc.fail_all(error.into());
            }
            return;
        }
    }
    let _ = stdin.shutdown().await;
}

async fn drain(mut stderr: Box<dyn AsyncRead + Send + Unpin>) {
    let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
}
