//! Port of `packages/daemon/src/vault-handlers.test.ts`: `vault.*` forwards to the desktop's Logins host under a
//! controller id derived from the caller's own connection, and never passes a secret or a native error back.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::server::{Handlers, Principal, RequestContext, Server, ServerOptions};
use daemon::vault_handlers;
use daemon::vault_host_broker::{VaultError, VaultRequest, VaultRequestOptions};
use futures_util::future::BoxFuture;
use protocol::ErrorCode;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn locked() -> Value {
    json!({ "state": "locked", "reason": null, "pendingDeletes": 0 })
}

fn context(signal: CancellationToken, local: bool) -> RequestContext {
    RequestContext {
        connection_id: "authenticated-one".into(),
        principal: if local {
            Principal::Local
        } else {
            Principal::Device {
                device_id: "device-one".into(),
            }
        },
        signal,
    }
}

type Calls = Arc<Mutex<Vec<(Value, bool)>>>;

/// A request function that records each command (and whether it came with a signal) and answers with `answer`.
fn recording(
    answer: impl Fn(&Value) -> BoxFuture<'static, Result<Value, VaultError>> + Send + Sync + 'static,
) -> (VaultRequest, Calls) {
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let request: VaultRequest = Arc::new(move |command: Value, options: VaultRequestOptions| {
        seen.lock().unwrap().push((
            command.clone(),
            options.signal.is_some() && options.authorized.is_some(),
        ));
        answer(&command)
    });
    (request, calls)
}

async fn server(request: VaultRequest) -> Server {
    Server::start(ServerOptions {
        port: 0,
        token: "synthetic-token".into(),
        handlers: vault_handlers::register(Handlers::default(), request),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn derives_local_or_device_controller_identity_from_trusted_context_and_forwards_no_secret_or_bot_payload()
 {
    let (request, calls) = recording(|_| Box::pin(async { Ok(locked()) }));
    let server = server(request).await;
    let status = server
        .dispatch(
            "vault.status",
            json!({}),
            Some(context(CancellationToken::new(), false)),
        )
        .await
        .unwrap();
    protocol::assert_valid("vault.status", &status);
    assert_eq!(
        calls.lock().unwrap().last().unwrap(),
        &(
            json!({ "controllerId": "rpc:authenticated-one", "action": { "kind": "status" } }),
            true
        )
    );
    server
        .dispatch(
            "vault.lock",
            json!({}),
            Some(context(CancellationToken::new(), true)),
        )
        .await
        .unwrap();
    assert_eq!(
        calls.lock().unwrap().last().unwrap().0,
        json!({ "controllerId": "rpc:authenticated-one", "action": { "kind": "lock" } })
    );
    let refused = server
        .dispatch("vault.unlock", json!({}), None)
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Forbidden);
    let secret = server
        .dispatch(
            "vault.entry",
            json!({ "botId": "one", "password": "SYNTHETIC_NEVER_TRANSPORT" }),
            Some(context(CancellationToken::new(), false)),
        )
        .await
        .unwrap_err();
    assert_eq!(secret.code, ErrorCode::BadParams);
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|(command, _)| command.to_string().contains("SYNTHETIC"))
    );
    server.close().await;
}

#[tokio::test]
async fn sends_urgent_disconnect_when_the_authenticated_lifetime_ends_discarding_late_native_results()
 {
    let finish: Arc<Mutex<Option<oneshot::Sender<Value>>>> = Arc::default();
    let parked = finish.clone();
    let (request, calls) = recording(move |command| {
        if command["action"]["kind"] == "entry" {
            let (send, receive) = oneshot::channel();
            *parked.lock().unwrap() = Some(send);
            Box::pin(async move { receive.await.map_err(|_| VaultError::Unavailable) })
        } else {
            Box::pin(async { Ok(json!({})) })
        }
    });
    let server = Arc::new(server(request).await);
    let lifetime = CancellationToken::new();
    let caller = server.clone();
    let signal = lifetime.clone();
    let pending = tokio::spawn(async move {
        caller
            .dispatch(
                "vault.entry",
                json!({ "botId": "one" }),
                Some(context(signal, false)),
            )
            .await
    });
    while finish.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    lifetime.cancel();
    let disconnect =
        json!({ "controllerId": "rpc:authenticated-one", "action": { "kind": "disconnect" } });
    for _ in 0..200 {
        if calls
            .lock()
            .unwrap()
            .iter()
            .any(|(command, _)| *command == disconnect)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|(command, _)| *command == disconnect),
        "the ended connection disconnects its controller"
    );
    let _ = finish
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(json!({ "status": "saved", "entry": null }));
    let refused = pending.await.unwrap().unwrap_err();
    assert!(refused.message.contains("safely"), "{}", refused.message);
}

#[tokio::test]
async fn does_not_expose_a_native_error_or_a_malformed_secret_bearing_result() {
    let (failing, _) = recording(|_| Box::pin(async { Err(VaultError::HostFailed) }));
    let server = server(failing).await;
    let refused = server
        .dispatch(
            "vault.status",
            json!({}),
            Some(context(CancellationToken::new(), false)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.message,
        "Logins could not complete the request safely. Check the host; any private browser remains private."
    );
    server.close().await;
    let (malformed, _) = recording(|_| {
        let mut leaking = locked();
        leaking["password"] = json!("SYNTHETIC_SECRET_RESULT");
        Box::pin(async move { Ok(leaking) })
    });
    let server = server_with(malformed).await;
    let refused = server
        .dispatch(
            "vault.status",
            json!({}),
            Some(context(CancellationToken::new(), false)),
        )
        .await
        .unwrap_err();
    assert!(refused.message.contains("Logins could not complete"));
    assert!(!refused.message.contains("SYNTHETIC"));
    server.close().await;
}

async fn server_with(request: VaultRequest) -> Server {
    server(request).await
}
