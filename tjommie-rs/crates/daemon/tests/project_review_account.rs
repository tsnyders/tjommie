//! Port of `packages/daemon/src/project-review-account.test.ts`. Probes and credentials are synthetic fakes, as in the
//! TypeScript fixture; the usage rows, the lease and the cap are the real account runtime's.

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use daemon::account_quota::{AccountUsageRead, now_ms};
use daemon::account_runtime::{AccountOps, AccountRuntime, AccountRuntimeOptions, CredentialStore};
use daemon::accounts_store::{AccountInput, create_account};
use daemon::bots::{create_bot, update_bot};
use daemon::cli::{BoxFuture, Env};
use daemon::context::Db;
use daemon::project_review::{ReviewContext, ReviewInstructions};
use daemon::project_review_account::review_account_resolver;
use daemon::project_review_runtime::{
    ReviewAccount, ReviewAccountResolver, ReviewAccountSelection,
};
use daemon::providers::{ProviderEvent, TokenUsage, Usage};
use daemon::server::{Handlers, RpcError, Server, ServerOptions};
use daemon::setup::Account;
use protocol::accounts::{AccountKind, SignInStart};
use protocol::model::{
    BotColor, BotInput, BotPatch, BotShape, CodexSandboxReadiness, CodexSetup, Provider,
    ProviderSetup, SetupFields,
};
use protocol::{ErrorCode, Event};
use tokio_util::sync::CancellationToken;

/// `probe`: every account is installed, signed in and ready.
struct Healthy;

fn healthy(account: &Account) -> ProviderSetup {
    let (version, fields) = match account.provider {
        Provider::Claude => ("2.1.314", None),
        Provider::Codex => ("0.160.0", Some(CodexSandboxReadiness::Ready)),
    };
    let setup = SetupFields {
        installed: true,
        version: Some(version.into()),
        signed_in: true,
        account: None,
        plan: None,
        error: None,
    };
    match fields {
        None => ProviderSetup::Claude(setup),
        Some(sandbox) => ProviderSetup::Codex(CodexSetup { setup, sandbox }),
    }
}

