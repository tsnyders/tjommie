//! Port of `packages/ui/lib/accounts.ts` (T1.4): the accounts screen's rules, from what an account says about its sign-in,
//! its limits and its spending to the one line each card shows. Not ported: `providerName`, which is `setup::provider_name`.

use std::collections::HashMap;

use jiff::{Timestamp, Zoned};
use protocol::accounts::{
    AccountInfo, AccountKind, AccountLimitWindow, AccountProviderUsage, AccountSpending,
    AccountUsageWindow, Credits, LimitState, SignInState, UsageStatus,
};
use protocol::model::{Bot, BotState, Provider, SETUP_CODEX_SANDBOX_REASON, SETUP_SIGN_IN_PREFIX};

use super::format::{clock, day_name, time_ago};
use super::setup::{plan_label, provider_name, report_of};

const HOUR_MS: i64 = 3_600_000;
/// A spending meter turns to the accent at this share of its cap.
const NEAR: f64 = 0.8;
/// The one reason the runtime gives for a subscription that is signed out.
const CHECK_REASON: &str = "Check or sign in to this account in Accounts.";
pub const RETRY_CAPTION: &str =
    "Retries one queued turn; it may use subscription allowance or API credit.";

/// What a limit means for an account right now (`LimitState`): blocked until a time, or blocked with no reset time known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitBlock {
    Until { at: String },
    Unknown,
}

/// Spent against the cap (`spendingMeter`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpendingMeter {
    pub share: f64,
    pub near: bool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusAction {
    OpenAgain,
    Cancel,
    TryAgain,
    Check,
    SignIn,
    AddKey,
    RaiseCap,
    RetryLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Busy,
    Ok,
    Limited,
    Muted,
    Problem,
}

/// The one line an account card shows (`StatusLine`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub tone: Tone,
    pub text: String,
    pub actions: Vec<StatusAction>,
}

/// The control a link points at (`focusReady`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    SignIn,
    Cap,
    Retry,
}

/// What `statusLine` is told about the card: whether a check has gone on too long, and whether the sign-in page's address
/// was kept, so it can be opened again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusFlags {
    pub stuck: bool,
    pub can_reopen: bool,
}

/// One row of a usage card (`UsageRow`).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub key: String,
    pub label: String,
    pub share: Option<f64>,
    pub value: String,
    pub near: bool,
}

/// A line of Extra usage or Credits, only what the provider said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra {
    pub label: String,
    pub value: String,
}

/// What a subscription's usage card shows (`ProviderView`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderView {
    pub plan: Option<String>,
    pub rows: Vec<UsageRow>,
    /// Set when the windows shown are from an earlier successful check: when.
    pub stale: Option<String>,
    /// Why the latest check failed, if it did.
    pub problem: Option<String>,
    pub extras: Vec<Extra>,
}

/// What a subscription's usage card says besides its title (`QuotaCard`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaCard {
    Limits,
    Checking,
    Problem {
        text: String,
    },
    SignedOut,
    /// Signed in, but no limit windows have been reported (yet).
    Unavailable,
}

/// The banner for a bot (`Banner`). It only offers to switch accounts: none happens by itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Banner<'a> {
    Setup {
        text: String,
        sandbox: bool,
        sign_in: bool,
    },
    SignIn {
        account: &'a AccountInfo,
    },
    AccountCheck {
        account: &'a AccountInfo,
        text: String,
    },
    Cap {
        account: &'a AccountInfo,
        unknown_prices: u64,
    },
    /// `until` is `None` when the reset is unknown.
    Limited {
        account: &'a AccountInfo,
        window: String,
        until: Option<String>,
        alternatives: Vec<&'a AccountInfo>,
    },
    Fallback {
        pinned: &'a AccountInfo,
        using: &'a AccountInfo,
        until: Option<String>,
    },
}

/// A string that JavaScript's truthiness takes as present: an empty one counts as missing.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// Milliseconds since the epoch of an ISO 8601 time (`Date.parse`), or `None` when it doesn't parse (`NaN`).
fn millis(value: &str) -> Option<i64> {
    value
        .parse::<Timestamp>()
        .ok()
        .map(|instant| instant.as_millisecond())
}

fn now_ms(now: &Zoned) -> i64 {
    now.timestamp().as_millisecond()
}

/// The cents of a non-negative decimal written `digits[.digits]`, rounded half up at the second place. Past u128 the cents
/// saturate at `u128::MAX`, rather than overflowing or reading as 0.
fn cents_of(decimal: &str) -> u128 {
    let (whole, fraction) = decimal.split_once('.').unwrap_or((decimal, ""));
    let digit = |index: usize| {
        fraction
            .chars()
            .nth(index)
            .and_then(|c| c.to_digit(10))
            .map_or(0, u128::from)
    };
    // The whole part is digits, so it fails to parse only by being past u128.
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u128>().unwrap_or(u128::MAX)
    };
    let cents = whole
        .saturating_mul(100)
        .saturating_add(digit(0) * 10 + digit(1));
    cents.saturating_add(u128::from(digit(2) >= 5))
}

/// `$${usd.toFixed(2)}`. `toFixed` rounds the exact value and a tie goes up, where Rust's `{:.2}` would round it to even,
/// so the digits come from a long exact expansion and are rounded here.
pub fn money(usd: f64) -> String {
    let sign = if usd < 0.0 { "-" } else { "" };
    let cents = cents_of(&format!("{:.30}", usd.abs()));
    format!("${sign}{}.{:02}", cents / 100, cents % 100)
}

/// `1234567` as "1,234,567".
fn group_thousands(whole: u128) -> String {
    let digits = whole.to_string();
    let count = digits.len();
    digits
        .chars()
        .enumerate()
        .fold(String::new(), |mut grouped, (index, digit)| {
            if index > 0 && (count - index).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(digit);
            grouped
        })
}

/// `Number(text).toLocaleString("en-US", { maximumFractionDigits: 2 })`: the number's shortest decimal form, rounded half
/// away from zero to two places, with thousands grouped. An empty text is 0, as `Number("")` is.
fn en_us_number(text: &str) -> String {
    let trimmed = text.trim();
    let value = if trimmed.is_empty() {
        0.0
    } else {
        trimmed.parse::<f64>().unwrap_or(f64::NAN)
    };
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-∞" } else { "∞" }.to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let cents = cents_of(&format!("{}", value.abs()));
    let fraction = match cents % 100 {
        0 => String::new(),
        rest if rest % 10 == 0 => format!(".{}", rest / 10),
        rest => format!(".{rest:02}"),
    };
    format!("{sign}{}{fraction}", group_thousands(cents / 100))
}

/// "9:14 AM" today, "Fri 9:14 AM" on another day.
/// The display zone is `now`'s: pass the system zone (`Zoned::now()`), since a UTC `now` puts "Today" on the wrong day east of Greenwich.
pub fn when(value: &str, now: &Zoned) -> String {
    let zone = now.time_zone();
    if day_name(value, now, false) == "Today" {
        clock(value, zone)
    } else {
        format!("{} {}", day_name(value, now, true), clock(value, zone))
    }
}

/// The provider's accounts: the default first, then the oldest (`accountsOf`).
pub fn accounts_of(accounts: &[AccountInfo], provider: Provider) -> Vec<&AccountInfo> {
    let mut listed: Vec<&AccountInfo> = accounts
        .iter()
        .filter(|account| account.provider == provider)
        .collect();
    // ISO 8601 times compare as text, which is the order `localeCompare` gives them.
    listed.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    listed
}

pub fn default_account(accounts: &[AccountInfo], provider: Provider) -> Option<&AccountInfo> {
    accounts
        .iter()
        .find(|account| account.provider == provider && account.is_default)
}

/// The account a bot runs on: the one it is using now, else its pin, else its provider's default (`accountFor`).
pub fn account_for<'a>(bot: &Bot, accounts: &'a [AccountInfo]) -> Option<&'a AccountInfo> {
    let id = bot
        .effective_account_id
        .clone()
        .flatten()
        .or_else(|| bot.account_id.clone().flatten());
    non_empty(id.as_deref())
        .and_then(|id| accounts.iter().find(|account| account.id == id))
        .or_else(|| default_account(accounts, bot.provider))
}

/// The bots pinned to an account. While there are any, it can't be removed (`botsOn`).
pub fn bots_on<'a>(account_id: &str, bots: &'a [Bot]) -> Vec<&'a Bot> {
    bots.iter()
        .filter(|bot| bot.account_id.as_ref().and_then(|pin| pin.as_deref()) == Some(account_id))
        .collect()
}

pub fn used_by(account_id: &str, bots: &[Bot]) -> usize {
    bots_on(account_id, bots).len()
}

/// "5-hour limit", "Weekly limit", "1.5-hour limit"; with no length, the window's own name (`limitLabel`).
pub fn limit_label(window: &AccountLimitWindow) -> String {
    let Some(minutes) = window.duration_minutes else {
        return window.window_key.clone();
    };
    if minutes == 10_080.0 {
        "Weekly limit".to_owned()
    } else if minutes % 1440.0 == 0.0 {
        format!("{}-day limit", minutes / 1440.0)
    } else if minutes % 60.0 == 0.0 {
        format!("{}-hour limit", minutes / 60.0)
    } else {
        // Math.round(minutes / 6) / 10, which JavaScript then prints as a number.
        format!("{}-hour limit", (minutes / 6.0 + 0.5).floor() / 10.0)
    }
}

