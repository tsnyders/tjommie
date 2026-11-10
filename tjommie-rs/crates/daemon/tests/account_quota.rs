//! Port of `packages/daemon/src/account-quota.test.ts`. The usage endpoint is a fake transport; no real account is read.

mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use daemon::account_quota::{
    AccountUsageRead, ClaudeUsageOptions, UsageFetch, UsageResponse, normalize_claude_usage,
    normalize_codex_usage, read_claude_account_usage,
};
use daemon::cli::BoxFuture;
use daemon::providers::LimitState;
use daemon::setup::{Account, parse_codex_account_limits};
use protocol::accounts::UsageScopeKind;
use protocol::model::Provider;
use serde_json::{Value, json};

fn quota() -> Value {
    json!({ "five_hour": { "utilization": 8, "resets_at": "2026-10-23T00:00:00Z" }, "seven_day": { "utilization": 5, "resets_at": null }, "extra_usage": { "is_enabled": false } })
}

fn primary() -> Value {
    json!({ "usedPercent": 31, "windowDurationMins": 10080, "resetsAt": null })
}

/// `codex(overrides)`: a `None` override removes the key, like TypeScript's `undefined`.
fn codex(overrides: &[(&str, Option<Value>)]) -> Value {
    let mut value = json!({ "ordinaryUsageAllowed": true, "rateLimits": { "primary": primary(), "planType": "pro", "spendControlReached": false,
        "credits": { "balance": "38917.83", "hasCredits": true, "unlimited": false } } });
    for (key, override_value) in overrides {
        match override_value {
            Some(override_value) => value[*key] = override_value.clone(),
            None => {
                value.as_object_mut().unwrap().remove(*key);
            }
        }
    }
    value
}

fn normalize_codex(value: &Value) -> AccountUsageRead {
    normalize_codex_usage(value, parse_codex_account_limits(value).as_deref())
}

fn with(mut value: Value, key: &str, field: Value) -> Value {
    value[key] = field;
    value
}

fn available(
    result: &AccountUsageRead,
) -> (
    &daemon::account_quota::AccountUsageData,
    &[daemon::account_quota::LimitSnapshot],
    bool,
) {
    match result {
        AccountUsageRead::Available {
            usage,
            limits,
            recovery_allowed,
        } => (usage, limits, *recovery_allowed),
        other => panic!("expected available usage, got {other:?}"),
    }
}

fn has_provider_block(limits: &[daemon::account_quota::LimitSnapshot]) -> bool {
    limits.iter().any(|snapshot| {
        snapshot.limit_id == "provider"
            && snapshot
                .windows
                .iter()
                .any(|window| window.state == LimitState::Reached)
    })
}

// describe("provider quota normalization")

#[test]
fn exposes_codex_credits_in_provider_units_plan_and_reported_duration_windows() {
    let result = normalize_codex(&codex(&[]));
    let (usage, _, recovery) = available(&result);
    assert!(recovery);
    assert_eq!(usage.plan_label.as_deref(), Some("pro"));
    let credits = usage.credits.as_ref().unwrap();
    assert_eq!(
        (
            credits.balance.as_deref(),
            credits.has_credits,
            credits.unlimited
        ),
        (Some("38917.83"), Some(true), Some(false))
    );
    let window = &usage.windows[0];
    assert_eq!(
        (
            window.label.as_str(),
            window.duration_minutes,
            window.used_percent,
            window.resets_at.as_deref()
        ),
        ("Weekly", Some(10080.0), Some(31.0), None)
    );
}

#[test]
fn does_not_infer_permission_recovery_from_healthy_percentages() {
    for permission in [Some(json!(false)), Some(Value::Null), None] {
        let result = normalize_codex(&codex(&[("ordinaryUsageAllowed", permission.clone())]));
        let (_, limits, recovery) = available(&result);
        assert!(!recovery, "{permission:?}");
        if permission == Some(json!(false)) {
            assert!(has_provider_block(limits));
        }
    }
}

#[test]
fn keeps_spend_control_denial_or_unavailable_data_separate_from_utilization() {
    for reached in [Some(json!(true)), Some(Value::Null), None] {
        let mut rate_limits = json!({ "primary": primary() });
        if let Some(reached) = &reached {
            rate_limits["spendControlReached"] = reached.clone();
        }
        let result = normalize_codex(&codex(&[("rateLimits", Some(rate_limits))]));
        let (_, limits, recovery) = available(&result);
        assert!(!recovery, "{reached:?}");
        if reached == Some(json!(true)) {
            assert!(
                limits
                    .iter()
                    .any(|snapshot| snapshot.limit_id == "provider")
            );
        }
    }
}

