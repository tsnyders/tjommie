//! `setup.ts`: each provider account's CLI check, sign-in, sandbox setup and quota reads.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use protocol::ErrorCode;
use protocol::accounts::SignInStart;
use protocol::model::{
    CodexSandboxReadiness, CodexSetup, ModelInfo, Provider, ProviderSetup, SetupFields,
};
use regex::Regex;
use rusqlite::Connection;
use serde_json::{Map, Value, json};

use crate::account_quota::{
    AccountUsageRead, ClaudeUsageOptions, LimitSnapshot, UsageFetch, normalize_codex_usage, now_ms,
    read_claude_account_usage,
};
use crate::accounts_store::{bind_legacy_bots, get_default_account, list_accounts, provider_name};
use crate::cli::{BoxFuture, CliExit, CliHost, Env, LaunchOptions, process_env};
use crate::ids::{new_id, now_iso};
use crate::providers::codex_rpc::{CodexServer, JsonRpcLines};
use crate::providers::{LimitState, LimitWindow, ProviderError, js};
use crate::server::RpcError;

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const SANDBOX_SETUP_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const RUN_TIMEOUT: Duration = Duration::from_secs(20);

/// One provider account as the CLI sees it: its folder is the CLI's config home, so bots never load the person's own
/// CLI setup (hooks, plugins, skills, memories, MCP servers).
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    pub folder: String,
}

fn limit_state(used: f64) -> LimitState {
    if used >= 100.0 {
        LimitState::Reached
    } else if used >= 90.0 {
        LimitState::Near
    } else {
        LimitState::Ok
    }
}

/// A present value: neither missing nor `null` (JavaScript's `!= null`).
fn present<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !value.is_null())
}

/// Codex's `account/rateLimits/read` as limit snapshots. A malformed or incomplete read is `None`: it must never clear
/// a known exhausted window.
pub fn parse_codex_account_limits(value: &Value) -> Option<Vec<LimitSnapshot>> {
    let value = value.as_object()?;
    let entries: Vec<(Option<&String>, &Value)> = match present(value, "rateLimitsByLimitId") {
        Some(multi) => multi
            .as_object()?
            .iter()
            .map(|(key, bucket)| (Some(key), bucket))
            .collect(),
        None => vec![(None, value.get("rateLimits").unwrap_or(&Value::Null))],
    };
    if entries.is_empty() || entries.len() > 32 {
        return None;
    }
    let mut snapshots = Vec::new();
    for (key, bucket) in entries {
        let bucket = bucket.as_object()?;
        let named = present(bucket, "limitId");
        let limit_id = match key {
            Some(key) => key.as_str(),
            None => named.map_or(Some("codex"), Value::as_str)?,
        };
        if limit_id.trim().is_empty()
            || js::utf16_len(limit_id) > 100
            || key.is_some_and(|key| named.is_some_and(|named| named.as_str() != Some(key)))
        {
            return None;
        }
        let mut windows = Vec::new();
        for window_key in ["primary", "secondary"] {
            let Some(window) = present(bucket, window_key) else {
                continue;
            };
            let window = window.as_object()?;
            let used = window
                .get("usedPercent")
                .and_then(Value::as_f64)
                .filter(|used| (0.0..=100.0).contains(used))?;
            let duration = match present(window, "windowDurationMins") {
                None => None,
                Some(duration) => Some(
                    duration
                        .as_f64()
                        .filter(|minutes| minutes.is_finite() && *minutes > 0.0)?,
                ),
            };
            let resets_at = match present(window, "resetsAt") {
                None => None,
                Some(reset) => Some(js::iso_from_millis(reset.as_f64()? * 1000.0)?),
            };
            windows.push(LimitWindow {
                window_key: window_key.into(),
                duration_minutes: duration,
                used_percent: Some(used),
                resets_at,
                state: limit_state(used),
            });
        }
        if let Some(reached) = present(bucket, "rateLimitReachedType") {
            if reached.as_str()?.trim().is_empty() {
                return None;
            }
            if !windows
                .iter()
                .any(|window| window.state == LimitState::Reached)
            {
                windows.push(LimitWindow {
                    window_key: "provider".into(),
                    duration_minutes: None,
                    used_percent: None,
                    resets_at: None,
                    state: LimitState::Reached,
                });
            }
        }
        if windows.is_empty() {
            return None;
        }
        snapshots.push(LimitSnapshot {
            limit_id: limit_id.to_string(),
            windows,
        });
    }
    Some(snapshots)
}

static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+\.\d+\.\d+").unwrap());