/// "42% · resets 3:10 PM", with how old the reading is once it is over an hour old (`limitValue`).
pub fn limit_value(window: &AccountLimitWindow, now: &Zoned) -> String {
    let used = match window.used_percent {
        None => "Usage unknown".to_owned(),
        Some(percent) => format!("{}%", (percent + 0.5).floor()),
    };
    let resets = non_empty(window.resets_at.as_deref())
        .map(|at| format!(" · resets {}", when(at, now)))
        .unwrap_or_default();
    let stale = millis(&window.observed_at)
        .filter(|observed| now_ms(now) - observed > HOUR_MS)
        .map(|_| format!(" · updated {}", time_ago(&window.observed_at, now)))
        .unwrap_or_default();
    format!("{used}{resets}{stale}")
}

pub fn limit_near(window: &AccountLimitWindow) -> bool {
    matches!(window.state, LimitState::Near | LimitState::Reached)
}

/// When the account is limited until: the latest reset among its reached windows that is still ahead, or `None`.
pub fn limited_until(account: &AccountInfo, now: &Zoned) -> Option<String> {
    let now_millis = now_ms(now);
    account
        .limits
        .iter()
        .filter(|window| window.state == LimitState::Reached)
        .filter_map(|window| {
            let at = window.resets_at.as_deref()?;
            let instant = millis(at)?;
            (instant > now_millis).then_some((instant, at))
        })
        .reduce(|latest, next| if next.0 > latest.0 { next } else { latest })
        .map(|(_, at)| at.to_owned())
}

/// What a limit means for an account right now (`limitState`). A window that has reached its limit and says nothing of
/// when it resets still blocks, as the runtime treats it.
pub fn limit_state(account: &AccountInfo, now: &Zoned) -> Option<LimitBlock> {
    if account
        .limits
        .iter()
        .any(|window| window.state == LimitState::Reached && window.resets_at.is_none())
    {
        return Some(LimitBlock::Unknown);
    }
    limited_until(account, now).map(|at| LimitBlock::Until { at })
}

pub fn limited_text(block: &LimitBlock, now: &Zoned) -> String {
    match block {
        LimitBlock::Until { at } => format!("Limited until {}", when(at, now)),
        LimitBlock::Unknown => "Limited. Reset time unknown.".to_owned(),
    }
}

/// The window with the highest use, if any use is known (`topLimit`). A tie keeps the first.
pub fn top_limit(accounts: &[AccountInfo]) -> Option<&AccountLimitWindow> {
    accounts
        .iter()
        .flat_map(|account| account.limits.iter())
        .filter(|window| window.used_percent.is_some())
        .reduce(|top, next| {
            if next.used_percent.unwrap_or(0.0) > top.used_percent.unwrap_or(0.0) {
                next
            } else {
                top
            }
        })
}

/// Spent against the cap: a share from 0 to 1, whether it is near the cap, and the line saying so (`spendingMeter`).
pub fn spending_meter(spending: &AccountSpending) -> SpendingMeter {
    let unknown = match spending.unknown_rows {
        0 => String::new(),
        1 => " + 1 turn with no price".to_owned(),
        rows => format!(" + {rows} turns with no price"),
    };
    let share = spending
        .cap_usd
        .filter(|cap| *cap > 0.0)
        .map_or(0.0, |cap| (spending.known_usd / cap).min(1.0));
    let text = match spending.cap_usd {
        None => format!("Spent {} in total{unknown}", money(spending.known_usd)),
        Some(cap) => format!(
            "Spent {} of {} cap in total{unknown}",
            money(spending.known_usd),
            money(cap)
        ),
    };
    SpendingMeter {
        share,
        near: share >= NEAR || spending.blocked,
        text,
    }
}

/// A limit with no known reset can be tried once, by hand, to see whether it has lifted (`canRetryLimit`). Only while
/// nothing known and in the future also blocks it, and never past a cap.
pub fn can_retry_limit(account: &AccountInfo, now: &Zoned) -> bool {
    matches!(limit_state(account, now), Some(LimitBlock::Unknown))
        && limited_until(account, now).is_none()
        && !account.spending.blocked
}

/// What a limited account offers: Retry queued work when that is allowed, and for a Codex subscription, Check again,
/// which asks Codex's own quota check. The two are different: one lets a queued turn run, the other only looks.
fn limit_actions(account: &AccountInfo, now: &Zoned) -> Vec<StatusAction> {
    let mut actions = Vec::new();
    if can_retry_limit(account, now) {
        actions.push(StatusAction::RetryLimit);
    }
    if account.provider == Provider::Codex
        && account.kind == AccountKind::Subscription
        && matches!(limit_state(account, now), Some(LimitBlock::Unknown))
    {
        actions.push(StatusAction::Check);
    }
    actions
}

/// Whether the control a link pointed at is on the card right now (`focusReady`). A card that is still checking has no
/// Sign in or Retry yet; the focus waits for it.
pub fn focus_ready(focus: Focus, account: &AccountInfo, actions: &[StatusAction]) -> bool {
    match focus {
        Focus::SignIn => {
            actions.contains(&StatusAction::SignIn) || actions.contains(&StatusAction::TryAgain)
        }
        Focus::Retry => actions.contains(&StatusAction::RetryLimit),
        // Edit cap is always on an API key's card.
        Focus::Cap => account.kind == AccountKind::ApiKey,
    }
}

fn line(tone: Tone, text: impl Into<String>, actions: Vec<StatusAction>) -> StatusLine {
    StatusLine {
        tone,
        text: text.into(),
        actions,
    }
}

/// The one line an account card shows, by the first rule that fits (`statusLine`).
pub fn status_line(account: &AccountInfo, now: &Zoned, flags: StatusFlags) -> StatusLine {
    if let Some(sign_in) = &account.sign_in {
        match sign_in.state {
            SignInState::Waiting => {
                let actions = if flags.can_reopen {
                    vec![StatusAction::OpenAgain, StatusAction::Cancel]
                } else {
                    vec![StatusAction::Cancel]
                };
                return line(Tone::Busy, "Finish signing in in your browser", actions);
            }
            SignInState::Failed => {
                let text = sign_in
                    .message
                    .clone()
                    .unwrap_or_else(|| "Sign-in didn't finish.".to_owned());
                return line(Tone::Problem, text, vec![StatusAction::TryAgain]);
            }
        }
    }
    if account.kind == AccountKind::Subscription {
        let Some(setup) = &account.setup else {
            return if flags.stuck {
                line(Tone::Muted, "Couldn't check yet", vec![StatusAction::Check])
            } else {
                line(Tone::Busy, "Checking", vec![])
            };
        };
        let report = report_of(setup);
        if let Some(error) = non_empty(report.error.as_deref()) {
            return line(Tone::Problem, error, vec![StatusAction::Check]);
        }
        if !report.signed_in {
            return line(
                Tone::Muted,
                "Not signed in. Agents on it wait.",
                vec![StatusAction::SignIn],
            );
        }
        if let Some(block) = limit_state(account, now) {
            return line(
                Tone::Limited,
                limited_text(&block, now),
                limit_actions(account, now),
            );
        }
        return line(Tone::Ok, "Signed in", vec![]);
    }
    if !account.has_credential {
        return line(Tone::Muted, "No key saved", vec![StatusAction::AddKey]);
    }
    if account.spending.blocked {
        let unknown = account.spending.unknown_rows;
        if unknown > 0 {
            let verb = if unknown == 1 {
                "turn has"
            } else {
                "turns have"
            };
            return line(
                Tone::Problem,
                format!("Stopped: {unknown} {verb} no price, so the cap can't be checked."),
                vec![],
            );
        }
        let cap = account.spending.cap_usd.unwrap_or(0.0);
        return line(
            Tone::Problem,
            format!("Reached its {} cap. Agents stopped using it.", money(cap)),
            vec![StatusAction::RaiseCap],
        );
    }
    if let Some(block) = limit_state(account, now) {
        return line(
            Tone::Limited,
            limited_text(&block, now),
            limit_actions(account, now),
        );
    }
    line(Tone::Ok, "Key saved", vec![])
}

/// What the picker shows on an account's row: limited, not signed in, or what kind it is (`pickerState`).
pub fn picker_state(account: &AccountInfo, now: &Zoned) -> String {
    match limit_state(account, now) {
        Some(LimitBlock::Until { at }) => format!("Limited until {}", when(&at, now)),
        Some(LimitBlock::Unknown) => "Limited".to_owned(),
        None if account.kind == AccountKind::Subscription
            && account
                .setup
                .as_ref()
                .is_some_and(|setup| !report_of(setup).signed_in) =>
        {
            "Not signed in".to_owned()
        }
        None if account.is_default => "Default".to_owned(),
        None if account.kind == AccountKind::ApiKey => "API key".to_owned(),
        None => String::new(),
    }
}

/// The account a provider's tab stands for: the one the bot has if it is on this provider, else the provider's default.
pub fn chosen_account<'a>(
    accounts: &'a [AccountInfo],
    provider: Provider,
    current: Option<&str>,
) -> Option<&'a AccountInfo> {
    non_empty(current)
        .and_then(|id| {
            accounts
                .iter()
                .find(|account| account.id == id && account.provider == provider)
        })
        .or_else(|| default_account(accounts, provider))
}

