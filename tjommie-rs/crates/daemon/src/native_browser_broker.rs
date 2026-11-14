//! `native-browser-broker.ts`: the daemon's side of `/browser-host`. Bots' browser jobs queue here and only the desktop
//! main process takes them, over the long poll with its private host token; jobs and page results never pass through
//! the renderer's event stream or the chat log. Urgent work (`cancel_tools`, a remote `control` or `disconnect`) goes on
//! the control lane; otherwise a bot has at most one job in flight.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use protocol::native_browser::NativeBrowserStreamMessage;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::host_poll::{self, Polls};
use crate::ids::new_id;
use crate::server::Route;

const MAX_PENDING: usize = 64;
const BODY_BYTES: usize = 8 * 1024 * 1024;
const ERROR_CHARS: usize = 2000;
/// `NativeBrowserBroker`'s default wait for the desktop.
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(45);

/// Re-checked before a job is sent and when its result arrives.
pub type Authorized = Arc<dyn Fn() -> bool + Send + Sync>;

/// Takes what the desktop posts on its own (live frames, the end of a stream). False means nobody is listening for it.
pub type StreamSink = Arc<dyn Fn(NativeBrowserStreamMessage) -> bool + Send + Sync>;

/// What the desktop needs to find the bot's browser profile and folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotIdentity {
    pub id: String,
    pub name: String,
    pub home_folder: String,
    /// Present when the stored bot has the field (as `null` or a folder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_folder: Option<Option<String>>,
}

impl From<&crate::bots::StoredBot> for BotIdentity {
    fn from(bot: &crate::bots::StoredBot) -> Self {
        BotIdentity {
            id: bot.id.clone(),
            name: bot.name.clone(),
            home_folder: bot.home_folder.clone(),
            project_folder: Some(bot.project_folder.clone()),
        }
    }
}

/// The browser's own words: a page error from the desktop, or one of the broker's fixed messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserError(pub String);

impl BrowserError {
    pub fn unavailable() -> Self {
        BrowserError("The native browser needs the Tjommie desktop app. Open or restart Tjommie and try again.".into())
    }
}

impl fmt::Display for BrowserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BrowserError {}

struct Pending {
    id: String,
    bot_id: String,
    command: Value,
    job: Value,
    sent: bool,
    /// The hand-off that set `sent`: only that hand-off's body, if dropped unwritten, may give the job back.
    handoff: u64,
    reply: oneshot::Sender<Result<Value, BrowserError>>,
    authorized: Option<Authorized>,
    settled: CancellationToken,
}

impl Pending {
    fn authorized(&self) -> bool {
        self.authorized
            .as_ref()
            .is_none_or(|authorized| authorized())
    }

    fn settle(self, result: Result<Value, BrowserError>) {
        self.settled.cancel();
        let _ = self.reply.send(result);
    }

    fn urgent(&self) -> bool {
        urgent(&self.command)
    }
}

/// `cancel_tools`, or a remote `control` or `disconnect`.
fn urgent(command: &Value) -> bool {
    match command["kind"].as_str() {
        Some("cancel_tools") => true,
        Some("remote") => matches!(
            command["action"]["kind"].as_str(),
            Some("control" | "disconnect")
        ),
        _ => false,
    }
}

#[derive(Default)]
struct State {
    pending: Vec<Pending>,
    polls: Polls,
    closed: bool,
    /// Where live frames go. None until the daemon installs one: then the desktop is told nobody is watching.
    stream: Option<StreamSink>,
    /// Hand-offs made so far; each gets its own number.
    handoffs: u64,
}

impl State {
    fn take(&mut self, id: &str) -> Option<Pending> {
        let index = self.pending.iter().position(|item| item.id == id)?;
        Some(self.pending.remove(index))
    }

    /// Settles every pending job that `matches`, with `error`.
    fn reject(&mut self, matches: impl Fn(&Pending) -> bool, error: &str) {
        let (rejected, kept) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|item| matches(item));
        self.pending = kept;
        for item in rejected {
            item.settle(Err(BrowserError(error.into())));
        }
    }
}

pub struct NativeBrowserBroker {
    secret: String,
    timeout: Duration,
    state: Mutex<State>,
}

impl NativeBrowserBroker {
    /// `secret` is the desktop host token, never the renderer's daemon token.
    pub fn new(secret: impl Into<String>, timeout: Duration) -> Arc<NativeBrowserBroker> {
        Arc::new(NativeBrowserBroker {
            secret: secret.into(),
            timeout,
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub async fn request(
        self: &Arc<Self>,
        bot: BotIdentity,
        command: Value,
        authorized: Option<Authorized>,
    ) -> Result<Value, BrowserError> {
        let (reply, answer) = oneshot::channel();
        let id = new_id();
        let settled = CancellationToken::new();
        {
            let mut state = self.state();
            if state.closed || self.secret.is_empty() || state.pending.len() >= MAX_PENDING {
                return Err(BrowserError::unavailable());
            }
            let job = json!({ "id": id, "bot": bot, "command": command });
            state.pending.push(Pending {
                id: id.clone(),
                bot_id: bot.id,
                command,
                job,
                sent: false,
                handoff: 0,
                reply,
                authorized,
                settled: settled.clone(),
            });
        }
        let broker = self.clone();
        let timeout = self.timeout;
        let expiring = id.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = settled.cancelled() => return,
                () = tokio::time::sleep(timeout) => {}
            }
            let Some(item) = broker.state().take(&expiring) else {
                return;
            };
            let message = if item.sent {
                "The native browser command timed out; its outcome is unknown. Check the page before retrying."
            } else {
                "The native browser command was not started because the desktop host did not become available in time."
            };
            item.settle(Err(BrowserError(message.into())));
            broker.dispatch();
        });
        self.dispatch();
        answer
            .await
            .unwrap_or_else(|_| Err(BrowserError::unavailable()))
    }

