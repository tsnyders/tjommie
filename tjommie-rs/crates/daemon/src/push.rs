//! `push.ts`: turns `notify` events into content-free relay pushes (kind, bot id and room id only). Per device: only the
//! kinds it asked for and the relay supports, one push per kind and bot every 30 s, and at most 30 non-approval pushes
//! an hour. A push the relay couldn't take gives its budget back.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use protocol::relay::PushResult;
use protocol::remote::PushKind;
use protocol::wire::Notify;
use tokio::time::Instant;

use crate::devices_store::PushTarget;
use crate::relay_client::{RelayClient, RelayError};
use crate::remote_host::RemoteHost;
use crate::server::RpcError;

const WINDOW: Duration = Duration::from_secs(3600);
const COALESCE: Duration = Duration::from_secs(30);
const MAX_PER_WINDOW: usize = 30;
const BATCH: usize = 16;

/// Who can be pushed to: the remote host.
pub trait PushHost: Send + Sync {
    fn push_targets(&self) -> Vec<PushTarget>;
    fn clear_push_token(&self, device_id: &str, token: &str) -> Result<(), RpcError>;
}

/// What sends pushes: the relay client.
pub trait PushRelay: Send + Sync {
    fn kinds(&self) -> Vec<PushKind>;
    fn push(
        &self,
        tokens: Vec<String>,
        kind: PushKind,
        bot_id: String,
        room_id: Option<String>,
    ) -> BoxFuture<'static, Result<Vec<PushResult>, RelayError>>;
}

impl PushHost for RemoteHost {
    fn push_targets(&self) -> Vec<PushTarget> {
        RemoteHost::push_targets(self)
    }
    fn clear_push_token(&self, device_id: &str, token: &str) -> Result<(), RpcError> {
        RemoteHost::clear_push_token(self, device_id, token)
    }
}

impl PushRelay for RelayClient {
    fn kinds(&self) -> Vec<PushKind> {
        RelayClient::kinds(self)
    }
    fn push(
        &self,
        tokens: Vec<String>,
        kind: PushKind,
        bot_id: String,
        room_id: Option<String>,
    ) -> BoxFuture<'static, Result<Vec<PushResult>, RelayError>> {
        let relay = self.clone();
        Box::pin(async move {
            RelayClient::push(&relay, tokens, kind, &bot_id, room_id.as_deref()).await
        })
    }
}

struct Stamp {
    id: u64,
    at: Instant,
    kind: PushKind,
    bot_id: String,
}

#[derive(Default)]
struct History {
    next: u64,
    devices: HashMap<String, Vec<Stamp>>,
}

/// `startPush`. Cheap to clone.
#[derive(Clone)]
pub struct Push {
    host: Arc<dyn PushHost>,
    relay: Arc<dyn PushRelay>,
    history: Arc<Mutex<History>>,
    failures: Arc<AtomicU64>,
}

/// The routing fields of a notification; its title and body never leave the PC.
fn route(notify: &Notify) -> (PushKind, &str, Option<&str>) {
    match notify {
        Notify::Approval { bot_id, .. } => (PushKind::Approval, bot_id, None),
        Notify::Limit { bot_id, .. } => (PushKind::Limit, bot_id, None),
        Notify::RoutineFailed { bot_id, .. } => (PushKind::RoutineFailed, bot_id, None),
        Notify::Done {
            bot_id, room_id, ..
        } => (PushKind::Done, bot_id, room_id.as_deref()),
        Notify::Failed {
            bot_id, room_id, ..
        } => (PushKind::Failed, bot_id, room_id.as_deref()),
    }
}

impl Push {
    pub fn new(host: Arc<dyn PushHost>, relay: Arc<dyn PushRelay>) -> Push {
        Push {
            host,
            relay,
            history: Arc::default(),
            failures: Arc::default(),
        }
    }

    /// Pushes that failed or came back unaccepted. Never their content.
    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// Hands the eligible devices to the relay now; their results are handled in the background.
    pub fn send(&self, notify: &Notify) {
        let (kind, bot_id, room_id) = route(notify);
        if !self.relay.kinds().contains(&kind) {
            return;
        }
        let now = Instant::now();
        let mut eligible: Vec<(PushTarget, u64)> = Vec::new();
        {
            let mut history = self.history.lock().unwrap();
            for target in self.host.push_targets() {
                if target
                    .kinds
                    .as_ref()
                    .is_some_and(|kinds| !kinds.contains(&kind))
                {
                    continue;
                }
                let mut recent: Vec<Stamp> = history
                    .devices
                    .remove(&target.device_id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|stamp| now.duration_since(stamp.at) < WINDOW)
                    .collect();
                let capped = kind != PushKind::Approval
                    && recent
                        .iter()
                        .filter(|stamp| stamp.kind != PushKind::Approval)
                        .count()
                        >= MAX_PER_WINDOW;
                let repeated = recent.iter().any(|stamp| {
                    stamp.kind == kind
                        && stamp.bot_id == bot_id
                        && now.duration_since(stamp.at) < COALESCE
                });
                if !capped && !repeated {
                    history.next += 1;
                    let id = history.next;
                    recent.push(Stamp {
                        id,
                        at: now,
                        kind,
                        bot_id: bot_id.to_owned(),
                    });
                    eligible.push((target.clone(), id));
                }
                history.devices.insert(target.device_id, recent);
            }
        }
        for batch in eligible.chunks(BATCH) {
            let batch = batch.to_vec();
            let tokens = batch
                .iter()
                .map(|(target, _)| target.token.clone())
                .collect();
            let sending =
                self.relay
                    .push(tokens, kind, bot_id.to_owned(), room_id.map(str::to_owned));
            let push = self.clone();
            tokio::spawn(async move {
                match sending.await {
                    Ok(results) if results.len() == batch.len() => {
                        for (result, (target, _)) in results.iter().zip(&batch) {
                            if !result.ok {
                                push.failures.fetch_add(1, Ordering::Relaxed);
                            }
                            if result.unregistered
                                && push
                                    .host
                                    .clear_push_token(&target.device_id, &target.token)
                                    .is_err()
                            {
                                push.failures.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Ok(_) => {
                        push.failures
                            .fetch_add(batch.len() as u64, Ordering::Relaxed);
                    }
                    Err(_) => {
                        push.failures
                            .fetch_add(batch.len() as u64, Ordering::Relaxed);
                        let mut history = push.history.lock().unwrap();
                        for (target, id) in &batch {
                            if let Some(stamps) = history.devices.get_mut(&target.device_id) {
                                stamps.retain(|stamp| stamp.id != *id);
                            }
                        }
                    }
                }
            });
        }
    }
}
