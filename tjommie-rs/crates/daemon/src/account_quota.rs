//! `account-quota.ts`: a subscription account's provider usage, normalized for display and for limit recovery.
//! Claude's comes from its OAuth usage endpoint with the account's own access token; Codex's from the app server.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use protocol::accounts::{
    AccountProviderUsage, AccountUsageWindow, Credits, UsageScope, UsageScopeKind, UsageStatus,
};
use regex::Regex;
use serde_json::{Map, Value};

use crate::accounts_store::iso;
use crate::cli::BoxFuture;
use crate::providers::{LimitState, LimitWindow, js};
use crate::setup::Account;

/// One provider limit's windows, as `replaceAccountLimits` stores them (`AccountLimitSnapshot` in `setup.ts`).
#[derive(Debug, Clone, PartialEq)]
pub struct LimitSnapshot {
    pub limit_id: String,
    pub windows: Vec<LimitWindow>,
}

/// The displayable part of `AccountProviderUsage`.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsageData {
    pub plan_label: Option<String>,
    pub windows: Vec<AccountUsageWindow>,
    pub credits: Option<Credits>,
    pub extra_usage_enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountUsageRead {
    /// `recovery_allowed`: the read is complete enough to clear an earlier exhausted window.
    Available {
        usage: AccountUsageData,
        limits: Vec<LimitSnapshot>,
        recovery_allowed: bool,
    },
    Unavailable {
        error: Option<String>,
    },
    Error {
        error: Option<String>,
    },
}

fn record(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn present<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !value.is_null())
}

static CONTROL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x00-\x1f\x7f]").unwrap());
static DECIMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(?:\.\d+)?$").unwrap());
static BALANCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+(?:\.\d+)?$").unwrap());
static NOT_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

/// Display text: control characters removed, trimmed and cut to `maximum` characters.
fn label(value: Option<&Value>, maximum: usize) -> Option<String> {
    let text = CONTROL.replace_all(value?.as_str()?, "");
    Some(js::slice(js::trim(&text), maximum).to_string()).filter(|text| !text.is_empty())
}

fn percent(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::String(text) if text.len() <= 32 && DECIMAL.is_match(text) => {
            text.parse::<f64>().ok()?
        }
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    (number.is_finite() && (0.0..=100.0).contains(&number)).then_some(number)
}

fn instant(value: Option<&Value>) -> Option<String> {
    iso(value?.as_str()?)
}

fn failed() -> AccountUsageRead {
    AccountUsageRead::Error {
        error: Some("Provider usage could not be read. Try again later.".into()),
    }
}

pub fn empty_provider_usage() -> AccountProviderUsage {
    AccountProviderUsage {
        status: UsageStatus::Unavailable,
        checked_at: None,
        last_successful_at: None,
        error: None,
        plan_label: None,
        windows: vec![],
        credits: None,
        extra_usage_enabled: None,
    }
}

fn state(used: f64) -> LimitState {
    if used >= 100.0 {
        LimitState::Reached
    } else if used >= 90.0 {
        LimitState::Near
    } else {
        LimitState::Ok
    }
}

