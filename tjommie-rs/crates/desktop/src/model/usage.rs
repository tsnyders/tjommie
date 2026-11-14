//! Port of `packages/ui/lib/usage.ts`: the Usage screen's ranges, its query, its numbers as words, its chart's bars and
//! ticks, its breakdown rows, and the throttle that spaces its reloads. Zones are IANA names as the wire has them; a zone
//! this build's database doesn't know gives `None` where the TypeScript would throw. The machine's own zone
//! (`localZone`) is left out: the caller passes the zone in.

use std::collections::HashMap;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, Zoned};
use protocol::accounts::{
    AccountInfo, AccountKind, DailyUsage, UsageAggregate, UsageCost, UsageCosts, UsageQuery,
    UsageReport, UsageTokens,
};
use protocol::model::{Bot, Provider, TimeZone as WireZone};

/// A report covers at most 366 days; this only keeps a bad range from looping long.
const MAX_DAYS: usize = 400;
/// The dates under a week's or a month's bars.
const DAY_TICKS: usize = 5;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The ranges the screen offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeKind {
    Today,
    Week,
    Month,
    Year,
}

/// The filters on a usage report: a bot, an account, a model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageFilters {
    pub bot: Option<String>,
    pub account: Option<String>,
    pub model: Option<String>,
}

/// The breakdown's grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Bot,
    Account,
    Model,
}

/// A range's bounds as the wire writes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRange {
    pub from: String,
    pub to: String,
}

/// The IANA zone a name stands for. UTC offsets, which the protocol also accepts, aren't resolved: jiff has no offset
/// parser, and the screen sends only the machine's own zone.
fn zone_of(name: &str) -> Option<TimeZone> {
    TimeZone::get(name).ok()
}

/// Midnight at the start of `day` in `zone`, as an instant.
fn midnight(day: Date, zone: &TimeZone) -> Option<Timestamp> {
    Some(day.at(0, 0, 0, 0).to_zoned(zone.clone()).ok()?.timestamp())
}

/// The calendar day an instant falls on in `zone`.
fn day_in(instant: Timestamp, zone: &TimeZone) -> Date {
    instant.to_zoned(zone.clone()).date()
}

/// The wire's ISO form, with milliseconds and "Z", as `Date.toISOString` writes it.
fn iso(instant: Timestamp) -> String {
    instant.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// A filter's value, when it has one (an empty one doesn't count).
fn present(value: &Option<String>) -> Option<String> {
    value.clone().filter(|value| !value.is_empty())
}

/// Identity of "today" in `zone`: changes at local midnight and with the zone, not with the report's end instant.
pub fn usage_day_key(now: &Zoned, zone: &str) -> Option<String> {
    let day = now.with_time_zone(zone_of(zone)?).date();
    Some(format!("{zone}:{day}"))
}

/// Milliseconds until the next midnight in `zone`, plus one, so a timer set for it fires just after the day turns. At
/// least 1.
pub fn until_usage_day(now: &Zoned, zone: &str) -> Option<i64> {
    let resolved = zone_of(zone)?;
    let tomorrow = now
        .with_time_zone(resolved.clone())
        .date()
        .tomorrow()
        .ok()?;
    let next = midnight(tomorrow, &resolved)?;
    Some((next.as_millisecond() - now.timestamp().as_millisecond() + 1).max(1))
}

/// From midnight of the range's first day in the zone (today, Monday, the 1st, or January 1st), to now.
pub fn range_for(kind: RangeKind, now: &Zoned, zone: &str) -> Option<UsageRange> {
    let resolved = zone_of(zone)?;
    let today = now.with_time_zone(resolved.clone()).date();
    let first = match kind {
        RangeKind::Today => today,
        RangeKind::Week => {
            // Monday is the first day of the week.
            let since_monday = i64::from(today.weekday().to_monday_zero_offset());
            today.checked_sub(since_monday.days()).ok()?
        }
        RangeKind::Month => today.first_of_month(),
        RangeKind::Year => today.first_of_year(),
    };
    Some(UsageRange {
        from: iso(midnight(first, &resolved)?),
        to: iso(now.timestamp()),
    })
}

/// The query for a range and filters. Filters with an empty value are left out.
pub fn usage_query(
    kind: RangeKind,
    filters: &UsageFilters,
    now: &Zoned,
    zone: &str,
) -> Option<UsageQuery> {
    let range = range_for(kind, now, zone)?;
    Some(UsageQuery {
        from: range.from,
        to: range.to,
        time_zone: WireZone::new(zone).ok()?,
        bot_id: present(&filters.bot),
        account_id: present(&filters.account),
        provider: None,
        model: present(&filters.model),
    })
}

/// The Usage screen's address for some filters: `/settings/?section=usage&bot=...`, with the query encoded the way
/// `URLSearchParams` encodes it.
pub fn usage_href(filters: &UsageFilters) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("section", "usage");
    for (key, value) in [
        ("bot", &filters.bot),
        ("account", &filters.account),
        ("model", &filters.model),
    ] {
        if let Some(value) = present(value) {
            query.append_pair(key, &value);
        }
    }
    format!("/settings/?{}", query.finish())
}

/// The filters in an address's query, from its `bot`, `account` and `model` values; empty ones are ignored.
pub fn filters_from(get: impl Fn(&str) -> Option<String>) -> UsageFilters {
    UsageFilters {
        bot: present(&get("bot")),
        account: present(&get("account")),
        model: present(&get("model")),
    }
}

/// "950", "12.4K", "1.2M", "3.4B".
pub fn tokens_text(count: u64) -> String {
    // `Math.round((count / divisor) * 10) / 10`: one decimal place, halves up.
    let scaled = |divisor: f64, suffix: &str| {
        let tenths = ((count as f64 / divisor) * 10.0 + 0.5).floor() / 10.0;
        format!("{tenths}{suffix}")
    };
    if count >= 999_950_000 {
        scaled(1e9, "B")
    } else if count >= 999_950 {
        scaled(1e6, "M")
    } else if count >= 1000 {
        scaled(1e3, "K")
    } else {
        count.to_string()
    }
}

/// A dollar amount to cents: "$1.20".
pub fn usd(amount: f64) -> String {
    // `toFixed(2)` rounds an exact tie up ($0.125 is "$0.13"); Rust's `{:.2}` rounds it to even. An exact tie is an odd
    // eighth (`amount * 8` is then an odd whole number, and multiplying by 8 is exact), so it's nudged up one unit in
    // the last place first. Costs are never negative.
    let eighths = amount * 8.0;
    let tie = amount >= 0.0 && eighths.fract() == 0.0 && eighths % 2.0 == 1.0;
    let value = if tie {
        f64::from_bits(amount.to_bits() + 1)
    } else {
        amount
    };
    format!("${value:.2}")
}