/// A sentence under the Account and model row when the account in use can't take work, or when changing it has a cost
/// (`accountHelp`).
pub fn account_help(account: Option<&AccountInfo>, now: &Zoned, changed: bool) -> Option<String> {
    if let Some(account) = account {
        if account.kind == AccountKind::Subscription
            && account
                .setup
                .as_ref()
                .is_some_and(|setup| !report_of(setup).signed_in)
        {
            return Some(format!("{} isn't signed in.", account.label));
        }
        if let Some(block) = limit_state(account, now) {
            return Some(match block {
                LimitBlock::Until { at } => {
                    format!("{} is limited until {}.", account.label, when(&at, now))
                }
                LimitBlock::Unknown => {
                    format!(
                        "{} is limited, and its reset time isn't known.",
                        account.label
                    )
                }
            });
        }
        if account.spending.blocked {
            return Some(format!("{} reached its cap.", account.label));
        }
    }
    changed.then(|| "Switching accounts starts a fresh session. The chat stays.".to_owned())
}

/// "Subscription · Max plan · name". Only a missing plan or account is left out (`subscriptionLabel`).
pub fn subscription_label(account: &AccountInfo) -> String {
    let report = account.setup.as_ref().map(report_of);
    let mut parts = vec!["Subscription".to_owned()];
    parts.extend(plan_label(report.and_then(|report| report.plan.as_deref())));
    parts.extend(report.and_then(|report| report.account.clone()));
    parts.join(" · ")
}

/// "Weekly · Fable": a window's name, with the model or surface it is for (`windowName`).
pub fn window_name(window: &AccountUsageWindow) -> String {
    match &window.scope {
        Some(scope) => format!("{} · {}", window.label, scope.label),
        None => window.label.clone(),
    }
}

/// "42% · resets 3:10 PM". An unknown use is "Usage unknown", never 0%. A reset already past (an old reading) is left out
/// rather than shown as ahead.
fn window_value(window: &AccountUsageWindow, now: &Zoned) -> String {
    let used = match window.used_percent {
        None => "Usage unknown".to_owned(),
        Some(percent) => format!("{}%", (percent + 0.5).floor()),
    };
    let resets = match non_empty(window.resets_at.as_deref()) {
        Some(at) if millis(at).is_some_and(|instant| instant > now_ms(now)) => {
            format!(" · resets {}", when(at, now))
        }
        _ => String::new(),
    };
    format!("{used}{resets}")
}

/// Credits are the provider's own units, not dollars: "Unlimited" only when the provider said so, "Unknown" with no
/// balance, otherwise the balance as a number.
pub fn credits_text(credits: &Credits) -> String {
    if credits.unlimited == Some(true) {
        return "Unlimited".to_owned();
    }
    match &credits.balance {
        None => "Unknown".to_owned(),
        Some(balance) => format!("{} left", en_us_number(balance)),
    }
}

fn extra_usage(enabled: bool) -> Extra {
    Extra {
        label: "Extra usage".to_owned(),
        value: if enabled { "Enabled" } else { "Disabled" }.to_owned(),
    }
}

fn problem_of(live: &AccountProviderUsage) -> Option<String> {
    (live.status == UsageStatus::Error).then(|| {
        live.error
            .clone()
            .unwrap_or_else(|| "Couldn't check the limits.".to_owned())
    })
}

/// "Last updated 3 hours ago", or that the time of the last update is unknown.
fn stale_text(last_successful: Option<&str>, now: &Zoned) -> String {
    match non_empty(last_successful) {
        Some(at) => format!("Last updated {}", time_ago(at, now)),
        None => "Last update time unknown".to_owned(),
    }
}

/// What a subscription's usage card shows, from the provider's own live reading when there is one, else from the windows
/// it last observed. A failed or unavailable check after a good one keeps the old windows, marked with when they were last
/// updated: never shown as zero or as fresh (`providerView`).
pub fn provider_view(account: &AccountInfo, now: &Zoned) -> ProviderView {
    let live = account.provider_usage.as_ref();
    let plan = live.and_then(|live| live.plan_label.clone()).or_else(|| {
        account
            .setup
            .as_ref()
            .and_then(|setup| report_of(setup).plan.clone())
    });
    if let Some(live) = live.filter(|live| !live.windows.is_empty()) {
        let rows = live
            .windows
            .iter()
            .map(|window| UsageRow {
                key: window.id.clone(),
                label: window_name(window),
                share: window.used_percent.map(|percent| percent / 100.0),
                value: window_value(window, now),
                near: window.used_percent.is_some_and(|percent| percent >= 80.0),
            })
            .collect();
        let stale = (live.status != UsageStatus::Available)
            .then(|| stale_text(live.last_successful_at.as_deref(), now));
        let mut extras = Vec::new();
        if let Some(enabled) = live.extra_usage_enabled {
            extras.push(extra_usage(enabled));
        }
        if let Some(credits) = &live.credits {
            extras.push(Extra {
                label: "Credits".to_owned(),
                value: credits_text(credits),
            });
        }
        return ProviderView {
            plan,
            rows,
            stale,
            problem: problem_of(live),
            extras,
        };
    }
    let rows = account
        .limits
        .iter()
        .map(|window| UsageRow {
            key: format!("{}:{}", window.limit_id, window.window_key),
            label: limit_label(window),
            share: window.used_percent.map(|percent| percent / 100.0),
            value: limit_value(window, now),
            near: limit_near(window),
        })
        .collect();
    let mut extras = Vec::new();
    if let Some(enabled) = live.and_then(|live| live.extra_usage_enabled) {
        extras.push(extra_usage(enabled));
    }
    if let Some(credits) = live.and_then(|live| live.credits.as_ref()) {
        extras.push(Extra {
            label: "Credits".to_owned(),
            value: credits_text(credits),
        });
    }
    // Extras kept from an earlier success after a failed or unavailable check are stale too.
    let stale = match live {
        Some(live) if live.status != UsageStatus::Available && !extras.is_empty() => {
            Some(stale_text(live.last_successful_at.as_deref(), now))
        }
        _ => None,
    };
    ProviderView {
        plan,
        rows,
        stale,
        problem: live.and_then(problem_of),
        extras,
    }
}

/// What a subscription's usage card says besides its title. Nothing is made up: with no limit windows reported it says so,
/// and Extra usage and Credits are shown only when the provider reported them (`quotaCard`).
pub fn quota_card(account: &AccountInfo) -> QuotaCard {
    if account
        .sign_in
        .as_ref()
        .is_some_and(|sign_in| sign_in.state == SignInState::Waiting)
    {
        return QuotaCard::Checking;
    }
    let Some(setup) = &account.setup else {
        return QuotaCard::Checking;
    };
    let report = report_of(setup);
    if let Some(error) = non_empty(report.error.as_deref()) {
        return QuotaCard::Problem {
            text: error.to_owned(),
        };
    }
    if !report.signed_in {
        return QuotaCard::SignedOut;
    }
    let live = account.provider_usage.as_ref();
    let has_windows = live.is_some_and(|live| !live.windows.is_empty());
    let has_extras =
        live.is_some_and(|live| live.extra_usage_enabled.is_some() || live.credits.is_some());
    if has_windows || !account.limits.is_empty() || has_extras {
        return QuotaCard::Limits;
    }
    if let Some(text) = live.and_then(problem_of) {
        return QuotaCard::Problem { text };
    }
    QuotaCard::Unavailable
}

/// Whether the account could take a bot's work now (`canTakeWork`).
fn can_take_work(account: &AccountInfo, now: &Zoned) -> bool {
    !account.spending.blocked
        && limit_state(account, now).is_none()
        && match account.kind {
            AccountKind::ApiKey => account.has_credential,
            AccountKind::Subscription => account
                .setup
                .as_ref()
                .is_some_and(|setup| report_of(setup).signed_in),
        }
}

/// `Date.parse(next.resetsAt ?? "") > Date.parse(latest.resetsAt ?? "")`: false when either doesn't parse, as NaN compares.
fn resets_later(next: &AccountLimitWindow, latest: &AccountLimitWindow) -> bool {
    match (
        next.resets_at.as_deref().and_then(millis),
        latest.resets_at.as_deref().and_then(millis),
    ) {
        (Some(next), Some(latest)) => next > latest,
        _ => false,
    }
}

