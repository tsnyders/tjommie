//! Ports of `account-handlers.test.ts` and the runner-free cases of `accounts-integration.test.ts`, over the WebSocket
//! with the real account runtime. Provider probes, sign-in, sign-out and API keys are synthetic.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{Client, TempRoot, connect, test_context};
use daemon::account_quota::AccountUsageRead;
use daemon::account_runtime::{AccountOps, AccountSelection, CredentialStore};
use daemon::accounts_store::{AccountInput, create_account, get_account};
use daemon::bots::get_stored_bot;
use daemon::cli::{BoxFuture, Env};
use daemon::context::{Context, handlers};
use daemon::providers::{Limit, LimitState};
use daemon::server::{Handlers, RpcError, Server, ServerOptions};
use daemon::setup::{Account, OnDone};
use protocol::ErrorCode;
use protocol::accounts::{AccountKind, SignInStart};
use protocol::model::{CodexSandboxReadiness, CodexSetup, Provider, ProviderSetup, SetupFields};
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// Healthy providers, as the TypeScript integration fixture's probe reports them.
#[derive(Default)]
struct Ops {
    logout_fails: AtomicBool,
    logouts: AtomicUsize,
}

impl AccountOps for Ops {
    fn check(&self, account: &Account) -> BoxFuture<'_, Result<ProviderSetup, RpcError>> {
        let fields = |version: &str| SetupFields {
            installed: true,
            version: Some(version.into()),
            signed_in: true,
            account: Some(format!("{}@example.test", account.id)),
            plan: Some("synthetic".into()),
            error: None,
        };
        let setup = match account.provider {
            Provider::Claude => ProviderSetup::Claude(fields("2.1.304")),
            Provider::Codex => ProviderSetup::Codex(CodexSetup {
                setup: fields("0.160.0"),
                sandbox: CodexSandboxReadiness::Ready,
            }),
        };
        Box::pin(async move { Ok(setup) })
    }

    fn check_usage(&self, _: &Account, _: &Path) -> BoxFuture<'_, AccountUsageRead> {
        Box::pin(async { AccountUsageRead::Unavailable { error: None } })
    }

    fn sign_in(&self, _: &Account, _: OnDone) -> BoxFuture<'_, Result<SignInStart, RpcError>> {
        Box::pin(async {
            Ok(SignInStart {
                url: None,
                attempt: "synthetic-attempt".into(),
            })
        })
    }

    fn cancel_sign_in(&self, _: &Account, _: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_sandbox(&self, _: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn logout(&self, _: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        self.logouts.fetch_add(1, Ordering::SeqCst);
        let fails = self.logout_fails.load(Ordering::SeqCst);
        Box::pin(async move {
            if fails {
                Err(RpcError::new(
                    ErrorCode::Internal,
                    "synthetic logout failure",
                ))
            } else {
                Ok(())
            }
        })
    }

    fn prepare_folder(&self, account: &Account, _: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(&account.folder)
    }

    fn account_env(&self, _: &Account) -> Env {
        Env::new()
    }
}

#[derive(Default)]
struct Keys {
    keys: Mutex<HashMap<String, String>>,
    writes: AtomicUsize,
    deletes: AtomicUsize,
    fail_delete: AtomicBool,
    pending_delete: Mutex<Option<oneshot::Receiver<()>>>,
}

impl CredentialStore for Keys {
    fn reference(&self, account_id: &str) -> Result<String, RpcError> {
        Ok(format!("synthetic-reference/{account_id}"))
    }

    fn read(&self, account_id: &str) -> BoxFuture<'_, Result<Option<String>, RpcError>> {
        let key = self.keys.lock().unwrap().get(account_id).cloned();
        Box::pin(async move { Ok(key) })
    }

    fn write(&self, account_id: &str, key: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.keys
            .lock()
            .unwrap()
            .insert(account_id.into(), key.into());
        Box::pin(async { Ok(()) })
    }

    fn delete(&self, account_id: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        let (pending, fails) = (
            self.pending_delete.lock().unwrap().take(),
            self.fail_delete.load(Ordering::SeqCst),
        );
        let account_id = account_id.to_string();
        Box::pin(async move {
            if let Some(pending) = pending {
                pending.await.unwrap();
            }
            if fails {
                return Err(RpcError::new(
                    ErrorCode::Internal,
                    "Synthetic credential-store failure",
                ));
            }
            self.keys.lock().unwrap().remove(&account_id);
            Ok(())
        })
    }
}

