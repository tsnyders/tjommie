//! `vault-host-broker.ts`: the daemon's side of `/vault-host`. `vault.*` work queues here and the desktop's protected
//! Logins host takes it over the long poll with its own token. A job is a strict command: no secret, no username, no
//! bot record. Urgent work (`lock`, `disconnect`, `cancel`) goes on the control lane and cancels the normal work it
//! supersedes; normal work goes one job at a time, and only while nothing urgent or unanswered is outstanding.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use protocol::vault::{VaultHostAction, VaultHostCommand};
use protocol::vault_host::{
    VAULT_HOST_BODY_BYTES, VaultHostJob, VaultHostReply, vault_host_result,
};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::host_poll::{self, Polls};
use crate::ids::new_id;
use crate::server::Route;

const MAX_PENDING: usize = 64;
const MAX_SEQUENCE: u64 = (1 << 53) - 1;

/// Fixed messages only: nothing from the host or the command ever reaches one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    Invalid,
    Unavailable,
    Cancelled,
    Expired,
    HostFailed,
    InvalidResponse,
}

impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            VaultError::Invalid => "Invalid Logins command.",
            VaultError::Unavailable => "Logins needs its protected desktop host. Open or restart Tjommie and try again.",
            VaultError::Cancelled => {
                "This Logins operation was cancelled. Any private browser remains private until its fields are cleared or it is closed."
            }
            VaultError::Expired => {
                "The Logins request expired. Check the host before retrying; any private browser remains private."
            }
            VaultError::HostFailed => "The protected desktop host could not complete this Logins operation.",
            VaultError::InvalidResponse => "The desktop returned an invalid Logins response.",
        })
    }
}

impl std::error::Error for VaultError {}

#[derive(Clone, Default)]
pub struct VaultRequestOptions {
    /// Cancels unsent normal work when it fires.
    pub signal: Option<CancellationToken>,
    /// Re-checked before the job is sent and when its result arrives.
    pub authorized: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// `VaultHostRequest`: takes a raw command, so a forged one is refused here rather than by the type system alone.
pub type VaultRequest = Arc<
    dyn Fn(Value, VaultRequestOptions) -> BoxFuture<'static, Result<Value, VaultError>>
        + Send
        + Sync,
>;

type Reply = oneshot::Sender<Result<Value, VaultError>>;

struct Pending {
    job: VaultHostJob,
    urgent: bool,
    sent: bool,
    /// A repeated urgent request joins the first one's answer.
    replies: Vec<Reply>,
    authorized: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Ends the job's watcher once it is settled.
    settled: CancellationToken,
}

impl Pending {
    fn authorized(&self) -> bool {
        self.authorized
            .as_ref()
            .is_none_or(|authorized| authorized())
    }