/// Codex's rate-limit read. `snapshots` is what `parse_codex_account_limits` made of the same value; `None` means it
/// was incomplete.
pub fn normalize_codex_usage(
    value: &Value,
    snapshots: Option<&[LimitSnapshot]>,
) -> AccountUsageRead {
    let Some(value) = value.as_object() else {
        return failed();
    };
    let Some(rate_limits) = record(value.get("rateLimits")) else {
        return failed();
    };
    let by_id = record(value.get("rateLimitsByLimitId"));
    let buckets: Vec<&Value> = match by_id {
        Some(map) => map.values().collect(),
        None => vec![&value["rateLimits"]],
    };
    if buckets.is_empty() || buckets.len() > 32 || !buckets.iter().all(|bucket| bucket.is_object())
    {
        return failed();
    }
    let buckets: Vec<&Map<String, Value>> =
        buckets.into_iter().filter_map(Value::as_object).collect();
    // A credits-only response is displayable, but never proves that a previous quota block recovered.
    let has_quota = |bucket: &&Map<String, Value>| {
        ["primary", "secondary", "rateLimitReachedType"]
            .iter()
            .any(|key| present(bucket, key).is_some())
    };
    if snapshots.is_none() && buckets.iter().any(has_quota) {
        return failed();
    }
    let mut limits: Vec<LimitSnapshot> =
        snapshots.map(<[LimitSnapshot]>::to_vec).unwrap_or_default();
    let mut windows = Vec::new();
    for snapshot in snapshots.unwrap_or_default() {
        // A present entry in the per-limit map wins even when it isn't an object, as JavaScript's `??` does.
        let bucket = match by_id.and_then(|map| present(map, &snapshot.limit_id)) {
            Some(entry) => entry.as_object(),
            None => buckets
                .iter()
                .copied()
                .find(|entry| {
                    entry.get("limitId").and_then(Value::as_str) == Some(snapshot.limit_id.as_str())
                })
                .or_else(|| (snapshot.limit_id == "codex").then_some(rate_limits)),
        };
        let model = bucket.and_then(|bucket| label(bucket.get("normalModelSlug"), 256));
        let name = bucket.and_then(|bucket| label(bucket.get("limitName"), 120));
        for window in snapshot
            .windows
            .iter()
            .filter(|window| window.window_key != "provider")
        {
            let title = match window.duration_minutes {
                Some(300.0) => "Session".to_string(),
                Some(10080.0) => "Weekly".to_string(),
                Some(minutes) => format!("{minutes} minute limit"),
                None => format!(
                    "{} limit",
                    if window.window_key == "primary" {
                        "Primary"
                    } else {
                        "Secondary"
                    }
                ),
            };
            let full = match &name {
                Some(name) => format!("{title} · {name}"),
                None => title,
            };
            windows.push(AccountUsageWindow {
                id: format!("{}:{}", snapshot.limit_id, window.window_key),
                label: js::slice(&full, 120).to_string(),
                scope: model.as_ref().map(|model| UsageScope {
                    kind: UsageScopeKind::Model,
                    id: Some(model.clone()),
                    label: name
                        .clone()
                        .unwrap_or_else(|| js::slice(model, 120).to_string()),
                }),
                duration_minutes: window.duration_minutes,
                used_percent: window.used_percent,
                resets_at: window.resets_at.clone(),
            });
        }
    }
    let raw_credits = record(rate_limits.get("credits")).or_else(|| {
        buckets
            .iter()
            .find_map(|bucket| record(bucket.get("credits")))
    });
    let credits = raw_credits.map(|credits| Credits {
        balance: credits
            .get("balance")
            .and_then(Value::as_str)
            .filter(|balance| BALANCE.is_match(balance) && js::utf16_len(balance) <= 100)
            .map(str::to_string),
        has_credits: credits.get("hasCredits").and_then(Value::as_bool),
        unlimited: credits.get("unlimited").and_then(Value::as_bool),
    });
    let permission_buckets: Vec<&Map<String, Value>> = std::iter::once(rate_limits)
        .chain(buckets.iter().copied())
        .collect();
    let spend_control =
        |bucket: &&Map<String, Value>| bucket.get("spendControlReached").and_then(Value::as_bool);
    let blocked = value.get("ordinaryUsageAllowed") == Some(&Value::Bool(false))
        || permission_buckets
            .iter()
            .any(|bucket| spend_control(bucket) == Some(true));
    if blocked {
        let permission = LimitWindow {
            window_key: "permission".into(),
            duration_minutes: None,
            used_percent: None,
            resets_at: None,
            state: LimitState::Reached,
        };
        match limits
            .iter_mut()
            .find(|snapshot| snapshot.limit_id == "provider")
        {
            Some(provider) => provider.windows.push(permission),
            None => limits.push(LimitSnapshot {
                limit_id: "provider".into(),
                windows: vec![permission],
            }),
        }
    }
    let plan_label = label(rate_limits.get("planType"), 120).or_else(|| {
        buckets
            .iter()
            .find(|bucket| bucket.get("planType").is_some_and(Value::is_string))
            .and_then(|bucket| label(bucket.get("planType"), 120))
    });
    windows.truncate(64);
    AccountUsageRead::Available {
        usage: AccountUsageData {
            plan_label,
            windows,
            credits,
            extra_usage_enabled: None,
        },
        limits,
        // Null or absent spend-control state is unavailable, not evidence that an old spend block cleared.
        recovery_allowed: snapshots.is_some()
            && value.get("ordinaryUsageAllowed") == Some(&Value::Bool(true))
            && permission_buckets
                .iter()
                .all(|bucket| spend_control(bucket) == Some(false)),
    }
}

