//! `vault-host.ts`: the desktop's client of the daemon's private `/vault-host` route. The daemon queues the Logins jobs
//! (`vault.*`); this client long-polls for them with the desktop host token, on a normal lane and an urgent one
//! (`?control=1`, for lock, cancel and disconnect), runs each through an [`Executor`] and posts the answer back.
//!
//! The client lives on a thread and runtime of its own, as the browser bridge does (`browser/host_bridge.rs`). Only typed
//! jobs, typed replies and [`HostEvent`]s cross into the app; nothing here touches a window. Any failure to reach the
//! daemon, or to read a job, fails closed: the vault locks, and the client tries again after a second.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_channel::Sender;
use futures::future::BoxFuture;
use protocol::vault::{VaultHostAction, VaultHostCommand};
use protocol::vault_host::{VAULT_HOST_BODY_BYTES, VaultHostJob};
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::watch;
use zeroize::Zeroizing;

use super::gate::{GateError, VaultReply, is_urgent};

/// How long a posted answer may wait for the daemon. A daemon that takes the connection and never answers must not hold
/// a lane for good (the browser bridge's rule).
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a poll may wait. The daemon answers 204 after 15 s.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(1);
/// The largest job the client reads. The daemon's jobs are a few hundred bytes.
const JOB_BYTES: usize = 16 * 1024;
/// A cancel or disconnect bars older normal work from its controller for this long.
const BARRIER_TTL: Duration = Duration::from_secs(60);
const MAX_BARRIERS: usize = 2048;
/// How long closing waits for the lock to finish before it gives up on confirming it.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
const LOCAL: &str = "local";
const UNCONFIRMED_CLOSE: &str = "The protected Logins host could not confirm a safe close.";
const TOO_LARGE: &str = "The Logins answer was too large to send.";

/// The private desktop host token that opens `/vault-host` and `/worker-host`. It is wiped from memory when dropped, and
/// it is never printed: `Debug` shows only that it exists.
#[derive(Clone)]
pub struct HostToken(Zeroizing<String>);

impl HostToken {
    pub fn new(value: impl Into<String>) -> HostToken {
        HostToken(Zeroizing::new(value.into()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for HostToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HostToken(<redacted>)")
    }
}

/// Where the daemon is, and the desktop's token for it.
#[derive(Debug)]
pub struct HostConfig {
    pub port: u16,
    pub token: HostToken,
}

/// What the client tells the app. The client never waits for the app: a full or closed channel drops the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    /// The daemon could not be reached or refused a job, so the vault was locked.
    Failed,
}

/// Runs a job's command and returns its typed result. It may wait for the app's own thread; the client never blocks it.
pub trait Executor: Send + Sync + 'static {
    fn execute(&self, command: VaultHostCommand) -> BoxFuture<'_, Result<VaultReply, GateError>>;
}

/// Why a step of a lane failed. Logged at debug level only: a poll that fails repeats every second while the daemon is
/// down, and nothing a daemon or vault said is ever kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostError {
    Unavailable,
    Oversized,
    Malformed,
    Refused,
}

/// An answer on the wire: `{ id, ok: true, result }` or `{ id, ok: false }`.
#[derive(Serialize)]
struct Answer<'a> {
    id: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a VaultReply>,
}

#[derive(Clone, Copy)]
struct Barrier {
    sequence: u64,
    expires: Instant,
}

#[derive(Default)]
struct Fence {
    /// Bumped whenever the channel fails closed: a job polled before that is refused.
    channel_epoch: u64,
    /// The highest lock sequence seen: older normal work is refused.
    global: u64,
    /// Per controller, the newest cancel or disconnect sequence: older normal work from it is refused.
    barriers: HashMap<String, Barrier>,
}

enum Admit {
    Run,
    Stale,
    Overflow,
}

/// What both lanes share.
struct Shared {
    client: reqwest::Client,
    url: String,
    token: HostToken,
    executor: Arc<dyn Executor>,
    events: Sender<HostEvent>,
    fence: Mutex<Fence>,
}

