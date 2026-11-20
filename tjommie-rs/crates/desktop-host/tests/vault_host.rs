//! Ports of `vault-host.test.ts` and `worker-isolation.test.ts`, run against the daemon's real `/vault-host` and
//! `/worker-host` routes, in-process on 127.0.0.1. Two cases need an orderings the real daemon never produces (a job
//! arriving after a newer cancel, a lease that changes lifetime); those run against a scripted loopback fake instead.
//! Nothing here touches the app's data folder, Credential Manager or the network beyond 127.0.0.1.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::server::{Handlers, Routes, Server, ServerOptions};
use daemon::vault_host_broker::{VaultError, VaultHostBroker, VaultRequestOptions};
use daemon::worker_boundary_host::{WorkerBoundary, WorkerBoundaryHost, WorkerLease};
use desktop_host::vault::gate::{Empty, GateError, VaultReply};
use desktop_host::vault::host::{Executor, HostConfig, HostEvent, HostToken, VaultHost};
use desktop_host::vault::worker::{RemoteWorkerIsolation, VaultIsolation};
use futures::future::BoxFuture;
use protocol::vault::{
    EntryStatus, VaultEntry, VaultEntryResult, VaultHostAction, VaultHostCommand, VaultState,
    VaultStatus,
};
use protocol::vault_host::VaultHostJob;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot};

/// The desktop host token the daemon's broker and the client share. The app's ordinary token never opens these routes.
fn host_secret() -> String {
    "h".repeat(43)
}

fn kind(action: &VaultHostAction) -> &'static str {
    match action {
        VaultHostAction::Status => "status",
        VaultHostAction::List(_) => "list",
        VaultHostAction::Unlock => "unlock",
        VaultHostAction::Lock => "lock",
        VaultHostAction::Entry(_) => "entry",
        VaultHostAction::Delete(_) => "delete",
        VaultHostAction::Fill(_) => "fill",
        VaultHostAction::Finish(_) => "finish",
        VaultHostAction::Disconnect => "disconnect",
        VaultHostAction::Cancel => "cancel",
    }
}

fn locked() -> VaultReply {
    VaultReply::Status(VaultStatus {
        state: VaultState::Locked,
        reason: None,
        pending_deletes: 0,
    })
}

fn locked_json() -> Value {
    json!({ "state": "locked", "reason": null, "pendingDeletes": 0 })
}

fn entry_action() -> VaultHostAction {
    VaultHostAction::Entry(VaultEntry {
        bot_id: "one".into(),
        id: None,
        revision: None,
    })
}

/// A command as the daemon's broker takes it: a raw JSON value, checked on the way in.
fn raw(controller: &str, action: VaultHostAction) -> Value {
    serde_json::to_value(VaultHostCommand {
        controller_id: controller.into(),
        action,
    })
    .expect("a command serializes")
}

/// The actions a client executor saw, in order.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<&'static str>>>);

impl Log {
    fn push(&self, action: &VaultHostAction) {
        self.0.lock().expect("log").push(kind(action));
    }

    fn kinds(&self) -> Vec<&'static str> {
        self.0.lock().expect("log").clone()
    }
}

/// An executor that answers at once, decided by the action.
struct Immediate<F>(F);

impl<F> Executor for Immediate<F>
where
    F: Fn(&VaultHostAction) -> Result<VaultReply, GateError> + Send + Sync + 'static,
{
    fn execute(&self, command: VaultHostCommand) -> BoxFuture<'_, Result<VaultReply, GateError>> {
        Box::pin(std::future::ready((self.0)(&command.action)))
    }
}

/// An executor whose answer is a future the test controls.
struct Deferred<F>(F);

impl<F, Fut> Executor for Deferred<F>
where
    F: Fn(VaultHostCommand) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<VaultReply, GateError>> + Send + 'static,
{
    fn execute(&self, command: VaultHostCommand) -> BoxFuture<'_, Result<VaultReply, GateError>> {
        Box::pin((self.0)(command))
    }
}

