//! `usage.ts`: recorded token use and cost, the usage report, and an API account's lifetime spend.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use protocol::ErrorCode;
use protocol::accounts::{
    AccountSpending, AccountUsage, BotUsage, DailyUsage, ModelUsage, UsageAggregate, UsageCost,
    UsageCosts, UsageQuery, UsageReport, UsageTokens,
};
use protocol::model::Provider;
use regex::Regex;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use crate::accounts_store::{bad, iso, provider_from, provider_name};
use crate::ids::{new_id, now_iso};
use crate::server::RpcError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostSource {
    Unknown,
    Reported,
    Estimated,
}

impl CostSource {
    fn name(self) -> &'static str {
        match self {
            CostSource::Unknown => "unknown",
            CostSource::Reported => "reported",
            CostSource::Estimated => "estimated",
        }
    }
}

/// Token counts are JavaScript numbers from the providers; they must be nonnegative safe integers.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageInput {
    pub id: Option<String>,
    pub bot_id: String,
    pub account_id: String,
    pub provider: Provider,
    pub model: String,
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
    pub reasoning: f64,
    pub cost_usd: Option<f64>,
    pub cost_source: CostSource,
    pub pricing_revision: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct UsageRow {
    id: String,
    bot_id: String,
    account_id: String,
    provider: String,
    model: String,
    input: i64,
    cached_input: i64,
    output: i64,
    reasoning: i64,
    cost_usd: Option<f64>,
    cost_source: Option<String>,
    created_at: String,
    account_kind: String,
    pricing_revision: Option<String>,
}

const ROW: &str = "id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at, account_kind, pricing_revision";

fn usage_row(row: &Row) -> rusqlite::Result<UsageRow> {
    Ok(UsageRow {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        account_id: row.get(2)?,
        provider: row.get(3)?,
        model: row.get(4)?,
        input: row.get(5)?,
        cached_input: row.get(6)?,
        output: row.get(7)?,
        reasoning: row.get(8)?,
        cost_usd: row.get(9)?,
        cost_source: row.get(10)?,
        created_at: row.get(11)?,
        account_kind: row.get(12)?,
        pricing_revision: row.get(13)?,
    })
}

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

fn safe_count(value: f64) -> bool {
    value.fract() == 0.0 && (0.0..=MAX_SAFE).contains(&value)
}

static INSTANT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}T.+(?:Z|[+-]\d{2}:\d{2})$").unwrap());

/// Records one provider usage event. The caller owns the transaction, including any session cumulative-cost baseline
/// update. A stable provider event id makes a repeated delivery harmless (`Ok(false)`); a conflicting one is an error.
pub fn record_usage(db: &Connection, input: &UsageInput) -> Result<bool, RpcError> {
    let counts = [
        input.input,
        input.cached_input,
        input.output,
        input.reasoning,
    ];
    if !counts.into_iter().all(safe_count) {
        return Err(bad("Usage token counts must be nonnegative safe integers"));
    }
    if input.input + input.cached_input + input.output > MAX_SAFE {
        return Err(bad("Usage token total is too large"));
    }
    if input
        .cost_usd
        .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
    {
        return Err(bad("Usage cost must be a nonnegative finite amount"));
    }
    if input.cost_usd.is_none() != (input.cost_source == CostSource::Unknown) {
        return Err(bad("Usage cost provenance must match its amount"));
    }
    if [&input.bot_id, &input.account_id, &input.model]
        .iter()
        .any(|text| text.trim().is_empty())
        || input.id.as_deref() == Some("")
    {
        return Err(bad("Usage identity is required"));
    }
    let account: Option<(String, String)> = db
        .query_row(
            "SELECT provider, kind FROM accounts WHERE id = ?1",
            [&input.account_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((_, account_kind)) = account.filter(|(provider, kind)| {
        provider == provider_name(input.provider)
            && matches!(kind.as_str(), "subscription" | "api_key")
    }) else {
        return Err(bad("Usage must identify its actual provider account"));
    };
    let created_at = input.created_at.clone().unwrap_or_else(now_iso);
    let created_at = iso(&created_at)
        .filter(|_| INSTANT.is_match(&created_at))
        .ok_or_else(|| bad("Usage timestamp must be an ISO instant"))?;
    let row = UsageRow {
        id: input.id.clone().unwrap_or_else(new_id),
        bot_id: input.bot_id.clone(),
        account_id: input.account_id.clone(),
        provider: provider_name(input.provider).into(),
        model: input.model.clone(),
        input: input.input as i64,
        cached_input: input.cached_input as i64,
        output: input.output as i64,
        reasoning: input.reasoning as i64,
        cost_usd: input.cost_usd,
        cost_source: Some(input.cost_source.name().into()),
        created_at,
        account_kind,
        pricing_revision: input.pricing_revision.clone(),
    };
    let existing = db
        .query_row(
            &format!("SELECT {ROW} FROM usage WHERE id = ?1"),
            [&row.id],
            usage_row,
        )
        .optional()?;
    if let Some(existing) = existing {
        let compared = if input.created_at.is_none() {
            UsageRow {
                created_at: existing.created_at.clone(),
                ..row
            }
        } else {
            row
        };
        if existing != compared {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "Conflicting usage event identity",
            ));
        }
        return Ok(false);
    }
    db.execute(
        &format!("INSERT INTO usage ({ROW}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"),
        params![
            row.id,
            row.bot_id,
            row.account_id,
            row.provider,
            row.model,
            row.input,
            row.cached_input,
            row.output,
            row.reasoning,
            row.cost_usd,
            row.cost_source,
            row.created_at,
            row.account_kind,
            row.pricing_revision
        ],
    )?;
    Ok(true)
}

