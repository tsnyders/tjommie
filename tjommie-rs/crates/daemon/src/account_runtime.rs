//! `account-runtime.ts`: live account state over the stored accounts. Setup checks, owned sign-in attempts, API keys,
//! provider limits, explicit retry permits and the leases running turns hold.
//!
//! Locking: the database lock is always taken before the state lock, and callbacks run with neither held. Callers must
//! not hold the database lock while calling in.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::FutureExt;
use futures_util::future::{Shared, join_all, try_join_all};
use protocol::ErrorCode;
use protocol::accounts::{
    AccountInfo, AccountKind, AccountLimitWindow, AccountProviderUsage, AccountSignIn,
    LimitState as StoredState, SignInStart, SignInState, UsageStatus,
};
use protocol::model::{
    Autonomy, CodexSandboxReadiness, CodexSetup, Provider, ProviderSetup, SetupFields,
};
use rusqlite::Connection;

use crate::account_pricing::has_pricing;
use crate::account_quota::{AccountUsageRead, LimitSnapshot, empty_provider_usage};
use crate::accounts_store::{
    AccountPatch, StoredAccount, account_budget, claim_cap_notification, get_account,
    get_default_account, get_fallback_account, list_account_limits, list_accounts,
    replace_account_limits, update_account,
};
use crate::bots::{StoredBot, get_stored_bot};
use crate::cli::{BoxFuture, Env};
use crate::ids::new_id;
use crate::providers::{ApiKey, Limit, LimitState, LimitWindow, js_iso};
use crate::server::RpcError;
use crate::setup::{Account, OnDone, SANDBOX_SETUP_TIMEOUT, Setup};
use crate::win::cred::{CredentialError, Namespace};

/// Where API keys live. Keys never reach the database, metadata or events.
pub trait CredentialStore: Send + Sync {
    fn reference(&self, account_id: &str) -> Result<String, RpcError>;
    fn read(&self, account_id: &str) -> BoxFuture<'_, Result<Option<String>, RpcError>>;
    fn write(&self, account_id: &str, key: &str) -> BoxFuture<'_, Result<(), RpcError>>;
    fn delete(&self, account_id: &str) -> BoxFuture<'_, Result<(), RpcError>>;
}

/// TypeScript throws these as plain errors, which the server answers as `internal` with the message.
fn credential_error(error: CredentialError) -> RpcError {
    RpcError::new(ErrorCode::Internal, error.to_string())
}

/// `WindowsAccountCredentials` over T2.1's Credential Manager namespace.
impl CredentialStore for Namespace {
    fn reference(&self, account_id: &str) -> Result<String, RpcError> {
        self.account_target(account_id).map_err(credential_error)
    }

    fn read(&self, account_id: &str) -> BoxFuture<'_, Result<Option<String>, RpcError>> {
        Box::pin(std::future::ready(
            self.read_account_key(account_id).map_err(credential_error),
        ))
    }

    fn write(&self, account_id: &str, key: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(std::future::ready(
            self.write_account_key(account_id, key)
                .map_err(credential_error),
        ))
    }

    fn delete(&self, account_id: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        Box::pin(std::future::ready(
            self.delete_account_key(account_id)
                .map_err(credential_error),
        ))
    }
}

/// The provider side of accounts: read-only probes and the owned sign-in lifecycle. `Setup` in production; synthetic
/// tests inject fakes.
pub trait AccountOps: Send + Sync {
    fn check(&self, account: &Account) -> BoxFuture<'_, ProviderSetup>;
    /// Whether the provider's CLI is installed, for a check that failed before its probe answered.
    fn installed(&self, _provider: Provider) -> bool {
        false
    }
    /// Older synthetic fixtures inject only the Codex limit probe. Production always reads full usage, whose
    /// permission gates govern recovery.
    fn uses_legacy_limit_probe(&self) -> bool {
        false
    }
    fn check_limits(&self, _account: &Account) -> BoxFuture<'_, Option<Vec<LimitSnapshot>>> {
        Box::pin(async { None })
    }
    fn check_usage(
        &self,
        account: &Account,
        accounts_root: &Path,
    ) -> BoxFuture<'_, AccountUsageRead>;
    fn sign_in(
        &self,
        account: &Account,
        on_done: OnDone,
    ) -> BoxFuture<'_, Result<SignInStart, RpcError>>;
    fn cancel_sign_in(
        &self,
        account: &Account,
        attempt: &str,
    ) -> BoxFuture<'_, Result<(), RpcError>>;
    fn prepare_sandbox(&self, account: &Account) -> BoxFuture<'_, Result<(), RpcError>>;
    fn logout(&self, account: &Account) -> BoxFuture<'_, Result<(), RpcError>>;
    fn prepare_folder(&self, account: &Account, data_root: &Path) -> io::Result<()>;
    fn account_env(&self, account: &Account) -> Env;
}

impl AccountOps for Setup {
    fn check(&self, account: &Account) -> BoxFuture<'_, ProviderSetup> {
        let account = account.clone();
        Box::pin(async move { self.check_account_setup(&account).await })
    }

    fn installed(&self, provider: Provider) -> bool {
        self.cli().find_cli(provider).is_some()
    }

    fn check_usage(
        &self,
        account: &Account,
        accounts_root: &Path,
    ) -> BoxFuture<'_, AccountUsageRead> {
        let (account, root) = (account.clone(), accounts_root.to_path_buf());
        Box::pin(async move { self.check_account_usage(&account, &root).await })
    }

    fn sign_in(
        &self,
        account: &Account,
        on_done: OnDone,
    ) -> BoxFuture<'_, Result<SignInStart, RpcError>> {
        let account = account.clone();
        Box::pin(async move { self.sign_in_account(&account, on_done).await })
    }

    fn cancel_sign_in(
        &self,
        account: &Account,
        attempt: &str,
    ) -> BoxFuture<'_, Result<(), RpcError>> {
        let (account, attempt) = (account.clone(), attempt.to_string());
        Box::pin(async move { self.cancel_account_sign_in(&account, &attempt).await })
    }

    fn prepare_sandbox(&self, account: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        let account = account.clone();
        Box::pin(async move {
            self.prepare_account_codex_sandbox(&account, SANDBOX_SETUP_TIMEOUT)
                .await
        })
    }

    fn logout(&self, account: &Account) -> BoxFuture<'_, Result<(), RpcError>> {
        let account = account.clone();
        Box::pin(async move { self.logout_account(&account).await })
    }

    fn prepare_folder(&self, account: &Account, data_root: &Path) -> io::Result<()> {
        self.prepare_account_folder(account, data_root)
    }

    fn account_env(&self, account: &Account) -> Env {
        Setup::account_env(self, account)
    }
}

