//! Port of `packages/daemon/src/vault-host-broker.test.ts`: the desktop's protected Logins host long-polls
//! `/vault-host` with the desktop host token. Jobs carry no secret and no bot; replies are checked against the method.

mod host_support;

use std::sync::Arc;
use std::time::Duration;

use daemon::server::{Handlers, Routes, Server, ServerOptions};
use daemon::vault_host_broker::{VaultError, VaultHostBroker, VaultRequestOptions};
use host_support::{get, post};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn token() -> String {
    "v".repeat(43)
}

fn auth() -> String {
    format!("Bearer {}", token())
}

fn status() -> Value {
    json!({ "state": "locked", "reason": null, "pendingDeletes": 0 })
}

struct Fixture {
    broker: Arc<VaultHostBroker>,
    server: Server,
    port: u16,
}

async fn fixture(timeout: Duration) -> Fixture {
    let broker = VaultHostBroker::new(token(), timeout);
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
            Routes::default().route("/vault-host", broker.route()),
        )
        .unwrap();
    let port = server.port();
    Fixture {
        broker,
        server,
        port,
    }
}

impl Fixture {
    fn request(&self, command: Value) -> tokio::task::JoinHandle<Result<Value, VaultError>> {
        let broker = self.broker.clone();
        tokio::spawn(async move {
            broker
                .request(command, VaultRequestOptions::default())
                .await
        })
    }

    async fn close(self) {
        self.broker.close();
        self.server.close().await;
    }
}

#[tokio::test]
async fn requires_the_independent_host_capability_and_rejects_browser_callers_raw_secrets_and_fake_bots()
 {
    let f = fixture(Duration::from_secs(1)).await;
    assert_eq!(
        get(
            f.port,
            "/vault-host",
            &[("Authorization", "Bearer ordinary-rpc-token")]
        )
        .await
        .status,
        401
    );
    assert_eq!(
        get(
            f.port,
            "/vault-host",
            &[("Authorization", &auth()), ("Origin", "app://tjommie")]
        )
        .await
        .status,
        403
    );
    for forged in [
        json!({ "controllerId": "local", "action": { "kind": "entry", "botId": "one", "password": "SYNTHETIC_NEVER_TRANSPORT" } }),
        json!({ "controllerId": "local", "action": { "kind": "status" }, "bot": { "homeFolder": "C:/private" } }),
    ] {
        let refused = f
            .broker
            .request(forged, VaultRequestOptions::default())
            .await
            .unwrap_err();
        assert_eq!(refused.to_string(), "Invalid Logins command.");
    }
    f.close().await;
}

#[tokio::test]
async fn delivers_strict_global_metadata_commands_once_without_a_stored_bot_payload() {
    let f = fixture(Duration::from_secs(1)).await;
    let result = f.request(json!({ "controllerId": "local", "action": { "kind": "status" } }));
    let job = get(f.port, "/vault-host", &[("Authorization", &auth())])
        .await
        .json();
    let mut keys: Vec<&String> = job.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["command", "id", "sequence"]);
    assert_eq!(
        job["command"],
        json!({ "controllerId": "local", "action": { "kind": "status" } })
    );
    protocol::parse::<protocol::vault_host::VaultHostJob>(job.clone()).unwrap();
    let reply = json!({ "id": job["id"], "ok": true, "result": status() }).to_string();
    assert_eq!(
        post(
            f.port,
            "/vault-host",
            &[("Authorization", &auth())],
            reply.as_bytes()
        )
        .await
        .status,
        204
    );
    assert_eq!(result.await.unwrap(), Ok(status()));
    assert_eq!(
        post(
            f.port,
            "/vault-host",
            &[("Authorization", &auth())],
            reply.as_bytes()
        )
        .await
        .status,
        404
    );
    f.close().await;
}

#[tokio::test]
async fn rejects_secret_bearing_host_responses_with_a_fixed_error_and_bounds_the_body() {
    let f = fixture(Duration::from_secs(1)).await;
    let pending = f.request(json!({ "controllerId": "local", "action": { "kind": "status" } }));
    let job = get(f.port, "/vault-host", &[("Authorization", &auth())])
        .await
        .json();
    let mut leaking = status();
    leaking["password"] = json!("SYNTHETIC_NEVER_RETURN");
    let reply = json!({ "id": job["id"], "ok": true, "result": leaking }).to_string();
    post(
        f.port,
        "/vault-host",
        &[("Authorization", &auth())],
        reply.as_bytes(),
    )
    .await;
    let refused = pending.await.unwrap().unwrap_err();
    assert_eq!(
        refused.to_string(),
        "The desktop returned an invalid Logins response."
    );
    assert!(!format!("{refused:?}").contains("SYNTHETIC"));
    let oversized = vec![b'x'; 4 * 1024 * 1024 + 1];
    assert_eq!(
        post(
            f.port,
            "/vault-host",
            &[("Authorization", &auth())],
            &oversized
        )
        .await
        .status,
        413
    );
    f.close().await;
}