/// A cost as words, with a note when it needs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostText {
    pub text: String,
    pub note: Option<String>,
}

/// A cost as words. A total that isn't known is never shown as $0: with some prices missing it is "At least", with none,
/// "Unknown".
pub fn cost_text(cost: &UsageCost) -> CostText {
    if cost.known_records + cost.unknown_records == 0 {
        return CostText {
            text: usd(0.0),
            note: None,
        };
    }
    if let Some(total) = cost.usd {
        let note = cost
            .estimated_usd
            .filter(|estimate| *estimate > 0.0)
            .map(|_| "Includes estimates".to_owned());
        return CostText {
            text: usd(total),
            note,
        };
    }
    if let Some(known) = cost.known_usd {
        let turns = if cost.unknown_records == 1 {
            "turn has"
        } else {
            "turns have"
        };
        return CostText {
            text: format!("At least {}", usd(known)),
            note: Some(format!("{} {turns} no price", cost.unknown_records)),
        };
    }
    CostText {
        text: "Unknown".to_owned(),
        note: None,
    }
}

/// The three kinds of cost added into one figure for the combined estimate: what the API keys spent, what the
/// subscriptions' tokens would have cost on the API, and what is unclassified. It is exact when every turn was priced,
/// "At least" when some were not, and unknown when none were. Tokens are never summed here, and no turn counts twice.
pub fn combined_cost(costs: &UsageCosts) -> UsageCost {
    let all = [&costs.api_spend, &costs.api_value, &costs.unclassified];
    let known_records: u64 = all.iter().map(|each| each.known_records).sum();
    let unknown_records: u64 = all.iter().map(|each| each.unknown_records).sum();
    // A fold from 0.0, not `sum()`, so an empty total is +0.0 and not -0.0.
    let known_usd = all
        .iter()
        .fold(0.0, |sum, each| sum + each.known_usd.unwrap_or(0.0));
    let estimated = all
        .iter()
        .fold(0.0, |sum, each| sum + each.estimated_usd.unwrap_or(0.0));
    UsageCost {
        usd: (unknown_records == 0).then_some(known_usd),
        known_usd: (known_records > 0).then_some(known_usd),
        known_records,
        unknown_records,
        reported_usd: None,
        estimated_usd: (estimated > 0.0).then_some(estimated),
    }
}

/// One cost line of an aggregate: its label, its cost as words, and its note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostLine {
    pub label: &'static str,
    pub text: String,
    pub note: Option<String>,
}

fn has_records(cost: &UsageCost) -> bool {
    cost.known_records + cost.unknown_records > 0
}

/// The cost lines of one aggregate: API spend and API value stay apart, and are never added together.
pub fn cost_lines(aggregate: &UsageAggregate) -> Vec<CostLine> {
    let costs = &aggregate.costs;
    [
        ("Spent", &costs.api_spend),
        ("API value", &costs.api_value),
        ("Other", &costs.unclassified),
    ]
    .into_iter()
    .filter(|(_, cost)| has_records(cost))
    .map(|(label, cost)| {
        let CostText { text, note } = cost_text(cost);
        CostLine { label, text, note }
    })
    .collect()
}

/// A cost after its label: "Spent at least $2.10", not "Spent At least $2.10".
pub fn labelled(label: &str, text: &str) -> String {
    match text.strip_prefix("At least") {
        Some(rest) => format!("{label} at least{rest}"),
        None => format!("{label} {text}"),
    }
}

/// One short line for a card: "12.4K tokens · $1.20 spent" (the cost of whichever kind it has).
pub fn summary_line(aggregate: &UsageAggregate) -> String {
    let lines = cost_lines(aggregate);
    let cost = if lines.is_empty() {
        usd(0.0)
    } else {
        lines
            .iter()
            .map(|line| format!("{} {}", line.text, line.label.to_lowercase()))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    format!("{} tokens · {cost}", tokens_text(aggregate.tokens.total))
}

/// The calendar days from `from` to `to` in `zone`, as "2026-10-19", a day with no usage still a day. Empty when a time
/// doesn't parse or the zone isn't one this build knows.
pub fn days_between(from: &str, to: &str, zone: &str) -> Vec<String> {
    let (Some(zone), Ok(first), Ok(last)) = (
        zone_of(zone),
        from.parse::<Timestamp>(),
        to.parse::<Timestamp>(),
    ) else {
        return Vec::new();
    };
    let last = day_in(last, &zone);
    let mut days = Vec::new();
    let mut day = day_in(first, &zone);
    while day <= last && days.len() < MAX_DAYS {
        days.push(day.to_string());
        match day.tomorrow() {
            Ok(next) => day = next,
            Err(_) => break,
        }
    }
    days
}

/// A day with no usage at all: the padding for a quiet day.
fn none() -> UsageAggregate {
    let no_cost = || UsageCost {
        usd: Some(0.0),
        known_usd: Some(0.0),
        known_records: 0,
        unknown_records: 0,
        reported_usd: None,
        estimated_usd: None,
    };
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
            api_spend: no_cost(),
            api_value: no_cost(),
            unclassified: no_cost(),
        },
    }
}

/// The report's days with a slot for every calendar day in its range, the quiet ones empty. Empty for a zone this build
/// doesn't know.
pub fn padded_daily(report: &UsageReport) -> Vec<DailyUsage> {
    let known: HashMap<&str, &DailyUsage> = report
        .daily
        .iter()
        .map(|day| (day.day.as_str(), day))
        .collect();
    days_between(&report.from, &report.to, report.time_zone.as_str())
        .into_iter()
        .map(|day| match known.get(day.as_str()) {
            Some(found) => (*found).clone(),
            None => DailyUsage { usage: none(), day },
        })
        .collect()
}

/// "Oct 19" for "2026-10-19". Both parts come from a day this module wrote, so both are there.
pub fn day_label(day: &str) -> String {
    let number = day
        .get(8..10)
        .and_then(|digits| digits.parse::<u32>().ok())
        .map_or_else(String::new, |number| number.to_string());
    format!("{} {number}", month_label(day))
}

/// "Oct" for "2026-10" (or for "2026-10-19"). Empty when the month doesn't read.
pub fn month_label(month: &str) -> &'static str {
    month
        .get(5..7)
        .and_then(|digits| digits.parse::<usize>().ok())
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| MONTHS.get(index))
        .copied()
        .unwrap_or("")
}

/// A date under the bars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    pub index: usize,
    pub label: String,
}

