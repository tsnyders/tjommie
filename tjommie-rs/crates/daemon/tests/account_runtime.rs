//! Port of `packages/daemon/src/account-runtime.test.ts`. Probes, sign-in and credentials are synthetic fakes.

mod common;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use daemon::account_quota::{
    AccountUsageRead, LimitSnapshot, normalize_claude_usage, normalize_codex_usage,
};
use daemon::account_runtime::{
    AccountOps, AccountRunContext, AccountRuntime, AccountRuntimeOptions, AccountSelection,
    CredentialStore,
};
use daemon::accounts_store::{
    AccountInput, AccountPatch, create_account, get_account, update_account,
};
use daemon::bots::{StoredBot, create_bot, get_stored_bot};
use daemon::cli::{BoxFuture, Env};
use daemon::providers::{Limit, LimitState, LimitWindow};
use daemon::server::RpcError;
use daemon::setup::{Account, parse_codex_account_limits};
use daemon::usage::{CostSource, UsageInput, record_usage};
use protocol::ErrorCode;
use protocol::accounts::{AccountKind, SignInStart, UsageStatus};
use protocol::model::{
    BotColor, BotInput, BotShape, CodexSandboxReadiness, CodexSetup, Provider, ProviderSetup,
    SetupFields,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::sync::oneshot;

type Pending<T> = BoxFuture<'static, T>;

/// A scripted async probe: queued answers first, then the default.
struct Script<T> {
    once: Mutex<VecDeque<Pending<T>>>,
    default: Box<dyn Fn(&Account) -> T + Send + Sync>,
    calls: Mutex<Vec<String>>,
}

impl<T: Send + 'static> Script<T> {
    fn new(default: impl Fn(&Account) -> T + Send + Sync + 'static) -> Self {
        Script {
            once: Mutex::default(),
            default: Box::new(default),
            calls: Mutex::default(),
        }
    }

    fn then(&self, value: T) {
        self.once
            .lock()
            .unwrap()
            .push_back(Box::pin(async move { value }));
    }

    /// An answer the test sends later.
    fn pending(&self) -> oneshot::Sender<T> {
        let (sender, receiver) = oneshot::channel();
        self.once.lock().unwrap().push_back(Box::pin(async move {
            receiver.await.expect("the test answers")
        }));
        sender
    }

    fn call(&self, account: &Account) -> Pending<T> {
        self.calls.lock().unwrap().push(account.id.clone());
        match self.once.lock().unwrap().pop_front() {
            Some(answer) => answer,
            None => {
                let value = (self.default)(account);
                Box::pin(async move { value })
            }
        }
    }

    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

fn healthy(account: &Account) -> ProviderSetup {
    let fields = SetupFields {
        installed: true,
        version: Some("2.1.304".into()),
        signed_in: true,
        account: Some("synthetic@example.invalid".into()),
        plan: Some("fixture".into()),
        error: None,
    };
    match account.provider {
        Provider::Claude => ProviderSetup::Claude(fields),
        Provider::Codex => ProviderSetup::Codex(CodexSetup {
            setup: fields,
            sandbox: CodexSandboxReadiness::Ready,
        }),
    }
}

type Login = Box<
    dyn Fn(&Account, Arc<dyn Fn(bool, String) + Send + Sync>) -> Result<SignInStart, RpcError>
        + Send
        + Sync,
>;

struct Ops {
    probe: Script<ProviderSetup>,
    /// `None`: the full usage probe is not injected, so the legacy limit probe runs (as in TypeScript's fixture).
    probe_usage: Option<Script<Result<AccountUsageRead, ()>>>,
    probe_limit: Script<Result<Option<Vec<LimitSnapshot>>, ()>>,
    login: Login,
    cancels: Mutex<Vec<(String, String)>>,
    logout: Script<Result<(), RpcError>>,
    /// The setup probe fails outright, as a worker termination does (`probe` throws in TypeScript).
    check_fails: AtomicBool,
    /// What the CLI lookup finds when a probe failed before it could say.
    cli_installed: AtomicBool,
}

impl AccountOps for Ops {
    fn check(&self, account: &Account) -> BoxFuture<'_, Result<ProviderSetup, RpcError>> {
        if self.check_fails.load(Ordering::SeqCst) {
            return Box::pin(async {
                Err(RpcError::new(
                    ErrorCode::Internal,
                    "synthetic worker termination",
                ))
            });
        }
        let answer = self.probe.call(account);
        Box::pin(async move { Ok(answer.await) })
    }

    fn installed(&self, _provider: Provider) -> bool {
        self.cli_installed.load(Ordering::SeqCst)
    }

    fn uses_legacy_limit_probe(&self) -> bool {
        self.probe_usage.is_none()
    }

    fn check_limits(&self, account: &Account) -> BoxFuture<'_, Option<Vec<LimitSnapshot>>> {
        let answer = self.probe_limit.call(account);
        Box::pin(async move { answer.await.unwrap_or(None) })
    }

    fn check_usage(
        &self,
        account: &Account,
        _accounts_root: &std::path::Path,
    ) -> BoxFuture<'_, AccountUsageRead> {
        let answer = self
            .probe_usage
            .as_ref()
            .expect("a usage probe")
            .call(account);
        Box::pin(async move {
            answer.await.unwrap_or(AccountUsageRead::Error {
                error: Some("Provider usage could not be read. Try again later.".into()),
            })
        })
    }

    fn sign_in(
        &self,
        account: &Account,
        on_done: Arc<dyn Fn(bool, String) + Send + Sync>,
    ) -> BoxFuture<'_, Result<SignInStart, RpcError>> {
        let started = (self.login)(account, on_done);
        Box::pin(async move { started })
    }

    fn cancel_sign_in(
        &self,
        account: &Account,
        attempt: &str,
    ) -> BoxFuture<'_, Result<(), RpcError>> {
        self.cancels
            .lock()
            .unwrap()
            .push((account.id.clone(), attempt.to_string()));
        Box::pin(async { Ok(()) })
    }

    fn prepare_sandbox(&self, _account: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn logout(&self, account: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        self.logout.call(account)
    }

    fn prepare_folder(
        &self,
        account: &Account,
        _data_root: &std::path::Path,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(&account.folder)
    }

    fn account_env(&self, account: &Account) -> Env {
        let key = if account.provider == Provider::Claude {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        Env::from([(key.to_string(), account.folder.clone())])
    }
}

#[derive(Default)]
struct Keys {
    keys: Mutex<HashMap<String, String>>,
    reads: AtomicUsize,
    deletes: AtomicUsize,
    pending_read: Mutex<Option<oneshot::Receiver<String>>>,
}

impl CredentialStore for Keys {
    fn reference(&self, account_id: &str) -> Result<String, RpcError> {
        Ok(format!("fixture/{account_id}"))
    }

    fn read(&self, account_id: &str) -> BoxFuture<'_, Result<Option<String>, RpcError>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let pending = self.pending_read.lock().unwrap().take();
        let stored = self.keys.lock().unwrap().get(account_id).cloned();
        Box::pin(async move {
            Ok(match pending {
                Some(pending) => Some(pending.await.unwrap()),
                None => stored,
            })
        })
    }

    fn write(&self, account_id: &str, key: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.keys
            .lock()
            .unwrap()
            .insert(account_id.into(), key.into());
        Box::pin(async { Ok(()) })
    }

    fn delete(&self, account_id: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        self.keys.lock().unwrap().remove(account_id);
        Box::pin(async { Ok(()) })
    }
}