    fn settle(self, result: Result<Value, VaultError>) {
        self.settled.cancel();
        for reply in self.replies {
            let _ = reply.send(result.clone());
        }
    }
}

#[derive(Default)]
struct State {
    pending: Vec<Pending>,
    polls: Polls,
    closed: bool,
    sequence: u64,
}

impl State {
    fn take(&mut self, id: &str) -> Option<Pending> {
        let index = self.pending.iter().position(|item| item.job.id == id)?;
        Some(self.pending.remove(index))
    }
}

fn urgent(action: &VaultHostAction) -> bool {
    matches!(
        action,
        VaultHostAction::Lock | VaultHostAction::Disconnect | VaultHostAction::Cancel
    )
}

fn kind(action: &VaultHostAction) -> std::mem::Discriminant<VaultHostAction> {
    std::mem::discriminant(action)
}

pub struct VaultHostBroker {
    secret: String,
    timeout: Duration,
    state: Mutex<State>,
}

impl VaultHostBroker {
    /// `secret` is the desktop host token; the daemon's ordinary token never opens this route.
    pub fn new(secret: impl Into<String>, timeout: Duration) -> Arc<VaultHostBroker> {
        Arc::new(VaultHostBroker {
            secret: secret.into(),
            timeout,
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn requester(self: &Arc<Self>) -> VaultRequest {
        let broker = self.clone();
        Arc::new(move |command, options| {
            let broker = broker.clone();
            Box::pin(async move { broker.request(command, options).await })
        })
    }

    pub async fn request(
        self: &Arc<Self>,
        raw: Value,
        options: VaultRequestOptions,
    ) -> Result<Value, VaultError> {
        let command: VaultHostCommand = protocol::parse(raw).map_err(|_| VaultError::Invalid)?;
        let answer = self.enqueue(command, options)?;
        self.dispatch();
        answer.await.unwrap_or(Err(VaultError::Unavailable))
    }

    fn enqueue(
        self: &Arc<Self>,
        command: VaultHostCommand,
        options: VaultRequestOptions,
    ) -> Result<oneshot::Receiver<Result<Value, VaultError>>, VaultError> {
        let is_urgent = urgent(&command.action);
        let (reply, answer) = oneshot::channel();
        let mut state = self.state();
        if state.closed || self.secret.is_empty() || state.sequence == MAX_SEQUENCE {
            return Err(VaultError::Unavailable);
        }
        let refused = options
            .signal
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
            || options
                .authorized
                .as_ref()
                .is_some_and(|authorized| !authorized());
        if !is_urgent && refused {
            return Err(VaultError::Cancelled);
        }
        let lock = matches!(command.action, VaultHostAction::Lock);
        if is_urgent {
            // A lock cancels every normal job; a disconnect or cancel only its own controller's.
            let superseded: Vec<String> = state
                .pending
                .iter()
                .filter(|item| {
                    !item.urgent
                        && (lock || item.job.command.controller_id == command.controller_id)
                })
                .map(|item| item.job.id.clone())
                .collect();
            for id in superseded {
                state
                    .take(&id)
                    .expect("listed above")
                    .settle(Err(VaultError::Cancelled));
            }
            let same = |item: &&mut Pending| {
                kind(&item.job.command.action) == kind(&command.action)
                    && (lock || item.job.command.controller_id == command.controller_id)
            };
            if let Some(existing) = state.pending.iter_mut().find(same) {
                existing.replies.push(reply);
                return Ok(answer);
            }
        }
        if state.pending.len() >= MAX_PENDING {
            return Err(VaultError::Unavailable);
        }
        state.sequence += 1;
        let sequence = state.sequence;
        let id = new_id();
        let settled = CancellationToken::new();
        let controller = command.controller_id.clone();
        state.pending.push(Pending {
            job: VaultHostJob {
                id: id.clone(),
                sequence,
                command,
            },
            urgent: is_urgent,
            sent: false,
            replies: vec![reply],
            authorized: options.authorized,
            settled: settled.clone(),
        });
        drop(state);
        let signal = options.signal.filter(|_| !is_urgent);
        tokio::spawn(
            self.clone()
                .watch(id, controller, is_urgent, signal, settled),
        );
        Ok(answer)
    }

    /// Expires the job, or cancels it when its caller's signal fires, unless it settles first.
    async fn watch(
        self: Arc<Self>,
        id: String,
        controller: String,
        is_urgent: bool,
        signal: Option<CancellationToken>,
        settled: CancellationToken,
    ) {
        let aborted = async {
            match &signal {
                Some(signal) => signal.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let error = tokio::select! {
            () = settled.cancelled() => return,
            () = tokio::time::sleep(self.timeout) => VaultError::Expired,
            () = aborted => VaultError::Cancelled,
        };
        let Some(item) = self.state().take(&id) else {
            return;
        };
        let sent = item.sent;
        item.settle(Err(error));
        // The host already started it: tell it to stop, so a private browser doesn't stay half-filled.
        if sent && !is_urgent {
            self.fire(&controller, "cancel");
        }
        self.dispatch();
    }

    /// Queues an urgent `cancel` or `disconnect` for a controller and forgets its answer.
    fn fire(self: &Arc<Self>, controller: &str, action: &str) {
        let broker = self.clone();
        let command = json!({ "controllerId": controller, "action": { "kind": action } });
        tokio::spawn(async move {
            let _ = broker
                .request(command, VaultRequestOptions::default())
                .await;
        });
    }

    fn dispatch(self: &Arc<Self>) {
        let mut disconnect = Vec::new();
        {
            let mut state = self.state();
            if state.closed {
                return;
            }
            let revoked: Vec<String> = state
                .pending
                .iter()
                .filter(|item| !item.urgent && !item.authorized())
                .map(|item| item.job.id.clone())
                .collect();
            for id in revoked {
                let item = state.take(&id).expect("listed above");
                if item.sent {
                    disconnect.push(item.job.command.controller_id.clone());
                }
                item.settle(Err(VaultError::Cancelled));
            }
            if let Some(index) = state
                .pending
                .iter()
                .position(|item| !item.sent && item.urgent)
            {
                Self::send(&mut state, index, true);
            }
            if !state.pending.iter().any(|item| item.urgent || item.sent)
                && let Some(index) = state.pending.iter().position(|item| !item.sent)
            {
                Self::send(&mut state, index, false);
            }
        }
        for controller in disconnect {
            self.fire(&controller, "disconnect");
        }
    }

    /// Hands a job to the lane's waiting GET. A GET whose caller left takes nothing.
    fn send(state: &mut State, index: usize, control: bool) {
        let Some(waiter) = state.polls.take(control) else {
            return;
        };
        let item = &mut state.pending[index];
        if waiter.send(host_poll::job(&item.job)).is_ok() {
            item.sent = true;
        }
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
        if parts.headers.contains_key(header::ORIGIN) {
            return StatusCode::FORBIDDEN.into_response();
        }
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
        let bytes = match host_poll::body(body, VAULT_HOST_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(refused) => return refused,
        };
        let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let Ok(reply) = protocol::parse::<VaultHostReply>(raw) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let item = {
            let mut state = self.state();
            match state
                .pending
                .iter()
                .position(|item| item.job.id == reply.id && item.sent)
            {
                Some(index) => state.pending.remove(index),
                None => return StatusCode::NOT_FOUND.into_response(),
            }
        };
        let result = if !item.urgent && !item.authorized() {
            Err(VaultError::Cancelled)
        } else if !reply.ok {
            Err(VaultError::HostFailed)
        } else {
            vault_host_result(
                &item.job.command.action,
                reply.result.unwrap_or(Value::Null),
            )
            .map_err(|_| VaultError::InvalidResponse)
        };
        item.settle(result);
        self.dispatch();
        StatusCode::NO_CONTENT.into_response()
    }

    pub fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        state.polls.close();
        for item in state.pending.drain(..) {
            item.settle(Err(VaultError::Unavailable));
        }
    }
}