impl AccountOps for Healthy {
    fn check(&self, account: &Account) -> BoxFuture<'_, ProviderSetup> {
        let setup = healthy(account);
        Box::pin(async move { setup })
    }

    /// `probeLimit: async () => null`: the legacy limit probe finds nothing.
    fn uses_legacy_limit_probe(&self) -> bool {
        true
    }

    fn check_usage(&self, _account: &Account, _root: &Path) -> BoxFuture<'_, AccountUsageRead> {
        Box::pin(async { AccountUsageRead::Error { error: None } })
    }

    fn sign_in(
        &self,
        _account: &Account,
        _on_done: Arc<dyn Fn(bool, String) + Send + Sync>,
    ) -> BoxFuture<'_, Result<SignInStart, RpcError>> {
        Box::pin(async {
            Err(RpcError::new(
                ErrorCode::BadState,
                "no sign-in in this fixture",
            ))
        })
    }

    fn cancel_sign_in(
        &self,
        _account: &Account,
        _attempt: &str,
    ) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_sandbox(&self, _account: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn logout(&self, _account: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_folder(&self, account: &Account, _data_root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(&account.folder)
    }

    fn account_env(&self, _account: &Account) -> Env {
        Env::new()
    }
}

/// The TypeScript `read` spy: keys in memory, and a count of how often one was read.
#[derive(Default)]
struct Keys {
    keys: Mutex<std::collections::HashMap<String, String>>,
    reads: AtomicUsize,
}

impl CredentialStore for Keys {
    fn reference(&self, account_id: &str) -> Result<String, RpcError> {
        Ok(format!("fixture/{account_id}"))
    }

    fn read(&self, account_id: &str) -> BoxFuture<'_, Result<Option<String>, RpcError>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let key = self.keys.lock().unwrap().get(account_id).cloned();
        Box::pin(async move { Ok(key) })
    }

    fn write(&self, account_id: &str, key: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.keys
            .lock()
            .unwrap()
            .insert(account_id.into(), key.into());
        Box::pin(async { Ok(()) })
    }

    fn delete(&self, account_id: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.keys.lock().unwrap().remove(account_id);
        Box::pin(async { Ok(()) })
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    server: Server,
    db: Db,
    accounts: AccountRuntime,
    keys: Arc<Keys>,
    /// The `broadcast` spy: how many `usage.changed` events went out.
    usage_changed: Arc<AtomicUsize>,
    context: ReviewContext,
    resolver: Arc<dyn ReviewAccountResolver>,
}

async fn fixture(api: bool) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let (accounts_dir, bots_dir) = (root.path().join("accounts"), root.path().join("bots"));
    std::fs::create_dir_all(&bots_dir).unwrap();
    let connection = common::test_db(root.path());
    common::no_cli_setup()
        .ensure_accounts(&connection, &accounts_dir, root.path())
        .unwrap();
    if api {
        create_account(
            &connection,
            AccountInput {
                id: Some("review-api".into()),
                provider: Provider::Claude,
                label: "Review fixture".into(),
                kind: AccountKind::ApiKey,
                folder: accounts_dir
                    .join("review-api")
                    .to_string_lossy()
                    .into_owned(),
                credential_ref: None,
                is_default: false,
                is_fallback: false,
                spending_cap_usd: Some(1.0),
            },
        )
        .unwrap();
    }
    let keys = Arc::new(Keys::default());
    let db: Db = Arc::new(Mutex::new(connection));
    let accounts = AccountRuntime::new(AccountRuntimeOptions {
        db: db.clone(),
        root: root.path().to_path_buf(),
        accounts: accounts_dir,
        credentials: keys.clone(),
        ops: Arc::new(Healthy),
        on_changed: None,
        on_sign_in_ended: None,
        on_cap: None,
        now: Arc::new(now_ms),
    });
    if api {
        accounts
            .set_api_key("review-api", "synthetic-fixture")
            .await
            .unwrap();
    }
    accounts.check_all().await.unwrap();
    keys.reads.store(0, Ordering::SeqCst);
    let bot = {
        let db = db.lock().unwrap();
        let bot = create_bot(
            &db,
            &bots_dir,
            &BotInput {
                name: "Reviewer".into(),
                color: BotColor::Blue,
                shape: BotShape::Circle,
                instructions: "Review".into(),
                provider: Provider::Claude,
                model: "claude-sonnet-4-6".into(),
                account_id: None,
                autonomy: None,
                project_folder: None,
                browser: None,
                can_message: None,
            },
        )
        .unwrap();
        if api {
            update_bot(
                &db,
                &bot.id,
                &BotPatch {
                    account_id: Some(Some("review-api".into())),
                    ..BotPatch::default()
                },
            )
            .unwrap();
        }
        bot
    };
    let server = Server::start(ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec![],
        defer_requests: true,
    })
    .await
    .unwrap();
    let usage_changed = Arc::new(AtomicUsize::new(0));
    let counted = usage_changed.clone();
    server.tap_events(move |event| {
        if matches!(event, Event::UsageChanged(_)) {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let context = ReviewContext {
        bot_id: bot.id,
        project_id: "project".into(),
        task_id: "task".into(),
        attempt_id: "attempt".into(),
        evaluation_id: "evaluation".into(),
        revision: 1,
        project_revision: 1,
        task_revision: 1,
        instructions: ReviewInstructions {
            reviewer: "Review".into(),
            role: "Review source".into(),
            title: "Task".into(),
            task: "Check the change".into(),
            acceptance: Vec::new(),
        },
    };
    let resolver = review_account_resolver(db.clone(), accounts.clone(), server.broadcaster());
    Fixture {
        _root: root,
        server,
        db,
        accounts,
        keys,
        usage_changed,
        context,
        resolver,
    }
}

#[tokio::test]
async fn keeps_subscription_reviews_unavailable_before_reading_credentials() {
    let h = fixture(false).await;
    let selection = h
        .resolver
        .resolve(&h.context, &CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(selection, ReviewAccountSelection::Unavailable(_)));
    assert_eq!(h.keys.reads.load(Ordering::SeqCst), 0);
    h.server.close().await;
}

#[tokio::test]
async fn charges_review_events_once_refreshes_the_remaining_cap_and_releases_the_account_lease() {
    let h = fixture(true).await;
    let account: Arc<dyn ReviewAccount> = match h
        .resolver
        .resolve(&h.context, &CancellationToken::new())
        .await
        .unwrap()
    {
        ReviewAccountSelection::Ready(account) => account,
        ReviewAccountSelection::Unavailable(reason) => panic!("{reason}"),
    };
    assert_eq!(account.max_budget_usd().unwrap(), Some(1.0));
    assert!(h.accounts.in_use("review-api"));
    let event = ProviderEvent::Usage(Usage {
        model: "claude-sonnet-4-6".into(),
        tokens: TokenUsage {
            input: 2.0,
            cached_input: 0.0,
            output: 1.0,
            reasoning: 0.0,
        },
        cost_usd: Some(0.2),
        total_cost_usd: None,
        event_id: Some("acceptance:1".into()),
    });
    account.on_event(event.clone()).unwrap();
    account.on_event(event).unwrap();
    let remaining = account.max_budget_usd().unwrap().unwrap();
    assert!((remaining - 0.8).abs() < 1e-9, "{remaining}");
    let rows: i64 =
        h.db.lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage", [], |row| row.get(0))
            .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(h.usage_changed.load(Ordering::SeqCst), 1);
    account.release();
    account.release();
    assert!(!h.accounts.in_use("review-api"));
    h.server.close().await;
}

#[tokio::test]
async fn does_not_acquire_a_lease_after_cancellation() {
    let h = fixture(true).await;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(h.resolver.resolve(&h.context, &cancelled).await.is_err());
    assert_eq!(h.keys.reads.load(Ordering::SeqCst), 0);
    assert!(!h.accounts.in_use("review-api"));
    h.server.close().await;
}