/// A clock the test can move (`Date.now`).
#[derive(Clone)]
struct Clock(Arc<Mutex<f64>>);

impl Clock {
    fn new() -> Self {
        Clock(Arc::new(Mutex::new(daemon::account_quota::now_ms())))
    }

    fn set(&self, ms: f64) {
        *self.0.lock().unwrap() = ms;
    }

    fn get(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    accounts: PathBuf,
    bots: PathBuf,
    db: Arc<Mutex<Connection>>,
    runtime: AccountRuntime,
    keys: Arc<Keys>,
    ops: Arc<Ops>,
    changed: Arc<Mutex<Vec<String>>>,
    caps: Arc<Mutex<Vec<(String, u8)>>>,
    ended: Arc<Mutex<Vec<(String, String)>>>,
    clock: Clock,
    bot: String,
}

fn ops() -> Ops {
    Ops {
        probe: Script::new(healthy),
        probe_usage: None,
        probe_limit: Script::new(|_| Ok(None)),
        login: Box::new(|_, _| {
            Err(RpcError::new(
                ErrorCode::BadState,
                "no login in this fixture",
            ))
        }),
        cancels: Mutex::default(),
        logout: Script::new(|_| Ok(())),
        check_fails: AtomicBool::new(false),
        cli_installed: AtomicBool::new(true),
    }
}

fn bot_input(name: &str, provider: Provider, model: &str) -> BotInput {
    BotInput {
        name: name.into(),
        color: BotColor::Blue,
        shape: BotShape::Circle,
        instructions: "Synthetic".into(),
        provider,
        model: model.into(),
        account_id: None,
        autonomy: None,
        project_folder: None,
        browser: None,
        can_message: None,
    }
}

fn fixture_with(ops: Ops) -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap();
    let (accounts, bots) = (root.path().join("accounts"), root.path().join("bots"));
    std::fs::create_dir_all(&bots).unwrap();
    let connection = common::test_db(root.path());
    let setup =
        daemon::setup::Setup::new(Arc::new(daemon::cli::SystemCli::new()), Arc::new(NoFetch));
    setup
        .ensure_accounts(&connection, &accounts, root.path())
        .unwrap();
    let bot = create_bot(
        &connection,
        &bots,
        &bot_input("Test", Provider::Codex, "gpt-5.3-codex"),
    )
    .unwrap()
    .id;
    let db = Arc::new(Mutex::new(connection));
    let (keys, ops) = (Arc::new(Keys::default()), Arc::new(ops));
    let (changed, caps, ended) = (
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    );
    let clock = Clock::new();
    let runtime = AccountRuntime::new(AccountRuntimeOptions {
        db: Arc::clone(&db),
        root: root.path().to_path_buf(),
        accounts: accounts.clone(),
        credentials: keys.clone(),
        ops: ops.clone(),
        on_changed: Some({
            let changed = Arc::clone(&changed);
            Arc::new(move |_: &AccountRuntime, id: &str| {
                changed.lock().unwrap().push(id.to_string())
            })
        }),
        on_sign_in_ended: Some({
            let ended = Arc::clone(&ended);
            Arc::new(move |_: &AccountRuntime, id: &str, attempt: &str| {
                ended
                    .lock()
                    .unwrap()
                    .push((id.to_string(), attempt.to_string()))
            })
        }),
        on_cap: Some({
            let caps = Arc::clone(&caps);
            Arc::new(move |_: &AccountRuntime, id: &str, threshold: u8| {
                caps.lock().unwrap().push((id.to_string(), threshold))
            })
        }),
        now: {
            let clock = clock.clone();
            Arc::new(move || clock.get())
        },
    });
    Fixture {
        _root: root,
        accounts,
        bots,
        db,
        runtime,
        keys,
        ops,
        changed,
        caps,
        ended,
        clock,
        bot,
    }
}

fn fixture() -> Fixture {
    fixture_with(ops())
}

struct NoFetch;

impl daemon::account_quota::UsageFetch for NoFetch {
    fn get<'a>(
        &'a self,
        _: &'a str,
        _: &'a [(&'static str, String)],
        _: std::time::Duration,
    ) -> BoxFuture<'a, Result<daemon::account_quota::UsageResponse, String>> {
        Box::pin(async { Err("no network in tests".to_string()) })
    }
}

impl Fixture {
    fn sql(&self, statement: &str) {
        self.db.lock().unwrap().execute_batch(statement).unwrap();
    }

    fn stored(&self, id: &str) -> StoredBot {
        get_stored_bot(&self.db.lock().unwrap(), id).unwrap()
    }

    /// As the runner inspects: with the database it already holds locked.
    fn inspect(&self) -> AccountSelection {
        let db = self.db.lock().unwrap();
        let bot = get_stored_bot(&db, &self.bot).unwrap();
        self.runtime.inspect_with(&db, &bot)
    }