fn empty_cost() -> UsageCost {
    UsageCost {
        usd: None,
        known_usd: None,
        known_records: 0,
        unknown_records: 0,
        reported_usd: None,
        estimated_usd: None,
    }
}

fn empty_aggregate() -> UsageAggregate {
    UsageAggregate {
        records: 0,
        tokens: UsageTokens {
            input: 0,
            cached_input: 0,
            output: 0,
            reasoning: 0,
            total: 0,
        },
        costs: UsageCosts {
            api_spend: empty_cost(),
            api_value: empty_cost(),
            unclassified: empty_cost(),
        },
    }
}

fn known_cost(row: &UsageRow) -> Option<f64> {
    row.cost_usd
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .filter(|_| matches!(row.cost_source.as_deref(), Some("reported" | "estimated")))
}

fn add(aggregate: &mut UsageAggregate, row: &UsageRow) {
    let count = |value: i64| u64::try_from(value).unwrap_or(0);
    aggregate.records += 1;
    let tokens = &mut aggregate.tokens;
    tokens.input += count(row.input);
    tokens.cached_input += count(row.cached_input);
    tokens.output += count(row.output);
    tokens.reasoning += count(row.reasoning);
    tokens.total += count(row.input) + count(row.cached_input) + count(row.output);
    let cost = match row.account_kind.as_str() {
        "subscription" => &mut aggregate.costs.api_value,
        "api_key" => &mut aggregate.costs.api_spend,
        _ => &mut aggregate.costs.unclassified,
    };
    match known_cost(row) {
        Some(usd) => {
            cost.known_records += 1;
            cost.known_usd = Some(cost.known_usd.unwrap_or(0.0) + usd);
            let source = if row.cost_source.as_deref() == Some("reported") {
                &mut cost.reported_usd
            } else {
                &mut cost.estimated_usd
            };
            *source = Some(source.unwrap_or(0.0) + usd);
        }
        None => cost.unknown_records += 1,
    }
    cost.usd = if cost.unknown_records > 0 {
        None
    } else {
        cost.known_usd
    };
}

static OFFSET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([+-])([01][0-9]|2[0-3])(?::?([0-5][0-9]))?$").unwrap());

/// An IANA name or a `±HH[:MM]` offset, as `Intl.DateTimeFormat` accepts them.
fn zone(name: &str) -> Option<jiff::tz::TimeZone> {
    if let Ok(zone) = jiff::tz::TimeZone::get(name) {
        return Some(zone);
    }
    let captures = OFFSET.captures(name)?;
    let minutes = captures[2].parse::<i32>().ok()? * 60
        + captures
            .get(3)
            .map_or(Some(0), |minutes| minutes.as_str().parse().ok())?;
    let seconds = if &captures[1] == "-" {
        -minutes * 60
    } else {
        minutes * 60
    };
    Some(jiff::tz::TimeZone::fixed(
        jiff::tz::Offset::from_seconds(seconds).ok()?,
    ))
}