/// The pending-entry executor of the urgent-lane cases: an entry waits until a lock, disconnect or cancel releases it.
fn waiting_entry(release: Arc<Notify>, seen: Log) -> impl Executor {
    Deferred(move |command: VaultHostCommand| {
        let (release, seen) = (release.clone(), seen.clone());
        async move {
            seen.push(&command.action);
            match command.action {
                VaultHostAction::Entry(_) => {
                    release.notified().await;
                    Ok(VaultReply::Entry(VaultEntryResult {
                        status: EntryStatus::Cancelled,
                        entry: None,
                    }))
                }
                VaultHostAction::Lock => {
                    release.notify_one();
                    Ok(locked())
                }
                VaultHostAction::Disconnect | VaultHostAction::Cancel => {
                    release.notify_one();
                    Ok(VaultReply::Empty(Empty {}))
                }
                _ => Ok(locked()),
            }
        }
    })
}

fn start_client(
    port: u16,
    executor: impl Executor,
) -> (VaultHost, async_channel::Receiver<HostEvent>) {
    let (events, received) = async_channel::unbounded();
    let host = VaultHost::start(
        HostConfig {
            port,
            token: HostToken::new(host_secret()),
        },
        Arc::new(executor),
        events,
    )
    .expect("the client starts");
    (host, received)
}