#[test]
fn requires_every_buckets_permission_state_and_does_not_sum_account_credits() {
    let by_id = json!({
        "codex": { "primary": primary(), "spendControlReached": false },
        "model": { "primary": with(primary(), "windowDurationMins", json!(300)), "spendControlReached": null, "normalModelSlug": "model-x", "limitName": "Model X" },
    });
    let result = normalize_codex(&codex(&[("rateLimitsByLimitId", Some(by_id))]));
    let (usage, _, recovery) = available(&result);
    assert!(!recovery);
    assert_eq!(
        usage.credits.as_ref().unwrap().balance.as_deref(),
        Some("38917.83")
    );
    let windows: Vec<(&str, &str)> = usage
        .windows
        .iter()
        .map(|window| (window.id.as_str(), window.label.as_str()))
        .collect();
    assert_eq!(
        windows,
        vec![
            ("codex:primary", "Weekly"),
            ("model:primary", "Session · Model X")
        ]
    );
    let scope = usage.windows[1].scope.as_ref().unwrap();
    assert_eq!(
        (scope.kind, scope.id.as_deref(), scope.label.as_str()),
        (UsageScopeKind::Model, Some("model-x"), "Model X")
    );
}

#[test]
fn does_not_discard_a_legacy_spend_control_denial_when_the_per_bucket_map_disagrees() {
    let value = codex(&[
        (
            "rateLimits",
            Some(json!({ "primary": primary(), "spendControlReached": true })),
        ),
        (
            "rateLimitsByLimitId",
            Some(json!({ "codex": { "primary": primary(), "spendControlReached": false } })),
        ),
    ]);
    let result = normalize_codex(&value);
    let (_, limits, recovery) = available(&result);
    assert!(!recovery);
    assert!(
        limits
            .iter()
            .any(|snapshot| snapshot.limit_id == "provider")
    );
}

#[test]
fn does_not_turn_missing_balances_into_zero_or_permit_credits_only_recovery() {
    let result = normalize_codex(&codex(&[(
        "rateLimits",
        Some(json!({ "credits": { "balance": "not-a-number", "unlimited": true } })),
    )]));
    let (usage, _, recovery) = available(&result);
    assert!(!recovery);
    assert!(usage.windows.is_empty());
    let credits = usage.credits.as_ref().unwrap();
    assert_eq!(
        (
            credits.balance.as_deref(),
            credits.has_credits,
            credits.unlimited
        ),
        (None, None, Some(true))
    );
    let over = normalize_codex(&codex(&[(
        "rateLimits",
        Some(json!({ "primary": with(primary(), "usedPercent", json!(101)) })),
    )]));
    assert!(matches!(over, AccountUsageRead::Error { .. }), "{over:?}");
}

#[test]
fn reconciles_legacy_and_modern_claude_model_windows_while_preserving_surface_identity_and_zero() {
    let value = with(
        with(quota(), "seven_day_opus", json!({ "utilization": 4 })),
        "limits",
        json!([
            { "kind": "weekly_scoped", "percent": 0, "scope": { "model": { "id": "opus-1", "display_name": "Opus" } } },
            { "kind": "weekly_scoped", "percent": 20, "scope": { "surface": { "id": "opus-1", "display_name": "Opus" } } },
        ]),
    );
    let result = normalize_claude_usage(&value, Some("Max 20x".into()));
    let (usage, limits, recovery) = available(&result);
    assert!(recovery);
    assert_eq!(
        (usage.plan_label.as_deref(), usage.extra_usage_enabled),
        (Some("Max 20x"), Some(false))
    );
    type Shown = (String, Option<UsageScopeKind>, Option<String>, Option<f64>);
    let windows: Vec<Shown> = usage
        .windows
        .iter()
        .map(|window| {
            (
                window.label.clone(),
                window.scope.as_ref().map(|scope| scope.kind),
                window.scope.as_ref().and_then(|scope| scope.id.clone()),
                window.used_percent,
            )
        })
        .collect();
    assert_eq!(
        windows,
        vec![
            ("Session".into(), None, None, Some(8.0)),
            ("Weekly".into(), None, None, Some(5.0)),
            (
                "Weekly · Opus".into(),
                Some(UsageScopeKind::Model),
                Some("opus-1".into()),
                Some(0.0)
            ),
            (
                "Weekly · Opus".into(),
                Some(UsageScopeKind::Surface),
                Some("opus-1".into()),
                Some(20.0)
            ),
        ]
    );
    assert_eq!(limits.len(), 1);
    assert_eq!(limits[0].limit_id, "claude");
    let keys: Vec<(&str, LimitState)> = limits[0]
        .windows
        .iter()
        .map(|window| (window.window_key.as_str(), window.state))
        .collect();
    assert_eq!(
        keys,
        vec![("five_hour", LimitState::Ok), ("seven_day", LimitState::Ok)]
    );
}