/// The account one turn runs on, fixed when the turn is selected.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRunContext {
    pub run_id: String,
    pub bot_id: String,
    pub provider: Provider,
    pub pinned_account_id: String,
    pub effective_account_id: String,
    pub kind: AccountKind,
    pub account_revision: u64,
    pub using_fallback: bool,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountSelection {
    Ready(AccountRunContext),
    /// `limited_until`: absent, an unknown reset (`Some(None)`), or the latest known reset.
    NotReady {
        reason: String,
        account_id: String,
        limited_until: Option<Option<String>>,
    },
}

#[derive(Debug, Clone)]
pub struct PreparedAccount {
    pub env: Env,
    pub api_key: Option<ApiKey>,
    pub codex_sandbox_ready: bool,
    pub max_budget_usd: Option<f64>,
}

pub type OnChanged = Arc<dyn Fn(&AccountRuntime, &str) + Send + Sync>;
/// `(account_id, attempt)`.
pub type OnSignInEnded = Arc<dyn Fn(&AccountRuntime, &str, &str) + Send + Sync>;
/// `(account_id, threshold)`: 80 or 100 percent of the spending cap.
pub type OnCap = Arc<dyn Fn(&AccountRuntime, &str, u8) + Send + Sync>;

pub struct AccountRuntimeOptions {
    pub db: Arc<Mutex<Connection>>,
    /// The data root and its `accounts` folder.
    pub root: PathBuf,
    pub accounts: PathBuf,
    pub credentials: Arc<dyn CredentialStore>,
    pub ops: Arc<dyn AccountOps>,
    pub on_changed: Option<OnChanged>,
    pub on_sign_in_ended: Option<OnSignInEnded>,
    pub on_cap: Option<OnCap>,
    /// `Date.now()` in milliseconds.
    pub now: Arc<dyn Fn() -> f64 + Send + Sync>,
}

type Pending<T> = Shared<BoxFuture<'static, Result<T, RpcError>>>;

/// An explicit retry of an unknown-reset limit: valid for one turn, until the account or its quota changes, or for
/// five minutes while unclaimed.
struct Permit {
    revision: u64,
    quota_version: u64,
    expires_at: f64,
    run_id: Option<String>,
    windows: Vec<AccountLimitWindow>,
}

#[derive(Default)]
struct State {
    setups: HashMap<String, ProviderSetup>,
    provider_usage: HashMap<String, AccountProviderUsage>,
    attempts: HashMap<String, AccountSignIn>,
    revisions: HashMap<String, u64>,
    holds: HashMap<String, usize>,
    runs: HashMap<String, AccountRunContext>,
    checking: HashMap<String, (u64, Pending<AccountInfo>)>,
    starting: HashMap<u64, Pending<SignInStart>>,
    next_task: u64,
    closed: bool,
    login_epochs: HashMap<String, u64>,
    quota_versions: HashMap<String, u64>,
    limit_retries: HashMap<String, Permit>,
}

impl State {
    fn revision(&self, id: &str) -> u64 {
        self.revisions.get(id).copied().unwrap_or(0)
    }

    fn quota_version(&self, id: &str) -> u64 {
        self.quota_versions.get(id).copied().unwrap_or(0)
    }

    fn waiting(&self, id: &str) -> bool {
        self.attempts
            .get(id)
            .is_some_and(|attempt| attempt.state == SignInState::Waiting)
    }

    fn in_use(&self, id: &str) -> bool {
        self.runs.values().any(|run| run.effective_account_id == id)
    }

    fn task(&mut self) -> u64 {
        self.next_task += 1;
        self.next_task
    }
}

struct Inner {
    options: AccountRuntimeOptions,
    state: Mutex<State>,
}

/// Cloning shares the runtime.
#[derive(Clone)]
pub struct AccountRuntime(Arc<Inner>);

/// `hold()`'s release: idempotent, and run on drop.
pub struct AccountHold {
    runtime: AccountRuntime,
    id: String,
    released: AtomicBool,
}

impl AccountHold {
    pub fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        let last = {
            let mut state = self.runtime.state();
            let count = state.holds.get(&self.id).copied().unwrap_or(1) - 1;
            if count > 0 {
                state.holds.insert(self.id.clone(), count);
                false
            } else {
                state.holds.remove(&self.id);
                true
            }
        };
        if last {
            self.runtime.notify(&self.id);
        }
    }
}

impl Drop for AccountHold {
    fn drop(&mut self) {
        self.release();
    }
}

fn fail(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadState, message)
}

/// A plain `Error` in TypeScript, answered as `internal`.
fn closed() -> RpcError {
    RpcError::new(ErrorCode::Internal, "Accounts are closed.")
}

const CHANGING: &str = "This account is changing. Work will continue when it is ready.";
const API_KEY_PLAN: &str = "API key (configured locally)";

fn account_of(stored: &StoredAccount) -> Account {
    Account {
        id: stored.id.clone(),
        provider: stored.provider,
        label: stored.label.clone(),
        folder: stored.folder.clone(),
    }
}

fn fields(setup: &ProviderSetup) -> &SetupFields {
    match setup {
        ProviderSetup::Claude(fields) => fields,
        ProviderSetup::Codex(codex) => &codex.setup,
    }
}

fn fields_mut(setup: &mut ProviderSetup) -> &mut SetupFields {
    match setup {
        ProviderSetup::Claude(fields) => fields,
        ProviderSetup::Codex(codex) => &mut codex.setup,
    }
}

/// JavaScript truthiness of an optional string.
fn present(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.is_empty())
}

/// `Date.parse` of a stored instant. `None` is `NaN`, which compares false.
fn parse_ms(text: &str) -> Option<f64> {
    crate::providers::js::date_parse(text)
}

fn version_at_least(version: Option<&str>, expected: [f64; 3]) -> bool {
    let Some(version) = version else {
        return false;
    };
    let parts: Vec<f64> = version
        .split('.')
        .map(crate::providers::js::number)
        .collect();
    let safe = |part: &f64| part.fract() == 0.0 && *part >= 0.0 && *part <= 9_007_199_254_740_991.0;
    if parts.len() < 3 || !parts[..3].iter().all(safe) {
        return false;
    }
    for (part, wanted) in parts.iter().zip(expected) {
        if *part != wanted {
            return *part > wanted;
        }
    }
    true
}

fn same_window(old: &AccountLimitWindow, window: &AccountLimitWindow) -> bool {
    old.limit_id == window.limit_id
        && old.window_key == window.window_key
        && old.observed_at == window.observed_at
}

fn stored_window(window: &AccountLimitWindow) -> LimitWindow {
    LimitWindow {
        window_key: window.window_key.clone(),
        duration_minutes: window.duration_minutes,
        used_percent: window.used_percent,
        resets_at: window.resets_at.clone(),
        state: match window.state {
            StoredState::Ok => LimitState::Ok,
            StoredState::Near => LimitState::Near,
            StoredState::Reached => LimitState::Reached,
        },
    }
}