/// A few dates to put under the bars: the first, the last, and some between, never closer than a bar-label's width
/// allows. `most` is how many the bars can take (the screen passes 5).
pub fn day_ticks(days: &[String], most: usize) -> Vec<Tick> {
    let Some(last) = days.len().checked_sub(1) else {
        return Vec::new();
    };
    let step = last.div_ceil(most.saturating_sub(1).max(1)).max(1);
    let mut indexes: Vec<usize> = (0..last).step_by(step).collect();
    // The one before the last is dropped when it's too near the last to read.
    if let Some(&previous) = indexes.last()
        && 2 * (last - previous) < step
    {
        indexes.pop();
    }
    indexes.push(last);
    indexes
        .into_iter()
        .filter_map(|index| {
            days.get(index).map(|day| Tick {
                index,
                label: day_label(day),
            })
        })
        .collect()
}

/// A bar: a day (or a month) and its share of the busiest.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub day: String,
    pub share: f64,
}

/// Bars with their label.
#[derive(Debug, Clone, PartialEq)]
pub struct Bars {
    pub bars: Vec<Bar>,
    pub label: String,
}

/// The first of the items with the largest total, as the TypeScript's `reduce` keeps it.
fn busiest<T>(items: &[T], total: impl Fn(&T) -> u64) -> Option<&T> {
    let mut top: Option<&T> = None;
    for item in items {
        if top.is_none_or(|best| total(item) > total(best)) {
            top = Some(item);
        }
    }
    top
}

/// `part` as a share of `whole`; 0 when the whole is 0.
fn share(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// Bars for the days: each day's tokens against the busiest day's.
pub fn daily_bars(daily: &[DailyUsage]) -> Bars {
    let top = busiest(daily, |day| day.usage.tokens.total);
    match top {
        Some(top) if top.usage.tokens.total > 0 => Bars {
            bars: daily
                .iter()
                .map(|day| Bar {
                    day: day.day.clone(),
                    share: share(day.usage.tokens.total, top.usage.tokens.total),
                })
                .collect(),
            label: format!(
                "Tokens per day, highest {} on {}",
                tokens_text(top.usage.tokens.total),
                top.day
            ),
        },
        _ => Bars {
            bars: daily
                .iter()
                .map(|day| Bar {
                    day: day.day.clone(),
                    share: 0.0,
                })
                .collect(),
            label: "No tokens in this range".to_owned(),
        },
    }
}

/// Whether a chart's bars are days or months.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Day,
    Month,
}

/// What the chart draws for a report.
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub bars: Vec<Bar>,
    pub label: String,
    pub ticks: Vec<Tick>,
    pub unit: Unit,
}

/// A month's total tokens.
struct MonthTotal {
    month: String,
    total: u64,
}

/// A year's days added up by month ("2026-10") in the report's own zone: a slot for every month from January to the last
/// one in the range, the empty ones too.
pub fn monthly_bars(days: &[DailyUsage]) -> Chart {
    let mut totals: Vec<MonthTotal> = Vec::new();
    for day in days {
        let month = day.day.get(..7).unwrap_or(day.day.as_str());
        let tokens = day.usage.tokens.total;
        if let Some(last) = totals.last_mut()
            && last.month == month
        {
            last.total += tokens;
        } else {
            totals.push(MonthTotal {
                month: month.to_owned(),
                total: tokens,
            });
        }
    }
    let top = busiest(totals.as_slice(), |each| each.total);
    let bars = totals
        .iter()
        .map(|each| Bar {
            day: each.month.clone(),
            share: top.map_or(0.0, |top| share(each.total, top.total)),
        })
        .collect();
    let ticks = totals
        .iter()
        .enumerate()
        .map(|(index, each)| Tick {
            index,
            label: month_label(&each.month).to_owned(),
        })
        .collect();
    let label = match top {
        Some(top) if top.total > 0 => format!(
            "Tokens per month, highest {} in {}",
            tokens_text(top.total),
            month_label(&top.month)
        ),
        _ => "No tokens in this range".to_owned(),
    };
    Chart {
        bars,
        label,
        ticks,
        unit: Unit::Month,
    }
}

/// What the chart draws for a report: days for a week or a month (a slot for every calendar day, with a few dates under
/// them), months for a year.
pub fn chart_data(report: &UsageReport, range: RangeKind) -> Chart {
    let days = padded_daily(report);
    if range == RangeKind::Year {
        return monthly_bars(&days);
    }
    let bars = daily_bars(&days);
    let labels: Vec<String> = days.iter().map(|day| day.day.clone()).collect();
    Chart {
        bars: bars.bars,
        label: bars.label,
        ticks: day_ticks(&labels, DAY_TICKS),
        unit: Unit::Day,
    }
}

/// A breakdown row: its key, its name, and its usage.
#[derive(Debug, Clone, PartialEq)]
pub struct Row<'a> {
    pub key: String,
    pub name: String,
    pub aggregate: &'a UsageAggregate,
}

fn provider_key(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

/// The breakdown rows, busiest first, with a name for something that has been deleted.
pub fn breakdown_rows<'a>(
    report: &'a UsageReport,
    by: By,
    bots: &[Bot],
    accounts: &[AccountInfo],
) -> Vec<Row<'a>> {
    let mut rows: Vec<Row<'a>> = match by {
        By::Bot => report
            .by_bot
            .iter()
            .map(|row| Row {
                key: row.bot_id.clone(),
                name: bots
                    .iter()
                    .find(|bot| bot.id == row.bot_id)
                    .map_or_else(|| "Deleted agent".to_owned(), |bot| bot.name.clone()),
                aggregate: &row.usage,
            })
            .collect(),
        By::Account => report
            .by_account
            .iter()
            .map(|row| Row {
                key: row.account_id.clone(),
                name: accounts
                    .iter()
                    .find(|account| account.id == row.account_id)
                    .map_or_else(
                        || "Removed account".to_owned(),
                        |account| account.label.clone(),
                    ),
                aggregate: &row.usage,
            })
            .collect(),
        By::Model => report
            .by_model
            .iter()
            .map(|row| Row {
                key: format!("{}:{}", provider_key(row.provider), row.model),
                name: row.model.clone(),
                aggregate: &row.usage,
            })
            .collect(),
    };
    // Stable, so rows with equal totals keep the order the report gave them.
    rows.sort_by_key(|row| std::cmp::Reverse(row.aggregate.tokens.total));
    rows
}