/// Polls `condition` until it holds, for up to `limit`.
async fn wait_for(limit: Duration, mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(limit, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the condition held in time");
}

async fn wait_until(condition: impl FnMut() -> bool) {
    wait_for(Duration::from_secs(5), condition).await;
}

/// The daemon side: the real `/vault-host` route on loopback.
struct Rig {
    broker: Arc<VaultHostBroker>,
    server: Server,
    port: u16,
}

async fn rig(timeout: Duration) -> Rig {
    let broker = VaultHostBroker::new(host_secret(), timeout);
    let server = Server::start(ServerOptions {
        port: 0,
        token: "ordinary-rpc-token".into(),
        handlers: Handlers::default(),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .expect("the daemon's server starts");
    server
        .activate_with(
            "ordinary-rpc-token",
            Handlers::default(),
            Routes::default().route("/vault-host", broker.route()),
        )
        .expect("the route mounts");
    let port = server.port();
    Rig {
        broker,
        server,
        port,
    }
}

impl Rig {
    async fn close(self) {
        self.broker.close();
        self.server.close().await;
    }
}

#[tokio::test]
async fn serves_a_status_job_from_the_real_route_and_answers_it() {
    let rig = rig(Duration::from_secs(5)).await;
    let seen = Log::default();
    let (host, _events) = start_client(
        rig.port,
        Immediate({
            let seen = seen.clone();
            move |action: &VaultHostAction| {
                seen.push(action);
                Ok(locked())
            }
        }),
    );
    let answer = rig
        .broker
        .request(
            raw("local", VaultHostAction::Status),
            VaultRequestOptions::default(),
        )
        .await;
    assert_eq!(answer, Ok(locked_json()));
    assert_eq!(seen.kinds(), ["status"]);
    host.close();
    rig.close().await;
}

#[tokio::test]
async fn a_failed_entry_is_answered_with_the_fixed_failure_and_no_detail() {
    let rig = rig(Duration::from_secs(5)).await;
    let (host, _events) = start_client(
        rig.port,
        Immediate(|action: &VaultHostAction| match action {
            VaultHostAction::Entry(_) => Err(GateError::Vault("SYNTHETIC_SECRET_ERROR".into())),
            _ => Ok(locked()),
        }),
    );
    let error = rig
        .broker
        .request(
            raw("rpc:one", entry_action()),
            VaultRequestOptions::default(),
        )
        .await
        .expect_err("the entry fails");
    assert_eq!(error, VaultError::HostFailed);
    assert!(!error.to_string().contains("SYNTHETIC"));
    host.close();
    rig.close().await;
}

/// `urgent` overtakes an entry that is waiting for the native window: the entry is cancelled and the urgent job answers.
async fn urgent_overtakes_a_pending_entry(urgent: VaultHostAction, expected: Value) {
    let rig = rig(Duration::from_secs(5)).await;
    let release = Arc::new(Notify::new());
    let seen = Log::default();
    let (host, _events) = start_client(rig.port, waiting_entry(release, seen.clone()));
    let broker = rig.broker.clone();
    let pending = tokio::spawn(async move {
        broker
            .request(
                raw("rpc:one", entry_action()),
                VaultRequestOptions::default(),
            )
            .await
    });
    wait_until(|| seen.kinds().contains(&"entry")).await;
    let urgent_kind = kind(&urgent);
    let answer = rig
        .broker
        .request(raw("rpc:one", urgent), VaultRequestOptions::default())
        .await;
    assert_eq!(answer, Ok(expected));
    assert_eq!(pending.await.expect("joined"), Err(VaultError::Cancelled));
    let kinds = seen.kinds();
    assert_eq!(kinds.first(), Some(&"entry"));
    assert_eq!(kinds.get(1), Some(&urgent_kind));
    host.close();
    rig.close().await;
}

#[tokio::test]
async fn urgent_lock_overtakes_a_pending_entry() {
    urgent_overtakes_a_pending_entry(VaultHostAction::Lock, locked_json()).await;
}

#[tokio::test]
async fn urgent_disconnect_overtakes_a_pending_entry() {
    urgent_overtakes_a_pending_entry(VaultHostAction::Disconnect, json!({})).await;
}

#[tokio::test]
async fn urgent_cancel_overtakes_a_pending_entry() {
    urgent_overtakes_a_pending_entry(VaultHostAction::Cancel, json!({})).await;
}

#[tokio::test]
async fn closing_the_host_locks_before_waiting_for_a_pending_native_operation() {
    let rig = rig(Duration::from_secs(5)).await;
    let release = Arc::new(Notify::new());
    let seen = Log::default();
    let (host, _events) = start_client(rig.port, waiting_entry(release, seen.clone()));
    let broker = rig.broker.clone();
    let pending = tokio::spawn(async move {
        broker
            .request(
                raw("rpc:one", entry_action()),
                VaultRequestOptions::default(),
            )
            .await
    });
    wait_until(|| seen.kinds().contains(&"entry")).await;
    host.close();
    wait_until(|| seen.kinds().contains(&"lock")).await;
    rig.broker.close();
    let _ = pending.await;
    rig.close().await;
}

#[tokio::test]
async fn an_unanswered_job_times_out_and_the_daemon_cancels_it() {
    let rig = rig(Duration::from_millis(500)).await;
    let seen = Log::default();
    let (host, _events) = start_client(
        rig.port,
        Deferred({
            let seen = seen.clone();
            move |command: VaultHostCommand| {
                let seen = seen.clone();
                async move {
                    seen.push(&command.action);
                    match command.action {
                        // The native window never answers: the entry stays unanswered.
                        VaultHostAction::Entry(_) => {
                            std::future::pending::<Result<VaultReply, GateError>>().await
                        }
                        _ => Ok(VaultReply::Empty(Empty {})),
                    }
                }
            }
        }),
    );
    let answer = rig
        .broker
        .request(
            raw("rpc:one", entry_action()),
            VaultRequestOptions::default(),
        )
        .await;
    assert_eq!(answer, Err(VaultError::Expired));
    wait_until(|| seen.kinds().contains(&"cancel")).await;
    host.close();
    rig.close().await;
}

#[tokio::test]
async fn shuts_down_cleanly_once_closed() {
    let rig = rig(Duration::from_secs(5)).await;
    let seen = Log::default();
    let (host, events) = start_client(
        rig.port,
        Immediate({
            let seen = seen.clone();
            move |action: &VaultHostAction| {
                seen.push(action);
                Ok(locked())
            }
        }),
    );
    rig.broker
        .request(
            raw("local", VaultHostAction::Status),
            VaultRequestOptions::default(),
        )
        .await
        .expect("the client is polling");
    host.close();
    // The client's thread ends by dropping its channel's sender: the receiver then closes.
    tokio::time::timeout(Duration::from_secs(5), async {
        while events.recv().await.is_ok() {}
    })
    .await
    .expect("the client's thread ended");
    assert!(seen.kinds().contains(&"lock"));
    rig.close().await;
}

// ---- a scripted loopback fake for the orderings the real daemon never produces ----

/// A status and body, as the fake answers.
type Canned = (u16, String);
type Script = Arc<dyn Fn(String, String, Vec<u8>) -> BoxFuture<'static, Canned> + Send + Sync>;

async fn scripted(script: Script) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(answer_one(socket, script.clone()));
        }
    });
    port
}