fn failed_setup(provider: Provider, installed: bool) -> ProviderSetup {
    let fields = SetupFields {
        installed,
        version: None,
        signed_in: false,
        account: None,
        plan: None,
        error: Some("This account could not be checked. Try again in Accounts.".into()),
    };
    match provider {
        Provider::Claude => ProviderSetup::Claude(fields),
        Provider::Codex => ProviderSetup::Codex(CodexSetup {
            setup: fields,
            sandbox: CodexSandboxReadiness::Unavailable,
        }),
    }
}

/// `#quotaMark`: changes whenever a limit snapshot is stored or a quota event arrives.
type QuotaMark = (u64, Vec<(String, String)>);

impl AccountRuntime {
    pub fn new(options: AccountRuntimeOptions) -> Self {
        AccountRuntime(Arc::new(Inner {
            options,
            state: Mutex::default(),
        }))
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.0
            .options
            .db
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> f64 {
        (self.0.options.now)()
    }

    fn notify(&self, id: &str) {
        let notify = {
            let state = self.state();
            !state.closed && !state.holds.contains_key(id)
        };
        if notify && let Some(on_changed) = &self.0.options.on_changed {
            on_changed(self, id);
        }
    }

    /// The account's authentication or configuration changed: running contexts are stale and its provider reading
    /// is dropped.
    pub fn changed(&self, id: &str) {
        {
            let mut state = self.state();
            *state.revisions.entry(id.into()).or_default() += 1;
            state.provider_usage.remove(id);
        }
        self.notify(id);
    }

    pub fn metadata_changed(&self, id: &str) {
        self.notify(id);
    }

    /// Blocks new work on the account until every hold is released; notifies once after the last.
    pub fn hold(&self, id: &str) -> AccountHold {
        {
            let mut state = self.state();
            *state.holds.entry(id.into()).or_default() += 1;
            *state.revisions.entry(id.into()).or_default() += 1;
        }
        AccountHold {
            runtime: self.clone(),
            id: id.into(),
            released: AtomicBool::new(false),
        }
    }

    pub fn in_use(&self, id: &str) -> bool {
        self.state().in_use(id)
    }

    fn info_in(db: &Connection, state: &State, id: &str) -> Result<AccountInfo, RpcError> {
        let account = get_account(db, id, false)?;
        Ok(AccountInfo {
            has_credential: account.kind == AccountKind::ApiKey && account.credential_ref.is_some(),
            setup: state.setups.get(id).cloned(),
            limits: list_account_limits(db, id)?,
            sign_in: state.attempts.get(id).cloned(),
            spending: account_budget(db, id)?,
            provider_usage: Some(
                state
                    .provider_usage
                    .get(id)
                    .cloned()
                    .unwrap_or_else(empty_provider_usage),
            ),
            id: account.id,
            provider: account.provider,
            label: account.label,
            kind: account.kind,
            is_default: account.is_default,
            is_fallback: account.is_fallback,
            spending_cap_usd: account.spending_cap_usd,
            created_at: account.created_at,
        })
    }

    pub fn info(&self, id: &str) -> Result<AccountInfo, RpcError> {
        let db = self.db();
        let state = self.state();
        Self::info_in(&db, &state, id)
    }

    pub fn list(&self) -> Result<Vec<AccountInfo>, RpcError> {
        let db = self.db();
        let state = self.state();
        list_accounts(&db, false)?
            .iter()
            .map(|account| Self::info_in(&db, &state, &account.id))
            .collect()
    }

    /// Runs one check per account at a time: a second call joins the running one. The check runs to completion even
    /// if no caller waits.
    pub fn check(&self, id: &str) -> BoxFuture<'static, Result<AccountInfo, RpcError>> {
        let mut state = self.state();
        if state.closed {
            return Box::pin(std::future::ready(Err(closed())));
        }
        if let Some((_, running)) = state.checking.get(id) {
            return Box::pin(running.clone());
        }
        let task = state.task();
        let (runtime, key) = (self.clone(), id.to_string());
        let handle = tokio::spawn(async move {
            let result = runtime.run_check(&key).await;
            let mut state = runtime.state();
            if state
                .checking
                .get(&key)
                .is_some_and(|(owner, _)| *owner == task)
            {
                state.checking.remove(&key);
            }
            result
        });
        let running = joined(handle);
        state.checking.insert(id.into(), (task, running.clone()));
        Box::pin(running)
    }

    async fn probe(
        &self,
        stored: &StoredAccount,
        account: &Account,
    ) -> Result<ProviderSetup, RpcError> {
        let ops = &self.0.options.ops;
        ops.prepare_folder(account, &self.0.options.root)?;
        let mut setup = ops.check(account).await;
        if stored.kind == AccountKind::ApiKey {
            let configured = stored.credential_ref.is_some()
                && self.0.options.credentials.read(&stored.id).await?.is_some();
            // This checks local installation and configuration only. No paid API request validates the key.
            let fields = fields_mut(&mut setup);
            fields.signed_in = configured && fields.installed;
            fields.account = None;
            fields.plan = Some(API_KEY_PLAN.into());
            if !configured {
                fields.error = Some("Add an API key in Accounts.".into());
            } else if fields.installed {
                fields.error = None;
            }
        }
        Ok(setup)
    }

    async fn run_check(&self, id: &str) -> Result<AccountInfo, RpcError> {
        let stored = get_account(&self.db(), id, false)?;
        let revision = self.state().revision(id);
        let account = account_of(&stored);
        let setup = match self.probe(&stored, &account).await {
            Ok(setup) => setup,
            Err(_) => failed_setup(
                stored.provider,
                self.0.options.ops.installed(stored.provider),
            ),
        };
        let current = {
            let state = self.state();
            !state.closed && state.revision(id) == revision
        };
        if current {
            let signed_in = fields(&setup).signed_in;
            self.state().setups.insert(id.into(), setup);
            if stored.kind == AccountKind::Subscription && signed_in {
                let before = self.quota_mark(id)?;
                let ops = &self.0.options.ops;
                if ops.uses_legacy_limit_probe() {
                    let blocked =
                        stored.provider == Provider::Codex && self.limit(&self.db(), id)?.0;
                    let snapshots = if blocked {
                        ops.check_limits(&account).await
                    } else {
                        None
                    };
                    if let Some(snapshots) = snapshots.filter(|snapshots| !snapshots.is_empty())
                        && self.unchanged(id, revision, &before)?
                    {
                        self.apply_usage_limits(id, &snapshots, true)?;
                    }
                } else {
                    let result = ops.check_usage(&account, &self.0.options.accounts).await;
                    let accepted = match self.unchanged(id, revision, &before) {
                        Ok(true) => self.accept_usage(id, result),
                        Ok(false) => Ok(()),
                        Err(error) => Err(error),
                    };
                    if accepted.is_err() && self.unchanged(id, revision, &before)? {
                        let error = AccountUsageRead::Error {
                            error: Some(
                                "Provider usage could not be read. Try again later.".into(),
                            ),
                        };
                        self.accept_usage(id, error)?;
                    }
                }
            } else {
                self.state().provider_usage.remove(id);
            }
            self.notify(id);
        }
        self.info(id)
    }