/// An account card's usage, "12.4K tokens · $3.10 API value": a subscription shows what its tokens would have cost on the
/// API, an API key what it spent. Never one added to the other.
pub fn account_usage_text(aggregate: &UsageAggregate, kind: AccountKind) -> String {
    let subscription = kind == AccountKind::Subscription;
    let cost = cost_text(if subscription {
        &aggregate.costs.api_value
    } else {
        &aggregate.costs.api_spend
    });
    format!(
        "{} tokens · {} {}",
        tokens_text(aggregate.tokens.total),
        cost.text,
        if subscription { "API value" } else { "spent" }
    )
}

/// A bot's day for its panel: "Today: 12.4K tokens · Spent $1.20" (the cost of whichever kind it has, or $0.00).
pub fn bot_day_text(aggregate: &UsageAggregate) -> String {
    let lines = cost_lines(aggregate);
    let cost = if lines.is_empty() {
        usd(0.0)
    } else {
        lines
            .iter()
            .map(|line| labelled(line.label, &line.text))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    format!(
        "Today: {} tokens · {cost}",
        tokens_text(aggregate.tokens.total)
    )
}

/// A held value and the key of the spec it answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Held<T> {
    pub key: String,
    pub value: T,
}

/// What was read, shown only for the spec it answered: another range or filter has no report (not the last one's) until
/// its own arrives.
pub fn matching<'a, T>(held: Option<&'a Held<T>>, key: Option<&str>) -> Option<&'a T> {
    held.filter(|held| Some(held.key.as_str()) == key)
        .map(|held| &held.value)
}

/// Runs a job at most once per `ms` while poked, never later than `ms` after the first poke: more pokes while one is
/// waiting change nothing, so a steady stream of events can't push the run off for ever (a debounce would). The
/// TypeScript's timer and callback are the caller's here: time is passed in as milliseconds, and the caller runs the job
/// when `take_due` says so (the desktop's timer does that when it fires).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Throttle {
    ms: u64,
    due: Option<u64>,
}

impl Throttle {
    pub fn new(ms: u64) -> Self {
        Self { ms, due: None }
    }

    /// A poke at `now`: a run becomes due `ms` later, unless one already is.
    pub fn poke(&mut self, now: u64) {
        if self.due.is_none() {
            self.due = Some(now.saturating_add(self.ms));
        }
    }

    /// Whether a run is due at `now`. Taking it clears it.
    pub fn take_due(&mut self, now: u64) -> bool {
        match self.due {
            Some(due) if due <= now => {
                self.due = None;
                true
            }
            _ => false,
        }
    }

    /// Drops a run that's waiting.
    pub fn cancel(&mut self) {
        self.due = None;
    }
}

