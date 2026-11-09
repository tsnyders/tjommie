//! `accounts.ts`

use serde::{Deserialize, Serialize};

use crate::Invalid;
use crate::model::{Provider, ProviderSetup, TimeZone};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Subscription,
    ApiKey,
}

/// One API key of at most 2560 UTF-8 bytes, without whitespace or control characters.
fn api_key(value: &str) -> Option<String> {
    (value.len() <= 2560 && !value.chars().any(|c| c <= ' ' || c == '\x7f'))
        .then(|| value.to_owned())
}

refined_string!(
    ApiKey,
    api_key,
    "Expected one API key of at most 2560 UTF-8 bytes"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitState {
    Ok,
    Near,
    Reached,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimitWindow {
    pub account_id: String,
    pub limit_id: String,
    pub window_key: String,
    pub duration_minutes: Option<f64>,
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub state: LimitState,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSpending {
    pub known_usd: f64,
    pub unknown_rows: u64,
    pub cap_usd: Option<f64>,
    pub blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignInState {
    Waiting,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSignIn {
    pub attempt: String,
    pub state: SignInState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageScopeKind {
    Model,
    Surface,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageScope {
    pub kind: UsageScopeKind,
    pub id: Option<String>,
    pub label: String,
}

/// Provider-reported allowance metadata. Separate from recorded token costs and local API spending caps; provider credits
/// are not dollars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageWindow {
    pub id: String,
    pub label: String,
    pub scope: Option<UsageScope>,
    pub duration_minutes: Option<f64>,
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageStatus {
    Available,
    Unavailable,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    /// A decimal number as text, in the provider's credit unit.
    pub balance: Option<String>,
    pub has_credits: Option<bool>,
    pub unlimited: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProviderUsage {
    pub status: UsageStatus,
    pub checked_at: Option<String>,
    pub last_successful_at: Option<String>,
    pub error: Option<String>,
    pub plan_label: Option<String>,
    pub windows: Vec<AccountUsageWindow>,
    pub credits: Option<Credits>,
    pub extra_usage_enabled: Option<bool>,
}

/// Public metadata only. Credential references, folders and secrets stay in the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    pub kind: AccountKind,
    pub is_default: bool,
    pub is_fallback: bool,
    pub spending_cap_usd: Option<f64>,
    pub created_at: String,
    pub has_credential: bool,
    pub setup: Option<ProviderSetup>,
    pub limits: Vec<AccountLimitWindow>,
    pub sign_in: Option<AccountSignIn>,
    pub spending: AccountSpending,
    /// Older snapshots have no live provider reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_usage: Option<AccountProviderUsage>,
}

/// `accounts.create` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AccountInput {
    Subscription {
        provider: Provider,
        label: String,
    },
    ApiKey {
        provider: Provider,
        label: String,
        api_key: ApiKey,
        spending_cap_usd: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_fallback: Option<bool>,
    },
}

/// The provider, kind and credential reference never change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_fallback: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spending_cap_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<ApiKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTokens {
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    pub usd: Option<f64>,
    pub known_usd: Option<f64>,
    pub known_records: u64,
    pub unknown_records: u64,
    pub reported_usd: Option<f64>,
    pub estimated_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCosts {
    pub api_spend: UsageCost,
    pub api_value: UsageCost,
    pub unclassified: UsageCost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageAggregate {
    pub records: u64,
    pub tokens: UsageTokens,
    pub costs: UsageCosts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "UsageQueryFields")]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    pub from: String,
    pub to: String,
    pub time_zone: TimeZone,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageQueryFields {
    from: String,
    to: String,
    #[serde(default = "utc")]
    time_zone: TimeZone,
    bot_id: Option<String>,
    account_id: Option<String>,
    provider: Option<Provider>,
    model: Option<String>,
}

fn utc() -> TimeZone {
    TimeZone::new("UTC").expect("UTC is a time zone")
}

impl TryFrom<UsageQueryFields> for UsageQuery {
    type Error = Invalid;

    /// A positive range of at most 366 days.
    fn try_from(fields: UsageQueryFields) -> Result<Self, Invalid> {
        let millis = |instant: &str| {
            instant
                .parse::<jiff::Timestamp>()
                .map(|instant| instant.as_millisecond())
                .ok()
        };
        let duration = millis(&fields.to)
            .zip(millis(&fields.from))
            .map(|(to, from)| to - from);
        if !duration.is_some_and(|duration| duration > 0 && duration <= 366 * 86_400_000) {
            return Err(Invalid(
                "Expected a positive range of at most 366 days".into(),
            ));
        }
        let UsageQueryFields {
            from,
            to,
            time_zone,
            bot_id,
            account_id,
            provider,
            model,
        } = fields;
        Ok(Self {
            from,
            to,
            time_zone,
            bot_id,
            account_id,
            provider,
            model,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyUsage {
    #[serde(flatten)]
    pub usage: UsageAggregate,
    pub day: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotUsage {
    #[serde(flatten)]
    pub usage: UsageAggregate,
    pub bot_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    #[serde(flatten)]
    pub usage: UsageAggregate,
    pub account_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
    #[serde(flatten)]
    pub usage: UsageAggregate,
    pub provider: Provider,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub from: String,
    pub to: String,
    pub time_zone: TimeZone,
    pub summary: UsageAggregate,
    pub daily: Vec<DailyUsage>,
    pub by_bot: Vec<BotUsage>,
    pub by_account: Vec<AccountUsage>,
    pub by_model: Vec<ModelUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountsUpdate {
    pub id: String,
    pub patch: AccountPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsCheck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountAttempt {
    pub id: String,
    pub attempt: String,
}

/// A sign-in started: `url` is for the trusted desktop opener.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInStart {
    pub url: Option<String>,
    pub attempt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSignInEnded {
    pub account_id: String,
    pub attempt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageChanged {
    pub account_id: String,
    pub bot_id: String,
}