#[tokio::test]
async fn expires_absent_host_work_and_rejects_unsent_work_when_its_request_aborts() {
    let f = fixture(Duration::from_millis(20)).await;
    let expired = f
        .broker
        .request(
            json!({ "controllerId": "local", "action": { "kind": "status" } }),
            VaultRequestOptions::default(),
        )
        .await;
    assert!(expired.unwrap_err().to_string().contains("expired"));
    let f = fixture(Duration::from_secs(5)).await;
    let abort = CancellationToken::new();
    let options = VaultRequestOptions {
        signal: Some(abort.clone()),
        authorized: None,
    };
    let broker = f.broker.clone();
    let listed = tokio::spawn(async move {
        broker
            .request(
                json!({ "controllerId": "local", "action": { "kind": "list" } }),
                options,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    abort.cancel();
    assert!(
        listed
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    f.close().await;
}

#[tokio::test]
async fn repeated_cancel_also_removes_newly_queued_work_while_the_earlier_cancel_is_awaiting_the_host()
 {
    let f = fixture(Duration::from_secs(1)).await;
    let first = f.request(json!({ "controllerId": "rpc:one", "action": { "kind": "cancel" } }));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let queued = f.request(
        json!({ "controllerId": "rpc:one", "action": { "kind": "entry", "botId": "one" } }),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    let repeated = f.request(json!({ "controllerId": "rpc:one", "action": { "kind": "cancel" } }));
    assert!(
        queued
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    let job = get(
        f.port,
        "/vault-host?control=1",
        &[("Authorization", &auth())],
    )
    .await
    .json();
    assert_eq!(job["command"]["action"]["kind"], "cancel");
    let reply = json!({ "id": job["id"], "ok": true, "result": {} }).to_string();
    post(
        f.port,
        "/vault-host",
        &[("Authorization", &auth())],
        reply.as_bytes(),
    )
    .await;
    // The repeat joined the first cancel: one job, one answer for both.
    assert_eq!(first.await.unwrap(), Ok(json!({})));
    assert_eq!(repeated.await.unwrap(), Ok(json!({})));
    f.close().await;
}

#[tokio::test]
async fn answers_a_replaced_poll_with_204_and_waiting_polls_with_503_on_close() {
    let f = fixture(Duration::from_secs(2)).await;
    let port = f.port;
    let stale =
        tokio::spawn(async move { get(port, "/vault-host", &[("Authorization", &auth())]).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let fresh =
        tokio::spawn(async move { get(port, "/vault-host", &[("Authorization", &auth())]).await });
    assert_eq!(
        stale.await.unwrap().status,
        204,
        "a newer poll replaces the older one"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    f.broker.close();
    assert_eq!(fresh.await.unwrap().status, 503);
    assert_eq!(
        get(port, "/vault-host", &[("Authorization", &auth())])
            .await
            .status,
        503
    );
    f.server.close().await;
}

#[tokio::test]
async fn sends_urgent_work_on_the_control_lane_after_cancelling_earlier_normal_work() {
    let f = fixture(Duration::from_secs(2)).await;
    let listed = f.request(json!({ "controllerId": "rpc:two", "action": { "kind": "list" } }));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let lock = f.request(json!({ "controllerId": "rpc:one", "action": { "kind": "lock" } }));
    assert!(
        listed
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled"),
        "a lock cancels every normal job"
    );
    let control = get(
        f.port,
        "/vault-host?control=1",
        &[("Authorization", &auth())],
    )
    .await
    .json();
    assert_eq!(control["command"]["action"]["kind"], "lock");
    let reply = json!({ "id": control["id"], "ok": true, "result": status() }).to_string();
    post(
        f.port,
        "/vault-host",
        &[("Authorization", &auth())],
        reply.as_bytes(),
    )
    .await;
    assert_eq!(lock.await.unwrap(), Ok(status()));
    f.close().await;
}