struct Harness {
    server: Server,
    client: Client,
    root: TempRoot,
    context: Context,
    keys: Arc<Keys>,
    ops: Arc<Ops>,
    next: i64,
}

async fn harness() -> Harness {
    let root = TempRoot::new();
    let options = ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec![],
        defer_requests: true,
    };
    let server = Server::start(options).await.unwrap();
    let (keys, ops) = (Arc::new(Keys::default()), Arc::new(Ops::default()));
    let context = test_context(&root.0, server.broadcaster(), ops.clone(), keys.clone());
    server.activate("t", handlers(&context)).unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    Harness {
        server,
        client,
        root,
        context,
        keys,
        ops,
        next: 0,
    }
}

impl Harness {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.client.call(self.next, method, params).await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params).await;
        assert_eq!(response["ok"], true, "{method}: {response}");
        response["result"].clone()
    }

    async fn error(&mut self, method: &str, params: Value) -> String {
        let response = self.call(method, params).await;
        assert_eq!(response["ok"], false, "{method}: {response}");
        response["error"]["message"].as_str().unwrap().to_string()
    }

    /// `account-handlers.test.ts`'s account: a Claude API key with a $5 cap.
    fn synthetic(&self) {
        let input = AccountInput {
            id: Some("synthetic".into()),
            provider: Provider::Claude,
            label: "Synthetic".into(),
            kind: AccountKind::ApiKey,
            folder: self.root.0.join("account").to_string_lossy().into_owned(),
            credential_ref: None,
            is_default: false,
            is_fallback: false,
            spending_cap_usd: Some(5.0),
        };
        create_account(&self.context.db.lock().unwrap(), input).unwrap();
    }

    fn deleted(&self, id: &str) -> bool {
        get_account(&self.context.db.lock().unwrap(), id, true)
            .unwrap()
            .deleted_at
            .is_some()
    }

    fn account_events(&self) -> usize {
        self.client
            .events
            .iter()
            .filter(|event| event["event"] == "accounts.changed")
            .count()
    }

    /// Starts a turn's account lease for `bot`, as the runner does.
    async fn lease(&self, bot: &str) -> daemon::account_runtime::AccountRunContext {
        let stored = get_stored_bot(&self.context.db.lock().unwrap(), bot).unwrap();
        let runtime = &self.context.accounts.runtime;
        let AccountSelection::Ready(context) = runtime.inspect(&stored) else {
            panic!("the bot's account is ready");
        };
        runtime.prepare(&context).await.unwrap();
        context
    }
}

/// The response to request `id`, sent earlier with `send`.
async fn response(client: &mut Client, id: i64) -> Value {
    loop {
        let frame = client.next().await;
        if frame["type"] != "event" {
            assert_eq!(frame["id"], id, "{frame}");
            return frame;
        }
        client.events.push(frame);
    }
}

fn bot(name: &str, account: Option<&str>) -> Value {
    let mut input = json!({ "name": name, "color": "blue", "shape": "circle", "provider": "claude", "model": "sonnet",
        "instructions": "Synthetic account tests", "autonomy": "ask", "browser": false });
    if let Some(account) = account {
        input["accountId"] = json!(account);
    }
    input
}

// account-handlers.test.ts

#[tokio::test]
async fn saves_only_a_secret_reference_and_returns_safe_metadata_when_creating_an_api_account() {
    let mut h = harness().await;
    let created = h
        .ok("accounts.create", json!({ "provider": "codex", "label": "API", "kind": "api_key", "apiKey": "synthetic-test-key", "spendingCapUsd": 10 }))
        .await;
    assert_eq!(created["hasCredential"], true);
    let id = created["id"].as_str().unwrap();
    let stored = get_account(&h.context.db.lock().unwrap(), id, false).unwrap();
    assert!(
        stored
            .credential_ref
            .unwrap()
            .starts_with("synthetic-reference/")
    );
    assert!(!created.to_string().contains("synthetic-test-key"));
    assert!(created.get("folder").is_none());
    let schema: String = h
        .context
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'accounts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!schema.contains("api_key_value"));
    h.server.close().await;
}

#[tokio::test]
async fn checks_account_pins_before_deleting_credentials() {
    let mut h = harness().await;
    h.synthetic();
    h.ok("bots.create", bot("Pinned", Some("synthetic"))).await;
    assert!(
        h.error("accounts.remove", json!({ "id": "synthetic" }))
            .await
            .contains("another account")
    );
    assert_eq!(h.keys.deletes.load(Ordering::SeqCst), 0);
    assert!(!h.deleted("synthetic"));
    h.server.close().await;
}

