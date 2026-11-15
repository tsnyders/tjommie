//! Usage in Settings (Phase 3 plan T5.5; `usage/usage-section.tsx`, `provider-usage.tsx`, `lib/use-usage.ts`; row 7):
//! the range, the subscriptions' quota cards with their refresh (the provider's own free check, never a paid turn),
//! tokens, what the API keys spent and what the subscriptions' tokens would have cost on the API — each on its own,
//! never added up except in the labelled estimate — the days as bars, and the breakdown by agent, account or model. A
//! report shows only for the range and day it was read for; `usage.changed` reads it again (a burst is one read).
//! Usage deep links with filters (an agent's or an account's) aren't routed yet, so there are no filter chips.

use std::time::Duration;

use futures::StreamExt as _;
use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, relative,
};
use jiff::Zoned;
use protocol::accounts::{AccountInfo, AccountKind, AccountsCheck, UsageCost, UsageReport};
use protocol::model::Bot;
use protocol::wire::method;

use super::diagnostics::value;
use crate::guard::guarded_async;
use crate::model::accounts::{QuotaCard, provider_view, quota_card};
use crate::model::labels::error_text;
use crate::model::setup::provider_name;
use crate::model::state::Connection;
use crate::model::usage::{
    By, RangeKind, Unit, UsageFilters, breakdown_rows, chart_data, combined_cost, cost_lines, cost_text, day_label,
    empty_usage, labelled, month_label, tokens_text, usage_day_key, usage_query,
};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Look, RadioGroup, problem};
use crate::ui::primitives::button::{IconButton, IconVariant};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

const RANGES: [(RangeKind, &str, &str); 4] = [
    (RangeKind::Today, "today", "Today"),
    (RangeKind::Week, "week", "This week"),
    (RangeKind::Month, "month", "This month"),
    (RangeKind::Year, "year", "This year"),
];
const BREAKDOWNS: [(By, &str, &str); 3] = [
    (By::Bot, "bot", "By agent"),
    (By::Account, "account", "By account"),
    (By::Model, "model", "By model"),
];

/// The local zone's name, as the query takes it.
fn local_zone_name(now: &Zoned) -> String {
    now.time_zone().iana_name().unwrap_or("UTC").to_owned()
}

/// The section's state (`useUsage` and the provider cards' refresh).
struct Usage {
    range: RangeKind,
    by: By,
    /// The report, with the range and day it answered.
    held: Option<(String, UsageReport)>,
    failed: Option<(String, String)>,
    /// What was last asked: the key and the event count.
    asked: Option<(String, u64)>,
    generation: u64,
    /// `usage.changed` events so far: one more reads again.
    changes: u64,
    /// The quota refresh under way: `Some("all")` or an account id.
    busy: Option<String>,
    quota_problem: Option<SharedString>,
    _events: Task<()>,
}

