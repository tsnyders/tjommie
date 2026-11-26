//! `worker-isolation.ts`: `RemoteWorkerIsolation`, the desktop's client of the daemon's private `/worker-host` route. It asks
//! whether the Windows worker boundary is available, and takes short leases while isolated work runs. A lease is renewed
//! every second. It stops asserting the moment a renewal fails, comes back from another daemon lifetime, or runs past its
//! time, and it never revives.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use reqwest::header::CONTENT_TYPE;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use super::host::HostToken;

const UNAVAILABLE: &str = "Logins needs the verified Windows worker account and a protected connection to its provider host.";
/// `AbortSignal.timeout(8000)` in the TS: status, acquire and release.
const CALL_TIMEOUT: Duration = Duration::from_secs(8);
/// `AbortSignal.timeout(3000)` in the TS: a renewal must come back quickly or the lease is retired.
const RENEW_TIMEOUT: Duration = Duration::from_secs(3);
const HEARTBEAT: Duration = Duration::from_secs(1);
const MIN_EXPIRY_MS: u64 = 500;
const MAX_EXPIRY_MS: u64 = 5000;

/// The isolation is missing or lost. Its text says what Logins needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsolationError;

impl fmt::Display for IsolationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(UNAVAILABLE)
    }
}

impl std::error::Error for IsolationError {}

/// The isolation Logins runs its private work in (`VaultIsolation`).
pub trait VaultIsolation: Send + Sync {
    /// `None` when isolated work can run, else why not.
    fn unavailable_reason(&self) -> BoxFuture<'_, Option<String>>;
    fn acquire(&self) -> BoxFuture<'_, Result<Box<dyn VaultIsolationLease>, IsolationError>>;
}

/// A lease on the isolation. It stops asserting once it lapses, is released, or the isolation is closed.
pub trait VaultIsolationLease: Send + Sync {
    fn assert(&self) -> Result<(), IsolationError>;
    fn release(&self);
}

#[derive(Serialize)]
struct Command<'a> {
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusReply {
    lifetime: String,
    available: bool,
    /// On the wire, not shown: the app says what is unavailable in its own words.
    #[allow(dead_code)]
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseReply {
    lifetime: String,
    id: String,
    #[serde(rename = "expiresInMs")]
    expires_in_ms: u64,
}

fn valid_uuid(value: &str) -> bool {
    Uuid::try_parse(value).is_ok()
}

fn lease_ok(reply: &LeaseReply) -> bool {
    valid_uuid(&reply.lifetime)
        && valid_uuid(&reply.id)
        && (MIN_EXPIRY_MS..=MAX_EXPIRY_MS).contains(&reply.expires_in_ms)
}

/// The HTTP side, shared by the isolation and its leases.
struct Link {
    client: reqwest::Client,
    url: String,
    token: HostToken,
    closed: AtomicBool,
}

impl Link {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// One command to `/worker-host`. Every failure is the same unavailable error, and none of the body is kept.
    async fn call<T: DeserializeOwned>(
        &self,
        kind: &str,
        id: Option<&str>,
        limit: Duration,
    ) -> Result<T, IsolationError> {
        if self.closed() {
            return Err(IsolationError);
        }
        let body = serde_json::to_vec(&Command { kind, id }).map_err(|_| IsolationError)?;
        let response = self
            .client
            .post(self.url.as_str())
            .timeout(limit)
            .bearer_auth(self.token.expose())
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| IsolationError)?;
        if !response.status().is_success() {
            return Err(IsolationError);
        }
        let bytes = response.bytes().await.map_err(|_| IsolationError)?;
        serde_json::from_slice(&bytes).map_err(|_| IsolationError)
    }
}

/// What a held lease knows of itself.
struct Held {
    lifetime: String,
    id: String,
    expires: Instant,
    active: bool,
}

impl Held {
    /// Still held, unexpired, and the isolation is open.
    fn live(&self, closed: bool) -> bool {
        self.active && !closed && Instant::now() < self.expires
    }
}