#[tokio::test]
async fn holds_the_account_across_asynchronous_credential_deletion_and_rejects_a_competing_mutation()
 {
    let mut h = harness().await;
    h.synthetic();
    let (finish, pending) = oneshot::channel();
    *h.keys.pending_delete.lock().unwrap() = Some(pending);
    h.client
        .send(json!({ "type": "req", "id": 100, "method": "accounts.remove", "params": { "id": "synthetic" } }))
        .await;
    while h.keys.deletes.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let competing = h
        .error(
            "accounts.update",
            json!({ "id": "synthetic", "patch": { "label": "Race" } }),
        )
        .await;
    assert!(competing.contains("already changing"), "{competing}");
    // Bots can't take the account mid-change either (daemon.ts:330-333).
    let taking = h
        .error("bots.create", bot("Taker", Some("synthetic")))
        .await;
    assert!(taking.contains("This account is changing"), "{taking}");
    finish.send(()).unwrap();
    assert_eq!(response(&mut h.client, 100).await["ok"], true);
    assert!(h.deleted("synthetic"));
    // The barrier is gone: the next change reaches the account, which is now removed.
    let after = h
        .error(
            "accounts.update",
            json!({ "id": "synthetic", "patch": { "label": "After" } }),
        )
        .await;
    assert!(!after.contains("already changing"), "{after}");
    h.server.close().await;
}

#[tokio::test]
async fn keeps_the_account_available_for_a_retry_when_credential_deletion_fails() {
    let mut h = harness().await;
    h.synthetic();
    h.keys.fail_delete.store(true, Ordering::SeqCst);
    assert!(
        h.error("accounts.remove", json!({ "id": "synthetic" }))
            .await
            .contains("credential-store failure")
    );
    assert!(!h.deleted("synthetic"));
    h.ok(
        "accounts.update",
        json!({ "id": "synthetic", "patch": { "label": "Still here" } }),
    )
    .await;
    h.server.close().await;
}

#[tokio::test]
async fn rejects_combined_credential_and_settings_updates_before_changing_either() {
    let mut h = harness().await;
    h.synthetic();
    let message = h
        .error(
            "accounts.update",
            json!({ "id": "synthetic", "patch": { "apiKey": "synthetic", "label": "Changed" } }),
        )
        .await;
    assert!(message.contains("separately"), "{message}");
    assert_eq!(h.keys.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        get_account(&h.context.db.lock().unwrap(), "synthetic", false)
            .unwrap()
            .label,
        "Synthetic"
    );
    h.server.close().await;
}

#[tokio::test]
async fn does_not_leave_a_mutation_barrier_behind_when_the_account_is_missing() {
    let mut h = harness().await;
    for _ in 0..2 {
        let message = h.error("accounts.remove", json!({ "id": "missing" })).await;
        assert!(message.contains("not found"), "{message}");
    }
    h.server.close().await;
}

#[tokio::test]
async fn renames_an_in_use_account_without_invalidating_its_running_authentication_context() {
    let mut h = harness().await;
    h.ok("accounts.check", json!({})).await;
    let pinned = h.ok("bots.create", bot("Working", None)).await;
    let running = h.lease(pinned["id"].as_str().unwrap()).await;
    let before = h.account_events();
    let renamed = h
        .ok("accounts.update", json!({ "id": running.effective_account_id, "patch": { "label": "Renamed during work" } }))
        .await;
    assert_eq!(renamed["label"], "Renamed during work");
    // No hold and no authentication change: the running turn's context is still current.
    h.context.accounts.runtime.assert_current(&running).unwrap();
    h.ok("accounts.list", json!({})).await;
    assert!(h.account_events() > before);
    h.context.accounts.runtime.release(&running);
    h.server.close().await;
}

#[tokio::test]
async fn requires_an_idle_account_before_authorizing_one_queued_work_limit_retry() {
    let mut h = harness().await;
    h.ok("accounts.check", json!({})).await;
    let pinned = h.ok("bots.create", bot("Limited", None)).await;
    let running = h.lease(pinned["id"].as_str().unwrap()).await;
    let reached = Limit {
        state: LimitState::Reached,
        used_percent: Some(100.0),
        resets_at: None,
        limit_id: None,
        windows: None,
    };
    let runtime = h.context.accounts.runtime.clone();
    runtime.observe_limit(&running, &reached);
    let id = running.effective_account_id.clone();
    assert!(
        h.error("accounts.retryLimit", json!({ "id": id }))
            .await
            .contains("Stop the agents")
    );
    runtime.release(&running);
    assert_eq!(
        h.ok("accounts.retryLimit", json!({ "id": id })).await["id"],
        id.as_str()
    );
    h.server.close().await;
}