fn claude_window(
    value: Option<&Value>,
    id: &str,
    title: &str,
    duration: f64,
    scope: Option<UsageScope>,
) -> Option<AccountUsageWindow> {
    let value = value?.as_object()?;
    let used = percent(present(value, "utilization").or_else(|| value.get("percent")))?;
    Some(AccountUsageWindow {
        id: id.into(),
        label: js::slice(title, 120).to_string(),
        scope,
        duration_minutes: Some(duration),
        used_percent: Some(used),
        resets_at: instant(value.get("resets_at")),
    })
}

/// Claude's OAuth usage response. Model and surface windows are display-only; only the account's session and weekly
/// windows become limits, and recovery needs every scoped window to be readable and none exhausted.
pub fn normalize_claude_usage(value: &Value, plan_label: Option<String>) -> AccountUsageRead {
    let Some(value) = value.as_object() else {
        return failed();
    };
    let mut windows: Vec<AccountUsageWindow> = Vec::new();
    for (key, title, duration) in [
        ("five_hour", "Session", 300.0),
        ("seven_day", "Weekly", 10080.0),
    ] {
        windows.extend(claude_window(value.get(key), key, title, duration, None));
    }
    let account_windows = windows.clone();
    let (mut scoped_complete, mut scoped_exhausted) = (true, false);
    for (key, name) in [
        ("seven_day_opus", "Opus"),
        ("seven_day_omelette", "Omelette"),
    ] {
        let scope = UsageScope {
            kind: UsageScopeKind::Model,
            id: None,
            label: name.into(),
        };
        let id = format!("weekly:model:{}", name.to_lowercase());
        match claude_window(
            value.get(key),
            &id,
            &format!("Weekly · {name}"),
            10080.0,
            Some(scope),
        ) {
            Some(window) => {
                scoped_exhausted |= window.used_percent == Some(100.0);
                windows.push(window);
            }
            None if present(value, key).is_some() => scoped_complete = false,
            None => {}
        }
    }
    let limits = present(value, "limits");
    if limits.is_some_and(|limits| !limits.is_array()) {
        scoped_complete = false;
    }
    let entries = limits
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for entry in entries.iter().take(128) {
        let Some(entry) = entry
            .as_object()
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("weekly_scoped"))
        else {
            scoped_complete = false;
            continue;
        };
        let Some(scope) = record(entry.get("scope")) else {
            scoped_complete = false;
            continue;
        };
        let kind = if record(scope.get("model")).is_some() {
            Some((UsageScopeKind::Model, "model"))
        } else if record(scope.get("surface")).is_some() {
            Some((UsageScopeKind::Surface, "surface"))
        } else {
            None
        };
        let Some((kind, kind_name)) = kind else {
            scoped_complete = false;
            continue;
        };
        let scope = record(scope.get(kind_name)).expect("checked above");
        let id = label(scope.get("id"), 256);
        let Some(name) = label(scope.get("display_name"), 120)
            .or_else(|| id.as_ref().map(|id| js::slice(id, 120).to_string()))
        else {
            scoped_complete = false;
            continue;
        };
        let fallback = NOT_ID.replace_all(&name.to_lowercase(), "_").into_owned();
        let window_id = format!("weekly:{kind_name}:{}", id.as_deref().unwrap_or(&fallback));
        let window_id = js::slice(&window_id, 256).to_string();
        let usage_scope = UsageScope {
            kind,
            id: id.clone(),
            label: name.clone(),
        };
        let Some(window) = claude_window(
            Some(&Value::Object(entry.clone())),
            &window_id,
            &format!("Weekly · {name}"),
            10080.0,
            Some(usage_scope),
        ) else {
            scoped_complete = false;
            continue;
        };
        scoped_exhausted |= window.used_percent == Some(100.0);
        let twin = windows.iter().position(|candidate| {
            candidate.scope.as_ref().is_some_and(|candidate| {
                candidate.kind == kind
                    && match (&id, &candidate.id) {
                        (Some(id), Some(other)) => id == other,
                        _ => candidate.label.to_lowercase() == name.to_lowercase(),
                    }
            })
        });
        match twin {
            Some(index) => windows[index] = window,
            None => windows.push(window),
        }
    }
    if entries.len() > 128 || windows.len() > 64 {
        scoped_complete = false;
    }
    let extra_usage_enabled = record(value.get("extra_usage"))
        .and_then(|extra| extra.get("is_enabled"))
        .and_then(Value::as_bool);
    if windows.is_empty() && extra_usage_enabled.is_none() {
        return failed();
    }
    let mut used_ids = HashSet::new();
    for window in &mut windows {
        let base = window.id.clone();
        let mut suffix = 1;
        while used_ids.contains(&window.id) {
            suffix += 1;
            window.id = format!("{}:{suffix}", js::slice(&base, 248));
        }
        used_ids.insert(window.id.clone());
    }
    let limits = if account_windows.is_empty() {
        vec![]
    } else {
        let windows = account_windows
            .iter()
            .map(|window| LimitWindow {
                window_key: window.id.clone(),
                duration_minutes: window.duration_minutes,
                used_percent: window.used_percent,
                resets_at: window.resets_at.clone(),
                state: state(window.used_percent.unwrap_or(0.0)),
            })
            .collect();
        vec![LimitSnapshot {
            limit_id: "claude".into(),
            windows,
        }]
    };
    windows.truncate(64);
    AccountUsageRead::Available {
        usage: AccountUsageData {
            plan_label,
            windows,
            credits: None,
            extra_usage_enabled,
        },
        limits,
        recovery_allowed: account_windows.len() == 2 && scoped_complete && !scoped_exhausted,
    }
}

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const FILE_LIMIT: u64 = 64 * 1024;
pub const RESPONSE_LIMIT: usize = 128 * 1024;
const COOLDOWN_LIMIT: usize = 128;