pub fn parse_version(text: &str) -> Option<String> {
    VERSION
        .find(text)
        .map(|version| version.as_str().to_string())
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClaudeStatus {
    pub signed_in: bool,
    pub account: Option<String>,
    pub plan: Option<String>,
}

fn try_parse_claude_status(text: &str) -> Option<ClaudeStatus> {
    let status: Value = serde_json::from_str(text).ok()?;
    let status = status.as_object()?;
    let logged_in = status.get("loggedIn")?.as_bool()?;
    let method = status
        .get("authMethod")
        .map_or_else(|| "undefined".to_string(), js::string);
    Some(ClaudeStatus {
        signed_in: logged_in
            && !["api_key", "api_key_helper", "third_party"].contains(&method.as_str()),
        account: status
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string),
        plan: status
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// `claude auth status --json`. Anything else is signed out.
pub fn parse_claude_status(text: &str) -> ClaudeStatus {
    try_parse_claude_status(text).unwrap_or_default()
}

fn provider_title(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    }
}

fn setup_of(
    provider: Provider,
    fields: SetupFields,
    sandbox: CodexSandboxReadiness,
) -> ProviderSetup {
    match provider {
        Provider::Claude => ProviderSetup::Claude(fields),
        Provider::Codex => ProviderSetup::Codex(CodexSetup {
            setup: fields,
            sandbox,
        }),
    }
}

fn not_installed(provider: Provider) -> ProviderSetup {
    let error = format!("{} is not installed", provider_title(provider));
    let fields = SetupFields {
        installed: false,
        version: None,
        signed_in: false,
        account: None,
        plan: None,
        error: Some(error),
    };
    setup_of(provider, fields, CodexSandboxReadiness::Unavailable)
}

fn failed_check(provider: Provider, version: Option<String>, error: &str) -> ProviderSetup {
    let fields = SetupFields {
        installed: true,
        version,
        signed_in: false,
        account: None,
        plan: None,
        error: Some(error.into()),
    };
    setup_of(provider, fields, CodexSandboxReadiness::Unavailable)
}

fn bad_state(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadState, message)
}

fn internal(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(ErrorCode::Internal, error.to_string())
}

fn rpc_error(error: ProviderError) -> RpcError {
    internal(error)
}

/// `Read(//c/...)`: Claude matches absolute Windows paths as POSIX drive paths, prefixed with `//`.
fn claude_read_rules(data_root: &Path) -> Vec<String> {
    let resolved = crate::win::cred::resolve(data_root)
        .unwrap_or_else(|_| data_root.to_string_lossy().into_owned())
        .replace('\\', "/");
    let root = match resolved.split_once(':') {
        Some((drive, rest))
            if drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            format!("/{}{rest}", drive.to_lowercase())
        }
        _ => resolved,
    };
    vec![
        format!("Read(/{root}/accounts/**)"),
        format!("Read(/{root}/daemon-token)"),
        format!("Read(/{root}/data/**)"),
    ]
}

static MANAGED_RULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Read\((//.+)/(?:accounts/\*\*|daemon-token|data/\*\*|tjommie\.db\*)\)$").unwrap()
});
static GLOB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*?\[\]{}]").unwrap());
static DRIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/([a-z])(/|$)").unwrap());

/// Tjommie's managed read rules for data roots that no longer exist. Partial groups, globs, live roots and roots in an
/// uncertain state (access denied) are kept.
fn stale_claude_read_rules(existing: &[String]) -> HashSet<String> {
    let rules: HashSet<&str> = existing.iter().map(String::as_str).collect();
    let (mut checked, mut stale) = (HashSet::new(), HashSet::new());
    for rule in existing {
        let Some(root) = MANAGED_RULE
            .captures(rule)
            .map(|captures| captures[1].to_string())
        else {
            continue;
        };
        if GLOB.is_match(&root) || !checked.insert(root.clone()) {
            continue;
        }
        let folder = DRIVE.replace(&root[1..], "$1:$2").into_owned();
        if !Path::new(&folder).is_absolute() {
            continue;
        }
        let managed = claude_read_rules(Path::new(&folder));
        // Tjommie always writes this exact group together.
        let legacy_db_rule = format!("Read({root}/tjommie.db*)");
        if !managed.contains(rule) && *rule != legacy_db_rule {
            continue;
        }
        if !managed.iter().all(|entry| rules.contains(entry.as_str()))
            && !(*rule == legacy_db_rule && rules.contains(legacy_db_rule.as_str()))
        {
            continue;
        }
        if let Err(error) = std::fs::symlink_metadata(&folder)
            && error.kind() == io::ErrorKind::NotFound
        {
            stale.extend(managed);
            if rules.contains(legacy_db_rule.as_str()) {
                stale.insert(legacy_db_rule);
            }
        }
    }
    stale
}

/// A settings object and its deny rules.
type ClaudeSettings = (Map<String, Value>, Vec<String>);