#[test]
fn keeps_exhausted_model_or_surface_windows_display_only_but_prevents_clearing_an_existing_generic_block()
 {
    let value = with(
        quota(),
        "limits",
        json!([{ "kind": "weekly_scoped", "percent": 100, "scope": { "surface": { "display_name": "Code" } } }]),
    );
    let result = normalize_claude_usage(&value, None);
    let (_, limits, recovery) = available(&result);
    assert!(!recovery);
    assert!(
        limits[0]
            .windows
            .iter()
            .all(|window| window.state == LimitState::Ok)
    );
    let legacy = with(
        with(quota(), "seven_day_opus", json!({ "utilization": 100 })),
        "limits",
        json!([{ "kind": "weekly_scoped", "percent": 0, "scope": { "model": { "display_name": "Opus" } } }]),
    );
    assert!(!available(&normalize_claude_usage(&legacy, None)).2);
}

#[test]
fn incomplete_claude_responses_cannot_authorize_recovery() {
    for (key, extra) in [
        ("seven_day_opus", json!({})),
        ("limits", json!([{ "kind": "future_shape" }])),
        ("seven_day", Value::Null),
    ] {
        let result = normalize_claude_usage(&with(quota(), key, extra.clone()), None);
        assert!(!available(&result).2, "{key}: {extra}");
    }
}

#[test]
fn accepts_numeric_provider_percentages_without_coercing_missing_values_or_booleans_to_zero() {
    let result = normalize_claude_usage(
        &with(quota(), "five_hour", json!({ "utilization": "8.5" })),
        None,
    );
    let used: Vec<Option<f64>> = available(&result)
        .0
        .windows
        .iter()
        .map(|window| window.used_percent)
        .collect();
    assert_eq!(used, vec![Some(8.5), Some(5.0)]);
    for utilization in [Value::Null, json!(false), json!(""), json!(" ")] {
        let result = normalize_claude_usage(
            &with(quota(), "five_hour", json!({ "utilization": utilization })),
            None,
        );
        let (usage, _, recovery) = available(&result);
        assert!(!recovery);
        assert_eq!(usage.windows[0].id, "seven_day");
    }
}

#[test]
fn bounds_displayed_windows_and_uses_distinct_ids_without_truncation_authorizing_recovery() {
    let limits: Vec<Value> = (0..129)
        .map(|i| json!({ "kind": "weekly_scoped", "percent": if i == 128 { 100 } else { 0 }, "scope": { "model": { "id": i.to_string(), "display_name": "Model" } } }))
        .collect();
    let result = normalize_claude_usage(&with(quota(), "limits", json!(limits)), None);
    let (usage, _, recovery) = available(&result);
    assert_eq!(usage.windows.len(), 64);
    assert!(!recovery);
    let ids: std::collections::HashSet<&str> = usage
        .windows
        .iter()
        .map(|window| window.id.as_str())
        .collect();
    assert_eq!(ids.len(), 64);
}

// describe("selected Claude account usage read")

const TOKEN: &str = "synthetic-access-token-never-real";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

type Call = (String, Vec<(String, String)>, Duration);

#[derive(Default)]
struct FakeFetch {
    calls: Mutex<Vec<Call>>,
    responses: Mutex<VecDeque<Result<UsageResponse, String>>>,
}

fn ok(body: &Value) -> UsageResponse {
    UsageResponse {
        status: 200,
        url: USAGE_URL.into(),
        retry_after: None,
        content_length: None,
        body: body.to_string().into_bytes(),
    }
}

