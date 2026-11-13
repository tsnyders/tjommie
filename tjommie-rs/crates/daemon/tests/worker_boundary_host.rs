//! Port of `packages/daemon/src/worker-boundary-host.test.ts`: `/worker-host`, the desktop's lifetime channel for the
//! Windows worker boundary. The TypeScript test drives it with the desktop's `RemoteWorkerIsolation`; this one checks the
//! same wire answers directly.

mod host_support;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::server::{Handlers, Routes, Server, ServerOptions};
use daemon::worker_boundary_host::{WorkerBoundary, WorkerBoundaryHost, WorkerLease};
use futures_util::future::BoxFuture;
use host_support::{Reply, post};
use serde_json::{Value, json};
use tokio::sync::oneshot;

#[derive(Default)]
struct Fake {
    generation: Arc<AtomicU64>,
    lost: Arc<AtomicBool>,
    releases: Arc<AtomicU32>,
    acquires: AtomicU32,
    /// When set, the next acquire signals `started` and waits for `finish`.
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

struct FakeLease {
    /// Like the TypeScript test's late lease: it never fails its own check, so only the host can refuse it.
    always_valid: bool,
    generation: Arc<AtomicU64>,
    initial: u64,
    lost: Arc<AtomicBool>,
    releases: Arc<AtomicU32>,
}

impl WorkerLease for FakeLease {
    fn valid(&self) -> bool {
        self.always_valid
            || (self.generation.load(Ordering::SeqCst) == self.initial
                && !self.lost.load(Ordering::SeqCst))
    }
    fn release(&self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}

impl WorkerBoundary for Fake {
    fn unavailable_reason(&self) -> BoxFuture<'static, Option<String>> {
        let lost = self.lost.load(Ordering::SeqCst);
        Box::pin(async move { lost.then(|| "synthetic boundary lost".to_owned()) })
    }
    fn acquire(&self) -> BoxFuture<'static, Option<Box<dyn WorkerLease>>> {
        self.acquires.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        let lease = FakeLease {
            always_valid: gate.is_some(),
            generation: self.generation.clone(),
            initial: self.generation.load(Ordering::SeqCst),
            lost: self.lost.clone(),
            releases: self.releases.clone(),
        };
        Box::pin(async move {
            if let Some((started, finish)) = gate {
                let _ = started.send(());
                let _ = finish.await;
            }
            Some(Box::new(lease) as Box<dyn WorkerLease>)
        })
    }
    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fixture {
    fake: Arc<Fake>,
    host: Arc<WorkerBoundaryHost>,
    port: u16,
    token: String,
    _server: Server,
}

async fn fixture() -> Fixture {
    let fake = Arc::new(Fake::default());
    let token = format!("synthetic-{}", daemon::ids::new_id());
    let host = WorkerBoundaryHost::new(fake.clone(), token.clone());
    let server = Server::start(ServerOptions {
        port: 0,
        token: "ordinary-rpc-token".into(),
        handlers: Handlers::default(),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap();
    server
        .activate_with(
            "ordinary-rpc-token",
            Handlers::default(),
            Routes::default().route("/worker-host", host.route()),
        )
        .unwrap();
    Fixture {
        fake,
        host,
        port: server.port(),
        token,
        _server: server,
    }
}

impl Fixture {
    async fn request(&self, body: Value) -> Reply {
        post(
            self.port,
            "/worker-host",
            &[("Authorization", &format!("Bearer {}", self.token))],
            body.to_string().as_bytes(),
        )
        .await
    }

    fn releases(&self) -> u32 {
        self.fake.releases.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn authenticates_host_status_and_leases_independently_from_ordinary_rpc() {
    let f = fixture().await;
    let ordinary = post(
        f.port,
        "/worker-host",
        &[("Authorization", "Bearer ordinary-rpc-token")],
        json!({ "kind": "acquire" }).to_string().as_bytes(),
    )
    .await;
    assert_eq!(ordinary.status, 401);
    assert_eq!(f.fake.acquires.load(Ordering::SeqCst), 0);
    let status = f.request(json!({ "kind": "status" })).await.json();
    assert_eq!(
        (&status["available"], &status["reason"]),
        (&json!(true), &Value::Null)
    );
    let lifetime = status["lifetime"].clone();
    let lease = f.request(json!({ "kind": "acquire" })).await.json();
    assert_eq!(
        (&lease["lifetime"], &lease["expiresInMs"]),
        (&lifetime, &json!(5000))
    );
    let renewed = f
        .request(json!({ "kind": "renew", "id": lease["id"] }))
        .await
        .json();
    assert_eq!(
        renewed,
        json!({ "lifetime": lifetime, "id": lease["id"], "expiresInMs": 5000 })
    );
    f.host.close();
    assert_eq!(f.request(json!({ "kind": "status" })).await.status, 503);
    assert_eq!(f.releases(), 1);
}

#[tokio::test]
async fn releases_a_late_acquisition_after_host_close_instead_of_returning_a_usable_lease() {
    let f = fixture().await;
    let (started, ready) = oneshot::channel();
    let (finish, waiting) = oneshot::channel();
    *f.fake.gate.lock().unwrap() = Some((started, waiting));
    let (port, token) = (f.port, f.token.clone());
    let response = tokio::spawn(async move {
        post(
            port,
            "/worker-host",
            &[("Authorization", &format!("Bearer {token}"))],
            json!({ "kind": "acquire" }).to_string().as_bytes(),
        )
        .await
    });
    ready.await.unwrap();
    f.host.close();
    finish.send(()).unwrap();
    assert_eq!(response.await.unwrap().status, 503);
    assert_eq!(f.releases(), 1);
}

#[tokio::test]
async fn rechecks_isolation_on_renew_and_retires_the_failed_lease_immediately() {
    let f = fixture().await;
    let acquired = f.request(json!({ "kind": "acquire" })).await.json();
    f.fake.lost.store(true, Ordering::SeqCst);
    assert_eq!(
        f.request(json!({ "kind": "renew", "id": acquired["id"] }))
            .await
            .status,
        503
    );
    assert_eq!(f.releases(), 1);
    assert_eq!(
        f.request(json!({ "kind": "renew", "id": acquired["id"] }))
            .await
            .status,
        503
    );
    f.host.close();
    assert_eq!(f.releases(), 1);
}

#[tokio::test]
async fn bounds_active_leases_and_rejects_unknown_lifetime_lease_ids() {
    let f = fixture().await;
    for _ in 0..8 {
        assert_eq!(f.request(json!({ "kind": "acquire" })).await.status, 200);
    }
    assert_eq!(f.request(json!({ "kind": "acquire" })).await.status, 503);
    assert_eq!(
        f.request(json!({ "kind": "renew", "id": daemon::ids::new_id() }))
            .await
            .status,
        503
    );
    f.host.close();
    assert_eq!(f.releases(), 8);
}

#[tokio::test]
async fn refuses_malformed_commands_and_lets_an_unrenewed_lease_lapse() {
    let f = fixture().await;
    for body in [
        json!({ "kind": "acquire", "extra": 1 }),
        json!({ "kind": "launch" }),
        json!({ "kind": "renew" }),
    ] {
        assert_eq!(f.request(body.clone()).await.status, 503, "{body}");
    }
    let big = json!({ "kind": "status", "pad": "x".repeat(1100) });
    assert_eq!(f.request(big).await.status, 413);
    let get = host_support::get(
        f.port,
        "/worker-host",
        &[("Authorization", &format!("Bearer {}", f.token))],
    )
    .await;
    assert_eq!(get.status, 405);
    let lease = f.request(json!({ "kind": "acquire" })).await.json();
    assert_eq!(
        f.request(json!({ "kind": "release", "id": lease["id"] }))
            .await
            .json(),
        json!({ "lifetime": lease["lifetime"] })
    );
    assert_eq!(f.releases(), 1);
    f.request(json!({ "kind": "acquire" })).await.json();
    tokio::time::sleep(Duration::from_millis(10_500)).await;
    assert_eq!(
        f.releases(),
        2,
        "a lease nobody renews for 10 s is released"
    );
}