    fn ready(&self) -> bool {
        matches!(self.inspect(), AccountSelection::Ready(_))
    }

    fn context(&self) -> AccountRunContext {
        match self.inspect() {
            AccountSelection::Ready(context) => context,
            AccountSelection::NotReady { reason, .. } => panic!("{reason}"),
        }
    }

    fn api(&self, id: &str, fallback: bool) {
        create_account(
            &self.db.lock().unwrap(),
            AccountInput {
                id: Some(id.into()),
                provider: Provider::Codex,
                label: id.into(),
                kind: AccountKind::ApiKey,
                folder: self.accounts.join(id).to_string_lossy().into_owned(),
                credential_ref: None,
                is_default: false,
                is_fallback: fallback,
                spending_cap_usd: Some(1.0),
            },
        )
        .unwrap();
    }

    fn update(&self, id: &str, patch: AccountPatch) {
        update_account(&self.db.lock().unwrap(), id, patch).unwrap();
    }

    fn usage(&self, id: &str, model: &str, cost: Option<f64>) {
        let input = UsageInput {
            id: Some(id.into()),
            bot_id: self.bot.clone(),
            account_id: "api".into(),
            provider: Provider::Codex,
            model: model.into(),
            input: 1.0,
            cached_input: 0.0,
            output: 0.0,
            reasoning: 0.0,
            cost_usd: cost,
            cost_source: if cost.is_some() {
                CostSource::Estimated
            } else {
                CostSource::Unknown
            },
            pricing_revision: None,
            created_at: None,
        };
        record_usage(&self.db.lock().unwrap(), &input).unwrap();
    }

    fn limits(&self, id: &str) -> Vec<protocol::accounts::AccountLimitWindow> {
        self.runtime.info(id).unwrap().limits
    }
}

fn reached(resets_at: Option<String>) -> Limit {
    Limit {
        state: LimitState::Reached,
        used_percent: Some(100.0),
        resets_at,
        limit_id: None,
        windows: None,
    }
}

fn window(
    key: &str,
    duration: Option<f64>,
    used: f64,
    resets_at: Option<String>,
    state: LimitState,
) -> LimitWindow {
    LimitWindow {
        window_key: key.into(),
        duration_minutes: duration,
        used_percent: Some(used),
        resets_at,
        state,
    }
}

fn detailed(windows: Vec<LimitWindow>) -> Limit {
    Limit {
        state: LimitState::Reached,
        used_percent: Some(100.0),
        resets_at: None,
        limit_id: Some("codex".into()),
        windows: Some(windows),
    }
}

fn codex_reading(ordinary: Value, spend_control: Value) -> AccountUsageRead {
    let value = json!({ "ordinaryUsageAllowed": ordinary, "rateLimits": { "limitId": "codex", "planType": "pro", "spendControlReached": spend_control,
        "primary": { "usedPercent": 31, "windowDurationMins": 10080, "resetsAt": null }, "credits": { "balance": "38917.83", "unlimited": false, "hasCredits": true } } });
    normalize_codex_usage(&value, parse_codex_account_limits(&value).as_deref())
}

fn healthy_reading() -> AccountUsageRead {
    codex_reading(json!(true), json!(false))
}

fn with_usage() -> Ops {
    Ops {
        probe_usage: Some(Script::new(|_| Ok(healthy_reading()))),
        ..ops()
    }
}

// describe("account runtime")