/// Whether the report has no records.
pub fn empty_usage(report: &UsageReport) -> bool {
    report.summary.records == 0
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    use protocol::accounts::{AccountUsage, BotUsage, ModelUsage, UsageTokens};
    use protocol::model::{Bot, Provider, TimeZone as WireZone};
    use serde_json::json;

    use super::*;

    const JOHANNESBURG: &str = "Africa/Johannesburg";
    const NEW_YORK: &str = "America/New_York";

    fn now_at(iso: &str) -> Zoned {
        iso.parse::<Timestamp>().unwrap().to_zoned(TimeZone::UTC)
    }

    fn millis(iso: &str) -> i64 {
        iso.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn range(kind: RangeKind, now: &str, zone: &str) -> UsageRange {
        range_for(kind, &now_at(now), zone).unwrap()
    }

    fn filters(bot: Option<&str>, account: Option<&str>, model: Option<&str>) -> UsageFilters {
        UsageFilters {
            bot: bot.map(str::to_owned),
            account: account.map(str::to_owned),
            model: model.map(str::to_owned),
        }
    }

    /// The filters an address's query carries, read the way the screen reads it.
    fn read_back(query: &str) -> UsageFilters {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        filters_from(|name| {
            pairs
                .iter()
                .find(|(key, _)| key.as_str() == name)
                .map(|(_, value)| value.clone())
        })
    }

    /// A cost with nothing in it: `$0.00` known, no records.
    fn cost() -> UsageCost {
        UsageCost {
            usd: Some(0.0),
            known_usd: Some(0.0),
            known_records: 0,
            unknown_records: 0,
            reported_usd: None,
            estimated_usd: None,
        }
    }

    fn costs(api_spend: UsageCost, api_value: UsageCost, unclassified: UsageCost) -> UsageCosts {
        UsageCosts {
            api_spend,
            api_value,
            unclassified,
        }
    }

    /// An aggregate of `total` tokens, one record when there are any.
    fn aggregate_with(total: u64, costs: UsageCosts) -> UsageAggregate {
        UsageAggregate {
            records: u64::from(total > 0),
            tokens: UsageTokens {
                input: total,
                cached_input: 0,
                output: 0,
                reasoning: 0,
                total,
            },
            costs,
        }
    }

    fn aggregate(total: u64) -> UsageAggregate {
        aggregate_with(total, costs(cost(), cost(), cost()))
    }

    fn day(date: &str, total: u64) -> DailyUsage {
        DailyUsage {
            usage: aggregate(total),
            day: date.to_owned(),
        }
    }

    fn report_with(from: &str, to: &str, zone: &str, daily: Vec<DailyUsage>) -> UsageReport {
        UsageReport {
            from: from.to_owned(),
            to: to.to_owned(),
            time_zone: WireZone::new(zone).unwrap(),
            summary: aggregate(0),
            daily,
            by_bot: Vec::new(),
            by_account: Vec::new(),
            by_model: Vec::new(),
        }
    }

    /// A report from `(day, tokens)` pairs.
    fn year(from: &str, to: &str, zone: &str, daily: &[(&str, u64)]) -> UsageReport {
        report_with(
            from,
            to,
            zone,
            daily
                .iter()
                .map(|(date, total)| day(date, *total))
                .collect(),
        )
    }

    fn bot(id: &str, name: &str) -> Bot {
        serde_json::from_value(json!({
            "id": id, "name": name, "color": "orange", "shape": "circle", "instructions": "",
            "provider": "claude", "model": "sonnet", "autonomy": "ask", "homeFolder": "C:/b",
            "projectFolder": null, "browser": false, "paused": false, "canMessage": [],
            "state": "idle", "setupReason": null, "limitedUntil": null, "createdAt": "2026-09-27T00:00:00Z"
        }))
        .unwrap()
    }

    fn account(id: &str, label: &str, kind: &str) -> AccountInfo {
        serde_json::from_value(json!({
            "id": id, "provider": "claude", "label": label, "kind": kind, "isDefault": false,
            "isFallback": false, "spendingCapUsd": null, "createdAt": "2026-09-27T00:00:00Z",
            "hasCredential": true, "setup": null, "limits": [], "signIn": null,
            "spending": {"knownUsd": 0.0, "unknownRows": 0, "capUsd": null, "blocked": false}
        }))
        .unwrap()
    }

    /// The names of breakdown rows, in order.
    fn names(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn changes_identity_at_local_midnight_and_on_timezone_changes() {
        let before = now_at("2026-10-22T21:59:59Z");
        assert_eq!(until_usage_day(&before, JOHANNESBURG), Some(1001));
        assert_ne!(
            usage_day_key(&before, JOHANNESBURG),
            usage_day_key(&now_at("2026-10-22T22:00:00Z"), JOHANNESBURG)
        );
        assert_ne!(
            usage_day_key(&before, "UTC"),
            usage_day_key(&before, JOHANNESBURG)
        );
    }

    #[test]
    fn schedules_midnight_across_23_hour_and_25_hour_daylight_saving_days() {
        assert_eq!(
            until_usage_day(&now_at("2026-03-08T05:00:00Z"), NEW_YORK),
            Some(23 * 3_600_000 + 1)
        );
        assert_eq!(
            until_usage_day(&now_at("2026-11-01T04:00:00Z"), NEW_YORK),
            Some(25 * 3_600_000 + 1)
        );
    }

    #[test]
    fn starts_today_at_the_zones_midnight_not_utcs() {
        // 2026-10-22 01:30 in Johannesburg (UTC+2) is still the 21st in UTC.
        let now = "2026-10-21T23:30:00Z";
        assert_eq!(
            range(RangeKind::Today, now, JOHANNESBURG),
            UsageRange {
                from: "2026-10-21T22:00:00.000Z".to_owned(),
                to: "2026-10-21T23:30:00.000Z".to_owned(),
            }
        );
        assert_eq!(
            range(RangeKind::Today, now, "UTC").from,
            "2026-10-21T00:00:00.000Z"
        );
    }

    #[test]
    fn starts_the_year_on_january_1st_at_the_zones_midnight_to_now() {
        let now = "2026-10-22T10:00:00Z";
        assert_eq!(
            range(RangeKind::Year, now, "UTC"),
            UsageRange {
                from: "2026-01-01T00:00:00.000Z".to_owned(),
                to: "2026-10-22T10:00:00.000Z".to_owned(),
            }
        );
        assert_eq!(
            range(RangeKind::Year, now, JOHANNESBURG).from,
            "2025-12-31T22:00:00.000Z"
        );
        // New York in winter: EST, not October's offset.
        assert_eq!(
            range(RangeKind::Year, now, NEW_YORK).from,
            "2026-01-01T05:00:00.000Z"
        );
        // Still the 31st there.
        assert_eq!(
            range(RangeKind::Year, "2026-01-01T00:30:00Z", NEW_YORK).from,
            "2025-01-01T05:00:00.000Z"
        );
        let late = range(RangeKind::Year, "2026-12-31T23:00:00Z", "UTC");
        // Inside what the daemon takes: at most 366 days.
        assert!(millis(&late.to) - millis(&late.from) <= 366 * 86_400_000);
    }

    #[test]
    fn starts_the_week_on_monday_and_the_month_on_the_1st() {
        let now = "2026-10-22T10:00:00Z"; // a Thursday
        assert_eq!(
            range(RangeKind::Week, now, "UTC").from,
            "2026-10-19T00:00:00.000Z"
        );
        assert_eq!(
            range(RangeKind::Month, now, "UTC").from,
            "2026-10-01T00:00:00.000Z"
        );
        // A Sunday belongs to the week that began the Monday before.
        assert_eq!(
            range(RangeKind::Week, "2026-10-18T10:00:00Z", "UTC").from,
            "2026-10-12T00:00:00.000Z"
        );
    }

    #[test]
    fn lands_on_local_midnight_across_a_daylight_saving_change() {
        // New York: the clocks went back on 2026-11-01, so the month starts at EDT midnight.
        let now = "2026-11-03T15:00:00Z";
        assert_eq!(
            range(RangeKind::Month, now, NEW_YORK).from,
            "2026-11-01T04:00:00.000Z"
        );
        // EST by then.
        assert_eq!(
            range(RangeKind::Today, now, NEW_YORK).from,
            "2026-11-03T05:00:00.000Z"
        );
        // Spring forward on 2026-03-08: that midnight is still EST, the next day's is EDT.
        assert_eq!(
            range(RangeKind::Today, "2026-03-08T20:00:00Z", NEW_YORK).from,
            "2026-03-08T05:00:00.000Z"
        );
        assert_eq!(
            range(RangeKind::Today, "2026-03-09T20:00:00Z", NEW_YORK).from,
            "2026-03-09T04:00:00.000Z"
        );
    }

    #[test]
    fn builds_a_query_with_the_zone_and_only_the_filters_set() {
        let now = "2026-10-22T10:00:00Z";
        let query = usage_query(
            RangeKind::Today,
            &UsageFilters::default(),
            &now_at(now),
            "UTC",
        )
        .unwrap();
        assert_eq!(query.from, "2026-10-22T00:00:00.000Z");
        assert_eq!(query.to, "2026-10-22T10:00:00.000Z");
        assert_eq!(query.time_zone.as_str(), "UTC");
        assert_eq!(
            (&query.bot_id, &query.account_id, &query.model),
            (&None, &None, &None)
        );
        let filtered = usage_query(
            RangeKind::Today,
            &filters(Some("b1"), Some("a1"), Some("m")),
            &now_at(now),
            "UTC",
        )
        .unwrap();
        assert_eq!(filtered.bot_id.as_deref(), Some("b1"));
        assert_eq!(filtered.account_id.as_deref(), Some("a1"));
        assert_eq!(filtered.model.as_deref(), Some("m"));
    }

    #[test]
    fn writes_the_address_for_some_filters_and_reads_it_back() {
        assert_eq!(
            usage_href(&UsageFilters::default()),
            "/settings/?section=usage"
        );
        let href = usage_href(&filters(Some("b 1"), None, Some("gpt")));
        assert_eq!(href, "/settings/?section=usage&bot=b+1&model=gpt");
        let query = href.split_once('?').map_or("", |(_, query)| query);
        assert_eq!(read_back(query), filters(Some("b 1"), None, Some("gpt")));
    }

    #[test]
    fn reads_the_filters_from_the_address_ignoring_empty_ones() {
        assert_eq!(
            read_back("section=usage&bot=b1&account=&model=gpt"),
            filters(Some("b1"), None, Some("gpt"))
        );
    }

    #[test]
    fn shortens_counts_950_12_4k_1_2m_3_4b() {
        assert_eq!(
            [950, 12_400, 1_200_000, 3_400_000_000, 999_999].map(tokens_text),
            ["950", "12.4K", "1.2M", "3.4B", "1M"]
        );
    }

    #[test]
    fn is_0_00_only_when_there_is_nothing_and_never_shows_an_unknown_cost_as_0() {
        assert_eq!(
            cost_text(&cost()),
            CostText {
                text: "$0.00".to_owned(),
                note: None
            }
        );
        assert_eq!(
            cost_text(&UsageCost {
                usd: Some(1.2),
                known_records: 3,
                ..cost()
            }),
            CostText {
                text: "$1.20".to_owned(),
                note: None
            }
        );
        assert_eq!(
            cost_text(&UsageCost {
                usd: Some(1.2),
                known_records: 3,
                estimated_usd: Some(0.4),
                ..cost()
            }),
            CostText {
                text: "$1.20".to_owned(),
                note: Some("Includes estimates".to_owned())
            }
        );
        assert_eq!(
            cost_text(&UsageCost {
                usd: None,
                known_usd: Some(0.5),
                known_records: 1,
                unknown_records: 2,
                ..cost()
            }),
            CostText {
                text: "At least $0.50".to_owned(),
                note: Some("2 turns have no price".to_owned())
            }
        );
        assert_eq!(
            cost_text(&UsageCost {
                usd: None,
                known_usd: None,
                unknown_records: 4,
                ..cost()
            }),
            CostText {
                text: "Unknown".to_owned(),
                note: None
            }
        );
    }

    #[test]
    fn keeps_spend_and_api_value_on_lines_of_their_own_and_never_adds_them() {
        let both = aggregate_with(
            1000,
            costs(
                UsageCost {
                    usd: Some(2.0),
                    known_records: 1,
                    ..cost()
                },
                UsageCost {
                    usd: Some(7.0),
                    known_records: 2,
                    ..cost()
                },
                cost(),
            ),
        );
        let lines: Vec<String> = cost_lines(&both)
            .iter()
            .map(|line| format!("{} {}", line.label, line.text))
            .collect();
        assert_eq!(lines, ["Spent $2.00", "API value $7.00"]);
        assert!(cost_lines(&aggregate(0)).is_empty());
        assert_eq!(
            summary_line(&both),
            "1K tokens · $2.00 spent · $7.00 api value"
        );
        assert_eq!(summary_line(&aggregate(0)), "0 tokens · $0.00");
    }

    #[test]
    fn sizes_each_day_against_the_busiest_and_says_the_highest_in_words() {
        let days = [
            day("2026-10-20", 500),
            day("2026-10-21", 2000),
            day("2026-10-22", 0),
        ];
        let bars = daily_bars(&days);
        let shares: Vec<f64> = bars.bars.iter().map(|bar| bar.share).collect();
        assert_eq!(shares, [0.25, 1.0, 0.0]);
        assert_eq!(bars.label, "Tokens per day, highest 2K on 2026-10-21");
        assert!(daily_bars(&[]).bars.is_empty());
        assert_eq!(
            daily_bars(&[day("2026-10-22", 0)]).label,
            "No tokens in this range"
        );
    }

    #[test]
    fn lists_rows_busiest_first_naming_what_was_deleted() {
        let mut report = report_with(
            "2026-10-19T00:00:00Z",
            "2026-10-22T00:00:00Z",
            "UTC",
            vec![],
        );
        report.by_bot = vec![
            BotUsage {
                usage: aggregate(10),
                bot_id: "gone".to_owned(),
            },
            BotUsage {
                usage: aggregate(900),
                bot_id: "b1".to_owned(),
            },
        ];
        report.by_account = vec![
            AccountUsage {
                usage: aggregate(5),
                account_id: "a9".to_owned(),
            },
            AccountUsage {
                usage: aggregate(50),
                account_id: "a1".to_owned(),
            },
        ];
        report.by_model = vec![ModelUsage {
            usage: aggregate(7),
            provider: Provider::Claude,
            model: "sonnet".to_owned(),
        }];
        assert_eq!(
            names(&breakdown_rows(
                &report,
                By::Bot,
                &[bot("b1", "Scout")],
                &[]
            )),
            ["Scout", "Deleted agent"]
        );
        assert_eq!(
            names(&breakdown_rows(
                &report,
                By::Account,
                &[],
                &[account("a1", "Personal", "subscription")]
            )),
            ["Personal", "Removed account"]
        );
        assert_eq!(
            names(&breakdown_rows(&report, By::Model, &[], &[])),
            ["sonnet"]
        );
    }

    #[test]
    fn is_empty_when_there_are_no_records() {
        let mut report = report_with(
            "2026-10-19T00:00:00Z",
            "2026-10-22T00:00:00Z",
            "UTC",
            vec![],
        );
        report.summary = aggregate(0);
        assert!(empty_usage(&report));
        report.summary = aggregate(10);
        assert!(!empty_usage(&report));
    }

    #[test]
    fn shows_a_subscriptions_api_value_and_an_api_keys_spend_never_both() {
        let both = aggregate_with(
            12_400,
            costs(
                UsageCost {
                    usd: Some(3.1),
                    known_records: 1,
                    ..cost()
                },
                UsageCost {
                    usd: Some(8.0),
                    known_records: 2,
                    ..cost()
                },
                cost(),
            ),
        );
        assert_eq!(
            account_usage_text(&both, AccountKind::Subscription),
            "12.4K tokens · $8.00 API value"
        );
        assert_eq!(
            account_usage_text(&both, AccountKind::ApiKey),
            "12.4K tokens · $3.10 spent"
        );
        assert_eq!(
            account_usage_text(&aggregate(0), AccountKind::ApiKey),
            "0 tokens · $0.00 spent"
        );
    }

    #[test]
    fn shows_a_bots_day_with_the_cost_it_has_or_0_00_for_nothing() {
        let both = aggregate_with(
            12_400,
            costs(
                UsageCost {
                    usd: Some(3.1),
                    known_records: 1,
                    ..cost()
                },
                UsageCost {
                    usd: Some(8.0),
                    known_records: 2,
                    ..cost()
                },
                cost(),
            ),
        );
        assert_eq!(
            bot_day_text(&both),
            "Today: 12.4K tokens · Spent $3.10 · API value $8.00"
        );
        // A lone subscription value is still labelled.
        let value_only = aggregate_with(
            900,
            costs(
                cost(),
                UsageCost {
                    usd: Some(4.0),
                    known_records: 1,
                    ..cost()
                },
                cost(),
            ),
        );
        assert_eq!(
            bot_day_text(&value_only),
            "Today: 900 tokens · API value $4.00"
        );
        assert_eq!(bot_day_text(&aggregate(0)), "Today: 0 tokens · $0.00");
    }

    #[test]
    fn shows_a_held_report_only_for_the_spec_it_answered() {
        let held = Held {
            key: "week".to_owned(),
            value: "report",
        };
        assert_eq!(matching(Some(&held), Some("week")), Some(&"report"));
        // Another range doesn't show the last one's totals.
        assert_eq!(matching(Some(&held), Some("month")), None);
        assert_eq!(matching(None::<&Held<&str>>, Some("week")), None);
        assert_eq!(matching(Some(&held), None), None);
    }

    #[test]
    fn runs_once_per_interval_under_a_steady_stream_of_pokes_never_pushed_off() {
        // Pokes every 100 ms for 3.5 s, with the timer checked 100 ms after each poke, as the TypeScript test advances
        // its fake clock.
        let mut throttle = Throttle::new(1000);
        let mut runs = 0;
        for now in (0..3500).step_by(100) {
            throttle.poke(now);
            if throttle.take_due(now + 100) {
                runs += 1;
            }
        }
        // A debounce would not have run at all.
        assert_eq!(runs, 3);
    }

    #[test]
    fn does_not_run_without_a_poke_and_cancel_drops_a_waiting_run() {
        let mut throttle = Throttle::new(1000);
        assert!(!throttle.take_due(5000));
        throttle.poke(5000);
        throttle.cancel();
        assert!(!throttle.take_due(10_000));
    }

    #[test]
    fn lists_every_calendar_day_from_the_first_to_the_last_in_the_zone_empty_ones_too() {
        assert_eq!(
            days_between("2026-10-19T00:00:00Z", "2026-10-22T10:00:00Z", "UTC"),
            ["2026-10-19", "2026-10-20", "2026-10-21", "2026-10-22"]
        );
        assert_eq!(
            days_between("2026-10-22T10:00:00Z", "2026-10-22T11:00:00Z", "UTC"),
            ["2026-10-22"]
        );
    }

    #[test]
    fn reads_the_days_in_the_reports_zone_not_utcs() {
        // 22:00Z on the 21st is already the 22nd in Johannesburg.
        assert_eq!(
            days_between("2026-10-21T22:00:00Z", "2026-10-22T10:00:00Z", JOHANNESBURG),
            ["2026-10-22"]
        );
    }

    #[test]
    fn counts_a_25_hour_and_a_23_hour_day_as_one_day_each_across_daylight_saving() {
        assert_eq!(
            days_between("2026-11-01T04:00:00Z", "2026-11-03T15:00:00Z", NEW_YORK),
            ["2026-11-01", "2026-11-02", "2026-11-03"]
        );
        assert_eq!(
            days_between("2026-03-07T05:00:00Z", "2026-03-10T20:00:00Z", NEW_YORK),
            ["2026-03-07", "2026-03-08", "2026-03-09", "2026-03-10"]
        );
    }

    #[test]
    fn stays_bounded_for_a_bad_range() {
        assert!(days_between("2020-01-01T00:00:00Z", "2030-01-01T00:00:00Z", "UTC").len() <= 400);
        assert!(days_between("2026-10-22T00:00:00Z", "2026-10-01T00:00:00Z", "UTC").is_empty());
    }

    #[test]
    fn pads_a_reports_days_so_one_busy_day_takes_its_own_slot_not_the_whole_range() {
        let report = report_with(
            "2026-10-19T00:00:00Z",
            "2026-10-25T10:00:00Z",
            "UTC",
            vec![day("2026-10-21", 900)],
        );
        let padded = padded_daily(&report);
        let days: Vec<&str> = padded.iter().map(|day| day.day.as_str()).collect();
        assert_eq!(
            days,
            [
                "2026-10-19",
                "2026-10-20",
                "2026-10-21",
                "2026-10-22",
                "2026-10-23",
                "2026-10-24",
                "2026-10-25"
            ]
        );
        let totals: Vec<u64> = padded.iter().map(|day| day.usage.tokens.total).collect();
        assert_eq!(totals, [0, 0, 900, 0, 0, 0, 0]);
        let bars = daily_bars(&padded).bars;
        assert_eq!(bars.len(), 7);
        assert_eq!(bars.iter().filter(|bar| bar.share > 0.0).count(), 1);
    }

    #[test]
    fn puts_modest_date_ticks_under_the_bars_the_first_the_last_and_a_few_between() {
        let days = days_between("2026-10-01T00:00:00Z", "2026-10-31T10:00:00Z", "UTC");
        let ticks = day_ticks(&days, 5);
        assert_eq!(
            ticks[0],
            Tick {
                index: 0,
                label: "Oct 1".to_owned()
            }
        );
        assert_eq!(
            ticks[ticks.len() - 1],
            Tick {
                index: 30,
                label: "Oct 31".to_owned()
            }
        );
        assert!(ticks.len() <= 6);
        assert_eq!(
            day_ticks(&["2026-10-22".to_owned()], 5),
            [Tick {
                index: 0,
                label: "Oct 22".to_owned()
            }]
        );
        assert!(day_ticks(&[], 5).is_empty());
        assert_eq!(day_label("2026-03-09"), "Mar 9");
    }

    #[test]
    fn adds_the_known_costs_of_spend_value_and_unclassified_once_each() {
        let sum = combined_cost(&costs(
            UsageCost {
                usd: Some(3.0),
                known_usd: Some(3.0),
                known_records: 2,
                ..cost()
            },
            UsageCost {
                usd: Some(8.0),
                known_usd: Some(8.0),
                known_records: 4,
                ..cost()
            },
            UsageCost {
                usd: Some(1.0),
                known_usd: Some(1.0),
                known_records: 1,
                ..cost()
            },
        ));
        assert_eq!(sum.usd, Some(12.0));
        assert_eq!(sum.known_usd, Some(12.0));
        assert_eq!(sum.known_records, 7);
        assert_eq!(sum.unknown_records, 0);
        assert_eq!(cost_text(&sum).text, "$12.00");
    }

    #[test]
    fn says_at_least_when_some_turns_have_no_price_and_unknown_when_none_do() {
        let partial = combined_cost(&costs(
            UsageCost {
                usd: None,
                known_usd: Some(3.0),
                known_records: 2,
                unknown_records: 1,
                ..cost()
            },
            UsageCost {
                usd: Some(8.0),
                known_usd: Some(8.0),
                known_records: 4,
                ..cost()
            },
            cost(),
        ));
        assert_eq!(partial.usd, None);
        assert_eq!(partial.known_usd, Some(11.0));
        assert_eq!(partial.unknown_records, 1);
        assert_eq!(
            cost_text(&partial),
            CostText {
                text: "At least $11.00".to_owned(),
                note: Some("1 turn has no price".to_owned())
            }
        );
        let none = combined_cost(&costs(
            UsageCost {
                usd: None,
                known_usd: None,
                unknown_records: 2,
                ..cost()
            },
            UsageCost {
                usd: None,
                known_usd: None,
                unknown_records: 3,
                ..cost()
            },
            cost(),
        ));
        assert_eq!(none.usd, None);
        assert_eq!(none.known_usd, None);
        assert_eq!(none.unknown_records, 5);
        assert_eq!(cost_text(&none).text, "Unknown");
    }

    #[test]
    fn is_0_00_for_nothing_at_all_and_carries_estimates_through() {
        assert_eq!(
            cost_text(&combined_cost(&costs(cost(), cost(), cost()))).text,
            "$0.00"
        );
        let estimated = combined_cost(&costs(
            UsageCost {
                usd: Some(1.0),
                known_usd: Some(1.0),
                known_records: 1,
                estimated_usd: Some(0.4),
                ..cost()
            },
            UsageCost {
                usd: Some(2.0),
                known_usd: Some(2.0),
                known_records: 1,
                estimated_usd: Some(0.1),
                ..cost()
            },
            cost(),
        ));
        assert!((estimated.estimated_usd.unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(
            cost_text(&estimated),
            CostText {
                text: "$3.00".to_owned(),
                note: Some("Includes estimates".to_owned())
            }
        );
    }

    #[test]
    fn draws_twelve_bars_for_a_full_year_however_many_days_it_has_with_every_month_named() {
        let report = year(
            "2026-01-01T00:00:00Z",
            "2026-12-31T23:00:00Z",
            "UTC",
            &[("2026-03-05", 400), ("2026-10-22", 900)],
        );
        let chart = chart_data(&report, RangeKind::Year);
        assert_eq!(chart.unit, Unit::Month);
        assert_eq!(chart.bars.len(), 12);
        let labels: Vec<&str> = chart.ticks.iter().map(|tick| tick.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"
            ]
        );
        assert_eq!(chart.bars[9].share, 1.0);
        assert!((chart.bars[2].share - 400.0 / 900.0).abs() < 1e-9);
        // Quiet months are still slots.
        assert_eq!(chart.bars.iter().filter(|bar| bar.share == 0.0).count(), 10);
        assert_eq!(chart.label, "Tokens per month, highest 900 in Oct");
    }

    #[test]
    fn keeps_a_leap_years_extra_day_in_february_and_the_31st_in_december() {
        let report = year(
            "2028-01-01T00:00:00Z",
            "2028-12-31T23:30:00Z",
            "UTC",
            &[
                ("2028-02-29", 300),
                ("2028-02-28", 100),
                ("2028-12-31", 800),
            ],
        );
        let chart = chart_data(&report, RangeKind::Year);
        // 366 days, still twelve slots.
        assert_eq!(chart.bars.len(), 12);
        // Feb 28 and Feb 29 together.
        assert!((chart.bars[1].share - 400.0 / 800.0).abs() < 1e-9);
        assert_eq!(chart.bars[11].share, 1.0);
    }

    #[test]
    fn stops_at_the_month_of_the_ranges_last_day_and_reads_months_in_the_reports_zone() {
        // In Johannesburg the 31st has already come when UTC is still on the 30th, so December is the last slot.
        let december = chart_data(
            &year(
                "2026-12-30T22:00:00Z",
                "2026-12-31T21:30:00Z",
                JOHANNESBURG,
                &[],
            ),
            RangeKind::Year,
        );
        let labels: Vec<&str> = december
            .ticks
            .iter()
            .map(|tick| tick.label.as_str())
            .collect();
        assert_eq!(labels, ["Dec"]);
        // New York's first moments of January are still December 31st there, so no January slot.
        let edge = chart_data(
            &year(
                "2026-01-01T05:00:00Z",
                "2027-01-01T04:30:00Z",
                NEW_YORK,
                &[],
            ),
            RangeKind::Year,
        );
        assert_eq!(edge.bars.len(), 12);
        assert_eq!(edge.ticks[edge.ticks.len() - 1].label, "Dec");
        // The year to date: only the months so far.
        let to_date = range(RangeKind::Year, "2026-10-22T10:00:00Z", "UTC");
        let report = report_with(&to_date.from, &to_date.to, "UTC", vec![]);
        assert_eq!(chart_data(&report, RangeKind::Year).bars.len(), 10);
    }

    #[test]
    fn keeps_weeks_and_months_as_days_with_a_slot_for_every_day() {
        let report = year(
            "2026-10-19T00:00:00Z",
            "2026-10-25T10:00:00Z",
            "UTC",
            &[("2026-10-21", 900)],
        );
        let chart = chart_data(&report, RangeKind::Week);
        assert_eq!(chart.unit, Unit::Day);
        assert_eq!(chart.bars.len(), 7);
    }

    #[test]
    fn sums_by_month_without_losing_or_doubling_tokens() {
        let days = [
            day("2026-01-30", 10),
            day("2026-01-31", 20),
            day("2026-02-01", 5),
        ];
        let bars = monthly_bars(&days).bars;
        let months: Vec<&str> = bars.iter().map(|bar| bar.day.as_str()).collect();
        assert_eq!(months, ["2026-01", "2026-02"]);
        let shares: Vec<f64> = bars.iter().map(|bar| bar.share).collect();
        assert_eq!(shares, [1.0, 5.0 / 30.0]);
        assert_eq!(month_label("2026-11"), "Nov");
        assert!(monthly_bars(&[]).bars.is_empty());
    }

    // Decisions the TypeScript didn't have: money rounds the way JavaScript's `toFixed` does on exact ties, and a zone
    // this build doesn't know is `None` (or empty) rather than an exception.

    #[test]
    fn rounds_money_to_cents_the_way_javascript_does_on_exact_ties() {
        assert_eq!(usd(0.125), "$0.13");
        assert_eq!(usd(1.005), "$1.00");
        assert_eq!(usd(0.5), "$0.50");
    }

    #[test]
    fn a_zone_this_build_does_not_know_gives_no_range_and_no_days() {
        let now = now_at("2026-10-22T10:00:00Z");
        assert_eq!(range_for(RangeKind::Today, &now, "Mars/Olympus"), None);
        assert_eq!(
            usage_query(
                RangeKind::Today,
                &UsageFilters::default(),
                &now,
                "Mars/Olympus"
            ),
            None
        );
        assert!(
            days_between(
                "2026-10-19T00:00:00Z",
                "2026-10-22T00:00:00Z",
                "Mars/Olympus"
            )
            .is_empty()
        );
    }
}