    fn dispatch(self: &Arc<Self>) {
        let mut state = self.state();
        if !state.polls.waiting(false) && !state.polls.waiting(true) {
            return;
        }
        state.reject(
            |item| !item.sent && !item.authorized(),
            "Browser session is no longer authorized.",
        );
        if let Some(index) = state
            .pending
            .iter()
            .position(|item| !item.sent && item.urgent())
        {
            self.send(&mut state, index, true);
        }
        let ready = state.pending.iter().position(|item| {
            !item.sent
                && !item.urgent()
                && !state.pending.iter().any(|other| {
                    other.id != item.id
                        && other.bot_id == item.bot_id
                        && (other.sent || other.urgent())
                })
        });
        if let Some(index) = ready {
            self.send(&mut state, index, false);
        }
    }

    /// Hands a job to the lane's waiting GET. A GET whose caller left takes nothing. If the connection then drops the
    /// response before it was written, the job goes back to the next poll (`lost` below).
    fn send(self: &Arc<Self>, state: &mut State, index: usize, control: bool) {
        let Some(waiter) = state.polls.take(control) else {
            return;
        };
        state.handoffs += 1;
        let handoff = state.handoffs;
        let item = &mut state.pending[index];
        let lost = self.lost(item.id.clone(), handoff);
        if waiter.send(host_poll::job_or(&item.job, lost)).is_ok() {
            item.sent = true;
            item.handoff = handoff;
        }
    }

    /// What a dropped, unwritten response does. It can run under `state`'s lock (a response refused by a GET that just
    /// left is dropped there), so it only schedules the work.
    fn lost(self: &Arc<Self>, id: String, handoff: u64) -> Box<dyn FnOnce() + Send + Sync> {
        let (broker, runtime) = (self.clone(), tokio::runtime::Handle::current());
        Box::new(move || {
            runtime.spawn(async move { broker.requeue(&id, handoff) });
        })
    }

    /// Gives a job back to the next poll, if `handoff` is still the one it is out on. A job already answered or expired is
    /// gone, and one handed on again since carries a newer number: both stay as they are.
    fn requeue(self: &Arc<Self>, id: &str, handoff: u64) {
        {
            let mut state = self.state();
            let Some(item) = state
                .pending
                .iter_mut()
                .find(|item| item.id == id && item.sent && item.handoff == handoff)
            else {
                return;
            };
            item.sent = false;
        }
        self.dispatch();
    }

    pub fn route(self: &Arc<Self>) -> Route {
        let broker = self.clone();
        Arc::new(move |request| {
            let broker = broker.clone();
            Box::pin(async move { broker.handle(request).await })
        })
    }

    async fn handle(self: Arc<Self>, request: axum::extract::Request) -> Response {
        let (parts, body) = request.into_parts();
        if !host_poll::bearer(&parts.headers, &self.secret) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if self.state().closed {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if parts.method == Method::GET {
            let waiting = self.state().polls.wait(host_poll::is_control(&parts.uri));
            self.dispatch();
            return host_poll::answer(waiting).await;
        }
        if parts.method != Method::POST {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let bytes = match host_poll::body(body, BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(refused) => return refused,
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        // A post with a `kind` is the desktop's own (a live frame, the end of a stream), not the answer to a job.
        if value.get("kind").is_some() {
            return self.stream_message(value);
        }
        let (Some(id), Some(ok)) = (
            value.get("id").and_then(Value::as_str),
            value.get("ok").and_then(Value::as_bool),
        ) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let item = {
            let mut state = self.state();
            match state
                .pending
                .iter()
                .position(|item| item.id == id && item.sent)
            {
                Some(index) => state.pending.remove(index),
                None => return StatusCode::NOT_FOUND.into_response(),
            }
        };
        self.dispatch();
        let result = if !item.authorized() {
            Err(BrowserError(
                "Browser session is no longer authorized; the result was discarded.".into(),
            ))
        } else if ok {
            Ok(value.get("result").cloned().unwrap_or(Value::Null))
        } else {
            let message = match value.get("error").and_then(Value::as_str) {
                Some(error) => error.chars().take(ERROR_CHARS).collect(),
                None => "The native browser command failed.".into(),
            };
            Err(BrowserError(message))
        };
        item.settle(result);
        StatusCode::NO_CONTENT.into_response()
    }

    /// Where the desktop's live frames and stream ends go.
    pub fn on_stream(&self, sink: StreamSink) {
        self.state().stream = Some(sink);
    }

    /// 204 when taken, 404 when nobody is watching (the desktop stops sending), 400 for anything malformed.
    fn stream_message(&self, value: Value) -> Response {
        let Ok(message) = protocol::parse::<NativeBrowserStreamMessage>(value) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let sink = self.state().stream.clone();
        if sink.is_some_and(|sink| sink(message)) {
            StatusCode::NO_CONTENT.into_response()
        } else {
            StatusCode::NOT_FOUND.into_response()
        }
    }

    /// Ends every job of a bot whose browser closed.
    pub fn close_bot(&self, bot_id: &str) {
        self.state().reject(
            |item| item.bot_id == bot_id,
            "This agent's native browser closed.",
        );
    }

    /// Ends a bot's tool calls (its MCP session closed); remote viewing goes on.
    pub fn cancel_tools(&self, bot_id: &str) {
        self.state().reject(
            |item| item.bot_id == bot_id && item.command["kind"] == "tool",
            "The browser tool session closed.",
        );
    }

    pub fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        state.polls.close();
        state.reject(|_| true, &BrowserError::unavailable().0);
    }
}