impl FakeFetch {
    fn then(&self, response: Result<UsageResponse, String>) {
        self.responses.lock().unwrap().push_back(response);
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl UsageFetch for FakeFetch {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'static str, String)],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<UsageResponse, String>> {
        let headers = headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect();
        self.calls
            .lock()
            .unwrap()
            .push((url.into(), headers, timeout));
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(ok(&quota())));
        Box::pin(async move { response })
    }
}

struct Fixture {
    root: tempfile::TempDir,
    account: Account,
    credential: PathBuf,
    fetch: FakeFetch,
}

fn fixture(oauth: &[(&str, Option<Value>)]) -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap();
    let folder = root.path().join("selected");
    std::fs::create_dir(&folder).unwrap();
    let account = Account {
        id: "selected".into(),
        provider: Provider::Claude,
        label: "Synthetic".into(),
        folder: folder.to_string_lossy().into_owned(),
    };
    let credential = folder.join(".credentials.json");
    let mut credentials = json!({ "accessToken": TOKEN, "refreshToken": "synthetic-refresh-never-read", "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x" });
    for (key, value) in oauth {
        match value {
            Some(value) => credentials[*key] = value.clone(),
            None => {
                credentials.as_object_mut().unwrap().remove(*key);
            }
        }
    }
    std::fs::write(
        &credential,
        json!({ "claudeAiOauth": credentials }).to_string(),
    )
    .unwrap();
    Fixture {
        root,
        account,
        credential,
        fetch: FakeFetch::default(),
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

impl Fixture {
    async fn read(&self, account: &Account) -> AccountUsageRead {
        self.read_at(account, self.root.path(), &now).await
    }

    async fn read_at(
        &self,
        account: &Account,
        root: &Path,
        at: &(dyn Fn() -> f64 + Send + Sync),
    ) -> AccountUsageRead {
        read_claude_account_usage(
            account,
            &ClaudeUsageOptions {
                accounts_root: root.to_path_buf(),
                fetch: &self.fetch,
                now: at,
            },
        )
        .await
    }
}

fn is_unavailable(result: &AccountUsageRead) -> bool {
    matches!(result, AccountUsageRead::Unavailable { .. })
}

#[tokio::test]
async fn uses_only_the_fixed_https_get_with_the_selected_access_token_and_never_writes_or_returns_credentials()
 {
    let h = fixture(&[]);
    let before = std::fs::read_to_string(&h.credential).unwrap();
    let result = h.read(&h.account).await;
    let (usage, _, _) = available(&result);
    assert_eq!(
        (usage.plan_label.as_deref(), usage.extra_usage_enabled),
        (Some("Max 20x"), Some(false))
    );
    let calls = h.fetch.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![(
            USAGE_URL.to_string(),
            vec![
                ("Authorization".to_string(), format!("Bearer {TOKEN}")),
                ("Accept".to_string(), "application/json".to_string()),
                ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
            ],
            Duration::from_secs(15)
        )]
    );
    let shown = format!("{result:?}");
    assert!(!shown.contains(TOKEN) && !shown.contains("refresh"));
    assert_eq!(std::fs::read_to_string(&h.credential).unwrap(), before);
}

#[tokio::test]
async fn does_not_fetch_with_an_expired_missing_or_unsafe_credential() {
    for oauth in [
        ("expiresAt", Some(json!(1))),
        ("accessToken", Some(json!(""))),
        ("accessToken", Some(json!("bad\r\nheader"))),
        ("accessToken", None),
    ] {
        let h = fixture(std::slice::from_ref(&oauth));
        assert!(is_unavailable(&h.read(&h.account).await), "{oauth:?}");
        assert_eq!(h.fetch.calls(), 0);
    }
}

#[tokio::test]
async fn rejects_wrong_identity_oversized_credential_files_and_credential_directory_links_before_transport()
 {
    let h = fixture(&[]);
    assert!(is_unavailable(
        &h.read(&Account {
            id: "wrong".into(),
            ..h.account.clone()
        })
        .await
    ));
    assert!(is_unavailable(
        &h.read(&Account {
            id: "../selected".into(),
            ..h.account.clone()
        })
        .await
    ));
    std::fs::write(&h.credential, "x".repeat(64 * 1024 + 1)).unwrap();
    assert!(is_unavailable(&h.read(&h.account).await));

    let other = fixture(&[]);
    let links = tempfile::tempdir().unwrap();
    let linked_root = links.path().join("linked");
    common::junction(other.root.path(), &linked_root);
    let through_root = Account {
        folder: linked_root.join("selected").to_string_lossy().into_owned(),
        ..other.account.clone()
    };
    assert!(is_unavailable(
        &h.read_at(&through_root, &linked_root, &now).await
    ));
    let root = tempfile::tempdir().unwrap();
    let linked_folder = root.path().join("selected");
    common::junction(Path::new(&other.account.folder), &linked_folder);
    let through_folder = Account {
        folder: linked_folder.to_string_lossy().into_owned(),
        ..other.account.clone()
    };
    assert!(is_unavailable(
        &h.read_at(&through_folder, root.path(), &now).await
    ));
    assert_eq!(h.fetch.calls(), 0);
}