async fn read_more(socket: &mut TcpStream, raw: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 4096];
    match socket.read(&mut chunk).await {
        Ok(read) if read > 0 => {
            raw.extend_from_slice(&chunk[..read]);
            true
        }
        _ => false,
    }
}

/// One request per connection: the script answers when it is ready, or never.
async fn answer_one(mut socket: TcpStream, script: Script) {
    let mut raw = Vec::new();
    let head_end = loop {
        if let Some(at) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        if !read_more(&mut socket, &mut raw).await {
            return;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request = lines.next().unwrap_or_default().split(' ');
    let method = request.next().unwrap_or_default().to_string();
    let path = request.next().unwrap_or_default().to_string();
    let length: usize = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    while raw.len() < head_end + 4 + length {
        if !read_more(&mut socket, &mut raw).await {
            return;
        }
    }
    let body = raw[head_end + 4..head_end + 4 + length].to_vec();
    let (status, payload) = script(method, path, body).await;
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        _ => "Not Found",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn job(id: &str, sequence: u64, action: VaultHostAction) -> String {
    serde_json::to_string(&VaultHostJob {
        id: id.into(),
        sequence,
        command: VaultHostCommand {
            controller_id: "rpc:one".into(),
            action,
        },
    })
    .expect("a job serializes")
}

#[derive(Deserialize)]
struct Answered {
    id: String,
    ok: bool,
}

#[tokio::test]
async fn rejects_an_entry_job_whose_body_arrives_after_its_controllers_newer_cancel_barrier() {
    const OLD: &str = "41f8bac7-358c-48cf-9f48-a3a358804da6";
    const CANCEL: &str = "83c70656-e163-478e-a934-6a5801f5ef18";
    // The first normal poll is held until the cancel has been answered, so the older entry arrives after it.
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let go_rx = Arc::new(Mutex::new(Some(go_rx)));
    let go_tx = Arc::new(Mutex::new(Some(go_tx)));
    let sent_cancel = Arc::new(AtomicBool::new(false));
    let replies: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let script: Script = {
        let (go_rx, go_tx, sent_cancel, replies) = (
            go_rx.clone(),
            go_tx.clone(),
            sent_cancel.clone(),
            replies.clone(),
        );
        Arc::new(move |method: String, path: String, body: Vec<u8>| {
            let (go_rx, go_tx, sent_cancel, replies) = (
                go_rx.clone(),
                go_tx.clone(),
                sent_cancel.clone(),
                replies.clone(),
            );
            Box::pin(async move {
                if method == "POST" {
                    let Ok(answered) = serde_json::from_slice::<Answered>(&body) else {
                        return (400, String::new());
                    };
                    if answered.id == CANCEL
                        && let Some(go) = go_tx.lock().expect("go").take()
                    {
                        let _ = go.send(());
                    }
                    replies
                        .lock()
                        .expect("replies")
                        .push((answered.id, answered.ok));
                    return (204, String::new());
                }
                if path.contains("control=1") {
                    if !sent_cancel.swap(true, Ordering::SeqCst) {
                        return (200, job(CANCEL, 2, VaultHostAction::Cancel));
                    }
                    return std::future::pending().await;
                }
                let go = go_rx.lock().expect("go").take();
                match go {
                    Some(go) => {
                        let _ = go.await;
                        (200, job(OLD, 1, entry_action()))
                    }
                    None => std::future::pending().await,
                }
            })
        })
    };
    let port = scripted(script).await;
    let seen = Log::default();
    let (host, _events) = start_client(
        port,
        Immediate({
            let seen = seen.clone();
            move |action: &VaultHostAction| {
                seen.push(action);
                Ok(VaultReply::Empty(Empty {}))
            }
        }),
    );
    wait_until(|| replies.lock().expect("replies").len() == 2).await;
    assert_eq!(
        *replies.lock().expect("replies"),
        vec![(CANCEL.to_string(), true), (OLD.to_string(), false)]
    );
    assert_eq!(seen.kinds(), ["cancel"]);
    host.close();
}

#[tokio::test]
async fn an_oversized_job_fails_the_channel_closed_without_running_anything() {
    let script: Script = Arc::new(|method: String, path: String, _body: Vec<u8>| {
        Box::pin(async move {
            if method == "GET" && !path.contains("control=1") {
                // Past the client's 16 KiB job limit.
                return (200, "x".repeat(17 * 1024));
            }
            std::future::pending().await
        })
    });
    let port = scripted(script).await;
    let seen = Log::default();
    let (host, events) = start_client(
        port,
        Immediate({
            let seen = seen.clone();
            move |action: &VaultHostAction| {
                seen.push(action);
                Ok(locked())
            }
        }),
    );
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("an event in time")
        .expect("the client is running");
    assert_eq!(event, HostEvent::Failed);
    wait_until(|| seen.kinds().contains(&"lock")).await;
    assert!(seen.kinds().iter().all(|kind| *kind == "lock"));
    host.close();
}

// ---- worker isolation: `worker-isolation.test.ts`, against the daemon's `/worker-host` ----

/// A worker boundary whose calls the test can hold. `unavailable_reason` calls from `block_status_from` on (1-based)
/// wait for `hold`; `acquire` waits for `hold` when `hold_acquire` is set and reports that it has begun on `entered`.
struct Boundary {
    block_status_from: usize,
    hold_acquire: bool,
    calls: AtomicUsize,
    hold: Arc<Notify>,
    entered: Arc<Notify>,
}

impl Boundary {
    fn idle() -> Boundary {
        Boundary {
            block_status_from: 0,
            hold_acquire: false,
            calls: AtomicUsize::new(0),
            hold: Arc::new(Notify::new()),
            entered: Arc::new(Notify::new()),
        }
    }
}

struct Lease;

impl WorkerLease for Lease {
    fn valid(&self) -> bool {
        true
    }

    fn release(&self) {}
}

impl WorkerBoundary for Boundary {
    fn unavailable_reason(&self) -> BoxFuture<'static, Option<String>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let hold = (self.block_status_from != 0 && call >= self.block_status_from)
            .then(|| self.hold.clone());
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            None
        })
    }

    fn acquire(&self) -> BoxFuture<'static, Option<Box<dyn WorkerLease>>> {
        let (hold, entered, hold_it) = (self.hold.clone(), self.entered.clone(), self.hold_acquire);
        Box::pin(async move {
            entered.notify_one();
            if hold_it {
                hold.notified().await;
            }
            let lease: Box<dyn WorkerLease> = Box::new(Lease);
            Some(lease)
        })
    }

    fn invalidate(&self) {}
}