/// The settings, or `None` (logged) when the file isn't the shape Tjommie can merge into. Filesystem errors are
/// errors.
fn read_claude_settings(file: &Path) -> io::Result<Option<ClaudeSettings>> {
    let contents = if file.exists() {
        std::fs::read_to_string(file)?
    } else {
        "{}".to_string()
    };
    let parsed = (|| {
        let Ok(Value::Object(settings)) = serde_json::from_str::<Value>(&contents) else {
            return None;
        };
        let permissions = match settings.get("permissions") {
            None => Map::new(),
            Some(Value::Object(permissions)) => permissions.clone(),
            Some(_) => return None,
        };
        let deny = match permissions.get("deny") {
            None => vec![],
            Some(Value::Array(rules)) => rules
                .iter()
                .map(|rule| rule.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()?,
            Some(_) => return None,
        };
        Some((settings, deny))
    })();
    if parsed.is_none() {
        // No parser error: it can contain values from the settings file.
        eprintln!(
            "Could not read Claude's settings. The existing file was left unchanged. {}",
            file.display()
        );
    }
    Ok(parsed)
}

fn harden_claude_settings(folder: &Path, data_root: &Path) -> io::Result<()> {
    let file = folder.join("settings.json");
    let Some((mut settings, existing)) = read_claude_settings(&file)? else {
        return Ok(());
    };
    let stale = stale_claude_read_rules(&existing);
    let mut seen = HashSet::new();
    let deny: Vec<String> = existing
        .iter()
        .filter(|rule| !stale.contains(*rule))
        .cloned()
        .chain([
            "Edit(**/.claude/**)".to_string(),
            "Write(**/.claude/**)".to_string(),
        ])
        .chain(claude_read_rules(data_root))
        .filter(|rule| seen.insert(rule.clone()))
        .collect();
    if deny == existing {
        return Ok(());
    }
    let mut permissions = settings
        .get("permissions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    permissions.insert("deny".into(), json!(deny));
    settings.insert("permissions".into(), Value::Object(permissions));
    std::fs::write(&file, format!("{}\n", js::pretty(&Value::Object(settings))))
}

/// The owner's callback for a finished sign-in: `(ok, attempt)`.
pub type OnDone = Arc<dyn Fn(bool, String) + Send + Sync>;
type Cancel = Arc<dyn Fn() -> BoxFuture<'static, Result<(), RpcError>> + Send + Sync>;

/// One provider sign-in. Tjommie never sees a password or token; each CLI stores its login in the account folder.
struct Attempt {
    id: String,
    provider: Provider,
    folder: String,
    /// `None` while starting; then Claude's `Some(None)` or Codex's `Some(Some(url))`.
    url: Mutex<Option<Option<String>>>,
    finished: AtomicBool,
    cleanup: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    cancel: Mutex<Option<Cancel>>,
    on_done: OnDone,
    registry: Weak<Inner>,
}

impl Attempt {
    fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    fn finish(self: &Arc<Self>, ok: bool) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(cleanup) = self.cleanup.lock().expect("cleanup lock").take() {
            cleanup();
        }
        if let Some(inner) = self.registry.upgrade() {
            let mut sign_ins = inner.sign_ins.lock().expect("sign-ins lock");
            if sign_ins
                .get(&self.folder)
                .is_some_and(|running| Arc::ptr_eq(running, self))
            {
                sign_ins.remove(&self.folder);
            }
        }
        (self.on_done)(ok, self.id.clone());
    }
}

struct Inner {
    cli: Arc<dyn CliHost>,
    fetch: Arc<dyn UsageFetch>,
    sign_ins: Mutex<HashMap<String, Arc<Attempt>>>,
    sandbox_setups: Mutex<HashSet<String>>,
}

/// Provider setup over a CLI host. Cloning shares its sign-in and sandbox-setup state.
#[derive(Clone)]
pub struct Setup(Arc<Inner>);

impl Setup {
    pub fn new(cli: Arc<dyn CliHost>, fetch: Arc<dyn UsageFetch>) -> Self {
        Setup(Arc::new(Inner {
            cli,
            fetch,
            sign_ins: Mutex::default(),
            sandbox_setups: Mutex::default(),
        }))
    }

    pub fn cli(&self) -> &Arc<dyn CliHost> {
        &self.0.cli
    }

    fn mapped(&self, account: &Account) -> Account {
        Account {
            folder: self.0.cli.provider_account_folder(&account.folder),
            ..account.clone()
        }
    }

    /// One Tjommie-owned account per provider, created once with its own folder, then each account folder prepared
    /// and legacy bots pinned.
    pub fn ensure_accounts(
        &self,
        db: &Connection,
        accounts_dir: &Path,
        data_root: &Path,
    ) -> Result<Vec<Account>, RpcError> {
        for provider in [Provider::Claude, Provider::Codex] {
            if list_accounts(db, true)?
                .iter()
                .any(|entry| entry.provider == provider)
            {
                continue;
            }
            let id = format!("{}-main", provider_name(provider));
            let folder = accounts_dir.join(&id);
            std::fs::create_dir_all(&folder)?;
            db.execute(
                "INSERT OR IGNORE INTO accounts (id, provider, label, kind, folder, created_at) VALUES (?1, ?2, ?3, 'subscription', ?4, ?5)",
                rusqlite::params![
                    id,
                    provider_name(provider),
                    if provider == Provider::Claude { "Claude" } else { "Codex" },
                    folder.to_string_lossy(),
                    now_iso()
                ],
            )?;
        }
        for account in list_accounts(db, false)? {
            let account = Account {
                id: account.id,
                provider: account.provider,
                label: account.label,
                folder: account.folder,
            };
            self.prepare_account_folder(&account, data_root)?;
        }
        for provider in [Provider::Claude, Provider::Codex] {
            let accounts: Vec<_> = list_accounts(db, false)?
                .into_iter()
                .filter(|account| account.provider == provider)
                .collect();
            if !accounts.is_empty() && !accounts.iter().any(|account| account.is_default) {
                db.execute(
                    "UPDATE accounts SET is_default = 1 WHERE id = ?1",
                    [get_default_account(db, provider)?.id],
                )?;
            }
        }
        bind_legacy_bots(db)?;
        let accounts = list_accounts(db, false)?;
        Ok(accounts
            .into_iter()
            .map(|account| Account {
                id: account.id,
                provider: account.provider,
                label: account.label,
                folder: account.folder,
            })
            .collect())
    }

    pub fn prepare_account_folder(&self, account: &Account, data_root: &Path) -> io::Result<()> {
        let account = self.mapped(account);
        let folder = Path::new(&account.folder);
        std::fs::create_dir_all(folder)?;
        if std::fs::symlink_metadata(folder)?.file_type().is_symlink() {
            return Err(io::Error::other("The account folder must not be a link."));
        }
        if account.provider == Provider::Claude {
            crate::providers::claude_files::sweep_claude_session_files(folder);
            harden_claude_settings(folder, data_root)?;
        }
        Ok(())
    }

    pub fn account_for(&self, db: &Connection, provider: Provider) -> Result<Account, RpcError> {
        let account = get_default_account(db, provider)?;
        Ok(self.mapped(&Account {
            id: account.id,
            provider,
            label: account.label,
            folder: account.folder,
        }))
    }

    /// `accountEnv`: the account folder as the CLI's config home, over a scrubbed environment.
    pub fn account_env(&self, account: &Account) -> Env {
        self.account_env_from(account, &process_env())
    }

    pub fn account_env_from(&self, account: &Account, base: &Env) -> Env {
        let key = if account.provider == Provider::Claude {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        self.0.cli.cli_env_from(
            &Env::from([(key.to_string(), account.folder.clone())]),
            base,
        )
    }

    /// `checkSetup`: both providers' default accounts.
    pub fn check_setup(&self, db: &Connection) -> BoxFuture<'static, Vec<ProviderSetup>> {
        let accounts = [Provider::Claude, Provider::Codex]
            .map(|provider| (provider, self.account_for(db, provider)));
        let setup = self.clone();
        Box::pin(async move {
            let mut setups = Vec::new();
            for (provider, account) in accounts {
                setups.push(match account {
                    Ok(account) => setup.check_account_setup(&account).await,
                    Err(_) => failed_check(provider, None, "Add an account in Accounts."),
                });
            }
            setups
        })
    }

    pub async fn check_account_setup(&self, account: &Account) -> ProviderSetup {
        match account.provider {
            Provider::Claude => self.claude_setup(account).await,
            Provider::Codex => self.codex_setup(account).await,
        }
    }

    async fn claude_setup(&self, account: &Account) -> ProviderSetup {
        let account = self.mapped(account);
        let Some(cmd) = self.0.cli.find_cli(Provider::Claude) else {
            return not_installed(Provider::Claude);
        };
        let env = self.account_env(&account);
        let (version_args, status_args) = (
            ["--version".to_string()],
            ["auth".to_string(), "status".into()],
        );
        let (version, status) = tokio::join!(
            self.0.cli.run_cli(&cmd, &version_args, &env, RUN_TIMEOUT),
            self.0.cli.run_cli(&cmd, &status_args, &env, RUN_TIMEOUT)
        );
        let Some(version) = version.ok().filter(|version| version.code == Some(0)) else {
            return failed_check(Provider::Claude, None, "Claude Code version check failed");
        };
        let version = parse_version(&version.stdout);
        let status = status
            .ok()
            .filter(|status| status.code.is_some())
            .and_then(|status| try_parse_claude_status(&status.stdout));
        let Some(status) = status else {
            return failed_check(Provider::Claude, version, "Claude sign-in check failed");
        };
        let fields = SetupFields {
            installed: true,
            version,
            signed_in: status.signed_in,
            account: status.account,
            plan: status.plan,
            error: None,
        };
        ProviderSetup::Claude(fields)
    }

    async fn codex_setup(&self, account: &Account) -> ProviderSetup {
        let account = self.mapped(account);
        let Some(cmd) = self.0.cli.find_cli(Provider::Codex) else {
            return not_installed(Provider::Codex);
        };
        let env = self.account_env(&account);
        let version = self
            .0
            .cli
            .run_cli(&cmd, &["--version".to_string()], &env, RUN_TIMEOUT)
            .await;
        let Some(version) = version.ok().filter(|version| version.code == Some(0)) else {
            return failed_check(Provider::Codex, None, "Codex version check failed");
        };
        let version = parse_version(&version.stdout);
        let checked = async {
            let server = self
                .0
                .cli
                .start_codex_server(&cmd, &env, &account.folder, false)
                .await?;
            let read = server.rpc.request("account/read", json!({})).await;
            let sandbox = match &read {
                Ok(_) => codex_sandbox_status(&server.rpc, &account.folder).await,
                Err(_) => CodexSandboxReadiness::Unavailable,
            };
            close_server(&server).await;
            Ok::<_, ProviderError>((read?, sandbox))
        };
        match checked.await {
            Ok((read, sandbox)) => {
                let signed_in = read.get("account").filter(|account| account.is_object());
                let fields = SetupFields {
                    installed: true,
                    version,
                    signed_in: signed_in.is_some_and(|account| {
                        account.get("type").and_then(Value::as_str) != Some("apiKey")
                    }),
                    account: signed_in
                        .and_then(|account| account.get("email"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    plan: signed_in
                        .and_then(|account| account.get("planType"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    error: None,
                };
                ProviderSetup::Codex(CodexSetup {
                    setup: fields,
                    sandbox,
                })
            }
            Err(_) => failed_check(Provider::Codex, version, "Codex account check failed."),
        }
    }

    /// ChatGPT subscription metadata only: never creates a thread or starts a model turn.
    async fn read_codex_account_limits(&self, account: &Account) -> Option<Value> {
        let account = self.mapped(account);
        if account.provider != Provider::Codex {
            return None;
        }
        let cmd = self.0.cli.find_cli(Provider::Codex)?;
        let server = self
            .0
            .cli
            .start_codex_server(&cmd, &self.account_env(&account), &account.folder, false)
            .await
            .ok()?;
        let read = server
            .rpc
            .request_with_timeout(
                "account/rateLimits/read",
                json!({}),
                Duration::from_secs(15),
            )
            .await;
        (server.close)();
        let closed = tokio::time::timeout(
            Duration::from_secs(5),
            crate::cli::wait_for_cli_exit(&server.exit, server.retirement.as_ref()),
        )
        .await;
        if !matches!(closed, Ok(Ok(()))) {
            return None; // The account check process did not close.
        }
        read.ok()
    }

    pub async fn check_account_limits(&self, account: &Account) -> Option<Vec<LimitSnapshot>> {
        parse_codex_account_limits(&self.read_codex_account_limits(account).await?)
    }

    pub async fn check_account_usage(
        &self,
        account: &Account,
        accounts_root: &Path,
    ) -> AccountUsageRead {
        let account = self.mapped(account);
        let root = self
            .0
            .cli
            .worker_accounts_root()
            .unwrap_or_else(|| accounts_root.to_path_buf());
        if account.provider == Provider::Claude {
            let options = ClaudeUsageOptions {
                accounts_root: root,
                fetch: self.0.fetch.as_ref(),
                now: &now_ms,
            };
            return read_claude_account_usage(&account, &options).await;
        }
        let value = self
            .read_codex_account_limits(&account)
            .await
            .unwrap_or(Value::Null);
        normalize_codex_usage(&value, parse_codex_account_limits(&value).as_deref())
    }

    pub fn prepare_codex_sandbox(
        &self,
        db: &Connection,
        timeout: Duration,
    ) -> BoxFuture<'static, Result<(), RpcError>> {
        let account = self.account_for(db, Provider::Codex);
        let setup = self.clone();
        Box::pin(async move {
            setup
                .prepare_account_codex_sandbox(&account?, timeout)
                .await
        })
    }

    /// An explicit user action. A failed or declined setup is never retried automatically.
    pub async fn prepare_account_codex_sandbox(
        &self,
        account: &Account,
        timeout: Duration,
    ) -> Result<(), RpcError> {
        let account = self.mapped(account);
        if account.provider != Provider::Codex {
            return Err(RpcError::new(
                ErrorCode::BadParams,
                "Choose a Codex account.",
            ));
        }
        if !self
            .0
            .sandbox_setups
            .lock()
            .expect("sandbox lock")
            .insert(account.folder.clone())
        {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "Codex's sandbox setup is already running.",
            ));
        }
        let result = async {
            let cmd =
                self.0.cli.find_cli(Provider::Codex).ok_or_else(|| {
                    RpcError::new(ErrorCode::NotInstalled, "Codex is not installed")
                })?;
            let cwd = tempfile::Builder::new()
                .prefix("tjommie-codex-setup-")
                .tempdir()?;
            let cwd_text = cwd.path().to_string_lossy().into_owned();
            let env = self.account_env(&account);
            let server = self
                .0
                .cli
                .start_codex_server(&cmd, &env, &cwd_text, false)
                .await
                .map_err(rpc_error)?;
            let outcome = run_sandbox_setup(&server.rpc, &cwd_text, timeout).await;
            close_server(&server).await;
            outcome?;
            let fresh = self
                .0
                .cli
                .start_codex_server(&cmd, &env, &cwd_text, false)
                .await
                .map_err(rpc_error)?;
            let readiness = codex_sandbox_status(&fresh.rpc, &cwd_text).await;
            close_server(&fresh).await;
            if readiness != CodexSandboxReadiness::Ready {
                return Err(bad_state("Codex's sandbox is still not ready after setup."));
            }
            Ok(())
        }
        .await;
        self.0
            .sandbox_setups
            .lock()
            .expect("sandbox lock")
            .remove(&account.folder);
        result
    }

    /// Explicit owner removal only. The provider removes its own authentication using the selected account home;
    /// Tjommie never reads or copies a credential file.
    pub async fn logout_account(&self, account: &Account) -> Result<(), RpcError> {
        let account = self.mapped(account);
        let cmd = self.0.cli.find_cli(account.provider).ok_or_else(|| {
            RpcError::new(
                ErrorCode::NotInstalled,
                "Install this account's provider before removing its local sign-in.",
            )
        })?;
        let args: Vec<String> = if account.provider == Provider::Claude {
            vec!["auth".into(), "logout".into()]
        } else {
            vec!["logout".into()]
        };
        match self
            .0
            .cli
            .run_cli(&cmd, &args, &self.account_env(&account), RUN_TIMEOUT)
            .await
        {
            Ok(output) if output.code == Some(0) => Ok(()),
            _ => Err(bad_state(
                "This account could not sign out. It has been kept so you can retry removal.",
            )),
        }
    }

    pub fn sign_in(
        &self,
        db: &Connection,
        provider: Provider,
        on_done: OnDone,
    ) -> BoxFuture<'static, Result<SignInStart, RpcError>> {
        let account = self.account_for(db, provider);
        let setup = self.clone();
        Box::pin(async move { setup.sign_in_account(&account?, on_done).await })
    }

    pub async fn cancel_account_sign_in(
        &self,
        account: &Account,
        id: &str,
    ) -> Result<(), RpcError> {
        let account = self.mapped(account);
        let attempt = self
            .0
            .sign_ins
            .lock()
            .expect("sign-ins lock")
            .get(&account.folder)
            .filter(|attempt| attempt.id == id)
            .cloned();
        let attempt =
            attempt.ok_or_else(|| bad_state("That sign-in attempt is no longer active."))?;
        let cancel = attempt.cancel.lock().expect("cancel lock").clone();
        match cancel {
            Some(cancel) => cancel().await,
            None => {
                attempt.finish(false);
                Ok(())
            }
        }
    }

    /// Starts the provider's own sign-in for this account and calls `on_done` once it finishes or times out. Claude:
    /// hidden `claude auth login`, which opens its own browser callback. Codex: the app server's sign-in URL, opened in
    /// the browser.
    pub async fn sign_in_account(
        &self,
        account: &Account,
        on_done: OnDone,
    ) -> Result<SignInStart, RpcError> {
        let account = self.mapped(account);
        let provider = account.provider;
        let attempt = {
            let mut sign_ins = self.0.sign_ins.lock().expect("sign-ins lock");
            if let Some(running) = sign_ins.get(&account.folder) {
                let url = running.url.lock().expect("url lock").clone();
                let name = if provider == Provider::Claude {
                    "Claude"
                } else {
                    "Codex"
                };
                let Some(url) = url.filter(|_| running.provider == provider) else {
                    return Err(RpcError::new(
                        ErrorCode::Busy,
                        format!("{name} sign-in is already running."),
                    ));
                };
                if let Some(url) = url.as_deref().filter(|_| provider == Provider::Codex) {
                    let _ = self.0.cli.open_url(url);
                }
                return Ok(SignInStart {
                    url,
                    attempt: running.id.clone(),
                });
            }
            let attempt = Arc::new(Attempt {
                id: new_id(),
                provider,
                folder: account.folder.clone(),
                url: Mutex::new(None),
                finished: AtomicBool::new(false),
                cleanup: Mutex::new(None),
                cancel: Mutex::new(None),
                on_done,
                registry: Arc::downgrade(&self.0),
            });
            sign_ins.insert(account.folder.clone(), Arc::clone(&attempt));
            attempt
        };
        let started = match provider {
            Provider::Claude => self.start_claude_sign_in(&account, &attempt).await,
            Provider::Codex => self.start_codex_sign_in(&account, &attempt).await,
        };
        if started.is_err() {
            attempt.finish(false);
        }
        started
    }

    async fn start_claude_sign_in(
        &self,
        account: &Account,
        attempt: &Arc<Attempt>,
    ) -> Result<SignInStart, RpcError> {
        let cmd = self.0.cli.find_cli(Provider::Claude).ok_or_else(|| {
            RpcError::new(ErrorCode::NotInstalled, "Claude Code is not installed")
        })?;
        let env = self.account_env(account);
        let before = self
            .0
            .cli
            .run_cli(
                &cmd,
                &["auth".to_string(), "status".into()],
                &env,
                RUN_TIMEOUT,
            )
            .await
            .map_err(internal)?;
        if attempt.finished() {
            return Err(bad_state("The sign-in attempt was cancelled."));
        }
        if before.code.is_none() || try_parse_claude_status(&before.stdout).is_none() {
            return Err(bad_state("Claude sign-in check failed."));
        }
        let args = vec!["auth".to_string(), "login".into(), "--claudeai".into()];
        let options = LaunchOptions {
            cwd: account.folder.clone(),
            env,
            claude_api_key: None,
        };
        let child = self
            .0
            .cli
            .launch(&cmd, args, options)
            .await
            .map_err(internal)?;
        // `auth login` opens the automatic loopback URL itself. Its printed URL is the manual-code fallback, so it is
        // never offered as a reopen link, and no auth output is logged or returned.
        *attempt.url.lock().expect("url lock") = Some(None);
        let (exit, kill_tree) = (child.exit.clone(), child.kill_tree.clone());
        tokio::spawn(drain(child.stdout));
        tokio::spawn(drain(child.stderr));
        // Claude's login reads stdin ("Paste code here if prompted") and end-of-input would abort it, so the pipe stays
        // open, unwritten, until the process exits.
        let stdin = child.stdin;
        let cancelling = Arc::new(AtomicBool::new(false));
        let deadline = tokio::spawn({
            let attempt = Arc::downgrade(attempt);
            async move {
                tokio::time::sleep(SIGN_IN_TIMEOUT).await;
                let cancel = attempt
                    .upgrade()
                    .and_then(|attempt| attempt.cancel.lock().expect("cancel lock").clone());
                if let Some(cancel) = cancel {
                    let _ = cancel().await; // A failed containment keeps the attempt for an explicit retry.
                }
            }
        });
        *attempt.cleanup.lock().expect("cleanup lock") = Some(Box::new(move || deadline.abort()));
        let cancel: Cancel = {
            let (attempt, exit, cancelling) =
                (Arc::clone(attempt), exit.clone(), Arc::clone(&cancelling));
            Arc::new(move || {
                let (attempt, exit, kill_tree, cancelling) = (
                    Arc::clone(&attempt),
                    exit.clone(),
                    kill_tree.clone(),
                    Arc::clone(&cancelling),
                );
                Box::pin(async move {
                    cancelling.store(true, Ordering::SeqCst);
                    if exit.status().is_none() {
                        kill_tree().await.map_err(internal)?;
                    }
                    exit.wait().await;
                    attempt.finish(false);
                    Ok(())
                })
            })
        };
        *attempt.cancel.lock().expect("cancel lock") = Some(cancel);
        exit.on_exit({
            let attempt = Arc::clone(attempt);
            move |exit| {
                drop(stdin);
                let ok = matches!(exit, CliExit::Exited { code: Some(0), .. })
                    && !cancelling.load(Ordering::SeqCst);
                attempt.finish(ok);
            }
        });
        Ok(SignInStart {
            url: None,
            attempt: attempt.id.clone(),
        })
    }

    async fn start_codex_sign_in(
        &self,
        account: &Account,
        attempt: &Arc<Attempt>,
    ) -> Result<SignInStart, RpcError> {
        let cmd = self
            .0
            .cli
            .find_cli(Provider::Codex)
            .ok_or_else(|| RpcError::new(ErrorCode::NotInstalled, "Codex is not installed"))?;
        let server = Arc::new(
            self.0
                .cli
                .start_codex_server(&cmd, &self.account_env(account), &account.folder, false)
                .await
                .map_err(rpc_error)?,
        );
        if attempt.finished() {
            close_server(&server).await;
            return Err(bad_state("The sign-in attempt was cancelled."));
        }
        let timer = tokio::spawn({
            let attempt = Arc::downgrade(attempt);
            async move {
                tokio::time::sleep(SIGN_IN_TIMEOUT).await;
                if let Some(attempt) = attempt.upgrade() {
                    attempt.finish(false);
                }
            }
        });
        *attempt.cleanup.lock().expect("cleanup lock") = Some(Box::new({
            let server = Arc::clone(&server);
            move || {
                timer.abort();
                (server.close)();
            }
        }));
        let login_id: Arc<Mutex<Option<String>>> = Arc::default();
        let early: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::default();
        server.rpc.set_on_notification({
            let (attempt, login_id, early) = (
                Arc::downgrade(attempt),
                Arc::clone(&login_id),
                Arc::clone(&early),
            );
            move |method, params| {
                let Value::Object(params) = params else {
                    return;
                };
                if method != "account/login/completed" {
                    return;
                }
                let current = login_id.lock().expect("login lock").clone();
                match current {
                    None => {
                        let mut early = early.lock().expect("early lock");
                        if early.len() < 10 {
                            early.push(params);
                        }
                    }
                    Some(current)
                        if params.get("loginId").and_then(Value::as_str)
                            == Some(current.as_str()) =>
                    {
                        if let Some(attempt) = attempt.upgrade() {
                            attempt.finish(params.get("success") == Some(&Value::Bool(true)));
                        }
                    }
                    Some(_) => {}
                }
            }
        });
        let cancel: Cancel = {
            let (attempt, server, login_id) = (
                Arc::clone(attempt),
                Arc::clone(&server),
                Arc::clone(&login_id),
            );
            Arc::new(move || {
                let (attempt, server, login_id) = (
                    Arc::clone(&attempt),
                    Arc::clone(&server),
                    login_id.lock().expect("login lock").clone(),
                );
                Box::pin(async move {
                    if let Some(login_id) = login_id {
                        let _ = server
                            .rpc
                            .request("account/login/cancel", json!({ "loginId": login_id }))
                            .await;
                    }
                    attempt.finish(false);
                    let _ = crate::cli::wait_for_cli_exit(&server.exit, server.retirement.as_ref())
                        .await;
                    Ok(())
                })
            })
        };
        *attempt.cancel.lock().expect("cancel lock") = Some(cancel);
        let login = server
            .rpc
            .request("account/login/start", json!({ "type": "chatgpt" }))
            .await
            .map_err(rpc_error)?;
        let (Some(id), Some(url)) = (
            login
                .get("loginId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty()),
            login.get("authUrl").and_then(Value::as_str),
        ) else {
            return Err(bad_state("Codex returned an invalid sign-in attempt."));
        };
        *login_id.lock().expect("login lock") = Some(id.to_string());
        if attempt.finished() {
            let _ = server
                .rpc
                .request("account/login/cancel", json!({ "loginId": id }))
                .await;
            return Err(bad_state("The sign-in attempt was cancelled."));
        }
        *attempt.url.lock().expect("url lock") = Some(Some(url.to_string()));
        let completed = early
            .lock()
            .expect("early lock")
            .iter()
            .find(|event| event.get("loginId").and_then(Value::as_str) == Some(id))
            .cloned();
        if let Some(completed) = completed {
            attempt.finish(completed.get("success") == Some(&Value::Bool(true)));
        }
        if !attempt.finished() {
            let _ = self.0.cli.open_url(url);
        }
        Ok(SignInStart {
            url: Some(url.to_string()),
            attempt: attempt.id.clone(),
        })
    }

    /// `listModels`: Claude's aliases, or the Codex app server's visible models for the default account.
    pub fn list_models(
        &self,
        db: &Connection,
        provider: Provider,
    ) -> BoxFuture<'static, Result<Vec<ModelInfo>, RpcError>> {
        if provider == Provider::Claude {
            return Box::pin(async { Ok(crate::models::claude_models()) });
        }
        let account = self.account_for(db, Provider::Codex);
        let setup = self.clone();
        Box::pin(async move {
            let cmd =
                setup.0.cli.find_cli(Provider::Codex).ok_or_else(|| {
                    RpcError::new(ErrorCode::NotInstalled, "Codex is not installed")
                })?;
            let account = account?;
            let server = setup
                .0
                .cli
                .start_codex_server(&cmd, &setup.account_env(&account), &account.folder, false)
                .await
                .map_err(rpc_error)?;
            let listed = server.rpc.request("model/list", json!({})).await;
            close_server(&server).await;
            let data = listed.map_err(rpc_error)?;
            let models = data["data"].as_array().cloned().unwrap_or_default();
            Ok(models
                .iter()
                .filter(|model| model["hidden"] != Value::Bool(true))
                .map(|model| ModelInfo {
                    id: model["model"].as_str().unwrap_or_default().to_string(),
                    label: model["displayName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    is_default: model["isDefault"] == Value::Bool(true),
                })
                .collect())
        })
    }
}

async fn drain(mut output: Box<dyn tokio::io::AsyncRead + Send + Unpin>) {
    let _ = tokio::io::copy(&mut output, &mut tokio::io::sink()).await;
}

/// Closes a server and waits for its process (or its worker's proof that the tree has gone).
async fn close_server(server: &CodexServer) {
    (server.close)();
    let _ = crate::cli::wait_for_cli_exit(&server.exit, server.retirement.as_ref()).await;
}

async fn codex_sandbox_status(rpc: &JsonRpcLines, cwd: &str) -> CodexSandboxReadiness {
    let (Ok(config), Ok(readiness)) = (
        rpc.request("config/read", json!({ "cwd": cwd })).await,
        rpc.request("windowsSandbox/readiness", json!({})).await,
    ) else {
        return CodexSandboxReadiness::Unavailable;
    };
    if !config["config"].is_object() || !readiness.is_object() {
        return CodexSandboxReadiness::Unavailable;
    }
    if config["config"]["windows"]["sandbox"] != "elevated" {
        return CodexSandboxReadiness::NotConfigured;
    }
    match readiness["status"].as_str() {
        Some("ready") => CodexSandboxReadiness::Ready,
        Some("notConfigured") => CodexSandboxReadiness::NotConfigured,
        Some("updateRequired") => CodexSandboxReadiness::UpdateRequired,
        _ => CodexSandboxReadiness::Unavailable,
    }
}

async fn run_sandbox_setup(
    rpc: &JsonRpcLines,
    cwd: &str,
    timeout: Duration,
) -> Result<(), RpcError> {
    let (completed, finished) = tokio::sync::watch::channel(None::<(bool, Option<String>)>);
    rpc.set_on_notification(move |method, params| {
        if method == "windowsSandbox/setupCompleted"
            && params.get("mode").and_then(Value::as_str) == Some("elevated")
        {
            let error = params
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string);
            completed.send_replace(Some((
                params.get("success") == Some(&Value::Bool(true)),
                error,
            )));
        }
    });
    let started = rpc
        .request(
            "windowsSandbox/setupStart",
            json!({ "mode": "elevated", "cwd": cwd }),
        )
        .await
        .map_err(rpc_error)?;
    if started.get("started") != Some(&Value::Bool(true)) {
        return Err(bad_state("Codex did not start sandbox setup."));
    }
    let mut finished = finished;
    let outcome = tokio::time::timeout(timeout, finished.wait_for(Option::is_some)).await;
    let Ok(Ok(outcome)) = outcome else {
        return Err(bad_state(
            "Codex's sandbox setup timed out after 15 minutes.",
        ));
    };
    let (success, error) = outcome.clone().expect("waited for a completion");
    if !success {
        return Err(bad_state(
            error
                .as_deref()
                .unwrap_or("Codex's sandbox setup was declined or failed."),
        ));
    }
    Ok(())
}