/// One HTTPS response, its body cut at `RESPONSE_LIMIT + 1` bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageResponse {
    pub status: u16,
    /// The final URL; a redirect changes it.
    pub url: String,
    pub retry_after: Option<String>,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

/// The HTTPS GET behind the usage read: never follows a redirect. Tests replace it, as TypeScript injects `fetch`.
pub trait UsageFetch: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'static str, String)],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<UsageResponse, String>>;
}

/// The real transport: reqwest over Windows' own TLS, with no redirects and no proxy from the environment, like
/// Node's fetch.
pub struct HttpsFetch(reqwest::Client);

impl HttpsFetch {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .https_only(true)
            .build()
            .map_err(|error| error.to_string())?;
        Ok(HttpsFetch(client))
    }
}

impl UsageFetch for HttpsFetch {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'static str, String)],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<UsageResponse, String>> {
        Box::pin(async move {
            let mut request = self.0.get(url).timeout(timeout);
            for (name, value) in headers {
                request = request.header(*name, value);
            }
            let mut response = request
                .send()
                .await
                .map_err(|error| error.without_url().to_string())?;
            let (status, url) = (response.status().as_u16(), response.url().to_string());
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let content_length = response.content_length();
            let mut body = Vec::new();
            while body.len() <= RESPONSE_LIMIT {
                match response
                    .chunk()
                    .await
                    .map_err(|error| error.without_url().to_string())?
                {
                    Some(chunk) => body.extend_from_slice(&chunk),
                    None => break,
                }
            }
            body.truncate(RESPONSE_LIMIT + 1);
            Ok(UsageResponse {
                status,
                url,
                retry_after,
                content_length,
                body,
            })
        })
    }
}