/// The banner for a bot that needs setup, by the first rule that fits. `None` for no banner.
fn setup_banner<'a>(bot: &Bot, account: Option<&'a AccountInfo>) -> Option<Banner<'a>> {
    let reason = bot.setup_reason.as_deref().unwrap_or("");
    if let Some(account) = account
        && account.kind == AccountKind::Subscription
        && reason == CHECK_REASON
    {
        let setup = account.setup.as_ref()?;
        let report = report_of(setup);
        if report.signed_in
            || account
                .sign_in
                .as_ref()
                .is_some_and(|sign_in| sign_in.state == SignInState::Waiting)
        {
            return None;
        }
        if report.installed && non_empty(report.error.as_deref()).is_none() {
            return Some(Banner::SignIn { account });
        }
        let text = if report.installed {
            format!("{} could not be checked.", account.label)
        } else {
            format!("{} needs setup.", account.label)
        };
        return Some(Banner::AccountCheck { account, text });
    }
    if reason == SETUP_CODEX_SANDBOX_REASON {
        return Some(Banner::Setup {
            text: "Codex's sandbox isn't set up yet. Windows asks for admin permission once."
                .to_owned(),
            sandbox: true,
            sign_in: false,
        });
    }
    let signs_in = reason.starts_with(SETUP_SIGN_IN_PREFIX);
    if let Some(account) = account
        && signs_in
    {
        return Some(Banner::SignIn { account });
    }
    let text = if signs_in {
        format!(
            "{} isn't signed in for Tjommie yet.",
            provider_name(bot.provider)
        )
    } else {
        reason.to_owned()
    };
    Some(Banner::Setup {
        text,
        sandbox: false,
        sign_in: signs_in,
    })
}

/// The banner for a bot, by the first rule that fits; `None` for none. It only ever offers to switch accounts: no switch
/// happens by itself, and none is remembered (`bannerFor`).
pub fn banner_for<'a>(
    bot: &Bot,
    accounts: Option<&'a [AccountInfo]>,
    now: &Zoned,
) -> Option<Banner<'a>> {
    let account = accounts.and_then(|accounts| account_for(bot, accounts));
    if bot.state == BotState::NeedsSetup {
        return setup_banner(bot, account);
    }
    let (Some(accounts), Some(account)) = (accounts, account) else {
        return None;
    };
    if account.spending.blocked {
        return Some(Banner::Cap {
            account,
            unknown_prices: account.spending.unknown_rows,
        });
    }
    if bot.state == BotState::Limited && !bot.using_fallback.unwrap_or(false) {
        let reached: Vec<&AccountLimitWindow> = account
            .limits
            .iter()
            .filter(|window| window.state == LimitState::Reached)
            .collect();
        let hit = reached
            .iter()
            .copied()
            .find(|window| window.resets_at.is_none())
            .or_else(|| {
                reached.iter().copied().reduce(|latest, next| {
                    if resets_later(next, latest) {
                        next
                    } else {
                        latest
                    }
                })
            });
        let alternatives = accounts_of(accounts, bot.provider)
            .into_iter()
            .filter(|other| other.id != account.id && can_take_work(other, now))
            .collect();
        return Some(Banner::Limited {
            account,
            window: hit.map_or_else(|| "limit".to_owned(), limit_label),
            until: bot
                .limited_until
                .clone()
                .or_else(|| hit.and_then(|window| window.resets_at.clone())),
            alternatives,
        });
    }
    if bot.using_fallback == Some(true) {
        let pinned_id = bot.account_id.clone().flatten();
        let pinned = match non_empty(pinned_id.as_deref()) {
            Some(id) => accounts.iter().find(|other| other.id == id),
            None => default_account(accounts, bot.provider),
        };
        if let Some(pinned) = pinned
            && pinned.id != account.id
            && let Some(limit) = limit_state(pinned, now)
        {
            return Some(Banner::Fallback {
                pinned,
                using: account,
                until: match limit {
                    LimitBlock::Until { at } => Some(at),
                    LimitBlock::Unknown => None,
                },
            });
        }
    }
    None
}

/// A dialog is closed only by its own completion (`closeEpoch`): a close for any other epoch changes nothing.
pub fn close_epoch<T>(current: Option<T>, epoch: u64, epoch_of: impl Fn(&T) -> u64) -> Option<T> {
    current.filter(|current| epoch_of(current) != epoch)
}

/// The sign-in pages kept by account (`rememberUrl`). A sign-in that gave no address keeps nothing, and an older address
/// is dropped with it.
pub fn remember_url(
    urls: &HashMap<String, String>,
    id: &str,
    url: Option<&str>,
) -> HashMap<String, String> {
    let mut kept = urls.clone();
    kept.remove(id);
    if let Some(url) = non_empty(url) {
        kept.insert(id.to_owned(), url.to_owned());
    }
    kept
}

/// Takes the key out of the field to send it: the field is cleared before anything is awaited, so a request that fails
/// or is slow never leaves the key on screen (`takeKey`). The trim is JavaScript's, as in `bot_form`.
pub fn take_key(typed: &str, clear: impl FnOnce()) -> String {
    let key = typed.trim().to_owned();
    clear();
    key
}

/// Whether the text is `\d+(\.\d{1,2})?`, JavaScript's `DOLLARS`: ASCII digits, at most two decimals.
fn is_dollars(value: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    match value.split_once('.') {
        None => digits(value),
        Some((whole, fraction)) => {
            digits(whole) && (1..=2).contains(&fraction.len()) && digits(fraction)
        }
    }
}

/// The cap a person typed, in dollars, or `None` if it isn't a positive amount the daemon takes (at most a million).
pub fn parse_cap(text: &str) -> Option<f64> {
    let value = text.trim();
    if !is_dollars(value) {
        return None;
    }
    let amount: f64 = value.parse().ok()?;
    (amount > 0.0 && amount <= 1_000_000.0).then_some(amount)
}