    fn quota_mark(&self, id: &str) -> Result<QuotaMark, RpcError> {
        let db = self.db();
        let mut statement = db.prepare(
            "SELECT limit_id, observed_at FROM account_limit_snapshots WHERE account_id = ?1 ORDER BY limit_id",
        )?;
        let rows = statement
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok((self.state().quota_version(id), rows))
    }

    fn unchanged(&self, id: &str, revision: u64, before: &QuotaMark) -> Result<bool, RpcError> {
        let current = {
            let state = self.state();
            !state.closed && state.revision(id) == revision
        };
        Ok(current && self.quota_mark(id)? == *before)
    }

    fn accept_usage(&self, id: &str, result: AccountUsageRead) -> Result<(), RpcError> {
        let checked_at = Some(js_iso(self.now()));
        let reading = match result {
            AccountUsageRead::Available {
                usage,
                limits,
                recovery_allowed,
            } => {
                self.apply_usage_limits(id, &limits, recovery_allowed)?;
                AccountProviderUsage {
                    status: UsageStatus::Available,
                    last_successful_at: checked_at.clone(),
                    checked_at,
                    error: None,
                    plan_label: usage.plan_label,
                    windows: usage.windows,
                    credits: usage.credits,
                    extra_usage_enabled: usage.extra_usage_enabled,
                }
            }
            failed => {
                let (status, error) = match failed {
                    AccountUsageRead::Unavailable { error } => (UsageStatus::Unavailable, error),
                    AccountUsageRead::Error { error } => (UsageStatus::Error, error),
                    AccountUsageRead::Available { .. } => unreachable!("matched above"),
                };
                let previous = self
                    .state()
                    .provider_usage
                    .get(id)
                    .cloned()
                    .unwrap_or_else(empty_provider_usage);
                AccountProviderUsage {
                    status,
                    checked_at,
                    error,
                    ..previous
                }
            }
        };
        self.state().provider_usage.insert(id.into(), reading);
        Ok(())
    }

    /// Stores a usage read's limits. A read without affirmative recovery cannot erase an earlier reported block, even
    /// when its displayed utilization is below 100 percent.
    fn apply_usage_limits(
        &self,
        id: &str,
        limits: &[LimitSnapshot],
        recovery_allowed: bool,
    ) -> Result<(), RpcError> {
        let db = self.db();
        let observed_at = self.next_quota_stamp(&db, id)?;
        let previous = list_account_limits(&db, id)?;
        for snapshot in limits {
            let mut windows = snapshot.windows.clone();
            if !recovery_allowed {
                let reached = previous.iter().filter(|window| {
                    window.limit_id == snapshot.limit_id && window.state == StoredState::Reached
                });
                for old in reached {
                    match windows
                        .iter()
                        .position(|window| window.window_key == old.window_key)
                    {
                        None => windows.push(stored_window(old)),
                        Some(index) if windows[index].state != LimitState::Reached => {
                            windows[index] = stored_window(old);
                        }
                        Some(_) => {}
                    }
                }
            }
            replace_account_limits(&db, id, &snapshot.limit_id, &observed_at, &windows)?;
        }
        if recovery_allowed
            && !limits
                .iter()
                .any(|snapshot| snapshot.limit_id == "provider")
        {
            replace_account_limits(&db, id, "provider", &observed_at, &[])?;
        }
        Ok(())
    }