fn worker_token() -> HostToken {
    HostToken::new(host_secret())
}

/// The daemon's real `/worker-host` route, with the given boundary, on loopback.
async fn worker_server(boundary: Boundary) -> (Server, u16) {
    let host = WorkerBoundaryHost::new(Arc::new(boundary), host_secret());
    let server = Server::start(ServerOptions {
        port: 0,
        token: "ordinary-rpc-token".into(),
        handlers: Handlers::default(),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .expect("the daemon's server starts");
    server
        .activate_with(
            "ordinary-rpc-token",
            Handlers::default(),
            Routes::default().route("/worker-host", host.route()),
        )
        .expect("the route mounts");
    let port = server.port();
    (server, port)
}

#[tokio::test]
async fn an_expired_lease_is_not_revived_by_a_heartbeat_that_arrives_late() {
    // The first heartbeat renews; the second waits on the boundary, so the lease runs out before it answers.
    let hold = Arc::new(Notify::new());
    let boundary = Boundary {
        block_status_from: 2,
        hold: hold.clone(),
        ..Boundary::idle()
    };
    let (server, port) = worker_server(boundary).await;
    let isolation = RemoteWorkerIsolation::new(port, worker_token()).expect("isolation");
    let lease = VaultIsolation::acquire(&isolation).await.expect("acquired");
    wait_for(Duration::from_secs(15), || lease.assert().is_err()).await;
    hold.notify_one();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(lease.assert().is_err());
    isolation.close();
    server.close().await;
}

#[tokio::test]
async fn a_heartbeat_from_a_replacement_daemon_lifetime_retires_the_lease() {
    #[derive(Deserialize)]
    struct Command {
        kind: String,
    }
    let (first, second, id) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    let script: Script = Arc::new(move |_method: String, _path: String, body: Vec<u8>| {
        let (first, second, id) = (first.clone(), second.clone(), id.clone());
        Box::pin(async move {
            let kind = serde_json::from_slice::<Command>(&body)
                .map(|command| command.kind)
                .unwrap_or_default();
            // A renewal comes from a daemon that has restarted: its lifetime differs from the lease's.
            let lifetime = if kind == "acquire" { first } else { second };
            (
                200,
                json!({ "lifetime": lifetime, "id": id, "expiresInMs": 5000 }).to_string(),
            )
        })
    });
    let port = scripted(script).await;
    let isolation = RemoteWorkerIsolation::new(port, worker_token()).expect("isolation");
    let lease = VaultIsolation::acquire(&isolation).await.expect("acquired");
    wait_for(Duration::from_secs(5), || lease.assert().is_err()).await;
    isolation.close();
}

#[tokio::test]
async fn a_late_acquire_or_status_answer_after_close_is_refused() {
    let entered = Arc::new(Notify::new());
    let hold = Arc::new(Notify::new());
    let boundary = Boundary {
        hold_acquire: true,
        hold: hold.clone(),
        entered: entered.clone(),
        ..Boundary::idle()
    };
    let (server, port) = worker_server(boundary).await;
    let isolation = Arc::new(RemoteWorkerIsolation::new(port, worker_token()).expect("isolation"));
    let pending = {
        let isolation = isolation.clone();
        tokio::spawn(async move { VaultIsolation::acquire(&*isolation).await.map(|_| ()) })
    };
    entered.notified().await;
    isolation.close();
    // The daemon's answer lands after the close: the client must not hand the lease out.
    hold.notify_one();
    assert!(pending.await.expect("joined").is_err());
    assert!(
        VaultIsolation::unavailable_reason(&*isolation)
            .await
            .is_some()
    );
    server.close().await;
}

#[test]
fn an_isolation_without_a_port_or_a_token_is_refused() {
    assert!(RemoteWorkerIsolation::new(0, worker_token()).is_err());
    assert!(RemoteWorkerIsolation::new(4720, HostToken::new("")).is_err());
}