#[tokio::test]
async fn rejects_a_non_file_credential_target_without_reading_its_contents() {
    let h = fixture(&[]);
    std::fs::remove_file(&h.credential).unwrap();
    std::fs::create_dir(&h.credential).unwrap();
    assert!(is_unavailable(&h.read(&h.account).await));
    assert_eq!(h.fetch.calls(), 0);
}

#[tokio::test]
async fn rejects_hard_linked_credentials_so_the_selected_identity_cannot_alias_another_account_file()
 {
    let h = fixture(&[]);
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::hard_link(&h.credential, elsewhere.path().join("alias.json")).unwrap();
    assert!(is_unavailable(&h.read(&h.account).await));
    assert_eq!(h.fetch.calls(), 0);
}

#[tokio::test]
async fn binds_an_abort_deadline_and_redacts_a_transports_abort_diagnostics() {
    let h = fixture(&[]);
    h.fetch.then(Err(format!("private {TOKEN}")));
    let result = h.read(&h.account).await;
    assert_eq!(h.fetch.calls.lock().unwrap()[0].2, Duration::from_secs(15));
    assert_eq!(
        result,
        AccountUsageRead::Error {
            error: Some("Provider usage could not be read. Try again later.".into())
        }
    );
}

#[tokio::test]
async fn returns_only_generic_errors_on_http_failures() {
    for status in [401, 403, 500, 302] {
        let h = fixture(&[]);
        h.fetch.then(Ok(UsageResponse {
            status,
            body: TOKEN.as_bytes().to_vec(),
            ..ok(&quota())
        }));
        let result = h.read(&h.account).await;
        assert!(
            !matches!(result, AccountUsageRead::Available { .. }),
            "{status}"
        );
        assert!(!format!("{result:?}").contains(TOKEN));
    }
}

#[tokio::test]
async fn honors_a_429_retry_delay_without_another_credential_bearing_request() {
    let h = fixture(&[]);
    let start = now();
    h.fetch.then(Ok(UsageResponse {
        status: 429,
        retry_after: Some("30".into()),
        body: b"private body".to_vec(),
        ..ok(&quota())
    }));
    let early = move || start;
    h.read_at(&h.account, h.root.path(), &early).await;
    h.read_at(&h.account, h.root.path(), &early).await;
    assert_eq!(h.fetch.calls(), 1);
    let later = move || start + 30_001.0;
    assert!(matches!(
        h.read_at(&h.account, h.root.path(), &later).await,
        AccountUsageRead::Available { .. }
    ));
    assert_eq!(h.fetch.calls(), 2);
}

#[tokio::test]
async fn rejects_redirects_oversized_or_invalid_bodies_and_echoed_credentials_without_exposing_details()
 {
    let h = fixture(&[]);
    let echoed = with(
        quota(),
        "limits",
        json!([{ "kind": "weekly_scoped", "percent": 0, "scope": { "model": { "display_name": TOKEN } } }]),
    );
    h.fetch.then(Ok(UsageResponse {
        url: "https://example.invalid/stolen".into(),
        ..ok(&quota())
    }));
    h.fetch.then(Ok(UsageResponse {
        body: vec![b'x'; 128 * 1024 + 1],
        ..ok(&quota())
    }));
    h.fetch.then(Ok(UsageResponse {
        body: b"not JSON".to_vec(),
        ..ok(&quota())
    }));
    h.fetch.then(Ok(ok(&echoed)));
    h.fetch.then(Err(format!("private {TOKEN}")));
    for _ in 0..5 {
        let result = h.read(&h.account).await;
        assert!(
            matches!(result, AccountUsageRead::Error { .. }),
            "{result:?}"
        );
        assert!(!format!("{result:?}").contains(TOKEN));
    }
}