    /// Checks every account, at most three at a time, and starts no new check after `close`.
    pub fn check_all(&self) -> BoxFuture<'static, Result<Vec<AccountInfo>, RpcError>> {
        let runtime = self.clone();
        Box::pin(async move {
            let pending = Mutex::new(VecDeque::from(list_accounts(&runtime.db(), false)?));
            let workers = pending.lock().map_or(0, |pending| pending.len().min(3));
            try_join_all((0..workers).map(|_| async {
                loop {
                    if runtime.state().closed {
                        return Ok::<_, RpcError>(());
                    }
                    let next = pending
                        .lock()
                        .ok()
                        .and_then(|mut pending| pending.pop_front());
                    let Some(next) = next else { return Ok(()) };
                    runtime.check(&next.id).await?;
                }
            }))
            .await?;
            runtime.list()
        })
    }

    pub async fn set_api_key(&self, id: &str, key: &str) -> Result<(), RpcError> {
        let account = get_account(&self.db(), id, false)?;
        if account.kind != AccountKind::ApiKey {
            return Err(fail("Only API-key accounts can store an API key."));
        }
        if self.in_use(id) {
            return Err(fail(
                "Stop the bots using this account before replacing its key.",
            ));
        }
        self.changed(id);
        let credentials = &self.0.options.credentials;
        credentials.write(id, key).await?;
        let reference = credentials.reference(id)?;
        let patch = AccountPatch {
            credential_ref: Some(Some(reference)),
            ..AccountPatch::default()
        };
        update_account(&self.db(), id, patch)?;
        if let Some(previous) = self.state().setups.get_mut(id) {
            let fields = fields_mut(previous);
            fields.signed_in = fields.installed;
            fields.account = None;
            fields.plan = Some(API_KEY_PLAN.into());
            if fields.installed {
                fields.error = None;
            }
        }
        self.notify(id);
        Ok(())
    }

    /// Removes an API key, or signs a subscription out through its provider.
    pub async fn delete_credential(&self, id: &str) -> Result<(), RpcError> {
        let account = get_account(&self.db(), id, false)?;
        if self.in_use(id) {
            return Err(fail(
                "Stop the bots using this account before removing its key.",
            ));
        }
        self.changed(id);
        if account.kind == AccountKind::ApiKey {
            self.0.options.credentials.delete(id).await?;
            let patch = AccountPatch {
                credential_ref: Some(None),
                ..AccountPatch::default()
            };
            update_account(&self.db(), id, patch)?;
        } else {
            if self.state().waiting(id) {
                return Err(fail("Cancel this account's sign-in before removing it."));
            }
            if self
                .0
                .options
                .ops
                .logout(&account_of(&account))
                .await
                .is_err()
            {
                return Err(fail(
                    "This account could not sign out. It has been kept so you can retry removal.",
                ));
            }
        }
        self.state().setups.remove(id);
        self.notify(id);
        Ok(())
    }

    /// Starts the provider's own sign-in. It runs to its start even if no caller waits, and `close` waits for it.
    pub fn sign_in(&self, id: &str) -> BoxFuture<'static, Result<SignInStart, RpcError>> {
        let mut state = self.state();
        if state.closed {
            return Box::pin(std::future::ready(Err(closed())));
        }
        let task = state.task();
        let (runtime, id) = (self.clone(), id.to_string());
        let handle = tokio::spawn(async move {
            let result = runtime.run_sign_in(&id).await;
            runtime.state().starting.remove(&task);
            result
        });
        let starting = joined(handle);
        state.starting.insert(task, starting.clone());
        Box::pin(starting)
    }

    async fn run_sign_in(&self, id: &str) -> Result<SignInStart, RpcError> {
        let stored = get_account(&self.db(), id, false)?;
        if stored.kind != AccountKind::Subscription {
            return Err(fail("API-key accounts do not use subscription sign-in."));
        }
        if self.in_use(id) {
            return Err(fail("Stop the bots using this account before signing in."));
        }
        if self.state().waiting(id) {
            return Err(fail("Sign-in is already waiting for this account."));
        }
        self.changed(id);
        let epoch = {
            let mut state = self.state();
            let epoch = state.login_epochs.entry(id.into()).or_default();
            *epoch += 1;
            *epoch
        };
        let ended = Arc::new(AtomicBool::new(false));
        let account = account_of(&stored);
        let on_done = self.on_sign_in_done(id, stored.provider, epoch, Arc::clone(&ended));
        let ops = &self.0.options.ops;
        let started = async {
            ops.prepare_folder(&account, &self.0.options.root)?;
            let start = ops.sign_in(&account, on_done).await?;
            let closing = self.state().closed;
            if closing && !ended.load(Ordering::SeqCst) {
                ops.cancel_sign_in(&account, &start.attempt).await?;
                return Err(fail("Accounts closed while sign-in was starting."));
            }
            if !ended.load(Ordering::SeqCst) {
                let waiting = AccountSignIn {
                    attempt: start.attempt.clone(),
                    state: SignInState::Waiting,
                    message: None,
                };
                self.state().attempts.insert(id.into(), waiting);
            }
            self.notify(id);
            Ok::<_, RpcError>(start)
        };
        started.await.map_err(|_| {
            fail("Sign-in could not start. Check the provider installation and try again.")
        })
    }

    fn on_sign_in_done(
        &self,
        id: &str,
        provider: Provider,
        epoch: u64,
        ended: Arc<AtomicBool>,
    ) -> OnDone {
        let (runtime, id) = (Arc::downgrade(&self.0), id.to_string());
        Arc::new(move |ok: bool, attempt: String| {
            ended.store(true, Ordering::SeqCst);
            let Some(runtime) = runtime.upgrade().map(AccountRuntime) else {
                return;
            };
            {
                let mut state = runtime.state();
                if state.closed || state.login_epochs.get(&id) != Some(&epoch) {
                    return;
                }
                if ok {
                    state.attempts.remove(&id);
                } else {
                    let message = if provider == Provider::Claude {
                        "Claude sign-in did not finish. Allow the browser to return to this computer, then retry. Code-paste sign-in is not supported here."
                    } else {
                        "Sign-in did not finish. You can try again."
                    };
                    let failed = AccountSignIn {
                        attempt: attempt.clone(),
                        state: SignInState::Failed,
                        message: Some(message.into()),
                    };
                    state.attempts.insert(id.clone(), failed);
                }
            }
            runtime.notify(&id);
            if let Some(on_ended) = &runtime.0.options.on_sign_in_ended {
                on_ended(&runtime, &id, &attempt);
            }
            if ok {
                drop(runtime.check(&id)); // The check is spawned; a failure only shows in the next check.
            }
        })
    }

    pub async fn cancel_sign_in(&self, id: &str, attempt: &str) -> Result<(), RpcError> {
        let account = account_of(&get_account(&self.db(), id, false)?);
        {
            let mut state = self.state();
            let active = state.attempts.get(id).is_some_and(|running| {
                running.attempt == attempt && running.state == SignInState::Waiting
            });
            if !active {
                return Err(fail("That sign-in attempt is no longer active."));
            }
            *state.login_epochs.entry(id.into()).or_default() += 1;
        }
        if self
            .0
            .options
            .ops
            .cancel_sign_in(&account, attempt)
            .await
            .is_err()
        {
            return Err(fail(
                "The sign-in process could not be closed. Try cancelling again.",
            ));
        }
        self.state().attempts.remove(id);
        self.changed(id);
        if let Some(on_ended) = &self.0.options.on_sign_in_ended {
            on_ended(self, id, attempt);
        }
        Ok(())
    }

    pub async fn prepare_sandbox(&self, id: &str) -> Result<(), RpcError> {
        let stored = get_account(&self.db(), id, false)?;
        if stored.provider != Provider::Codex || self.in_use(id) {
            return Err(fail(
                "Stop this Codex account's bots before setting up its sandbox.",
            ));
        }
        self.changed(id);
        let account = account_of(&stored);
        self.0
            .options
            .ops
            .prepare_folder(&account, &self.0.options.root)?;
        if self.0.options.ops.prepare_sandbox(&account).await.is_err() {
            return Err(fail(
                "Codex sandbox setup did not finish. Try again in Accounts.",
            ));
        }
        self.check(id).await?;
        Ok(())
    }

    /// Reached windows whose reset is unknown or still ahead.
    fn active_reached(
        &self,
        db: &Connection,
        id: &str,
    ) -> Result<Vec<AccountLimitWindow>, RpcError> {
        let now = self.now();
        let mut windows = list_account_limits(db, id)?;
        windows.retain(|window| {
            window.state == StoredState::Reached
                && window
                    .resets_at
                    .as_deref()
                    .is_none_or(|reset| parse_ms(reset).is_some_and(|reset| reset > now))
        });
        Ok(windows)
    }

    /// `#limit`: whether the account is blocked, and its latest known reset (`None` when any reset is unknown).
    fn limit(&self, db: &Connection, id: &str) -> Result<(bool, Option<String>), RpcError> {
        let reached = self.active_reached(db, id)?;
        let until = if reached.iter().any(|window| window.resets_at.is_none()) {
            None
        } else {
            reached
                .iter()
                .filter_map(|window| window.resets_at.clone())
                .max()
        };
        Ok((!reached.is_empty(), until))
    }

    /// `#nextQuotaStamp`: now, or 1 ms after the newest stored snapshot, so quota events stay ordered.
    fn next_quota_stamp(&self, db: &Connection, id: &str) -> Result<String, RpcError> {
        let latest: Option<String> = db.query_row(
            "SELECT MAX(observed_at) FROM account_limit_snapshots WHERE account_id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let after = latest
            .as_deref()
            .and_then(parse_ms)
            .map_or(0.0, |latest| latest + 1.0);
        Ok(js_iso(self.now().max(after)))
    }

    fn can_retry(
        &self,
        db: &Connection,
        state: &State,
        id: &str,
        run_id: Option<&str>,
    ) -> Result<bool, RpcError> {
        let Some(permit) = state.limit_retries.get(id) else {
            return Ok(false);
        };
        let claimable = match &permit.run_id {
            None => permit.expires_at > self.now(),
            Some(owner) => Some(owner.as_str()) == run_id,
        };
        if permit.revision != state.revision(id)
            || permit.quota_version != state.quota_version(id)
            || !claimable
        {
            return Ok(false);
        }
        let reached = self.active_reached(db, id)?;
        Ok(!reached.is_empty()
            && reached.iter().all(|window| {
                window.resets_at.is_none()
                    && permit.windows.iter().any(|old| same_window(old, window))
            }))
    }

    /// An explicit retry of an unknown-reset limit, for the next turn on this account.
    pub fn retry_limit(&self, id: &str) -> Result<AccountInfo, RpcError> {
        {
            let db = self.db();
            let mut state = self.state();
            if state.closed {
                return Err(fail("Accounts are closed."));
            }
            let account = get_account(&db, id, false)?;
            if state.in_use(id) {
                return Err(fail(
                    "Stop the bots using this account before retrying its limit.",
                ));
            }
            let ready = state
                .setups
                .get(id)
                .map(fields)
                .is_some_and(|setup| setup.installed && setup.signed_in);
            if !ready || state.waiting(id) {
                return Err(fail(
                    "Check or sign in to this account before retrying work.",
                ));
            }
            if account.kind == AccountKind::ApiKey
                && (present(account.credential_ref.as_deref()).is_none()
                    || account_budget(&db, id)?.blocked)
            {
                return Err(fail(
                    "This API account's key or local spending cap prevents a retry.",
                ));
            }
            let reached = self.active_reached(&db, id)?;
            if reached.is_empty() || reached.iter().any(|window| window.resets_at.is_some()) {
                return Err(fail(
                    "Retry is available only when the provider has not supplied a reset time. Wait for known resets.",
                ));
            }
            let permit = Permit {
                revision: state.revision(id),
                quota_version: state.quota_version(id),
                expires_at: self.now() + 5.0 * 60_000.0,
                run_id: None,
                windows: reached,
            };
            state.limit_retries.insert(id.into(), permit);
        }
        self.notify(id);
        self.info(id)
    }

    /// Ends a retried turn. A successful turn clears only the retried windows still unchanged since the retry.
    pub fn complete_turn(&self, context: &AccountRunContext, successful: bool) -> bool {
        self.complete_turn_with(&self.db(), context, successful)
    }

    /// `complete_turn` with a database connection the caller already holds locked, as the runner does.
    pub fn complete_turn_with(
        &self,
        db: &Connection,
        context: &AccountRunContext,
        successful: bool,
    ) -> bool {
        let id = &context.effective_account_id;
        {
            let mut state = self.state();
            let claimed = state
                .limit_retries
                .get(id)
                .is_some_and(|permit| permit.run_id.as_deref() == Some(context.run_id.as_str()));
            if !claimed {
                return false;
            }
            let Some(permit) = state.limit_retries.remove(id) else {
                return false;
            };
            let current = !state.closed
                && permit.revision == state.revision(id)
                && permit.quota_version == state.quota_version(id);
            if successful
                && current
                && let Err(error) = self.clear_retried(db, id, &permit)
            {
                eprintln!("Could not clear a retried account limit. {}", error.message);
            }
        }
        // The runner is still completing the old turn; wake other bots after its retirement is installed.
        self.defer_notify(id);
        true
    }

    /// Notifies on its own task, for callers that hold locks the change callbacks take.
    pub fn defer_notify(&self, id: &str) {
        let (runtime, id) = (self.clone(), id.to_string());
        tokio::spawn(async move { runtime.notify(&id) });
    }

    fn clear_retried(&self, db: &Connection, id: &str, permit: &Permit) -> Result<(), RpcError> {
        let current = list_account_limits(db, id)?;
        let observed_at = self.next_quota_stamp(db, id)?;
        let mut limit_ids: Vec<&str> = Vec::new();
        for window in &permit.windows {
            if !limit_ids.contains(&window.limit_id.as_str()) {
                limit_ids.push(&window.limit_id);
            }
        }
        for limit_id in limit_ids {
            let windows: Vec<&AccountLimitWindow> = current
                .iter()
                .filter(|window| window.limit_id == limit_id)
                .collect();
            let remaining: Vec<LimitWindow> = windows
                .iter()
                .filter(|window| !permit.windows.iter().any(|old| same_window(old, window)))
                .map(|window| stored_window(window))
                .collect();
            if remaining.len() != windows.len() {
                replace_account_limits(db, id, limit_id, &observed_at, &remaining)?;
            }
        }
        Ok(())
    }

    /// `#reason`: why this account cannot run this bot's work now.
    fn reason(
        &self,
        db: &Connection,
        state: &State,
        account: &StoredAccount,
        bot: &StoredBot,
    ) -> Result<Option<&'static str>, RpcError> {
        let id = account.id.as_str();
        if state.closed || state.holds.contains_key(id) {
            return Ok(Some(CHANGING));
        }
        if state.waiting(id) {
            return Ok(Some("Finish signing in to this account."));
        }
        let Some(setup) = state
            .setups
            .get(id)
            .filter(|setup| fields(setup).installed && fields(setup).signed_in)
        else {
            return Ok(Some("Check or sign in to this account in Accounts."));
        };
        let sandbox_ready = matches!(
            setup,
            ProviderSetup::Codex(CodexSetup {
                sandbox: CodexSandboxReadiness::Ready,
                ..
            })
        );
        if account.provider == Provider::Codex && bot.autonomy == Autonomy::Folder && !sandbox_ready
        {
            return Ok(Some("Set up this account's Codex sandbox in Accounts."));
        }
        if account.kind == AccountKind::ApiKey {
            let version = fields(setup).version.as_deref();
            if account.provider == Provider::Claude && !version_at_least(version, [2.0, 1.0, 251.0])
            {
                return Ok(Some(
                    "Update Claude Code to 2.1.251 or later before using an API key with subprocess scrubbing.",
                ));
            }
            if account.provider == Provider::Codex && !version_at_least(version, [0.0, 160.0, 0.0])
            {
                return Ok(Some(
                    "Update Codex to 0.160.0 or later before using ephemeral API-key authentication.",
                ));
            }
            if present(account.credential_ref.as_deref()).is_none() {
                return Ok(Some("Add an API key to this account."));
            }
            if account_budget(db, id)?.blocked {
                return Ok(Some(
                    "This account's local spending cap is reached or its recorded cost is unknown.",
                ));
            }
            if account.provider == Provider::Codex && !has_pricing(account.provider, &bot.model) {
                return Ok(Some(
                    "This model has no supported local price. Choose a priced model before using an API key.",
                ));
            }
            if state
                .runs
                .values()
                .any(|run| run.effective_account_id == id && run.bot_id != bot.id)
            {
                return Ok(Some(
                    "Another bot is using this API account. Its queued work will wait.",
                ));
            }
        }
        Ok(None)
    }

    /// Chooses the account for a bot's next turn: its pinned (or default) account, or an opted-in fallback while the
    /// pinned subscription is limited.
    pub fn inspect(&self, bot: &StoredBot) -> AccountSelection {
        self.inspect_with(&self.db(), bot)
    }

    /// `inspect` with a database connection the caller already holds locked, as the runner does.
    pub fn inspect_with(&self, db: &Connection, bot: &StoredBot) -> AccountSelection {
        let state = self.state();
        self.select(db, &state, bot)
            .unwrap_or_else(|error| AccountSelection::NotReady {
                reason: error.message,
                account_id: bot.account_id.clone().unwrap_or_default(),
                limited_until: None,
            })
    }

    fn select(
        &self,
        db: &Connection,
        state: &State,
        bot: &StoredBot,
    ) -> Result<AccountSelection, RpcError> {
        let blocked = |reason: &str, account_id: &str, limited_until| AccountSelection::NotReady {
            reason: reason.into(),
            account_id: account_id.into(),
            limited_until,
        };
        let pinned = match present(bot.account_id.as_deref()) {
            Some(id) => get_account(db, id, false),
            None => get_default_account(db, bot.provider),
        };
        let Ok(pinned) = pinned else {
            let id = bot.account_id.as_deref().unwrap_or_default();
            return Ok(blocked(
                "Choose an account in this bot's settings.",
                id,
                None,
            ));
        };
        if pinned.provider != bot.provider {
            return Ok(blocked(
                "Choose an account for this bot's provider.",
                &pinned.id,
                None,
            ));
        }
        if state.holds.contains_key(&pinned.id) {
            return Ok(blocked(CHANGING, &pinned.id, None));
        }
        let mut selected = pinned.clone();
        let (limited, until) = self.limit(db, &pinned.id)?;
        if limited && !self.can_retry(db, state, &pinned.id, None)? {
            let fallback = match pinned.kind {
                AccountKind::Subscription => get_fallback_account(db, bot.provider)?,
                AccountKind::ApiKey => None,
            };
            let usable = match &fallback {
                None => false,
                Some(fallback) => {
                    self.reason(db, state, fallback, bot)?.is_none()
                        && !(self.limit(db, &fallback.id)?.0
                            && !self.can_retry(db, state, &fallback.id, None)?)
                }
            };
            match fallback.filter(|_| usable) {
                Some(fallback) => selected = fallback,
                None => {
                    let reason = "This account has reached its provider limit. Wait for reset or choose another account.";
                    return Ok(blocked(reason, &pinned.id, Some(until)));
                }
            }
        }
        if let Some(reason) = self.reason(db, state, &selected, bot)? {
            return Ok(blocked(reason, &selected.id, None));
        }
        Ok(AccountSelection::Ready(AccountRunContext {
            run_id: new_id(),
            bot_id: bot.id.clone(),
            provider: selected.provider,
            using_fallback: selected.id != pinned.id,
            pinned_account_id: pinned.id,
            account_revision: state.revision(&selected.id),
            effective_account_id: selected.id,
            kind: selected.kind,
            model: bot.model.clone(),
        }))
    }

    fn current(
        &self,
        db: &Connection,
        state: &State,
        context: &AccountRunContext,
    ) -> Result<StoredAccount, RpcError> {
        if state.closed {
            return Err(fail("Accounts are closed."));
        }
        let account = get_account(db, &context.effective_account_id, false)?;
        let bot = get_stored_bot(db, &context.bot_id)?;
        let repinned =
            present(bot.account_id.as_deref()).is_some_and(|id| id != context.pinned_account_id);
        if bot.provider != context.provider || repinned || bot.model != context.model {
            return Err(fail(
                "This bot's account or model changed before work could start.",
            ));
        }
        if state.holds.contains_key(&account.id)
            || state.revision(&account.id) != context.account_revision
            || account.provider != context.provider
            || account.kind != context.kind
        {
            return Err(fail(
                "This account changed before work could start. Retry when it is ready.",
            ));
        }
        let limited = self.limit(db, &account.id)?.0
            && !self.can_retry(db, state, &account.id, Some(&context.run_id))?;
        if limited
            || account.kind == AccountKind::ApiKey && account_budget(db, &account.id)?.blocked
        {
            return Err(fail(
                "This account's limit is reached. Queued work will wait.",
            ));
        }
        Ok(account)
    }

    pub fn assert_current(&self, context: &AccountRunContext) -> Result<(), RpcError> {
        self.assert_current_with(&self.db(), context)
    }

    /// `assert_current` with a database connection the caller already holds locked, as the runner does.
    pub fn assert_current_with(
        &self,
        db: &Connection,
        context: &AccountRunContext,
    ) -> Result<(), RpcError> {
        let state = self.state();
        self.current(db, &state, context).map(drop)
    }

    pub async fn prepare(&self, context: &AccountRunContext) -> Result<PreparedAccount, RpcError> {
        self.prepare_owned(context.clone()).await
    }

    /// Leases the account to this turn and reads what the CLI needs. A failure releases the lease.
    pub fn prepare_owned(
        &self,
        context: AccountRunContext,
    ) -> BoxFuture<'static, Result<PreparedAccount, RpcError>> {
        let runtime = self.clone();
        Box::pin(async move {
            runtime.lease(&context)?;
            let prepared = runtime.prepared(&context).await;
            if prepared.is_err() {
                runtime.release(&context);
            }
            prepared
        })
    }

    fn lease(&self, context: &AccountRunContext) -> Result<(), RpcError> {
        let db = self.db();
        let mut state = self.state();
        let account = self.current(&db, &state, context)?;
        let retrying = self.limit(&db, &account.id)?.0
            && self.can_retry(&db, &state, &account.id, Some(&context.run_id))?;
        let shared = state
            .runs
            .values()
            .any(|run| run.effective_account_id == account.id && run.run_id != context.run_id);
        if (account.kind == AccountKind::ApiKey || retrying) && shared {
            return Err(fail(
                "Another turn is using this API account. Queued work will wait.",
            ));
        }
        state.runs.insert(context.run_id.clone(), context.clone());
        if retrying && let Some(permit) = state.limit_retries.get_mut(&account.id) {
            permit.run_id = Some(context.run_id.clone());
        }
        Ok(())
    }

    async fn prepared(&self, context: &AccountRunContext) -> Result<PreparedAccount, RpcError> {
        let stored = get_account(&self.db(), &context.effective_account_id, false)?;
        let account = account_of(&stored);
        self.0
            .options
            .ops
            .prepare_folder(&account, &self.0.options.root)?;
        let key = match stored.kind {
            AccountKind::ApiKey => self.0.options.credentials.read(&stored.id).await?,
            AccountKind::Subscription => None,
        };
        self.assert_current(context)?;
        if stored.kind == AccountKind::ApiKey && key.is_none() {
            return Err(fail(
                "This API key is unavailable. Add it again in Accounts.",
            ));
        }
        let budget = account_budget(&self.db(), &stored.id)?;
        let codex_sandbox_ready = matches!(
            self.state().setups.get(&stored.id),
            Some(ProviderSetup::Codex(CodexSetup {
                sandbox: CodexSandboxReadiness::Ready,
                ..
            }))
        );
        let capped = stored.kind == AccountKind::ApiKey && stored.provider == Provider::Claude;
        Ok(PreparedAccount {
            env: self.0.options.ops.account_env(&account),
            api_key: key.map(ApiKey),
            codex_sandbox_ready,
            max_budget_usd: budget
                .cap_usd
                .filter(|_| capped)
                .map(|cap| cap - budget.known_usd),
        })
    }

    pub fn release(&self, context: &AccountRunContext) {
        if self.release_quiet(context) {
            self.notify(&context.effective_account_id);
        }
    }

    /// `release` without the notification: true if the run held the account, and the caller should notify.
    pub fn release_quiet(&self, context: &AccountRunContext) -> bool {
        let id = &context.effective_account_id;
        let mut state = self.state();
        if state
            .limit_retries
            .get(id)
            .is_some_and(|permit| permit.run_id.as_deref() == Some(context.run_id.as_str()))
        {
            state.limit_retries.remove(id);
        }
        state.runs.remove(&context.run_id).is_some()
    }

    /// A provider's limit event during a turn. Any event invalidates older reads and retry permits; a generic repeat
    /// of a known exhaustion keeps the more informative stored windows.
    pub fn observe_limit(&self, context: &AccountRunContext, event: &Limit) {
        if self.observe_limit_with(&self.db(), context, event) {
            self.notify(&context.effective_account_id);
        }
    }

    /// `observe_limit` with a database connection the caller already holds locked, as the runner does. True if the
    /// caller should notify.
    pub fn observe_limit_with(
        &self,
        db: &Connection,
        context: &AccountRunContext,
        event: &Limit,
    ) -> bool {
        match self.record_limit(db, &context.effective_account_id, event) {
            Ok(changed) => changed,
            Err(error) => {
                eprintln!("Could not record a provider limit. {}", error.message);
                false
            }
        }
    }

    fn record_limit(&self, db: &Connection, id: &str, event: &Limit) -> Result<bool, RpcError> {
        if get_account(db, id, false).is_err() {
            return Ok(false);
        }
        *self.state().quota_versions.entry(id.into()).or_default() += 1;
        if event.windows.is_none() && event.state == LimitState::Reached && self.limit(db, id)?.0 {
            return Ok(true);
        }
        let observed_at = self.next_quota_stamp(db, id)?;
        let limit_id = event.limit_id.as_deref();
        if event.windows.is_some()
            && present(limit_id).is_some_and(|limit_id| limit_id != "provider")
        {
            replace_account_limits(db, id, "provider", &observed_at, &[])?;
        }
        let primary = || {
            vec![LimitWindow {
                window_key: "primary".into(),
                duration_minutes: None,
                used_percent: event.used_percent,
                resets_at: event.resets_at.clone(),
                state: event.state,
            }]
        };
        let windows = event.windows.clone().unwrap_or_else(primary);
        replace_account_limits(
            db,
            id,
            limit_id.unwrap_or("provider"),
            &observed_at,
            &windows,
        )?;
        Ok(true)
    }

    /// Recorded usage changed: the 80 and 100 percent cap notifications are claimed once each.
    pub fn usage_changed(&self, id: &str) {
        let claimed = self.usage_changed_with(&self.db(), id);
        self.finish_usage_changed(id, claimed);
    }

    /// The database half of `usage_changed`, for a caller that already holds the connection locked: the cap
    /// thresholds this call claimed. Pass them to [`Self::finish_usage_changed`] without the lock.
    pub fn usage_changed_with(&self, db: &Connection, id: &str) -> Vec<u8> {
        [80, 100]
            .into_iter()
            .filter(|threshold| claim_cap_notification(db, id, *threshold).unwrap_or(false))
            .collect()
    }

    /// The callbacks half of `usage_changed`: each claimed threshold's cap notification, then the change.
    pub fn finish_usage_changed(&self, id: &str, claimed: Vec<u8>) {
        if let Some(on_cap) = &self.0.options.on_cap {
            for threshold in claimed {
                on_cap(self, id, threshold);
            }
        }
        self.notify(id);
    }

    /// [`Self::finish_usage_changed`] on its own task, for callers that hold locks the callbacks take.
    pub fn defer_usage_changed(&self, id: &str, claimed: Vec<u8>) {
        let (runtime, id) = (self.clone(), id.to_string());
        tokio::spawn(async move { runtime.finish_usage_changed(&id, claimed) });
    }

    /// Cancels waiting sign-ins and waits for running checks and starting sign-ins. Nothing new starts afterwards.
    pub async fn close(&self) {
        let waiting: Vec<(String, String)> = {
            let mut state = self.state();
            state.closed = true;
            state.limit_retries.clear();
            state
                .attempts
                .iter()
                .filter(|(_, attempt)| attempt.state == SignInState::Waiting)
                .map(|(id, attempt)| (id.clone(), attempt.attempt.clone()))
                .collect()
        };
        join_all(waiting.into_iter().map(|(id, attempt)| async move {
            let account = get_account(&self.db(), &id, false);
            if let Ok(account) = account {
                // No credentials in shutdown diagnostics.
                let _ = self
                    .0
                    .options
                    .ops
                    .cancel_sign_in(&account_of(&account), &attempt)
                    .await;
            }
        }))
        .await;
        let (checks, starts): (Vec<_>, Vec<_>) = {
            let mut state = self.state();
            state.attempts.clear();
            (
                state
                    .checking
                    .values()
                    .map(|(_, check)| check.clone())
                    .collect(),
                state.starting.values().cloned().collect(),
            )
        };
        join_all(checks).await;
        join_all(starts).await;
    }
}

/// A spawned task's result, shareable between waiters.
fn joined<T: Clone + Send + Sync + 'static>(
    handle: tokio::task::JoinHandle<Result<T, RpcError>>,
) -> Pending<T> {
    let joined: BoxFuture<'static, Result<T, RpcError>> = Box::pin(async move {
        handle.await.unwrap_or_else(|_| {
            Err(RpcError::new(
                ErrorCode::Internal,
                "The request failed unexpectedly.",
            ))
        })
    });
    joined.shared()
}