/// The desktop's client of `/worker-host` (`RemoteWorkerIsolation`).
pub struct RemoteWorkerIsolation {
    link: Arc<Link>,
}

impl RemoteWorkerIsolation {
    /// `port` is the daemon's, `token` the desktop host token. A port of 0 or an empty token is refused.
    pub fn new(port: u16, token: HostToken) -> Result<RemoteWorkerIsolation, IsolationError> {
        if port == 0 || token.is_empty() {
            return Err(IsolationError);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| IsolationError)?;
        Ok(RemoteWorkerIsolation {
            link: Arc::new(Link {
                client,
                url: format!("http://127.0.0.1:{port}/worker-host"),
                token,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Closes the isolation: every lease stops asserting now, and no call goes out again.
    pub fn close(&self) {
        self.link.closed.store(true, Ordering::SeqCst);
    }
}

impl VaultIsolation for RemoteWorkerIsolation {
    fn unavailable_reason(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            let link = &*self.link;
            match link.call::<StatusReply>("status", None, CALL_TIMEOUT).await {
                Ok(status)
                    if status.available && valid_uuid(&status.lifetime) && !link.closed() =>
                {
                    None
                }
                _ => Some(UNAVAILABLE.to_string()),
            }
        })
    }

    fn acquire(&self) -> BoxFuture<'_, Result<Box<dyn VaultIsolationLease>, IsolationError>> {
        Box::pin(async move {
            let link = self.link.clone();
            let reply: LeaseReply = link.call("acquire", None, CALL_TIMEOUT).await?;
            if !lease_ok(&reply) || link.closed() {
                return Err(IsolationError);
            }
            let held = Arc::new(Mutex::new(Held {
                expires: Instant::now() + Duration::from_millis(reply.expires_in_ms),
                lifetime: reply.lifetime,
                id: reply.id,
                active: true,
            }));
            tokio::spawn(heartbeat(link.clone(), held.clone()));
            let lease: Box<dyn VaultIsolationLease> = Box::new(Lease { link, held });
            Ok(lease)
        })
    }
}

/// Renews the lease once a second while it is live. A renewal that does not check out, or that comes back late, retires
/// the lease for good: a later renewal can never revive it.
async fn heartbeat(link: Arc<Link>, held: Arc<Mutex<Held>>) {
    let mut beat = tokio::time::interval(HEARTBEAT);
    beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first tick of an interval is immediate; the first renewal is due a second after the acquire.
    beat.tick().await;
    loop {
        beat.tick().await;
        let (lifetime, id) = {
            let mut state = held.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.live(link.closed()) {
                state.active = false;
                return;
            }
            (state.lifetime.clone(), state.id.clone())
        };
        let renewed = link
            .call::<LeaseReply>("renew", Some(&id), RENEW_TIMEOUT)
            .await;
        let mut state = held.lock().unwrap_or_else(PoisonError::into_inner);
        match renewed {
            Ok(updated)
                if lease_ok(&updated)
                    && updated.lifetime == lifetime
                    && updated.id == id
                    && state.live(link.closed()) =>
            {
                state.expires = Instant::now() + Duration::from_millis(updated.expires_in_ms);
            }
            _ => {
                state.active = false;
                return;
            }
        }
    }
}

/// A lease the isolation handed out. Releasing it, or letting it lapse, ends its heartbeat.
struct Lease {
    link: Arc<Link>,
    held: Arc<Mutex<Held>>,
}

impl VaultIsolationLease for Lease {
    fn assert(&self) -> Result<(), IsolationError> {
        let mut state = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if state.live(self.link.closed()) {
            return Ok(());
        }
        state.active = false;
        Err(IsolationError)
    }

    fn release(&self) {
        let id = {
            let mut state = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.active {
                return;
            }
            state.active = false;
            state.id.clone()
        };
        // Released with no runtime to send from: the daemon lapses the lease on its own, after its 10 s.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let link = self.link.clone();
            runtime.spawn(async move {
                let _ = link
                    .call::<IgnoredAny>("release", Some(&id), CALL_TIMEOUT)
                    .await;
            });
        }
    }
}