impl Usage {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut feed = store::events(cx, |event| matches!(event, protocol::Event::UsageChanged(_)));
        let events = cx.spawn(async move |this, cx| {
            guarded_async("follow the usage changes", async move {
                while feed.arrivals.next().await.is_some() {
                    // Usage events come in bursts while an agent works: one read a second at most.
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    while let Ok(()) = feed.arrivals.try_recv() {}
                    if this
                        .update(cx, |usage, cx| {
                            usage.changes += 1;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .await;
        });
        Usage {
            range: RangeKind::Week,
            by: By::Bot,
            held: None,
            failed: None,
            asked: None,
            generation: 0,
            changes: 0,
            busy: None,
            quota_problem: None,
            _events: events,
        }
    }
}

/// Reads the report when the range, the day or the events asked for another.
fn read(usage: &Entity<Usage>, key: &str, now: &Zoned, cx: &mut App) {
    if store::state(cx).connection != Connection::Ready {
        return;
    }
    let (range, changes) = (usage.read(cx).range, usage.read(cx).changes);
    let wanted = (key.to_owned(), changes);
    if usage.read(cx).asked.as_ref() == Some(&wanted) {
        return;
    }
    let Some(query) = usage_query(range, &UsageFilters::default(), now, &local_zone_name(now)) else {
        return;
    };
    let generation = usage.update(cx, |usage, _| {
        usage.asked = Some(wanted);
        usage.generation += 1;
        usage.generation
    });
    let call = store::request::<method::UsageQuery>(cx, query);
    let (usage, key) = (usage.downgrade(), key.to_owned());
    cx.spawn(async move |cx| {
        guarded_async("read the usage", async move {
            let read = call.await;
            cx.update(|cx| {
                let _ = usage.update(cx, |usage, cx| {
                    if usage.generation != generation {
                        return;
                    }
                    match read {
                        Ok(report) => {
                            usage.held = Some((key, report));
                            usage.failed = None;
                        }
                        Err(error) => usage.failed = Some((key, error_text(&error))),
                    }
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// The provider's free quota check: one account's, or all of them (`accounts.check`).
fn refresh(usage: &Entity<Usage>, id: Option<String>, cx: &mut App) {
    usage.update(cx, |usage, cx| {
        usage.busy = Some(id.clone().unwrap_or_else(|| "all".to_owned()));
        usage.quota_problem = None;
        cx.notify();
    });
    let call = store::request::<method::AccountsCheck>(cx, AccountsCheck { id });
    let usage = usage.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("check the provider limits", async move {
            let checked = call.await;
            cx.update(|cx| {
                let _ = usage.update(cx, |usage, cx| {
                    usage.busy = None;
                    usage.quota_problem = checked.err().map(|error| error_text(&error).into());
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// A tile: a label, a figure and maybe a note.
fn tile(id: &'static str, label: &'static str, value_text: String, note: Option<String>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(id, Role::Group)
        .aria_label(format!("{label}: {value_text}"))
        .min_w(r(150.))
        .flex_1()
        .p(r(theme.space.s16))
        .rounded(r(theme.radius.field))
        .bg(hsla(colors.surface.raised))
        .child(div().type_style(&theme.typography.desktop.small).text_color(hsla(colors.text.secondary)).child(label))
        .child(
            div()
                .mt(r(theme.space.s2))
                .type_style(&theme.typography.desktop.display_s)
                .text_color(hsla(colors.text.primary))
                .child(value_text),
        )
        .children(note.map(|note| {
            div().mt(r(theme.space.s2)).type_style(&theme.typography.desktop.small).text_color(hsla(colors.text.secondary)).child(note)
        }))
        .into_any_element()
}

fn cost_tile(id: &'static str, label: &'static str, cost: &UsageCost, cx: &App) -> AnyElement {
    let text = cost_text(cost);
    tile(id, label, text.text, text.note, cx)
}

/// A limit's meter: its label and value, and the share used (near the limit in the accent).
fn meter(id: String, label: &str, value_text: &str, share: Option<f64>, near: bool, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(id, Role::Meter)
        .aria_label(format!("{label}: {value_text}"))
        .flex()
        .flex_col()
        .gap(r(theme.space.s4))
        .child(
            div()
                .flex()
                .justify_between()
                .gap(r(theme.space.s12))
                .type_style(&theme.typography.desktop.small)
                .child(div().text_color(hsla(colors.text.secondary)).child(label.to_owned()))
                .child(div().text_color(hsla(colors.text.primary)).child(value_text.to_owned())),
        )
        .children(share.map(|share| {
            div().h(r(6.)).rounded_full().bg(hsla(colors.surface.sunk)).child(
                div()
                    .h_full()
                    .w(relative(share.clamp(0.0, 1.0) as f32))
                    .rounded_full()
                    .bg(hsla(if near { colors.accent.marker } else { colors.text.secondary })),
            )
        }))
        .into_any_element()
}

/// A subscription's quota card (`ProviderCard`).
fn provider_card(account: &AccountInfo, now: &Zoned, busy: bool, disabled: bool, usage: &Entity<Usage>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let id = &account.id;
    let view = provider_view(account, now);
    let refresh_one = {
        let (usage, id) = (usage.clone(), account.id.clone());
        let button = IconButton::new(format!("settings.usage.{id}.refresh"), "rotate-cw", format!("Refresh {}", account.label))
            .variant(IconVariant::Plain);
        let button = if disabled { button } else { button.on_press(move |_, cx| refresh(&usage, Some(id.clone()), cx)) };
        div().opacity(if disabled && !busy { 0.38 } else { 1.0 }).child(button)
    };
    let small = |id: String, text: String| value(id, text, cx);
    let body: Vec<AnyElement> = match quota_card(account) {
        QuotaCard::Limits => {
            let mut body: Vec<AnyElement> = if view.rows.is_empty() {
                vec![small(format!("settings.usage.{id}.none"), "Limits aren't available for this account yet.".to_owned())]
            } else {
                view.rows
                    .iter()
                    .map(|row| meter(format!("settings.usage.{id}.{}", row.key), &row.label, &row.value, row.share, row.near, cx))
                    .collect()
            };
            body.extend(view.extras.iter().map(|extra| small(format!("settings.usage.{id}.extra"), labelled(&extra.label, &extra.value))));
            body.extend(view.stale.clone().map(|stale| small(format!("settings.usage.{id}.stale"), stale)));
            body.extend(view.problem.clone().map(|said| problem(format!("settings.usage.{id}.problem"), said, cx)));
            body
        }
        QuotaCard::Unavailable => vec![small(
            format!("settings.usage.{id}.unavailable"),
            "Limits aren't available for this account yet. They show after its first message, or when you refresh.".to_owned(),
        )],
        QuotaCard::Checking => vec![small(format!("settings.usage.{id}.checking"), "Checking".to_owned())],
        QuotaCard::SignedOut => vec![small(
            format!("settings.usage.{id}.signed_out"),
            "Not signed in, so there are no limits to show.".to_owned(),
        )],
        QuotaCard::Problem { text } => vec![problem(format!("settings.usage.{id}.problem"), text, cx)],
    };
    let who = [Some(account.label.clone()), account.setup.as_ref().and_then(|setup| setup.account.clone())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    div()
        .semantic(format!("settings.usage.{id}"), Role::Group)
        .aria_label(format!("{}, {}", provider_name(account.provider), account.label))
        .flex()
        .flex_col()
        .gap(r(theme.space.s12))
        .p(r(theme.space.s16))
        .rounded(r(theme.radius.field))
        .bg(hsla(colors.surface.raised))
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s8))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .type_style(&theme.typography.desktop.ui_strong)
                        .text_color(hsla(colors.text.primary))
                        .child(provider_name(account.provider)),
                )
                .children(view.plan.clone().map(|plan| {
                    div()
                        .min_w_0()
                        .max_w(relative(0.45))
                        .truncate()
                        .px(r(theme.space.s8))
                        .rounded_full()
                        .bg(hsla(colors.surface.sunk))
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.primary))
                        .child(plan)
                }))
                .child(div().flex_1())
                .child(refresh_one),
        )
        .children(body)
        .child(small(format!("settings.usage.{id}.who"), who))
        .into_any_element()
}

/// Tokens per day (or month) as plain bars, the busiest full height, with a few dates under them and the zone.
fn bars(report: &UsageReport, range: RangeKind, cx: &App) -> Option<AnyElement> {
    let chart = chart_data(report, range);
    if chart.bars.is_empty() {
        return None;
    }
    let theme = theme(cx);
    let colors = palette(cx);
    let count = chart.bars.len();
    let columns: Vec<AnyElement> = chart
        .bars
        .iter()
        .map(|bar| {
            let height = if bar.share > 0.0 { (bar.share * 100.0).max(4.0) } else { 1.0 };
            div()
                .h_full()
                .min_w_0()
                .flex_1()
                .flex()
                .items_end()
                .px(gpui::px(1.))
                .child(div().w_full().h(relative((height / 100.0) as f32)).rounded_t(gpui::px(3.)).bg(hsla(colors.surface.sunk)))
                .into_any_element()
        })
        .collect();
    let ticks = chart
        .ticks
        .iter()
        .map(|tick| tick.label.clone())
        .collect::<Vec<_>>()
        .join(" · ");
    let unit = match chart.unit {
        Unit::Month => "Months",
        Unit::Day => "Days",
    };
    let _ = (count, day_label, month_label);
    Some(
        div()
            .mt(r(theme.space.s20))
            .p(r(theme.space.s16))
            .rounded(r(theme.radius.field))
            .bg(hsla(colors.surface.raised))
            .child(
                div()
                    .semantic("settings.usage.chart", Role::Image)
                    .aria_label(chart.label.clone())
                    .h(r(96.))
                    .flex()
                    .items_end()
                    .children(columns),
            )
            .child(value("settings.usage.chart.ticks", ticks, cx))
            .child(value("settings.usage.chart.zone", format!("{unit} in {}", report.time_zone.as_str()), cx))
            .into_any_element(),
    )
}

fn report_view(report: &UsageReport, usage: &Entity<Usage>, range: RangeKind, by: By, bots: &[Bot], accounts: &[AccountInfo], cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    if empty_usage(report) {
        return div()
            .mt(r(theme.space.s32))
            .child(value("settings.usage.empty", "No usage yet. It shows here after your agents' first messages.", cx))
            .into_any_element();
    }
    let costs = &report.summary.costs;
    let combined = cost_text(&combined_cost(costs));
    let combined_note = [combined.note, Some("API-equivalent value, not your subscription bill".to_owned())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let mut tiles = vec![
        tile("settings.usage.tokens", "Tokens", tokens_text(report.summary.tokens.total), None, cx),
        tile("settings.usage.combined", "Combined cost estimate", combined.text, Some(combined_note), cx),
        cost_tile("settings.usage.spent", "Spent (API keys)", &costs.api_spend, cx),
        cost_tile("settings.usage.value", "API value (subscriptions)", &costs.api_value, cx),
    ];
    if costs.unclassified.known_records + costs.unclassified.unknown_records > 0 {
        tiles.push(cost_tile("settings.usage.other", "Other", &costs.unclassified, cx));
    }
    let breakdown = BREAKDOWNS
        .iter()
        .fold(RadioGroup::new("settings.usage.by", "Breakdown", by).look(Look::Segmented), |group, (value, key, text)| {
            group.choice(*value, *key, *text)
        })
        .on_choose({
            let usage = usage.clone();
            move |next, _, cx| {
                usage.update(cx, |usage, cx| {
                    usage.by = next;
                    cx.notify();
                })
            }
        });
    let rows: Vec<AnyElement> = breakdown_rows(report, by, bots, accounts)
        .into_iter()
        .map(|row| {
            let lines: Vec<AnyElement> = cost_lines(row.aggregate)
                .into_iter()
                .map(|line| {
                    let text = match line.note {
                        Some(note) => format!("{} · {note}", labelled(line.label, &line.text)),
                        None => labelled(line.label, &line.text),
                    };
                    div().child(text).into_any_element()
                })
                .collect();
            div()
                .semantic(format!("settings.usage.row.{}", row.key), Role::ListItem)
                .aria_label(format!("{}: {} tokens", row.name, tokens_text(row.aggregate.tokens.total)))
                .px(r(theme.space.s16))
                .py(r(theme.space.s12))
                .rounded(r(theme.radius.field))
                .bg(hsla(colors.surface.raised))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(r(theme.space.s12))
                        .child(div().min_w_0().truncate().type_style(&theme.typography.desktop.body).text_color(hsla(colors.text.primary)).child(row.name.clone()))
                        .child(
                            div()
                                .flex_none()
                                .type_style(&theme.typography.desktop.ui)
                                .text_color(hsla(colors.text.secondary))
                                .child(format!("{} tokens", tokens_text(row.aggregate.tokens.total))),
                        ),
                )
                .child(div().mt(r(theme.space.s2)).flex().flex_col().type_style(&theme.typography.desktop.small).text_color(hsla(colors.text.secondary)).children(lines))
                .into_any_element()
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .child(div().mt(r(theme.space.s20)).flex().flex_wrap().gap(r(theme.space.s12)).children(tiles))
        .children((range != RangeKind::Today).then(|| bars(report, range, cx)).flatten())
        .child(
            div()
                .mt(r(theme.space.s32))
                .flex()
                .items_center()
                .justify_between()
                .gap(r(theme.space.s12))
                .child(div().type_style(&theme.typography.desktop.label).text_color(hsla(colors.text.tertiary)).child("Breakdown"))
                .child(breakdown),
        )
        .child(div().mt(r(theme.space.s12)).flex().flex_col().gap(r(theme.space.s8)).children(rows))
        .into_any_element()
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let usage = window.use_keyed_state("settings.usage.state", cx, |_, cx| Usage::new(cx));
    let now = Zoned::now();
    let range = usage.read(cx).range;
    let key = format!("{range:?}|{}", usage_day_key(&now, &local_zone_name(&now)).unwrap_or_default());
    read(&usage, &key, &now, cx);
    let state = store::state(cx);
    let (bots, accounts) = (state.bots.clone(), state.accounts.clone());
    let (report, failed, by, busy, quota_problem) = {
        let usage = usage.read(cx);
        (
            usage.held.as_ref().filter(|(held, _)| *held == key).map(|(_, report)| report.clone()),
            usage.failed.as_ref().filter(|(held, _)| *held == key).map(|(_, said)| said.clone()),
            usage.by,
            usage.busy.clone(),
            usage.quota_problem.clone(),
        )
    };
    let theme = theme(cx);
    let colors = palette(cx);
    let ranges = RANGES
        .iter()
        .fold(RadioGroup::new("settings.usage.range", "Range", range).look(Look::Segmented), |group, (value, key, text)| {
            group.choice(*value, *key, *text)
        })
        .on_choose({
            let usage = usage.clone();
            move |next, _, cx| {
                usage.update(cx, |usage, cx| {
                    usage.range = next;
                    cx.notify();
                })
            }
        });
    let subscriptions: Vec<&AccountInfo> = accounts
        .iter()
        .flatten()
        .filter(|account| account.kind == AccountKind::Subscription)
        .collect();
    let providers = (!subscriptions.is_empty()).then(|| {
        let refresh_all = {
            let usage = usage.clone();
            let button = IconButton::new("settings.usage.refresh_all", "rotate-cw", "Refresh all limits").variant(IconVariant::Plain);
            let button = if busy.is_some() { button } else { button.on_press(move |_, cx| refresh(&usage, None, cx)) };
            div().opacity(if busy.is_some() { 0.38 } else { 1.0 }).child(button)
        };
        div()
            .semantic("settings.usage.providers", Role::Group)
            .aria_label("Provider usage")
            .mt(r(theme.space.s20))
            .flex()
            .flex_col()
            .gap(r(theme.space.s12))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().type_style(&theme.typography.desktop.label).text_color(hsla(colors.text.tertiary)).child("Subscriptions"))
                    .child(refresh_all),
            )
            .children(quota_problem.map(|said| problem("settings.usage.quota_problem", said, cx)))
            .children(subscriptions.iter().map(|account| {
                provider_card(account, &now, busy.as_deref() == Some(account.id.as_str()), busy.is_some(), &usage, cx)
            }))
    });
    let body: AnyElement = match (&report, &failed) {
        (Some(report), _) => report_view(report, &usage, range, by, &bots, accounts.as_deref().unwrap_or(&[]), cx),
        (None, Some(_)) => div().into_any_element(),
        (None, None) => div().mt(r(theme.space.s32)).child(value("settings.usage.loading", "Loading usage", cx)).into_any_element(),
    };
    super::frame("usage", "Usage", "", cx)
        .child(div().mt(r(theme.space.s20)).child(ranges))
        .children(providers)
        .children(failed.map(|said| problem("settings.usage.problem", said, cx)))
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use protocol::accounts::UsageReport;
    use protocol::wire::method;
    use serde_json::{Value, json};

    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, click, draw, node, state, text};

    fn cost(usd: Option<f64>, known: u64, unknown: u64) -> Value {
        json!({ "usd": usd, "knownUsd": usd, "knownRecords": known, "unknownRecords": unknown, "reportedUsd": null, "estimatedUsd": usd })
    }

    fn aggregate(total: u64, spend: Option<f64>) -> Value {
        json!({
            "records": 3,
            "tokens": { "input": total / 2, "cachedInput": 0, "output": total / 2, "reasoning": 0, "total": total },
            "costs": { "apiSpend": cost(spend, 1, 0), "apiValue": cost(Some(0.42), 2, 0), "unclassified": cost(None, 0, 0) }
        })
    }

    fn report(total: u64) -> UsageReport {
        serde_json::from_value(json!({
            "from": "2026-11-09", "to": "2026-11-15", "timeZone": "UTC",
            "summary": aggregate(total, Some(1.25)),
            "daily": [{ "day": "2026-11-14", "aggregate": aggregate(total, Some(1.25)) }],
            "byBot": [{ "botId": "b1", "aggregate": aggregate(total, Some(1.25)) }],
            "byAccount": [], "byModel": []
        }))
        .unwrap()
    }

    fn rpc(total: u64) -> FakeRpc {
        FakeRpc::new().on::<method::UsageQuery>(move |_| Ok(report(total)))
    }

    fn calls_of(calls: &[(&'static str, Value)], method: &str) -> usize {
        calls.iter().filter(|(name, _)| *name == method).count()
    }

    /// usage-section.test.tsx: the tiles keep spend, API value and the estimate apart, and the breakdown names agents.
    #[gpui::test]
    fn shows_tokens_and_each_cost_on_its_own(cx: &mut TestAppContext) {
        let (cx, calls) = app(cx, Route::Settings(Section::Usage), state(vec![crate::ui::testing::bot("b1", "Ledger", protocol::model::BotState::Idle)], vec![], vec![]), rpc(1_500));
        draw(cx);
        assert!(text("settings.usage.tokens").starts_with("Tokens: "));
        assert!(text("settings.usage.spent").starts_with("Spent (API keys): "));
        assert!(text("settings.usage.value").starts_with("API value (subscriptions): "));
        assert!(text("settings.usage.combined").starts_with("Combined cost estimate: "));
        assert!(node("settings.usage.other").is_none(), "no unclassified records");
        assert!(text("settings.usage.row.b1").starts_with("Ledger: "));
        assert!(node("settings.usage.chart").is_some(), "a week has its bars");
        assert_eq!(calls_of(&calls.calls(), "usage.query"), 1);
        click(cx, "settings.usage.range.today");
        draw(cx);
        assert_eq!(calls_of(&calls.calls(), "usage.query"), 2, "another range is another read");
        assert!(node("settings.usage.chart").is_none(), "Today has no bars");
    }

    /// usage-section.test.tsx: "No usage yet" for an empty report.
    #[gpui::test]
    fn an_empty_report_says_no_usage_yet(cx: &mut TestAppContext) {
        let (cx, _) = app(cx, Route::Settings(Section::Usage), state(vec![], vec![], vec![]), rpc(0));
        draw(cx);
        assert!(text("settings.usage.empty").starts_with("No usage yet."));
    }

    /// use-usage.ts: a `usage.changed` reads the report again.
    #[gpui::test]
    fn reads_again_when_usage_changes(cx: &mut TestAppContext) {
        let (cx, calls) = app(cx, Route::Settings(Section::Usage), state(vec![], vec![], vec![]), rpc(10));
        draw(cx);
        assert_eq!(calls_of(&calls.calls(), "usage.query"), 1);
        cx.update(|_, cx| {
            let changed = serde_json::from_value(json!({ "accountId": "a1", "botId": "b1" })).unwrap();
            crate::ui::store::fake_event(cx, &protocol::Event::UsageChanged(changed));
        });
        cx.executor().advance_clock(std::time::Duration::from_secs(2));
        draw(cx);
        draw(cx);
        assert_eq!(calls_of(&calls.calls(), "usage.query"), 2);
        let _: AppState = state(vec![], vec![], vec![]);
    }
}