pub struct ClaudeUsageOptions<'a> {
    pub accounts_root: PathBuf,
    pub fetch: &'a dyn UsageFetch,
    /// Milliseconds since the epoch.
    pub now: &'a (dyn Fn() -> f64 + Send + Sync),
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
}

/// Rate-limited account folders and when they may be read again, oldest first.
static COOLDOWNS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());

struct ClaudeCredential {
    token: String,
    expires_at: Option<f64>,
    plan: Option<String>,
}

/// A file or folder's identity without following a link: (volume, file index, links, reparse point, directory, size).
type Identity = (u32, u64, u32, bool, bool, u64);

fn identity_of(file: &std::fs::File) -> io::Result<Identity> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle belongs to `file`, which outlives the call; `info` is a valid out pointer.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(io::Error::other)?;
    let attributes = info.dwFileAttributes;
    Ok((
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        info.nNumberOfLinks,
        attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0,
        attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
        (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
    ))
}

/// `lstat`: opens the path itself, never a link's target.
fn lstat(path: &Path) -> io::Result<Identity> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    identity_of(&file)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let resolve = |path: &Path| crate::win::cred::resolve(path).map(|text| text.to_lowercase());
    matches!((resolve(a), resolve(b)), (Ok(a), Ok(b)) if a == b)
}

static ACCOUNT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_-]{1,256}$").unwrap());
static ACCESS_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._~+/-]{1,16384}=*$").unwrap());

/// Reads only the selected account's own access token, refusing links, hard links, swapped files and oversized files.
/// The selected CLI owns this credential and all refresh and write operations.
fn read_claude_credential(account: &Account, root: &Path) -> io::Result<ClaudeCredential> {
    use std::io::Read;
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_string());
    let folder = Path::new(&account.folder);
    if !ACCOUNT_ID.is_match(&account.id) || !same_path(folder, &root.join(&account.id)) {
        return Err(invalid("Account identity mismatch"));
    }
    let anchors = [root.to_path_buf(), folder.to_path_buf()];
    let before = anchors
        .iter()
        .map(|path| lstat(path))
        .collect::<io::Result<Vec<_>>>()?;
    if before
        .iter()
        .any(|(_, _, _, link, directory, _)| *link || !directory)
    {
        return Err(invalid("Invalid account directory"));
    }
    let path = folder.join(".credentials.json");
    let stat = lstat(&path)?;
    let (_, _, links, link, directory, size) = stat;
    if directory || link || links != 1 || size > FILE_LIMIT {
        return Err(invalid("Invalid credential file"));
    }
    let file = std::fs::File::open(&path)?;
    let opened = identity_of(&file)?;
    if (opened.0, opened.1) != (stat.0, stat.1) || opened.2 != 1 || opened.5 > FILE_LIMIT {
        return Err(invalid("Credential file changed"));
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > FILE_LIMIT {
        return Err(invalid("Credential file too large"));
    }
    let after = anchors
        .iter()
        .chain([&path])
        .map(|path| lstat(path))
        .collect::<io::Result<Vec<_>>>()?;
    let moved = |(a, b): (&Identity, &Identity)| (a.0, a.1) != (b.0, b.1);
    if after.iter().any(|entry| entry.3)
        || before.iter().zip(&after).any(moved)
        || moved((&stat, &after[2]))
        || after[2].2 != 1
    {
        return Err(invalid("Credential path changed"));
    }
    let parsed: Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid credential JSON"))?;
    let oauth =
        record(parsed.get("claudeAiOauth")).ok_or_else(|| invalid("No subscription credential"))?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| ACCESS_TOKEN.is_match(token) && token.len() <= 16384)
        .ok_or_else(|| invalid("No subscription credential"))?
        .to_string();
    if ["subscriptionType", "rateLimitTier"].iter().any(|key| {
        oauth
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains(&token))
    }) {
        return Err(invalid("Invalid plan metadata"));
    }
    let subscription = label(oauth.get("subscriptionType"), 120);
    let tier = label(oauth.get("rateLimitTier"), 120)
        .and_then(|tier| tier.split('_').next_back().map(str::to_string));
    let plan = subscription.map(|subscription| {
        let mut chars = subscription.chars();
        let first: String = chars
            .next()
            .map(|first| first.to_uppercase().collect())
            .unwrap_or_default();
        let text = format!(
            "{first}{}{}",
            chars.as_str(),
            tier.map(|tier| format!(" {tier}")).unwrap_or_default()
        );
        js::slice(&text, 120).to_string()
    });
    let expires_at = oauth.get("expiresAt").and_then(Value::as_f64);
    Ok(ClaudeCredential {
        token,
        expires_at,
        plan,
    })
}

