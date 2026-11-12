//! `worker-boundary-host.ts`: `/worker-host`, the desktop's lifetime channel for the Windows worker boundary. With its
//! private host token the desktop asks whether isolation is available, and takes short leases (renewed every 5 s,
//! released after 10 s without a renewal) while it runs isolated work. A renewal re-checks the boundary, and a lease
//! that fails the check is retired at once. Every refusal is a bare 503.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use crate::host_poll;
use crate::ids::new_id;
use crate::server::Route;

const MAX_LEASES: usize = 8;
const BODY_BYTES: usize = 1024;
const LEASE_LAPSE: Duration = Duration::from_secs(10);
const RENEW_EVERY_MS: u64 = 5000;

/// The worker isolation itself: lane B's `WindowsWorkerBoundary` in the daemon.
pub trait WorkerBoundary: Send + Sync {
    /// `None` when isolated work can run, else why not.
    fn unavailable_reason(&self) -> BoxFuture<'static, Option<String>>;
    /// A lease, or `None` when isolation is unavailable.
    fn acquire(&self) -> BoxFuture<'static, Option<Box<dyn WorkerLease>>>;
    /// Every lease taken so far stops asserting.
    fn invalidate(&self);
}

pub trait WorkerLease: Send + Sync {
    /// Still backed by a live boundary (`assert()` in TypeScript, which throws instead).
    fn valid(&self) -> bool;
    fn release(&self);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
enum Kind {
    Status,
    Acquire,
    Renew,
    Release,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    kind: Kind,
    id: Option<String>,
}

struct Held {
    lease: Box<dyn WorkerLease>,
    /// Bumped by each renewal, so an older lapse timer leaves the lease alone.
    renewals: u64,
}

#[derive(Default)]
struct State {
    leases: HashMap<String, Held>,
    acquiring: usize,
}

pub struct WorkerBoundaryHost {
    boundary: Arc<dyn WorkerBoundary>,
    token: String,
    /// Changes when the daemon restarts, so the desktop can tell its leases are gone.
    lifetime: String,
    state: Mutex<State>,
    closed: AtomicBool,
}

/// Counts an acquisition in flight until it's dropped.
struct Acquiring<'a>(&'a WorkerBoundaryHost);

impl Drop for Acquiring<'_> {
    fn drop(&mut self) {
        self.0.state().acquiring -= 1;
    }
}

impl WorkerBoundaryHost {
    /// `token` is the desktop host token.
    pub fn new(
        boundary: Arc<dyn WorkerBoundary>,
        token: impl Into<String>,
    ) -> Arc<WorkerBoundaryHost> {
        Arc::new(WorkerBoundaryHost {
            boundary,
            token: token.into(),
            lifetime: new_id(),
            state: Mutex::default(),
            closed: AtomicBool::new(false),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn release(&self, id: &str) {
        let held = self.state().leases.remove(id);
        if let Some(held) = held {
            held.lease.release();
        }
    }

    /// Releases the lease unless it's renewed again within 10 s.
    fn lapse(self: &Arc<Self>, id: String, renewals: u64) {
        let host = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(LEASE_LAPSE).await;
            let Some(host) = host.upgrade() else { return };
            let current = host
                .state()
                .leases
                .get(&id)
                .is_some_and(|held| held.renewals == renewals);
            if current {
                host.release(&id);
            }
        });
    }

    pub fn route(self: &Arc<Self>) -> Route {
        let host = self.clone();
        Arc::new(move |request| {
            let host = host.clone();
            Box::pin(async move { host.handle(request).await })
        })
    }

    async fn handle(self: Arc<Self>, request: axum::extract::Request) -> Response {
        let (parts, body) = request.into_parts();
        if !host_poll::bearer(&parts.headers, &self.token) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if self.closed() {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if parts.method != Method::POST {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let bytes = match host_poll::body(body, BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(refused) => return refused,
        };
        match self.answer(&bytes).await {
            Some(value) => host_poll::job(&value),
            None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }

    /// The answer to one command, or `None` for a 503.
    async fn answer(self: &Arc<Self>, bytes: &[u8]) -> Option<serde_json::Value> {
        let command: Command = serde_json::from_slice(bytes).ok()?;
        if command
            .id
            .as_deref()
            .is_some_and(|id| uuid::Uuid::try_parse(id).is_err())
        {
            return None;
        }
        let lifetime = &self.lifetime;
        match command.kind {
            Kind::Status => {
                let reason = self.boundary.unavailable_reason().await;
                if self.closed() {
                    return None;
                }
                Some(
                    json!({ "lifetime": lifetime, "available": reason.is_none(), "reason": reason }),
                )
            }
            Kind::Release => {
                if let Some(id) = &command.id {
                    self.release(id);
                }
                Some(json!({ "lifetime": lifetime }))
            }
            Kind::Renew => {
                let id = command.id?;
                let renewals = self.state().leases.get(&id).map(|held| held.renewals);
                let healthy =
                    match renewals {
                        Some(renewals) => {
                            self.boundary.unavailable_reason().await.is_none()
                                && !self.closed()
                                && self.state().leases.get(&id).is_some_and(|held| {
                                    held.renewals == renewals && held.lease.valid()
                                })
                        }
                        None => false,
                    };
                if !healthy {
                    self.release(&id);
                    return None;
                }
                let renewals = {
                    let mut state = self.state();
                    let held = state.leases.get_mut(&id)?;
                    held.renewals += 1;
                    held.renewals
                };
                self.lapse(id.clone(), renewals);
                Some(json!({ "lifetime": lifetime, "id": id, "expiresInMs": RENEW_EVERY_MS }))
            }
            Kind::Acquire => {
                {
                    let mut state = self.state();
                    if state.leases.len() + state.acquiring >= MAX_LEASES {
                        return None;
                    }
                    state.acquiring += 1;
                }
                let _acquiring = Acquiring(self);
                let lease = self.boundary.acquire().await?;
                if self.closed() || !lease.valid() {
                    lease.release();
                    return None;
                }
                let id = new_id();
                self.state()
                    .leases
                    .insert(id.clone(), Held { lease, renewals: 0 });
                self.lapse(id.clone(), 0);
                Some(json!({ "lifetime": lifetime, "id": id, "expiresInMs": RENEW_EVERY_MS }))
            }
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.boundary.invalidate();
        let leases: Vec<Held> = self.state().leases.drain().map(|(_, held)| held).collect();
        for held in leases {
            held.lease.release();
        }
    }
}