#[tokio::test]
async fn refreshes_healthy_signed_in_subscriptions_exposes_initial_unavailable_state_and_skips_api_keys()
 {
    let h = fixture_with(with_usage());
    h.api("api", false);
    h.runtime.set_api_key("api", "synthetic").await.unwrap();
    let initial = h
        .runtime
        .info("codex-main")
        .unwrap()
        .provider_usage
        .unwrap();
    assert_eq!(
        (initial.status, initial.checked_at, initial.windows.len()),
        (UsageStatus::Unavailable, None, 0)
    );
    h.runtime.check_all().await.unwrap();
    let mut probed = h
        .ops
        .probe_usage
        .as_ref()
        .unwrap()
        .calls
        .lock()
        .unwrap()
        .clone();
    probed.sort();
    assert_eq!(probed, ["claude-main", "codex-main"]);
    let usage = h
        .runtime
        .info("codex-main")
        .unwrap()
        .provider_usage
        .unwrap();
    assert_eq!(usage.status, UsageStatus::Available);
    assert!(usage.checked_at.is_some() && usage.last_successful_at.is_some());
    assert_eq!(usage.credits.unwrap().balance.as_deref(), Some("38917.83"));
    assert_eq!(
        (
            usage.windows[0].label.as_str(),
            usage.windows[0].used_percent
        ),
        ("Weekly", Some(31.0))
    );
    let api = h.runtime.info("api").unwrap().provider_usage.unwrap();
    assert_eq!(
        (api.status, api.checked_at),
        (UsageStatus::Unavailable, None)
    );
    h.changed.lock().unwrap().clear();
    h.runtime.list().unwrap();
    assert!(h.changed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retains_successful_metadata_and_limit_blocks_on_refresh_failure_with_generic_errors_only()
{
    let h = fixture_with(with_usage());
    h.runtime.check("codex-main").await.unwrap();
    let previous = h
        .runtime
        .info("codex-main")
        .unwrap()
        .provider_usage
        .unwrap();
    h.runtime.observe_limit(&h.context(), &reached(None));
    h.ops.probe_usage.as_ref().unwrap().then(Err(()));
    h.runtime.check("codex-main").await.unwrap();
    let usage = h
        .runtime
        .info("codex-main")
        .unwrap()
        .provider_usage
        .unwrap();
    assert_eq!(usage.status, UsageStatus::Error);
    assert_eq!(
        usage.error.as_deref(),
        Some("Provider usage could not be read. Try again later.")
    );
    assert_eq!(
        (usage.windows.clone(), usage.last_successful_at.clone()),
        (
            previous.windows.clone(),
            previous.last_successful_at.clone()
        )
    );
    assert!(!h.ready());
    h.runtime.metadata_changed("codex-main");
    assert_eq!(
        h.runtime
            .info("codex-main")
            .unwrap()
            .provider_usage
            .unwrap()
            .last_successful_at,
        previous.last_successful_at
    );
    h.runtime.changed("codex-main");
    let cleared = h
        .runtime
        .info("codex-main")
        .unwrap()
        .provider_usage
        .unwrap();
    assert_eq!(
        (
            cleared.status,
            cleared.last_successful_at,
            cleared.windows.len()
        ),
        (UsageStatus::Unavailable, None, 0)
    );
}

#[tokio::test]
async fn blocks_explicit_codex_permission_denial_despite_healthy_percentages_and_requires_affirmative_permission_recovery()
 {
    let h = fixture_with(Ops {
        probe_usage: Some(Script::new(|_| {
            Ok(codex_reading(json!(false), json!(false)))
        })),
        ..ops()
    });
    h.runtime.check("codex-main").await.unwrap();
    assert!(!h.ready());
    assert!(
        h.limits("codex-main")
            .iter()
            .any(|limit| limit.limit_id == "provider" && limit.window_key == "permission")
    );
    let usage = h.ops.probe_usage.as_ref().unwrap();
    usage.then(Ok(codex_reading(Value::Null, json!(false))));
    usage.then(Ok(codex_reading(json!(true), Value::Null)));
    usage.then(Ok(healthy_reading()));
    h.runtime.check("codex-main").await.unwrap();
    assert!(!h.ready());
    h.runtime.check("codex-main").await.unwrap();
    assert!(!h.ready());
    h.runtime.check("codex-main").await.unwrap();
    assert_eq!(h.context().effective_account_id, "codex-main");
}

#[tokio::test]
async fn missing_codex_gates_cannot_clear_a_specific_exhausted_window_with_a_healthy_replacement_percentage()
 {
    let h = fixture_with(with_usage());
    h.runtime.check("codex-main").await.unwrap();
    h.runtime.observe_limit(
        &h.context(),
        &detailed(vec![window(
            "primary",
            Some(10080.0),
            100.0,
            None,
            LimitState::Reached,
        )]),
    );
    h.ops
        .probe_usage
        .as_ref()
        .unwrap()
        .then(Ok(codex_reading(Value::Null, json!(false))));
    h.runtime.check("codex-main").await.unwrap();
    let info = h.runtime.info("codex-main").unwrap();
    assert_eq!(
        info.provider_usage.unwrap().windows[0].used_percent,
        Some(31.0)
    );
    assert!(info.limits.iter().any(
        |limit| limit.state == protocol::accounts::LimitState::Reached
            && limit.used_percent == Some(100.0)
    ));
    assert!(!h.ready());
    h.runtime.check("codex-main").await.unwrap();
    assert_eq!(h.context().effective_account_id, "codex-main");
}

#[tokio::test]
async fn claude_model_exhaustion_is_display_only_for_healthy_accounts_but_preserves_an_existing_generic_rejection()
 {
    let value = json!({ "five_hour": { "utilization": 8 }, "seven_day": { "utilization": 5 }, "limits": [{ "kind": "weekly_scoped", "percent": 100, "scope": { "model": { "display_name": "Opus" } } }] });
    let reading = {
        let value = value.clone();
        move |_: &Account| Ok(normalize_claude_usage(&value, Some("Max".into())))
    };
    let h = fixture_with(Ops {
        probe_usage: Some(Script::new(reading)),
        ..ops()
    });
    h.runtime.check("claude-main").await.unwrap();
    let bot = create_bot(
        &h.db.lock().unwrap(),
        &h.bots,
        &bot_input("Claude", Provider::Claude, "sonnet"),
    )
    .unwrap();
    let AccountSelection::Ready(context) = h.runtime.inspect(&bot) else {
        panic!("expected ready")
    };
    h.runtime.observe_limit(&context, &reached(None));
    h.runtime.check("claude-main").await.unwrap();
    assert!(matches!(
        h.runtime.inspect(&h.stored(&bot.id)),
        AccountSelection::NotReady { .. }
    ));
    let mut cleared = value;
    cleared["limits"] = json!([]);
    h.ops
        .probe_usage
        .as_ref()
        .unwrap()
        .then(Ok(normalize_claude_usage(&cleared, Some("Max".into()))));
    h.runtime.check("claude-main").await.unwrap();
    assert!(matches!(
        h.runtime.inspect(&h.stored(&bot.id)),
        AccountSelection::Ready(_)
    ));
}

#[tokio::test]
async fn discards_display_metadata_and_recovery_from_a_probe_overtaken_by_a_streamed_quota_event_or_auth_change()
 {
    let h = fixture_with(with_usage());
    h.runtime.check("codex-main").await.unwrap();
    let previous = h
        .runtime
        .info("codex-main")
        .unwrap()
        .provider_usage
        .unwrap();
    let context = h.context();
    for auth_change in [false, true] {
        let finish = h.ops.probe_usage.as_ref().unwrap().pending();
        let checking = tokio::spawn(h.runtime.check("codex-main"));
        common_settle().await;
        if auth_change {
            h.runtime.changed("codex-main");
        } else {
            h.runtime.observe_limit(&context, &reached(None));
        }
        assert!(finish.send(Ok(healthy_reading())).is_ok());
        checking.await.unwrap().unwrap();
        let usage = h
            .runtime
            .info("codex-main")
            .unwrap()
            .provider_usage
            .unwrap();
        if auth_change {
            assert_eq!(
                (usage.status, usage.windows.len()),
                (UsageStatus::Unavailable, 0)
            );
        } else {
            assert_eq!(usage, previous);
        }
        assert!(!h.ready());
    }
}

async fn common_settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn adopts_existing_subscription_folders_without_credentials_and_has_side_effect_free_metadata_reads()
 {
    let h = fixture();
    assert!(!h.ready());
    h.runtime.check_all().await.unwrap();
    h.changed.lock().unwrap().clear();
    let context = h.context();
    assert_eq!(context.effective_account_id, "codex-main");
    let prepared = h.runtime.prepare(&context).await.unwrap();
    assert_eq!(
        prepared.env.get("CODEX_HOME"),
        Some(&h.accounts.join("codex-main").to_string_lossy().into_owned())
    );
    assert!(prepared.api_key.is_none());
    assert_eq!(h.keys.reads.load(Ordering::SeqCst), 0);
    h.runtime.list().unwrap();
    h.runtime.info("codex-main").unwrap();
    assert!(h.changed.lock().unwrap().is_empty());
    assert!(h.runtime.in_use("codex-main"));
    h.runtime.release(&context);
    assert!(!h.runtime.in_use("codex-main"));
}

#[tokio::test]
async fn stores_an_api_key_without_probing_or_starting_a_request_never_returns_it_in_metadata() {
    let h = fixture();
    h.api("api", false);
    h.runtime
        .set_api_key("api", "synthetic-secret")
        .await
        .unwrap();
    assert_eq!(h.ops.probe.count(), 0);
    let info = h.runtime.info("api").unwrap();
    assert_eq!((info.has_credential, info.setup), (true, None));
    let listed = serde_json::to_string(&h.runtime.list().unwrap()).unwrap();
    assert!(!listed.contains("synthetic-secret") && !listed.contains("fixture/api"));
    assert!(
        h.runtime
            .set_api_key("codex-main", "synthetic-secret")
            .await
            .unwrap_err()
            .message
            .contains("Only API-key")
    );
}

#[tokio::test]
async fn blocks_unknown_reset_limits_for_every_pinned_bot_and_only_uses_an_opted_in_api_fallback() {
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    let context = h.context();
    h.runtime.observe_limit(&context, &reached(None));
    assert!(matches!(
        h.inspect(),
        AccountSelection::NotReady {
            limited_until: Some(None),
            ..
        }
    ));
    h.api("api", false);
    h.runtime
        .set_api_key("api", "synthetic-secret")
        .await
        .unwrap();
    h.runtime.check("api").await.unwrap();
    assert!(!h.ready());
    h.update(
        "api",
        AccountPatch {
            is_fallback: Some(true),
            ..AccountPatch::default()
        },
    );
    let fallback = h.context();
    assert_eq!(
        (
            fallback.pinned_account_id.as_str(),
            fallback.effective_account_id.as_str(),
            fallback.using_fallback
        ),
        ("codex-main", "api", true)
    );
    let ok = Limit {
        state: LimitState::Ok,
        used_percent: Some(1.0),
        resets_at: None,
        limit_id: None,
        windows: None,
    };
    h.runtime.observe_limit(&context, &ok);
    let back = h.context();
    assert_eq!(
        (back.effective_account_id.as_str(), back.using_fallback),
        ("codex-main", false)
    );
}

#[tokio::test]
async fn uses_all_quota_windows_and_does_not_invent_a_reset_or_replace_known_exhaustion_with_an_unknown_turn_error()
 {
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    let context = h.context();
    let later = daemon::providers::js_iso(h.clock.get() + 60_000.0);
    h.runtime.observe_limit(
        &context,
        &Limit {
            resets_at: Some(later.clone()),
            ..detailed(vec![
                window(
                    "primary",
                    Some(300.0),
                    100.0,
                    Some(later),
                    LimitState::Reached,
                ),
                window("secondary", Some(10080.0), 100.0, None, LimitState::Reached),
            ])
        },
    );
    h.runtime.observe_limit(&context, &reached(None));
    assert_eq!(h.limits("codex-main").len(), 2);
    assert!(matches!(
        h.inspect(),
        AccountSelection::NotReady {
            limited_until: Some(None),
            ..
        }
    ));
}

#[tokio::test]
async fn requires_known_codex_model_pricing_and_serializes_api_account_turns() {
    let h = fixture();
    h.api("api", false);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check_all().await.unwrap();
    h.sql(&format!(
        "UPDATE bots SET account_id = 'api', model = 'unknown-alias' WHERE id = '{}'",
        h.bot
    ));
    assert!(
        matches!(h.inspect(), AccountSelection::NotReady { reason, .. } if reason.contains("price"))
    );
    h.sql(&format!(
        "UPDATE bots SET model = 'gpt-5.3-codex' WHERE id = '{}'",
        h.bot
    ));
    let (first, second) = (h.context(), h.context());
    h.runtime.prepare(&first).await.unwrap();
    assert!(
        h.runtime
            .prepare(&second)
            .await
            .unwrap_err()
            .message
            .contains("Another turn")
    );
    assert!(
        h.runtime
            .set_api_key("api", "replacement")
            .await
            .unwrap_err()
            .message
            .contains("Stop")
    );
    h.runtime.release(&first);
    let prepared = h.runtime.prepare(&second).await.unwrap();
    assert_eq!(
        prepared.api_key.map(|key| key.0),
        Some("synthetic-key".to_string())
    );
    h.runtime.release(&second);
}

#[tokio::test]
async fn rejects_a_prepared_account_changed_while_its_credential_read_was_pending_and_releases_its_lease()
 {
    let h = fixture();
    h.api("api", false);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check_all().await.unwrap();
    h.sql(&format!(
        "UPDATE bots SET account_id = 'api' WHERE id = '{}'",
        h.bot
    ));
    let (finish, pending) = oneshot::channel();
    *h.keys.pending_read.lock().unwrap() = Some(pending);
    let running = tokio::spawn(h.runtime.prepare_owned(h.context()));
    common_settle().await;
    let hold = h.runtime.hold("api");
    assert!(!h.ready());
    finish.send("synthetic-key".to_string()).unwrap();
    assert!(
        running
            .await
            .unwrap()
            .unwrap_err()
            .message
            .contains("changed")
    );
    assert!(!h.runtime.in_use("api"));
    hold.release();
    assert!(h.ready());
}

#[tokio::test]
async fn nested_holds_invalidate_stale_contexts_and_notify_once_after_the_final_release() {
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    let old = h.context();
    h.changed.lock().unwrap().clear();
    let (a, b) = (h.runtime.hold("codex-main"), h.runtime.hold("codex-main"));
    h.runtime.changed("codex-main");
    a.release();
    a.release();
    assert!(h.changed.lock().unwrap().is_empty());
    b.release();
    assert_eq!(*h.changed.lock().unwrap(), ["codex-main"]);
    assert!(
        h.runtime
            .prepare(&old)
            .await
            .unwrap_err()
            .message
            .contains("changed")
    );
}

#[tokio::test]
async fn caps_locally_recorded_api_spend_and_deduplicates_threshold_notifications() {
    let h = fixture();
    h.api("api", false);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check_all().await.unwrap();
    h.sql(&format!(
        "UPDATE bots SET account_id = 'api' WHERE id = '{}'",
        h.bot
    ));
    h.usage("one", "gpt-5.3-codex", Some(0.8));
    h.runtime.usage_changed("api");
    h.runtime.usage_changed("api");
    assert_eq!(*h.caps.lock().unwrap(), [("api".to_string(), 80)]);
    h.usage("two", "unknown", None);
    h.runtime.usage_changed("api");
    assert_eq!(
        h.caps.lock().unwrap().last(),
        Some(&("api".to_string(), 100))
    );
    assert!(
        matches!(h.inspect(), AccountSelection::NotReady { reason, .. } if reason.contains("unknown"))
    );
}

#[tokio::test]
async fn never_falls_back_to_the_same_limited_api_account() {
    let h = fixture();
    h.api("api", true);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check_all().await.unwrap();
    h.sql(&format!(
        "UPDATE bots SET account_id = 'api' WHERE id = '{}'",
        h.bot
    ));
    h.runtime.observe_limit(&h.context(), &reached(None));
    assert!(
        matches!(h.inspect(), AccountSelection::NotReady { account_id, .. } if account_id == "api")
    );
}

#[tokio::test]
async fn keeps_per_account_sandbox_readiness_separate_and_stops_new_probes_after_close() {
    let h = fixture();
    h.api("api", false);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check_all().await.unwrap();
    let fields = SetupFields {
        installed: true,
        version: Some("1".into()),
        signed_in: true,
        account: None,
        plan: None,
        error: None,
    };
    h.ops.probe.then(ProviderSetup::Codex(CodexSetup {
        setup: fields,
        sandbox: CodexSandboxReadiness::NotConfigured,
    }));
    h.runtime.check("api").await.unwrap();
    h.sql(&format!(
        "UPDATE bots SET autonomy = 'folder' WHERE id = '{}'",
        h.bot
    ));
    assert!(h.ready());
    h.sql(&format!(
        "UPDATE bots SET account_id = 'api' WHERE id = '{}'",
        h.bot
    ));
    assert!(
        matches!(h.inspect(), AccountSelection::NotReady { reason, .. } if reason.contains("sandbox"))
    );
    h.runtime.close().await;
    let calls = h.ops.probe.count();
    assert!(
        h.runtime
            .check("api")
            .await
            .unwrap_err()
            .message
            .contains("closed")
    );
    assert_eq!(h.ops.probe.count(), calls);
}

#[tokio::test]
async fn correlates_sign_in_cancellation_to_the_account_and_attempt_sanitizing_failure() {
    let complete: Arc<Mutex<Option<daemon::setup::OnDone>>> = Arc::default();
    let login: Login = {
        let complete = Arc::clone(&complete);
        Box::new(move |_, on_done| {
            *complete.lock().unwrap() = Some(on_done);
            Ok(SignInStart {
                url: Some("https://example.invalid/consent".into()),
                attempt: "attempt-1".into(),
            })
        })
    };
    let h = fixture_with(Ops { login, ..ops() });
    h.runtime.sign_in("codex-main").await.unwrap();
    let sign_in = h.runtime.info("codex-main").unwrap().sign_in.unwrap();
    assert_eq!(
        (sign_in.attempt.as_str(), sign_in.state),
        ("attempt-1", protocol::accounts::SignInState::Waiting)
    );
    assert!(
        !serde_json::to_string(&h.runtime.list().unwrap())
            .unwrap()
            .contains("consent")
    );
    assert!(
        h.runtime
            .cancel_sign_in("codex-main", "stale")
            .await
            .unwrap_err()
            .message
            .contains("no longer active")
    );
    h.runtime
        .cancel_sign_in("codex-main", "attempt-1")
        .await
        .unwrap();
    assert_eq!(
        *h.ops.cancels.lock().unwrap(),
        [("codex-main".to_string(), "attempt-1".to_string())]
    );
    (complete.lock().unwrap().clone().unwrap())(true, "attempt-1".into());
    assert_eq!(h.runtime.info("codex-main").unwrap().sign_in, None);
    assert_eq!(
        *h.ended.lock().unwrap(),
        [("codex-main".to_string(), "attempt-1".to_string())]
    );
}

#[tokio::test]
async fn explicit_subscription_removal_signs_out_only_the_selected_account_and_preserves_metadata_on_failure()
 {
    let h = fixture();
    h.ops.logout.then(Err(RpcError::new(
        ErrorCode::Internal,
        "private provider payload",
    )));
    h.runtime.check_all().await.unwrap();
    assert!(
        h.runtime
            .delete_credential("codex-main")
            .await
            .unwrap_err()
            .message
            .contains("kept")
    );
    assert_eq!(
        get_account(&h.db.lock().unwrap(), "codex-main", false)
            .unwrap()
            .deleted_at,
        None
    );
    assert_eq!(h.keys.deletes.load(Ordering::SeqCst), 0);
    h.runtime.delete_credential("codex-main").await.unwrap();
    assert_eq!(
        h.ops
            .logout
            .calls
            .lock()
            .unwrap()
            .last()
            .map(String::as_str),
        Some("codex-main")
    );
    assert_eq!(h.runtime.info("codex-main").unwrap().setup, None);
    assert_eq!(
        get_account(&h.db.lock().unwrap(), "claude-main", false)
            .unwrap()
            .deleted_at,
        None
    );
}

#[tokio::test]
async fn bounds_startup_checks_to_three_concurrent_probes_and_does_not_start_queued_checks_after_close()
 {
    let h = fixture();
    for index in 0..6 {
        h.api(&format!("a{index}"), false);
    }
    let pending: Vec<oneshot::Sender<ProviderSetup>> =
        (0..8).map(|_| h.ops.probe.pending()).collect();
    let checking = tokio::spawn(h.runtime.check_all());
    common_settle().await;
    assert_eq!(h.ops.probe.count(), 3);
    let closing = tokio::spawn({
        let runtime = h.runtime.clone();
        async move { runtime.close().await }
    });
    common_settle().await;
    let accounts: Vec<String> = h.ops.probe.calls.lock().unwrap().clone();
    for (sender, id) in pending.into_iter().zip(accounts) {
        let provider = if id.starts_with("claude") {
            Provider::Claude
        } else {
            Provider::Codex
        };
        let _ = sender.send(healthy(&Account {
            id,
            provider,
            label: String::new(),
            folder: String::new(),
        }));
    }
    closing.await.unwrap();
    let _ = checking.await.unwrap();
    assert_eq!(h.ops.probe.count(), 3);
}

#[tokio::test]
async fn suppresses_late_lifecycle_notifications_and_rejects_stale_preparation_after_teardown() {
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    let context = h.context();
    h.runtime.prepare(&context).await.unwrap();
    let hold = h.runtime.hold("codex-main");
    h.runtime.close().await;
    h.changed.lock().unwrap().clear();
    h.runtime.release(&context);
    hold.release();
    h.runtime.metadata_changed("codex-main");
    h.runtime.changed("codex-main");
    assert!(h.changed.lock().unwrap().is_empty());
    assert!(
        h.runtime
            .prepare(&context)
            .await
            .unwrap_err()
            .message
            .contains("Accounts are closed")
    );
}

fn snapshot(limit: &str, key: &str, duration: f64, used: f64, state: LimitState) -> LimitSnapshot {
    LimitSnapshot {
        limit_id: limit.into(),
        windows: vec![window(key, Some(duration), used, None, state)],
    }
}

#[tokio::test]
async fn recovers_unknown_reset_subscription_limits_only_from_a_successful_authoritative_free_read()
{
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    h.runtime.observe_limit(&h.context(), &reached(None));
    h.runtime.check("codex-main").await.unwrap();
    assert!(!h.ready());
    h.ops.probe_limit.then(Err(()));
    h.runtime.check("codex-main").await.unwrap();
    assert!(!h.ready());
    h.ops.probe_limit.then(Ok(Some(vec![
        snapshot("codex", "primary", 300.0, 20.0, LimitState::Ok),
        snapshot("other", "secondary", 10080.0, 100.0, LimitState::Reached),
    ])));
    h.runtime.check("codex-main").await.unwrap();
    let mut limit_ids: Vec<String> = h
        .limits("codex-main")
        .into_iter()
        .map(|limit| limit.limit_id)
        .collect();
    limit_ids.dedup();
    assert_eq!(limit_ids, ["codex", "other"]);
    assert!(!h.ready());
    h.ops.probe_limit.then(Ok(Some(vec![snapshot(
        "other",
        "secondary",
        10080.0,
        10.0,
        LimitState::Ok,
    )])));
    h.runtime.check("codex-main").await.unwrap();
    assert_eq!(h.context().effective_account_id, "codex-main");
    h.api("api", false);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check("api").await.unwrap();
    assert_eq!(h.ops.probe_limit.count(), 4);
}

#[tokio::test]
async fn orders_quota_events_within_one_millisecond_and_discards_a_free_read_overtaken_by_a_newer_limit()
 {
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    let context = h.context();
    h.clock.set(h.clock.get()); // frozen
    h.runtime.observe_limit(
        &context,
        &Limit {
            state: LimitState::Ok,
            used_percent: Some(0.0),
            resets_at: None,
            limit_id: None,
            windows: None,
        },
    );
    let first = h.limits("codex-main")[0].observed_at.clone();
    h.runtime.observe_limit(&context, &reached(None));
    assert!(h.limits("codex-main")[0].observed_at > first);
    let finish = h.ops.probe_limit.pending();
    let checking = tokio::spawn(h.runtime.check("codex-main"));
    common_settle().await;
    h.runtime.observe_limit(
        &context,
        &detailed(vec![window(
            "primary",
            Some(300.0),
            100.0,
            None,
            LimitState::Reached,
        )]),
    );
    assert!(
        finish
            .send(Ok(Some(vec![snapshot(
                "codex",
                "primary",
                300.0,
                0.0,
                LimitState::Ok
            )])))
            .is_ok()
    );
    checking.await.unwrap().unwrap();
    let limits = h.limits("codex-main");
    assert_eq!(limits.len(), 1);
    assert_eq!(
        (limits[0].limit_id.as_str(), limits[0].state),
        ("codex", protocol::accounts::LimitState::Reached)
    );
}

#[tokio::test]
async fn grants_only_one_prepared_turn_across_bots_and_leaves_a_failed_retry_blocked() {
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    let original = h.context();
    h.runtime.observe_limit(&original, &reached(None));
    let other = create_bot(
        &h.db.lock().unwrap(),
        &h.bots,
        &bot_input("Other", Provider::Codex, "gpt-5.3-codex"),
    )
    .unwrap();
    h.runtime.retry_limit("codex-main").unwrap();
    let one = h.context();
    let AccountSelection::Ready(selected) = h.runtime.inspect(&other) else {
        panic!("expected ready")
    };
    h.runtime.prepare(&one).await.unwrap();
    assert!(
        h.runtime
            .prepare(&selected)
            .await
            .unwrap_err()
            .message
            .contains("limit")
    );
    h.runtime.complete_turn(&one, false);
    h.runtime.release(&one);
    assert!(!h.ready());
    assert_eq!(
        h.limits("codex-main")[0].state,
        protocol::accounts::LimitState::Reached
    );
}

#[tokio::test]
async fn does_not_let_explicit_retries_bypass_known_resets_spending_caps_pricing_or_permit_expiry()
{
    let h = fixture();
    h.runtime.check_all().await.unwrap();
    let context = h.context();
    h.runtime.observe_limit(
        &context,
        &reached(Some(daemon::providers::js_iso(h.clock.get() + 60_000.0))),
    );
    assert!(
        h.runtime
            .retry_limit("codex-main")
            .unwrap_err()
            .message
            .contains("known resets")
    );
    h.runtime.observe_limit(
        &context,
        &detailed(vec![window(
            "primary",
            Some(300.0),
            100.0,
            None,
            LimitState::Reached,
        )]),
    );
    h.runtime.retry_limit("codex-main").unwrap();
    let now = h.clock.get();
    h.clock.set(now + 5.0 * 60_000.0);
    assert!(!h.ready());
    h.clock.set(now);
    h.api("api", false);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check("api").await.unwrap();
    h.sql(&format!(
        "UPDATE bots SET account_id = 'api' WHERE id = '{}'",
        h.bot
    ));
    let api = h.context();
    h.runtime.observe_limit(&api, &reached(None));
    h.runtime.retry_limit("api").unwrap();
    h.sql(&format!(
        "UPDATE bots SET model = 'unpriced' WHERE id = '{}'",
        h.bot
    ));
    assert!(
        matches!(h.inspect(), AccountSelection::NotReady { reason, .. } if reason.contains("price"))
    );
    h.usage("spend", "gpt-5.3-codex", Some(1.0));
    assert!(
        h.runtime
            .retry_limit("api")
            .unwrap_err()
            .message
            .contains("spending cap")
    );
}

#[tokio::test]
async fn clears_only_old_unknown_windows_and_preserves_newer_exhaustion() {
    for with_windows in [false, true] {
        let h = fixture();
        h.runtime.check_all().await.unwrap();
        let initial = h.context();
        h.runtime.observe_limit(&initial, &reached(None));
        h.runtime.retry_limit("codex-main").unwrap();
        let first = h.context();
        h.runtime.prepare(&first).await.unwrap();
        h.runtime.complete_turn(&first, true);
        h.runtime.release(&first);
        h.context();
        h.runtime.observe_limit(&initial, &reached(None));
        h.runtime.retry_limit("codex-main").unwrap();
        let second = h.context();
        h.runtime.prepare(&second).await.unwrap();
        let again = if with_windows {
            detailed(vec![window(
                "primary",
                Some(300.0),
                100.0,
                None,
                LimitState::Reached,
            )])
        } else {
            reached(None)
        };
        h.runtime.observe_limit(&second, &again);
        h.runtime.complete_turn(&second, true);
        h.runtime.release(&second);
        assert_eq!(
            h.limits("codex-main")[0].state,
            protocol::accounts::LimitState::Reached,
            "{with_windows}"
        );
        assert!(!h.ready(), "{with_windows}");
    }
}

#[tokio::test]
async fn retries_the_selected_pinned_subscription_or_an_already_opted_in_fallback_without_transferring_permits()
 {
    let h = fixture();
    h.api("api", true);
    h.runtime.set_api_key("api", "synthetic-key").await.unwrap();
    h.runtime.check_all().await.unwrap();
    let pinned = h.context();
    h.runtime.observe_limit(&pinned, &reached(None));
    let fallback = h.context();
    assert_eq!(fallback.effective_account_id, "api");
    h.runtime.retry_limit("codex-main").unwrap();
    assert_eq!(h.context().effective_account_id, "codex-main");
    let chosen = h.context();
    h.runtime.prepare(&chosen).await.unwrap();
    h.runtime.release(&chosen);
    h.runtime.observe_limit(&fallback, &reached(None));
    assert!(!h.ready());
    h.runtime.retry_limit("api").unwrap();
    let retry = h.context();
    assert_eq!(retry.effective_account_id, "api");
    h.update(
        "api",
        AccountPatch {
            is_fallback: Some(false),
            ..AccountPatch::default()
        },
    );
    assert!(!h.ready());
    h.update(
        "api",
        AccountPatch {
            is_fallback: Some(true),
            ..AccountPatch::default()
        },
    );
    h.runtime.prepare(&retry).await.unwrap();
    h.runtime.complete_turn(&retry, false);
    h.runtime.release(&retry);
    assert!(!h.ready());
}

// The Windows store over T2.1's namespace. Only paths that fail before Credential Manager is touched.
#[tokio::test]
async fn windows_credentials_use_the_namespace_targets_and_validate_before_the_store() {
    let root = tempfile::tempdir().unwrap();
    let namespace = daemon::win::cred::Namespace::new(root.path()).unwrap();
    let store: &dyn CredentialStore = &namespace;
    assert_eq!(
        store.reference("account-a").unwrap(),
        namespace.account_target("account-a").unwrap()
    );
    assert!(!store.reference("account-a").unwrap().contains("account-a"));
    let invalid = store.write("account-a", " spaced").await.unwrap_err();
    assert_eq!(
        (invalid.code, invalid.message.contains("2560")),
        (ErrorCode::Internal, true)
    );
    assert!(
        store
            .reference("")
            .unwrap_err()
            .message
            .contains("Windows Credential Manager")
    );
}

// `#check`'s catch (account-runtime.ts:90-92): a probe that fails outright (a worker termination during `--version`, say)
// leaves "could not be checked", never "version check failed", and `installed` comes from the CLI lookup.
#[tokio::test]
async fn reports_a_probe_that_fails_as_could_not_be_checked_with_installed_from_the_cli_lookup() {
    const MESSAGE: &str = "This account could not be checked. Try again in Accounts.";
    let h = fixture();
    h.ops.check_fails.store(true, Ordering::SeqCst);
    for installed in [true, false] {
        h.ops.cli_installed.store(installed, Ordering::SeqCst);
        h.runtime.check("codex-main").await.unwrap();
        let Some(ProviderSetup::Codex(codex)) = h.runtime.info("codex-main").unwrap().setup else {
            panic!("a Codex check leaves a Codex setup");
        };
        assert_eq!(
            (
                codex.setup.installed,
                codex.setup.version.clone(),
                codex.setup.signed_in,
                codex.setup.error.as_deref(),
                codex.sandbox
            ),
            (
                installed,
                None,
                false,
                Some(MESSAGE),
                CodexSandboxReadiness::Unavailable
            ),
            "installed: {installed}"
        );
    }
    h.runtime.check("claude-main").await.unwrap();
    let Some(ProviderSetup::Claude(claude)) = h.runtime.info("claude-main").unwrap().setup else {
        panic!("a Claude check leaves a Claude setup");
    };
    assert_eq!(
        (claude.signed_in, claude.error.as_deref()),
        (false, Some(MESSAGE))
    );
    // An account that could not be checked can't take work.
    assert!(!h.ready());
}