impl Shared {
    fn fence(&self) -> MutexGuard<'_, Fence> {
        self.fence.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One long poll. `None` is the daemon's 204: nothing to do.
    async fn poll(&self, control: bool) -> Result<Option<Vec<u8>>, HostError> {
        let url = if control {
            format!("{}?control=1", self.url)
        } else {
            self.url.clone()
        };
        let mut response = self
            .client
            .get(url)
            .bearer_auth(self.token.expose())
            .timeout(POLL_TIMEOUT)
            .send()
            .await
            .map_err(|_| HostError::Unavailable)?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(HostError::Unavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| HostError::Unavailable)? {
            if body.len() + chunk.len() > JOB_BYTES {
                return Err(HostError::Oversized);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Some(body))
    }

    async fn post(&self, body: Vec<u8>) -> Result<StatusCode, reqwest::Error> {
        let response = self
            .client
            .post(self.url.as_str())
            .bearer_auth(self.token.expose())
            .header(CONTENT_TYPE, "application/json")
            .timeout(ANSWER_TIMEOUT)
            .body(body)
            .send()
            .await?;
        Ok(response.status())
    }

    /// Posts an answer. 204 and 404 are the daemon taking it or having no use for it (the job timed out there). Any
    /// other status means the answer was not taken, so the lane fails closed.
    async fn answer(&self, id: &str, reply: Option<&VaultReply>) -> Result<(), HostError> {
        let answer = Answer {
            id,
            ok: reply.is_some(),
            result: reply,
        };
        let body = serde_json::to_vec(&answer).map_err(|_| HostError::Refused)?;
        if body.len() > VAULT_HOST_BODY_BYTES {
            return self.too_large(id).await;
        }
        match self.post(body).await {
            Ok(status) if status == StatusCode::NO_CONTENT || status == StatusCode::NOT_FOUND => {
                Ok(())
            }
            Ok(status) if status == StatusCode::PAYLOAD_TOO_LARGE && reply.is_some() => {
                self.too_large(id).await
            }
            Ok(_) => Err(HostError::Refused),
            Err(error) if error.is_timeout() => {
                // The daemon took the connection and never answered: the job is not delivered, and the lane is free.
                log::warn!("the daemon did not take a Logins answer in time");
                Ok(())
            }
            Err(_) => Err(HostError::Unavailable),
        }
    }

    /// An answer that does not fit goes back as a failure, so the daemon does not wait on the job.
    async fn too_large(&self, id: &str) -> Result<(), HostError> {
        log::warn!("{TOO_LARGE}");
        let body = serde_json::to_vec(&Answer {
            id,
            ok: false,
            result: None,
        })
        .map_err(|_| HostError::Refused)?;
        let _ = self.post(body).await;
        Ok(())
    }

    /// `lock`, run on the local controller. Locking twice is harmless, so concurrent callers each run it.
    // ponytail: no single-flight as the TS has; a second lock while one runs just locks again.
    async fn lock(&self) -> Result<(), GateError> {
        let command = VaultHostCommand {
            controller_id: LOCAL.into(),
            action: VaultHostAction::Lock,
        };
        self.executor.execute(command).await.map(|_| ())
    }

    /// The channel could not read the daemon or a job: fence what was polled before it, tell the app, lock the vault.
    async fn fail_closed(&self) {
        self.fence().channel_epoch += 1;
        let _ = self.events.try_send(HostEvent::Failed);
        let _ = self.lock().await;
    }

    /// Whether a polled job may run, and the barrier it sets (the TS's barrier rules, in order).
    fn admit(&self, job: &VaultHostJob, epoch: u64) -> Admit {
        let now = Instant::now();
        let mut fence = self.fence();
        fence.barriers.retain(|_, barrier| barrier.expires >= now);
        let owner = job.command.controller_id.as_str();
        let sequence = job.sequence;
        match &job.command.action {
            VaultHostAction::Lock => {
                fence.global = fence.global.max(sequence);
                Admit::Run
            }
            VaultHostAction::Disconnect | VaultHostAction::Cancel => {
                if fence.barriers.len() >= MAX_BARRIERS && !fence.barriers.contains_key(owner) {
                    return Admit::Overflow;
                }
                let previous = fence
                    .barriers
                    .get(owner)
                    .map_or(0, |barrier| barrier.sequence);
                fence.barriers.insert(
                    owner.to_string(),
                    Barrier {
                        sequence: previous.max(sequence),
                        expires: now + BARRIER_TTL,
                    },
                );
                Admit::Run
            }
            _ => {
                let barred = fence
                    .barriers
                    .get(owner)
                    .map_or(0, |barrier| barrier.sequence);
                if epoch != fence.channel_epoch || sequence <= fence.global || sequence <= barred {
                    Admit::Stale
                } else {
                    Admit::Run
                }
            }
        }
    }

    /// One poll and whatever it brings. A job is run and answered, refused, or the lane fails closed.
    async fn step(&self, control: bool, epoch: u64) -> Result<(), HostError> {
        let Some(bytes) = self.poll(control).await? else {
            return Ok(());
        };
        let raw = serde_json::from_slice::<Value>(&bytes).map_err(|_| HostError::Malformed)?;
        let Ok(job) = protocol::parse::<VaultHostJob>(raw) else {
            self.fail_closed().await;
            return Ok(());
        };
        if control && !is_urgent(&job.command.action) {
            self.answer(&job.id, None).await?;
            self.fail_closed().await;
            return Ok(());
        }
        match self.admit(&job, epoch) {
            Admit::Run => {}
            Admit::Stale => return self.answer(&job.id, None).await,
            Admit::Overflow => {
                self.fail_closed().await;
                return self.answer(&job.id, None).await;
            }
        }
        let action = job.command.action.clone();
        let reply = self
            .executor
            .execute(job.command)
            .await
            .ok()
            .filter(|reply| fits(&action, reply));
        self.answer(&job.id, reply.as_ref()).await
    }
}

/// The reply must be the result of its action's `vault.*` method, as `vaultHostResult` checks it. A mismatch is a failure.
fn fits(action: &VaultHostAction, reply: &VaultReply) -> bool {
    matches!(
        (action, reply),
        (
            VaultHostAction::Status | VaultHostAction::Unlock | VaultHostAction::Lock,
            VaultReply::Status(_)
        ) | (VaultHostAction::List(_), VaultReply::List(_))
            | (VaultHostAction::Entry(_), VaultReply::Entry(_))
            | (VaultHostAction::Delete(_), VaultReply::Delete(_))
            | (VaultHostAction::Fill(_), VaultReply::Fill(_))
            | (VaultHostAction::Finish(_), VaultReply::Finish(_))
            | (
                VaultHostAction::Cancel | VaultHostAction::Disconnect,
                VaultReply::Empty(_)
            )
    )
}

/// One lane's loop: poll, run, answer, and never ask for the next job before the last one is answered. A failure fails
/// the channel closed, then waits a second. Every await races the stop signal, so closing never waits on a job.
async fn lane(shared: Arc<Shared>, control: bool, mut stop: watch::Receiver<bool>) {
    loop {
        let epoch = shared.fence().channel_epoch;
        let outcome = tokio::select! {
            outcome = shared.step(control, epoch) => outcome,
            _ = stop.wait_for(|stopped| *stopped) => return,
        };
        if let Err(error) = outcome {
            log::debug!("the Logins host step failed: {error:?}");
            tokio::select! {
                () = shared.fail_closed() => {}
                _ = stop.wait_for(|stopped| *stopped) => return,
            }
            tokio::select! {
                () = tokio::time::sleep(RETRY_AFTER_ERROR) => {}
                _ = stop.wait_for(|stopped| *stopped) => return,
            }
        }
    }
}

/// Locks the vault as soon as the client stops, before any native work the client was waiting on is allowed to finish.
async fn lock_on_stop(shared: Arc<Shared>, mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
    match tokio::time::timeout(CLOSE_TIMEOUT, shared.lock()).await {
        Ok(Ok(())) => {}
        _ => log::warn!("{UNCONFIRMED_CLOSE}"),
    }
}

/// A running client. Dropping it, or calling [`VaultHost::close`], stops both lanes and locks the vault.
pub struct VaultHost {
    stop: watch::Sender<bool>,
}

impl VaultHost {
    /// Starts both lanes on a thread of their own. `events` gets what the app needs to know.
    pub fn start(
        config: HostConfig,
        executor: Arc<dyn Executor>,
        events: Sender<HostEvent>,
    ) -> io::Result<VaultHost> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io::Error::other)?;
        let shared = Arc::new(Shared {
            client,
            url: format!("http://127.0.0.1:{}/vault-host", config.port),
            token: config.token,
            executor,
            events,
            fence: Mutex::default(),
        });
        let (stop, stopped) = watch::channel(false);
        std::thread::Builder::new()
            .name("vault-host".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    tokio::join!(
                        lane(shared.clone(), false, stopped.clone()),
                        lane(shared.clone(), true, stopped.clone()),
                        lock_on_stop(shared, stopped),
                    );
                });
                // The client's channel closes with its sender here: an app that watches it sees the thread end.
                runtime.shutdown_background();
            })?;
        Ok(VaultHost { stop })
    }

    /// Stops both lanes and locks the vault. It returns at once: the lock runs on the client's own thread, so calling
    /// this from the app's thread cannot deadlock.
    pub fn close(&self) {
        let _ = self.stop.send_replace(true);
    }
}

impl Drop for VaultHost {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{HostToken, VaultReply, fits};
    use crate::vault::gate::Empty;
    use protocol::vault::{VaultHostAction, VaultState, VaultStatus};

    #[test]
    fn a_reply_must_be_the_result_of_its_action() {
        let status = VaultReply::Status(VaultStatus {
            state: VaultState::Locked,
            reason: None,
            pending_deletes: 0,
        });
        assert!(fits(&VaultHostAction::Lock, &status));
        assert!(!fits(&VaultHostAction::Cancel, &status));
        assert!(fits(&VaultHostAction::Cancel, &VaultReply::Empty(Empty {})));
        assert!(!fits(
            &VaultHostAction::Status,
            &VaultReply::Empty(Empty {})
        ));
    }

    #[test]
    fn the_host_token_never_appears_in_debug_output() {
        let token = HostToken::new("synthetic-host-capability");
        let shown = format!("{token:?}");
        assert!(!shown.contains("synthetic"));
        assert!(!token.is_empty());
        assert!(HostToken::new("").is_empty());
    }
}