// accounts-integration.test.ts: the cases that need no runner. The others run turns, so they come with T3.5.

#[tokio::test]
async fn keeps_the_pin_stable_when_choosing_another_provider_default() {
    let mut h = harness().await;
    h.ok("accounts.check", json!({})).await;
    let pinned = h.ok("bots.create", bot("Synthetic", None)).await;
    let other = h
        .ok(
            "accounts.create",
            json!({ "provider": "claude", "label": "New default", "kind": "subscription" }),
        )
        .await;
    h.ok(
        "accounts.update",
        json!({ "id": other["id"], "patch": { "isDefault": true } }),
    )
    .await;
    let listed = h.ok("bots.list", json!({})).await;
    let kept = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|bot| bot["id"] == pinned["id"])
        .unwrap();
    assert_eq!(kept["accountId"], pinned["accountId"]);
    assert_eq!(
        h.ok("bots.create", bot("Next", None)).await["accountId"],
        other["id"]
    );
    h.server.close().await;
}

#[tokio::test]
async fn does_not_leak_an_api_key_in_account_results_and_preserves_history_metadata_on_removal() {
    let mut h = harness().await;
    h.ok("accounts.check", json!({})).await;
    let key = "synthetic-only-not-a-real-api-key";
    let account = h
        .ok("accounts.create", json!({ "provider": "claude", "label": "Synthetic API", "kind": "api_key", "apiKey": key, "spendingCapUsd": 2 }))
        .await;
    let id = account["id"].as_str().unwrap().to_string();
    assert_eq!(
        h.keys.keys.lock().unwrap().get(&id).map(String::as_str),
        Some(key)
    );
    assert!(
        !h.ok("accounts.list", json!({}))
            .await
            .to_string()
            .contains(key)
    );
    assert!(account.get("credentialRef").is_none() && account.get("folder").is_none());
    h.ok("accounts.remove", json!({ "id": id })).await;
    assert!(!h.keys.keys.lock().unwrap().contains_key(&id));
    let listed = h.ok("accounts.list", json!({})).await;
    assert!(
        !listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == id.as_str())
    );
    assert!(h.deleted(&id));
    h.server.close().await;
}

#[tokio::test]
async fn keeps_a_subscription_visible_after_a_failed_local_logout_and_allows_retrying_removal() {
    let mut h = harness().await;
    h.ok("accounts.check", json!({})).await;
    h.ops.logout_fails.store(true, Ordering::SeqCst);
    let account = h
        .ok("accounts.create", json!({ "provider": "claude", "label": "Disposable subscription", "kind": "subscription" }))
        .await;
    let id = account["id"].as_str().unwrap().to_string();
    h.error("accounts.remove", json!({ "id": id })).await;
    let listed = h.ok("accounts.list", json!({})).await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == id.as_str())
    );
    h.ops.logout_fails.store(false, Ordering::SeqCst);
    h.ok("accounts.remove", json!({ "id": id })).await;
    assert_eq!(h.ops.logouts.load(Ordering::SeqCst), 2);
    let listed = h.ok("accounts.list", json!({})).await;
    assert!(
        !listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == id.as_str())
    );
    h.server.close().await;
}

// The setup.* methods over the default accounts (daemon.ts:422-436).

#[tokio::test]
async fn reports_setup_from_the_default_accounts_and_signs_in_through_them() {
    let mut h = harness().await;
    let setup = h.ok("setup.status", json!({})).await;
    let providers: Vec<&str> = setup
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["provider"].as_str().unwrap())
        .collect();
    assert_eq!(providers, ["claude", "codex"]);
    assert!(
        h.client
            .events
            .iter()
            .any(|event| event["event"] == "setup.changed")
    );
    let started = h.ok("setup.signIn", json!({ "provider": "claude" })).await;
    assert_eq!(started["attempt"], "synthetic-attempt");
    let info = h.context.accounts.runtime.info("claude-main").unwrap();
    assert_eq!(info.sign_in.unwrap().attempt, "synthetic-attempt");
    let codex = h.ok("setup.prepareCodexSandbox", json!({})).await;
    assert_eq!(
        (codex["provider"].as_str(), codex["sandbox"].as_str()),
        (Some("codex"), Some("ready"))
    );
    h.server.close().await;
}