/// Whether an API key can be sent: something typed, no whitespace inside (a pasted line break would make it a different
/// key), within the credential store's 2560 bytes (`keyReady`).
pub fn key_ready(key: &str) -> bool {
    let typed = key.trim();
    !typed.is_empty() && !typed.chars().any(char::is_whitespace) && typed.len() <= 2560
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// 22 October 2026, 10:00 UTC. The zone is fixed so the days don't depend on this machine's.
    fn now() -> Zoned {
        "2026-10-22T10:00:00[UTC]".parse().unwrap()
    }

    /// The TypeScript `later(hours)`: the instant that many hours from now, as the wire writes it.
    fn later(hours: i64) -> String {
        let now_ms = now().timestamp().as_millisecond();
        Timestamp::from_millisecond(now_ms + hours * HOUR_MS)
            .unwrap()
            .to_string()
    }

    /// `{ ...base, ...patch }`: the patch's keys replace the base's.
    fn with(mut base: Value, patch: Value) -> Value {
        if let (Some(fields), Value::Object(patch)) = (base.as_object_mut(), patch) {
            fields.extend(patch);
        }
        base
    }

    fn signed_in_setup() -> Value {
        json!({"provider": "claude", "installed": true, "version": "2.0.0", "signedIn": true, "account": "me@example.com", "plan": "max", "error": null})
    }

    fn signed_out_setup() -> Value {
        json!({"provider": "claude", "installed": true, "version": null, "signedIn": false, "account": null, "plan": null, "error": null})
    }

    /// A signed-out Claude report, with `patch` on top.
    fn setup_with(patch: Value) -> Value {
        with(signed_out_setup(), patch)
    }

    fn window_json(patch: Value) -> Value {
        with(
            json!({
                "accountId": "a1", "limitId": "l", "windowKey": "primary", "durationMinutes": 300,
                "usedPercent": 42, "resetsAt": later(2), "state": "ok", "observedAt": later(0)
            }),
            patch,
        )
    }

    fn window(patch: Value) -> AccountLimitWindow {
        serde_json::from_value(window_json(patch)).unwrap()
    }

    fn account_json(patch: Value) -> Value {
        with(
            json!({
                "id": "a1", "provider": "claude", "label": "Claude (Personal)", "kind": "subscription",
                "isDefault": true, "isFallback": false, "spendingCapUsd": null,
                "createdAt": "2026-10-01T00:00:00.000Z", "hasCredential": false,
                "setup": signed_in_setup(), "limits": [], "signIn": null,
                "spending": {"knownUsd": 0, "unknownRows": 0, "capUsd": null, "blocked": false}
            }),
            patch,
        )
    }

    fn account(patch: Value) -> AccountInfo {
        serde_json::from_value(account_json(patch)).unwrap()
    }

    /// An API key account, `k1`, with a 20 dollar cap and 5 spent.
    fn key(patch: Value) -> AccountInfo {
        let key = account_json(json!({
            "id": "k1", "label": "Claude key", "kind": "api_key", "isDefault": false, "isFallback": true,
            "spendingCapUsd": 20, "hasCredential": true, "setup": null,
            "spending": {"knownUsd": 5, "unknownRows": 0, "capUsd": 20, "blocked": false}
        }));
        serde_json::from_value(with(key, patch)).unwrap()
    }

    fn bot(patch: Value) -> Bot {
        serde_json::from_value(with(
            json!({
                "id": "b1", "name": "Scout", "color": "blue", "shape": "circle", "instructions": "",
                "provider": "claude", "model": "sonnet", "accountId": null, "effectiveAccountId": null,
                "usingFallback": false, "autonomy": "folder", "homeFolder": "C:/b", "projectFolder": null,
                "browser": false, "paused": false, "canMessage": [], "state": "idle", "setupReason": null,
                "limitedUntil": null, "createdAt": "2026-10-01T00:00:00.000Z"
            }),
            patch,
        ))
        .unwrap()
    }

    fn usage_window_json(patch: Value) -> Value {
        with(
            json!({"id": "w", "label": "Session", "scope": null, "durationMinutes": null, "usedPercent": null, "resetsAt": null}),
            patch,
        )
    }

    fn usage_window(patch: Value) -> AccountUsageWindow {
        serde_json::from_value(usage_window_json(patch)).unwrap()
    }

    /// The account's provider reading: available, with nothing in it, unless `patch` says otherwise.
    fn live(patch: Value) -> AccountInfo {
        account(json!({"providerUsage": with(json!({
            "status": "available", "checkedAt": later(0), "lastSuccessfulAt": later(0), "error": null,
            "planLabel": null, "windows": [], "credits": null, "extraUsageEnabled": null
        }), patch)}))
    }

    fn credits(value: Value) -> Credits {
        serde_json::from_value(value).unwrap()
    }

    fn id(account: Option<&AccountInfo>) -> Option<&str> {
        account.map(|account| account.id.as_str())
    }

    fn limited_window() -> Value {
        window_json(json!({"state": "reached", "resetsAt": later(2)}))
    }

    fn unknown_reset_window() -> Value {
        window_json(json!({"state": "reached", "resetsAt": null}))
    }

    #[test]
    fn names_the_5_hour_and_weekly_windows_other_lengths_and_a_window_with_no_length() {
        let labels: Vec<String> = [300, 10_080, 1440, 2880, 120, 90]
            .into_iter()
            .map(|minutes| {
                limit_label(&window(
                    json!({"durationMinutes": minutes, "windowKey": "w"}),
                ))
            })
            .collect();
        assert_eq!(
            labels,
            [
                "5-hour limit",
                "Weekly limit",
                "1-day limit",
                "2-day limit",
                "2-hour limit",
                "1.5-hour limit"
            ]
        );
        assert_eq!(
            limit_label(&window(
                json!({"durationMinutes": null, "windowKey": "monthly"})
            )),
            "monthly"
        );
    }

    #[test]
    fn says_the_use_and_the_reset_usage_unknown_for_no_reading_and_how_old_an_old_reading_is() {
        let now = now();
        assert_eq!(
            limit_value(&window(json!({})), &now),
            format!("42% · resets {}", clock(&later(2), now.time_zone()))
        );
        assert_eq!(
            limit_value(
                &window(json!({"usedPercent": null, "resetsAt": null})),
                &now
            ),
            "Usage unknown"
        );
        assert!(
            limit_value(&window(json!({"observedAt": later(-3)})), &now)
                .contains("updated 3 hours ago")
        );
    }

    #[test]
    fn is_limited_until_the_latest_reset_among_reached_windows_still_ahead() {
        let limited = account(json!({"limits": [
            window_json(json!({"state": "reached", "resetsAt": later(1)})),
            window_json(json!({"limitId": "w", "state": "reached", "resetsAt": later(30)})),
            window_json(json!({"limitId": "x", "state": "ok", "resetsAt": later(99)})),
        ]}));
        assert_eq!(limited_until(&limited, &now()), Some(later(30)));
        let past = account(
            json!({"limits": [window_json(json!({"state": "reached", "resetsAt": later(-1)}))]}),
        );
        assert_eq!(limited_until(&past, &now()), None);
        let near = account(json!({"limits": [window_json(json!({"state": "near"}))]}));
        assert_eq!(limited_until(&near, &now()), None);
    }

    #[test]
    fn finds_the_window_with_the_highest_known_use() {
        let busy = account(json!({"limits": [
            window_json(json!({"usedPercent": 10})),
            window_json(json!({"limitId": "w", "usedPercent": 90})),
            window_json(json!({"limitId": "u", "usedPercent": null})),
        ]}));
        assert_eq!(
            top_limit(&[busy]).and_then(|top| top.used_percent),
            Some(90.0)
        );
        let unknown_only = account(json!({"limits": [window_json(json!({"usedPercent": null}))]}));
        assert!(top_limit(&[unknown_only]).is_none());
    }

    #[test]
    fn still_blocks_as_its_own_state_and_a_reset_in_the_past_does_not() {
        let now = now();
        let unknown = account(json!({"limits": [unknown_reset_window()]}));
        assert_eq!(limit_state(&unknown, &now), Some(LimitBlock::Unknown));
        let until = account(json!({"limits": [limited_window()]}));
        assert_eq!(
            limit_state(&until, &now),
            Some(LimitBlock::Until { at: later(2) })
        );
        let past = account(
            json!({"limits": [window_json(json!({"state": "reached", "resetsAt": later(-1)}))]}),
        );
        assert_eq!(limit_state(&past, &now), None);
        let mixed = account(json!({"limits": [
            unknown_reset_window(),
            window_json(json!({"limitId": "w", "state": "reached", "resetsAt": later(2)})),
        ]}));
        assert_eq!(limit_state(&mixed, &now), Some(LimitBlock::Unknown));
    }

    #[test]
    fn says_the_reset_is_unknown_in_the_card_the_picker_and_the_help_line() {
        let now = now();
        let unknown = account(json!({"limits": [unknown_reset_window()]}));
        let line = status_line(&unknown, &now, StatusFlags::default());
        assert_eq!(line.tone, Tone::Limited);
        assert_eq!(line.text, "Limited. Reset time unknown.");
        assert_eq!(picker_state(&unknown, &now), "Limited");
        assert_eq!(
            account_help(Some(&unknown), &now, false).as_deref(),
            Some("Claude (Personal) is limited, and its reset time isn't known.")
        );
    }

    #[test]
    fn makes_a_banner_for_a_limited_bot_with_no_time_and_doesnt_offer_the_account_as_a_way_out() {
        let now = now();
        let unknown = account(json!({"limits": [unknown_reset_window()]}));
        let other = account(json!({"id": "a2", "label": "Work", "isDefault": false}));
        let same = account(
            json!({"id": "a3", "label": "Same", "isDefault": false, "limits": [unknown_reset_window()]}),
        );
        let list = [unknown, other, same];
        let banner = banner_for(
            &bot(json!({"state": "limited", "limitedUntil": null})),
            Some(list.as_slice()),
            &now,
        );
        match banner {
            Some(Banner::Limited {
                window: label,
                until,
                alternatives,
                ..
            }) => {
                assert_eq!(label, "5-hour limit");
                assert_eq!(until, None);
                let ids: Vec<&str> = alternatives.iter().map(|each| each.id.as_str()).collect();
                assert_eq!(ids, ["a2"]);
            }
            unexpected => panic!("expected a limited banner, got {unexpected:?}"),
        }
    }

    #[test]
    fn explains_a_fallback_whose_pinned_account_has_no_reset_time() {
        let unknown = account(json!({"limits": [unknown_reset_window()]}));
        let list = [unknown, key(json!({}))];
        let banner = banner_for(
            &bot(
                json!({"state": "working", "usingFallback": true, "accountId": "a1", "effectiveAccountId": "k1"}),
            ),
            Some(list.as_slice()),
            &now(),
        );
        assert!(matches!(banner, Some(Banner::Fallback { until: None, .. })));
    }

    #[test]
    fn is_offered_for_an_unknown_reset_with_nothing_known_and_in_the_future_and_not_otherwise() {
        let now = now();
        let unknown = account(json!({"limits": [unknown_reset_window()]}));
        assert!(can_retry_limit(&unknown, &now));
        assert_eq!(
            status_line(&unknown, &now, StatusFlags::default()).actions,
            [StatusAction::RetryLimit]
        );
        // a known future window also blocks it: wait for that
        let both = account(json!({"limits": [
            unknown_reset_window(),
            window_json(json!({"limitId": "w", "state": "reached", "resetsAt": later(3)})),
        ]}));
        assert!(!can_retry_limit(&both, &now));
        assert!(
            status_line(&both, &now, StatusFlags::default())
                .actions
                .is_empty()
        );
        // a known reset alone, or no limit at all: nothing to retry
        let known = account(json!({"limits": [limited_window()]}));
        assert!(!can_retry_limit(&known, &now));
        assert!(!can_retry_limit(&account(json!({})), &now));
    }

    #[test]
    fn is_not_offered_past_a_cap() {
        let now = now();
        let capped = key(json!({
            "limits": [unknown_reset_window()],
            "spending": {"knownUsd": 20, "unknownRows": 0, "capUsd": 20, "blocked": true}
        }));
        assert!(!can_retry_limit(&capped, &now));
        let uncapped = key(json!({"limits": [unknown_reset_window()]}));
        assert_eq!(
            status_line(&uncapped, &now, StatusFlags::default()).actions,
            [StatusAction::RetryLimit]
        );
    }

    #[test]
    fn keeps_check_again_apart_from_it_and_only_for_a_codex_subscription() {
        let now = now();
        let codex = account(json!({
            "provider": "codex",
            "limits": [unknown_reset_window()],
            "setup": {"provider": "codex", "installed": true, "version": null, "signedIn": true, "account": null, "plan": null, "error": null, "sandbox": "ready"}
        }));
        assert_eq!(
            status_line(&codex, &now, StatusFlags::default()).actions,
            [StatusAction::RetryLimit, StatusAction::Check]
        );
        let claude = account(json!({"limits": [unknown_reset_window()]}));
        assert!(
            !status_line(&claude, &now, StatusFlags::default())
                .actions
                .contains(&StatusAction::Check)
        );
        let codex_key = key(json!({"provider": "codex", "limits": [unknown_reset_window()]}));
        assert_eq!(
            status_line(&codex_key, &now, StatusFlags::default()).actions,
            [StatusAction::RetryLimit]
        );
    }

    #[test]
    fn says_what_a_retry_may_cost() {
        assert_eq!(
            RETRY_CAPTION,
            "Retries one queued turn; it may use subscription allowance or API credit."
        );
    }

    #[test]
    fn waits_for_the_control_a_link_names_not_while_the_card_is_still_checking_and_once_it_appears()
    {
        let now = now();
        let checking = status_line(
            &account(json!({"setup": null})),
            &now,
            StatusFlags::default(),
        );
        assert!(!focus_ready(
            Focus::Retry,
            &account(json!({})),
            &checking.actions
        ));
        assert!(!focus_ready(
            Focus::SignIn,
            &account(json!({})),
            &checking.actions
        ));
        // the check came back: the account is limited with no reset known, or signed out
        let limited = status_line(
            &account(json!({"limits": [unknown_reset_window()]})),
            &now,
            StatusFlags::default(),
        );
        assert!(focus_ready(
            Focus::Retry,
            &account(json!({})),
            &limited.actions
        ));
        let signed_out = status_line(
            &account(json!({"setup": signed_out_setup()})),
            &now,
            StatusFlags::default(),
        );
        assert!(focus_ready(
            Focus::SignIn,
            &account(json!({})),
            &signed_out.actions
        ));
        let failed = status_line(
            &account(json!({"signIn": {"attempt": "t", "state": "failed"}})),
            &now,
            StatusFlags::default(),
        );
        assert!(focus_ready(
            Focus::SignIn,
            &account(json!({})),
            &failed.actions
        )); // Try again
    }

    #[test]
    fn is_ready_for_a_cap_on_an_api_key_at_once_and_never_for_the_wrong_control() {
        assert!(focus_ready(Focus::Cap, &key(json!({})), &[]));
        assert!(!focus_ready(Focus::Cap, &account(json!({})), &[]));
        assert!(!focus_ready(
            Focus::Retry,
            &account(json!({})),
            &[StatusAction::SignIn]
        ));
        assert!(!focus_ready(
            Focus::SignIn,
            &account(json!({})),
            &[StatusAction::RetryLimit]
        ));
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Dialog {
        epoch: u64,
    }

    #[test]
    fn closes_only_the_dialog_that_asked_so_a_late_save_cant_close_the_next_one() {
        let epoch_of = |dialog: &Dialog| dialog.epoch;
        let first = Dialog { epoch: 1 };
        let second = Dialog { epoch: 2 };
        assert_eq!(close_epoch(Some(first), 1, epoch_of), None);
        // the first finished late, after the second opened
        assert_eq!(close_epoch(Some(second.clone()), 1, epoch_of), Some(second));
        assert_eq!(close_epoch(None::<Dialog>, 1, epoch_of), None);
    }

    #[test]
    fn takes_the_first_rule_that_fits_in_order() {
        let now = now();
        let waiting =
            account(json!({"signIn": {"attempt": "t1", "state": "waiting"}, "setup": null}));
        let reopen = status_line(
            &waiting,
            &now,
            StatusFlags {
                can_reopen: true,
                ..StatusFlags::default()
            },
        );
        assert_eq!(reopen.tone, Tone::Busy);
        assert_eq!(
            reopen.actions,
            [StatusAction::OpenAgain, StatusAction::Cancel]
        );
        assert_eq!(
            status_line(&waiting, &now, StatusFlags::default()).actions,
            [StatusAction::Cancel]
        );
        let denied = status_line(
            &account(json!({"signIn": {"attempt": "t1", "state": "failed", "message": "Denied."}})),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(denied.tone, Tone::Problem);
        assert_eq!(denied.text, "Denied.");
        assert_eq!(denied.actions, [StatusAction::TryAgain]);
        let silent = status_line(
            &account(json!({"signIn": {"attempt": "t1", "state": "failed"}})),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(silent.text, "Sign-in didn't finish.");
        let checking = status_line(
            &account(json!({"setup": null})),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(checking.text, "Checking");
        assert!(checking.actions.is_empty());
        let stuck = status_line(
            &account(json!({"setup": null})),
            &now,
            StatusFlags {
                stuck: true,
                ..StatusFlags::default()
            },
        );
        assert_eq!(stuck.actions, [StatusAction::Check]);
        let cli_failed = status_line(
            &account(json!({"setup": setup_with(json!({"error": "CLI failed"}))})),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(cli_failed.text, "CLI failed");
        assert_eq!(cli_failed.actions, [StatusAction::Check]);
        let not_signed_in = status_line(
            &account(json!({"setup": signed_out_setup()})),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(not_signed_in.text, "Not signed in. Agents on it wait.");
        assert_eq!(not_signed_in.actions, [StatusAction::SignIn]);
        let healthy = status_line(&account(json!({})), &now, StatusFlags::default());
        assert_eq!(healthy.text, "Signed in");
        assert_eq!(healthy.tone, Tone::Ok);
    }

    #[test]
    fn says_limited_until_a_time_ahead_of_plain_signed_in() {
        let now = now();
        let limited = account(json!({"limits": [limited_window()]}));
        let line = status_line(&limited, &now, StatusFlags::default());
        assert_eq!(line.tone, Tone::Limited);
        assert_eq!(
            line.text,
            format!("Limited until {}", clock(&later(2), now.time_zone()))
        );
    }

    #[test]
    fn reads_an_api_keys_state_no_key_over_its_cap_stopped_for_unpriced_turns_or_fine() {
        let now = now();
        let no_key = status_line(
            &key(json!({"hasCredential": false})),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(no_key.text, "No key saved");
        assert_eq!(no_key.actions, [StatusAction::AddKey]);
        let capped = status_line(
            &key(
                json!({"spending": {"knownUsd": 20, "unknownRows": 0, "capUsd": 20, "blocked": true}}),
            ),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(
            capped.text,
            "Reached its $20.00 cap. Agents stopped using it."
        );
        assert_eq!(capped.actions, [StatusAction::RaiseCap]);
        let unpriced = status_line(
            &key(
                json!({"spending": {"knownUsd": 3, "unknownRows": 4, "capUsd": 20, "blocked": true}}),
            ),
            &now,
            StatusFlags::default(),
        );
        assert_eq!(
            unpriced.text,
            "Stopped: 4 turns have no price, so the cap can't be checked."
        );
        assert!(unpriced.actions.is_empty());
        assert_eq!(
            status_line(&key(json!({})), &now, StatusFlags::default()).tone,
            Tone::Ok
        );
    }

    #[test]
    fn shows_spend_against_the_cap_near_it_from_80_and_what_has_no_price() {
        assert_eq!(
            spending_meter(&AccountSpending {
                known_usd: 5.0,
                unknown_rows: 0,
                cap_usd: Some(20.0),
                blocked: false
            }),
            SpendingMeter {
                share: 0.25,
                near: false,
                text: "Spent $5.00 of $20.00 cap in total".to_owned()
            }
        );
        let near = spending_meter(&AccountSpending {
            known_usd: 16.0,
            unknown_rows: 2,
            cap_usd: Some(20.0),
            blocked: false,
        });
        assert!(near.near);
        assert_eq!(
            near.text,
            "Spent $16.00 of $20.00 cap in total + 2 turns with no price"
        );
        let over = spending_meter(&AccountSpending {
            known_usd: 99.0,
            unknown_rows: 0,
            cap_usd: Some(20.0),
            blocked: true,
        });
        assert_eq!(over.share, 1.0);
    }

    #[test]
    fn orders_a_providers_accounts_with_the_default_first_and_finds_the_default() {
        let list = [
            key(json!({})),
            account(
                json!({"id": "a2", "label": "Second", "isDefault": false, "createdAt": "2026-10-02T00:00:00.000Z"}),
            ),
            account(json!({})),
            account(json!({"id": "c1", "provider": "codex", "label": "Codex", "isDefault": true})),
        ];
        let ids: Vec<&str> = accounts_of(&list, Provider::Claude)
            .iter()
            .map(|each| each.id.as_str())
            .collect();
        assert_eq!(ids, ["a1", "k1", "a2"]);
        assert_eq!(id(chosen_account(&list, Provider::Codex, None)), Some("c1"));
        assert_eq!(
            id(chosen_account(&list, Provider::Claude, Some("a2"))),
            Some("a2")
        );
        // another provider's account isn't this tab's
        assert_eq!(
            id(chosen_account(&list, Provider::Claude, Some("c1"))),
            Some("a1")
        );
    }

    #[test]
    fn names_an_accounts_state_on_its_picker_row() {
        let now = now();
        assert_eq!(picker_state(&account(json!({})), &now), "Default");
        assert_eq!(
            picker_state(&account(json!({"isDefault": false})), &now),
            ""
        );
        assert_eq!(picker_state(&key(json!({})), &now), "API key");
        assert_eq!(
            picker_state(&account(json!({"setup": signed_out_setup()})), &now),
            "Not signed in"
        );
        assert_eq!(
            picker_state(&account(json!({"limits": [limited_window()]})), &now),
            format!("Limited until {}", clock(&later(2), now.time_zone()))
        );
    }

    #[test]
    fn warns_about_the_account_in_use_and_about_a_switch() {
        let now = now();
        assert_eq!(account_help(Some(&account(json!({}))), &now, false), None);
        assert_eq!(
            account_help(Some(&account(json!({}))), &now, true).as_deref(),
            Some("Switching accounts starts a fresh session. The chat stays.")
        );
        assert_eq!(
            account_help(
                Some(&account(
                    json!({"label": "Work", "setup": signed_out_setup()})
                )),
                &now,
                true
            )
            .as_deref(),
            Some("Work isn't signed in.")
        );
        assert_eq!(
            account_help(
                Some(&key(json!({"label": "Key", "spending": {"knownUsd": 20, "unknownRows": 0, "capUsd": 20, "blocked": true}}))),
                &now,
                false
            )
            .as_deref(),
            Some("Key reached its cap.")
        );
    }

    #[test]
    fn finds_the_account_a_bot_runs_on_in_use_pinned_or_the_default() {
        let list = [account(json!({})), key(json!({}))];
        assert_eq!(
            id(account_for(
                &bot(json!({"effectiveAccountId": "k1", "accountId": "a1"})),
                &list
            )),
            Some("k1")
        );
        assert_eq!(
            id(account_for(&bot(json!({"accountId": "k1"})), &list)),
            Some("k1")
        );
        assert_eq!(id(account_for(&bot(json!({})), &list)), Some("a1"));
    }

    #[test]
    fn notifies_for_a_checked_signed_out_subscription_using_the_account_runtime_reason() {
        let now = now();
        let reason = "Check or sign in to this account in Accounts.";
        let waiting = bot(json!({"state": "needs_setup", "setupReason": reason}));
        let signed_out_list = [account(json!({"setup": signed_out_setup()}))];
        assert!(matches!(
            banner_for(&waiting, Some(signed_out_list.as_slice()), &now),
            Some(Banner::SignIn { account }) if account.id == "a1"
        ));
        let no_setup = [account(json!({"setup": null}))];
        assert!(banner_for(&waiting, Some(no_setup.as_slice()), &now).is_none());
        let signed_in = [account(json!({}))];
        assert!(banner_for(&waiting, Some(signed_in.as_slice()), &now).is_none());
        let pending = [account(
            json!({"setup": signed_out_setup(), "signIn": {"attempt": "pending", "state": "waiting"}}),
        )];
        assert!(banner_for(&waiting, Some(pending.as_slice()), &now).is_none());
        let check_failed = [account(
            json!({"setup": setup_with(json!({"error": "This account could not be checked. Try again in Accounts."}))}),
        )];
        assert!(matches!(
            banner_for(&waiting, Some(check_failed.as_slice()), &now),
            Some(Banner::AccountCheck { text, .. }) if text == "Claude (Personal) could not be checked."
        ));
        let not_installed = [account(
            json!({"setup": setup_with(json!({"installed": false}))}),
        )];
        assert!(matches!(
            banner_for(&waiting, Some(not_installed.as_slice()), &now),
            Some(Banner::AccountCheck { text, .. }) if text == "Claude (Personal) needs setup."
        ));
        let on_key = bot(json!({"state": "needs_setup", "setupReason": reason, "accountId": "k1"}));
        let key_list = [key(json!({}))];
        assert!(matches!(
            banner_for(&on_key, Some(key_list.as_slice()), &now),
            Some(Banner::Setup { sign_in: false, .. })
        ));
    }

    #[test]
    fn asks_to_sign_in_the_account_a_setup_bot_is_on_and_keeps_the_sandbox_banner() {
        let now = now();
        let signed_out_list = [account(json!({"setup": signed_out_setup()}))];
        let sign_in_bot =
            bot(json!({"state": "needs_setup", "setupReason": "Sign in to Claude Code"}));
        assert!(matches!(
            banner_for(&sign_in_bot, Some(signed_out_list.as_slice()), &now),
            Some(Banner::SignIn { account }) if account.id == "a1"
        ));
        let sandbox_bot = bot(
            json!({"state": "needs_setup", "setupReason": "Set up Codex's sandbox", "provider": "codex"}),
        );
        let none: Vec<AccountInfo> = Vec::new();
        assert!(matches!(
            banner_for(&sandbox_bot, Some(none.as_slice()), &now),
            Some(Banner::Setup { sandbox: true, .. })
        ));
    }

    #[test]
    fn says_a_capped_accounts_cap_first_with_no_switch_offered() {
        let now = now();
        let capped = key(
            json!({"spending": {"knownUsd": 20, "unknownRows": 0, "capUsd": 20, "blocked": true}}),
        );
        let list = [account(json!({})), capped];
        assert!(matches!(
            banner_for(
                &bot(json!({"accountId": "k1", "state": "limited"})),
                Some(list.as_slice()),
                &now
            ),
            Some(Banner::Cap {
                unknown_prices: 0,
                ..
            })
        ));
        let unpriced = key(
            json!({"spending": {"knownUsd": 3, "unknownRows": 2, "capUsd": 20, "blocked": true}}),
        );
        let unpriced_list = [unpriced];
        assert!(matches!(
            banner_for(
                &bot(json!({"accountId": "k1"})),
                Some(unpriced_list.as_slice()),
                &now
            ),
            Some(Banner::Cap {
                unknown_prices: 2,
                ..
            })
        ));
    }

    #[test]
    fn offers_only_the_accounts_that_could_take_over_for_a_limited_bot_and_never_switches_by_itself()
     {
        let now = now();
        let limited = account(json!({"limits": [limited_window()]}));
        let ok = account(json!({"id": "a2", "label": "Work", "isDefault": false}));
        let out = account(
            json!({"id": "a3", "label": "Out", "isDefault": false, "setup": signed_out_setup()}),
        );
        let also_limited = account(
            json!({"id": "a4", "isDefault": false, "limits": [window_json(json!({"state": "reached", "resetsAt": later(3)}))]}),
        );
        let other = account(json!({"id": "c1", "provider": "codex", "isDefault": true}));
        let list = [
            limited,
            ok,
            out,
            also_limited,
            other,
            key(json!({"hasCredential": false, "id": "k2"})),
        ];
        let banner = banner_for(
            &bot(json!({"state": "limited", "limitedUntil": later(2)})),
            Some(list.as_slice()),
            &now,
        );
        match banner {
            Some(Banner::Limited {
                window: label,
                until,
                alternatives,
                ..
            }) => {
                assert_eq!(label, "5-hour limit");
                assert_eq!(until, Some(later(2)));
                let ids: Vec<&str> = alternatives.iter().map(|each| each.id.as_str()).collect();
                assert_eq!(ids, ["a2"]);
            }
            unexpected => panic!("expected a limited banner, got {unexpected:?}"),
        }
    }

    #[test]
    fn explains_a_bot_that_carries_on_with_its_fallback_key_with_nothing_to_press() {
        let now = now();
        let limited = account(json!({"limits": [limited_window()]}));
        let list = [limited, key(json!({}))];
        let banner = banner_for(
            &bot(
                json!({"state": "working", "usingFallback": true, "accountId": "a1", "effectiveAccountId": "k1"}),
            ),
            Some(list.as_slice()),
            &now,
        );
        match banner {
            Some(Banner::Fallback {
                pinned,
                using,
                until,
            }) => {
                assert_eq!(pinned.id, "a1");
                assert_eq!(using.id, "k1");
                assert_eq!(until, Some(later(2)));
            }
            unexpected => panic!("expected a fallback banner, got {unexpected:?}"),
        }
    }

    #[test]
    fn has_nothing_to_say_for_a_healthy_bot_or_before_the_accounts_are_known() {
        let now = now();
        let list = [account(json!({}))];
        assert!(banner_for(&bot(json!({})), Some(list.as_slice()), &now).is_none());
        assert!(banner_for(&bot(json!({"state": "limited"})), None, &now).is_none());
    }

    #[test]
    fn lists_the_bots_pinned_to_an_account_which_keep_it_from_being_removed() {
        let bots = [
            bot(json!({"id": "b1", "accountId": "a1"})),
            bot(json!({"id": "b2", "accountId": null})),
            bot(json!({"id": "b3", "accountId": "k1"})),
        ];
        let pinned: Vec<&str> = bots_on("a1", &bots)
            .iter()
            .map(|each| each.id.as_str())
            .collect();
        assert_eq!(pinned, ["b1"]);
        assert!(bots_on("zz", &bots).is_empty());
    }

    #[test]
    fn only_keeps_the_address_by_account_and_drops_it_when_a_new_sign_in_gave_none() {
        let urls = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(account, url)| ((*account).to_owned(), (*url).to_owned()))
                .collect()
        };
        assert_eq!(
            remember_url(&HashMap::new(), "a1", Some("https://x.test/")),
            urls(&[("a1", "https://x.test/")])
        );
        assert_eq!(
            remember_url(
                &urls(&[("a1", "https://old.test/"), ("a2", "https://y.test/")]),
                "a1",
                None
            ),
            urls(&[("a2", "https://y.test/")])
        );
        assert_eq!(
            remember_url(
                &urls(&[("a1", "https://old.test/")]),
                "a1",
                Some("https://new.test/")
            ),
            urls(&[("a1", "https://new.test/")])
        );
    }

    #[test]
    fn clears_the_field_before_it_hands_the_key_over_so_a_failing_or_pending_request_never_leaves_it()
     {
        let mut steps: Vec<String> = Vec::new();
        let sent = take_key("  sk-example  ", || steps.push("cleared".to_owned()));
        steps.push(format!("sent {sent}"));
        assert_eq!(steps, ["cleared", "sent sk-example"]);
    }

    #[test]
    fn takes_a_cap_in_dollars_positive_with_at_most_two_decimals() {
        assert_eq!(
            [parse_cap("20"), parse_cap(" 12.5 "), parse_cap("0.01")],
            [Some(20.0), Some(12.5), Some(0.01)]
        );
        assert_eq!(
            [
                parse_cap(""),
                parse_cap("0"),
                parse_cap("-3"),
                parse_cap("1.234"),
                parse_cap("abc"),
                parse_cap("1000000.01")
            ],
            [None; 6]
        );
    }

    #[test]
    fn takes_a_key_that_is_something_typed_and_fits_the_credential_store() {
        assert!(!key_ready(""));
        assert!(!key_ready("   "));
        assert!(key_ready(" sk-example "));
        assert!(key_ready(&"k".repeat(2560)));
        assert!(!key_ready(&"k".repeat(2561)));
        // counted in bytes, not characters
        assert!(!key_ready(&"é".repeat(1281)));
        assert!(!key_ready("sk-one two"));
        assert!(!key_ready("sk-one\ntwo"));
    }

    #[test]
    fn shows_limits_when_there_are_windows_and_says_so_honestly_when_there_are_none() {
        assert_eq!(
            quota_card(&account(json!({"limits": [window_json(json!({}))]}))),
            QuotaCard::Limits
        );
        assert_eq!(
            quota_card(&account(json!({"limits": []}))),
            QuotaCard::Unavailable
        );
    }

    #[test]
    fn is_checking_before_the_first_answer_or_a_sign_in_is_waiting_and_shows_a_problem_or_a_signed_out_account_as_itself()
     {
        assert_eq!(
            quota_card(&account(json!({"setup": null}))),
            QuotaCard::Checking
        );
        assert_eq!(
            quota_card(&account(
                json!({"signIn": {"attempt": "t", "state": "waiting"}})
            )),
            QuotaCard::Checking
        );
        assert_eq!(
            quota_card(&account(
                json!({"setup": setup_with(json!({"error": "CLI failed"}))})
            )),
            QuotaCard::Problem {
                text: "CLI failed".to_owned()
            }
        );
        assert_eq!(
            quota_card(&account(json!({"setup": signed_out_setup()}))),
            QuotaCard::SignedOut
        );
    }

    #[test]
    fn names_a_window_with_its_model_or_surface() {
        assert_eq!(
            window_name(&usage_window(
                json!({"label": "Weekly", "scope": {"kind": "model", "id": "fable", "label": "Fable"}})
            )),
            "Weekly · Fable"
        );
        assert_eq!(
            window_name(&usage_window(json!({"label": "Session", "scope": null}))),
            "Session"
        );
    }

    #[test]
    fn writes_credits_as_the_providers_units() {
        assert_eq!(
            credits_text(&credits(json!({"balance": "38917.83", "unlimited": null}))),
            "38,917.83 left"
        );
        assert_eq!(
            credits_text(&credits(json!({"balance": "0", "unlimited": false}))),
            "0 left"
        );
        assert_eq!(
            credits_text(&credits(json!({"balance": "12", "unlimited": true}))),
            "Unlimited"
        );
        assert_eq!(
            credits_text(&credits(json!({"balance": null, "unlimited": null}))),
            "Unknown"
        );
    }

    #[test]
    fn takes_the_plan_from_the_reading_before_setup_and_the_rows_from_the_reading_before_the_observed_windows()
     {
        let now = now();
        let reading = live(json!({
            "planLabel": "Max 20x",
            "windows": [usage_window_json(json!({"id": "w", "label": "Weekly", "scope": null, "durationMinutes": 10080, "usedPercent": 85, "resetsAt": later(30)}))]
        }));
        let view = provider_view(&reading, &now);
        assert_eq!(view.plan.as_deref(), Some("Max 20x"));
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].key, "w");
        assert_eq!(view.rows[0].label, "Weekly");
        assert_eq!(view.rows[0].share, Some(0.85));
        assert!(view.rows[0].value.contains("85%"));
        assert!(view.rows[0].near);
        // no reading: setup's plan
        assert_eq!(
            provider_view(&account(json!({})), &now).plan.as_deref(),
            Some("max")
        );
    }

    #[test]
    fn marks_windows_kept_from_an_earlier_success_as_stale_and_leaves_out_a_reset_that_has_passed()
    {
        let now = now();
        let failed = live(json!({
            "status": "error",
            "error": "Offline",
            "lastSuccessfulAt": later(-5),
            "windows": [usage_window_json(json!({"id": "w", "label": "Weekly", "durationMinutes": null, "usedPercent": 40, "resetsAt": later(-1)}))]
        }));
        let view = provider_view(&failed, &now);
        assert_eq!(view.stale.as_deref(), Some("Last updated 5 hours ago"));
        assert_eq!(view.problem.as_deref(), Some("Offline"));
        // not "resets" a time that is already behind
        assert_eq!(view.rows[0].value, "40%");
    }

    #[test]
    fn never_shows_an_unknown_use_as_0() {
        let reading = live(json!({
            "windows": [usage_window_json(json!({"id": "w", "label": "Weekly", "usedPercent": null, "resetsAt": null}))]
        }));
        let view = provider_view(&reading, &now());
        assert_eq!(view.rows[0].share, None);
        assert_eq!(view.rows[0].value, "Usage unknown");
        assert!(!view.rows[0].near);
    }

    #[test]
    fn is_a_card_not_unavailable_when_only_extra_usage_or_credits_is_known() {
        assert_eq!(
            quota_card(&live(
                json!({"credits": {"balance": "5", "hasCredits": true, "unlimited": false}})
            )),
            QuotaCard::Limits
        );
        assert_eq!(
            quota_card(&live(json!({"extraUsageEnabled": false}))),
            QuotaCard::Limits
        );
        let empty = account(json!({"limits": [], "providerUsage": {
            "status": "available", "checkedAt": null, "lastSuccessfulAt": null, "error": null,
            "planLabel": null, "windows": [], "credits": null, "extraUsageEnabled": null
        }}));
        assert_eq!(quota_card(&empty), QuotaCard::Unavailable);
    }

    #[test]
    fn says_an_extras_only_reading_is_stale_after_a_failed_check() {
        let now = now();
        let failed = live(json!({
            "status": "error",
            "error": "Offline",
            "lastSuccessfulAt": later(-4),
            "credits": {"balance": "10", "hasCredits": true, "unlimited": false}
        }));
        let view = provider_view(&failed, &now);
        assert!(view.rows.is_empty());
        assert_eq!(
            view.extras,
            [Extra {
                label: "Credits".to_owned(),
                value: "10 left".to_owned()
            }]
        );
        assert_eq!(view.stale.as_deref(), Some("Last updated 4 hours ago"));
        let healthy =
            live(json!({"credits": {"balance": "10", "hasCredits": true, "unlimited": false}}));
        assert_eq!(provider_view(&healthy, &now).stale, None);
    }

    #[test]
    fn is_a_problem_card_when_the_check_failed_and_there_is_nothing_earlier_to_show() {
        let failed = account(json!({"limits": [], "providerUsage": {
            "status": "error", "checkedAt": null, "lastSuccessfulAt": null, "error": "No answer",
            "planLabel": null, "windows": [], "credits": null, "extraUsageEnabled": null
        }}));
        assert_eq!(
            quota_card(&failed),
            QuotaCard::Problem {
                text: "No answer".to_owned()
            }
        );
    }

    // Decisions the TypeScript didn't make explicit: JavaScript's rounding, written out.

    #[test]
    fn money_rounds_a_tie_up_as_javascript_does() {
        // 0.125 is exact in binary, so it is a tie: toFixed goes up; Rust's `{:.2}` would go to even.
        assert_eq!(money(0.125), "$0.13");
        assert_eq!(money(5.0), "$5.00");
        // 0.005 is a little above the tie in binary, so it goes up too.
        assert_eq!(money(0.005), "$0.01");
    }

    #[test]
    fn cents_of_saturates_on_an_amount_past_u128_without_panicking() {
        // 38 digits fit in a u128 but not once they are in cents: the sums must saturate, not overflow.
        let thirty_eight = format!("{}.99", "9".repeat(38));
        assert_eq!(cents_of(&thirty_eight), u128::MAX);
        // 40 digits don't parse as a u128 at all. They must saturate too, not read as 0 cents.
        let forty = format!("{}.50", "1".repeat(40));
        assert_eq!(cents_of(&forty), u128::MAX);
        // A missing whole part is still 0 and the fraction counts.
        assert_eq!(cents_of(".5"), 50);
    }

    #[test]
    fn credits_group_thousands_and_round_half_up_as_intl_does() {
        assert_eq!(
            credits_text(&credits(json!({"balance": "1234567.891"}))),
            "1,234,567.89 left"
        );
        assert_eq!(
            credits_text(&credits(json!({"balance": "2.345"}))),
            "2.35 left"
        );
    }

    #[test]
    fn subscription_label_joins_the_plan_and_the_account() {
        assert_eq!(
            subscription_label(&account(json!({}))),
            "Subscription · Max plan · me@example.com"
        );
    }
}