/// The usage report for a decoded `usage.query`.
pub fn query_usage(db: &Connection, query: &UsageQuery) -> Result<UsageReport, RpcError> {
    let invalid = || {
        bad(
            "Usage query requires valid instants, time zone, and a positive range of at most 366 days",
        )
    };
    let (from, to) = (
        iso(&query.from).ok_or_else(invalid)?,
        iso(&query.to).ok_or_else(invalid)?,
    );
    let zone = zone(query.time_zone.as_str()).ok_or_else(invalid)?;
    // julianday also handles old rows stored with different valid ISO offsets/precision.
    let mut filters = vec![
        "julianday(created_at) >= julianday(?)",
        "julianday(created_at) < julianday(?)",
    ];
    let mut values: Vec<Sql> = vec![Sql::Text(from.clone()), Sql::Text(to.clone())];
    let provider = query
        .provider
        .map(|provider| provider_name(provider).to_string());
    for (filter, value) in [
        ("bot_id = ?", &query.bot_id),
        ("account_id = ?", &query.account_id),
        ("provider = ?", &provider),
        ("model = ?", &query.model),
    ] {
        if let Some(value) = value {
            filters.push(filter);
            values.push(Sql::Text(value.clone()));
        }
    }
    let mut statement = db.prepare(&format!(
        "SELECT {ROW} FROM usage WHERE {} ORDER BY created_at, id",
        filters.join(" AND ")
    ))?;
    let rows: Vec<UsageRow> = statement
        .query_map(params_from_iter(values), usage_row)?
        .collect::<Result<_, _>>()?;
    let day_for = |at: &str| -> String {
        at.parse::<jiff::Timestamp>()
            .map(|instant| instant.to_zoned(zone.clone()).date().to_string())
            .unwrap_or_default()
    };
    let mut summary = empty_aggregate();
    // ponytail: groups sort by code point; TypeScript's localeCompare differs only for mixed-case or punctuated ids.
    let (mut daily, mut bots, mut accounts) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let mut models: BTreeMap<(String, String), UsageAggregate> = BTreeMap::new();
    for row in &rows {
        add(&mut summary, row);
        add(
            daily
                .entry(day_for(&row.created_at))
                .or_insert_with(empty_aggregate),
            row,
        );
        add(
            bots.entry(row.bot_id.clone())
                .or_insert_with(empty_aggregate),
            row,
        );
        add(
            accounts
                .entry(row.account_id.clone())
                .or_insert_with(empty_aggregate),
            row,
        );
        add(
            models
                .entry((row.provider.clone(), row.model.clone()))
                .or_insert_with(empty_aggregate),
            row,
        );
    }
    let by_model = models
        .into_iter()
        .map(|((provider, model), usage)| {
            Ok(ModelUsage {
                usage,
                provider: provider_from(&provider)?,
                model,
            })
        })
        .collect::<rusqlite::Result<_>>()?;
    Ok(UsageReport {
        from,
        to,
        time_zone: query.time_zone.clone(),
        summary,
        daily: daily
            .into_iter()
            .map(|(day, usage)| DailyUsage { usage, day })
            .collect(),
        by_bot: bots
            .into_iter()
            .map(|(bot_id, usage)| BotUsage { usage, bot_id })
            .collect(),
        by_account: accounts
            .into_iter()
            .map(|(account_id, usage)| AccountUsage { usage, account_id })
            .collect(),
        by_model,
    })
}

/// An account's lifetime local spend against its cap. Unknown costs block an API account: its spend can't be shown
/// to be under the cap.
pub fn account_budget(db: &Connection, id: &str) -> Result<AccountSpending, RpcError> {
    let account: Option<(String, Option<f64>)> = db
        .query_row(
            "SELECT kind, spending_cap_usd FROM accounts WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (kind, cap_usd) =
        account.ok_or_else(|| RpcError::new(ErrorCode::NotFound, "Account not found"))?;
    let (mut known_usd, mut unknown_rows) = (0.0, 0u64);
    if kind == "api_key" {
        let mut statement =
            db.prepare(&format!("SELECT {ROW} FROM usage WHERE account_id = ?1"))?;
        for row in statement.query_map([id], usage_row)? {
            let row = row?;
            match known_cost(&row).filter(|_| row.account_kind == "api_key") {
                Some(cost) => known_usd += cost,
                None => unknown_rows += 1,
            }
        }
    }
    let blocked =
        kind == "api_key" && cap_usd.is_some_and(|cap| unknown_rows > 0 || known_usd >= cap);
    Ok(AccountSpending {
        known_usd,
        unknown_rows,
        cap_usd,
        blocked,
    })
}