fn rate_limited() -> AccountUsageRead {
    AccountUsageRead::Error {
        error: Some("Claude usage is rate limited. Try again later.".into()),
    }
}

/// The selected Claude account's usage. Only its access token is used, for one fixed HTTPS GET.
pub async fn read_claude_account_usage(
    account: &Account,
    options: &ClaudeUsageOptions<'_>,
) -> AccountUsageRead {
    if account.provider != protocol::model::Provider::Claude {
        return AccountUsageRead::Unavailable { error: None };
    }
    let now = options.now;
    {
        let mut cooldowns = COOLDOWNS.lock().expect("cooldowns lock");
        cooldowns.retain(|(_, until)| *until > now());
        if cooldowns
            .iter()
            .any(|(folder, until)| *folder == account.folder && *until > now())
        {
            return rate_limited();
        }
    }
    let Ok(credential) = read_claude_credential(account, &options.accounts_root) else {
        return AccountUsageRead::Unavailable {
            error: Some(
                "Claude usage is unavailable for this sign-in. Check the account in Settings."
                    .into(),
            ),
        };
    };
    if credential
        .expires_at
        .is_some_and(|expires| expires <= now())
    {
        return AccountUsageRead::Unavailable {
            error: Some("Claude sign-in has expired. Sign in again to refresh usage.".into()),
        };
    }
    let headers = [
        ("Authorization", format!("Bearer {}", credential.token)),
        ("Accept", "application/json".to_string()),
        ("anthropic-beta", "oauth-2025-04-20".to_string()),
    ];
    let Ok(response) = options
        .fetch
        .get(USAGE_URL, &headers, Duration::from_secs(15))
        .await
    else {
        return failed();
    };
    if response.url != USAGE_URL {
        return failed();
    }
    if matches!(response.status, 401 | 403) {
        return AccountUsageRead::Unavailable {
            error: Some("Claude sign-in was rejected. Check the account in Settings.".into()),
        };
    }
    if response.status == 429 {
        let seconds = response
            .retry_after
            .as_deref()
            .and_then(|retry| retry.trim().parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite());
        let until = match (seconds, response.retry_after.as_deref()) {
            (Some(seconds), _) => Some(now() + seconds.max(0.0) * 1000.0),
            (None, Some(date)) => jiff::fmt::rfc2822::parse(date)
                .ok()
                .map(|date| date.timestamp().as_millisecond() as f64),
            (None, None) => None,
        };
        let until = until.map_or(now() + 60_000.0, |until| until.max(now() + 1000.0));
        let mut cooldowns = COOLDOWNS.lock().expect("cooldowns lock");
        if cooldowns.len() >= COOLDOWN_LIMIT {
            cooldowns.remove(0);
        }
        cooldowns.retain(|(folder, _)| *folder != account.folder);
        cooldowns.push((account.folder.clone(), until));
        return rate_limited();
    }
    if !(200..300).contains(&response.status) {
        return failed();
    }
    if response
        .content_length
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
        || response.body.len() > RESPONSE_LIMIT
    {
        return failed();
    }
    let Ok(raw) = serde_json::from_slice::<Value>(&response.body) else {
        return failed();
    };
    if raw.to_string().contains(&credential.token) {
        return failed();
    }
    let result = normalize_claude_usage(&raw, credential.plan);
    if format!("{result:?}").contains(&credential.token) {
        failed()
    } else {
        result
    }
}
