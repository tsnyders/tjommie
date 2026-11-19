//! Usage in Settings (Phase 3 plan T5.5; `usage/usage-section.tsx`, `usage/provider-usage.tsx`, `lib/use-usage.ts`;
//! row 7). The range; the subscriptions' limit cards with their refresh, which asks the provider's own free check and
//! never runs a paid turn; then tokens, what the API keys spent and what the subscriptions' tokens would have cost on the
//! API, each on its own and added up only in the labelled estimate; the days as bars; and the breakdown by agent, account
//! or model. A report shows only for the range and day it was read for, and `usage.changed` reads it again, at most once
//! a second.
//!
//! Left out: the filter chips (Settings routes no usage filters yet, so every `usage.changed` concerns the report), the
//! bars' hover titles (the app has no tooltip yet; the chart's sentence says the same), and the refresh icon's spin while
//! a check runs (`IconButton` has no busy state, so the refreshes dim instead).

use std::time::Duration;

use futures::StreamExt as _;
use gpui::{
    AnyElement, App, Context, Entity, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, px, relative,
};
use jiff::Zoned;
use protocol::accounts::{AccountInfo, AccountKind, AccountsCheck, UsageCost, UsageReport};
use protocol::wire::method;

use crate::guard::guarded_async;
use crate::model::accounts::{ProviderView, QuotaCard, provider_view, quota_card};
use crate::model::labels::error_text;
use crate::model::setup::{provider_name, report_of};
use crate::model::state::Connection;
use crate::model::usage::{
    By, Generation, Held, REFETCH_MS, RangeKind, Row, TICK_CAP_MS, Unit, UsageFilters, breakdown_rows,
    chart_data, clock_tick_ms, combined_cost, cost_lines, cost_text, empty_usage, labelled, matching,
    tokens_text, usage_day_key, usage_query,
};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Look, RadioGroup, problem};
use crate::ui::primitives::button::IconButton;
use crate::ui::projects::line;
use crate::ui::routines::editor::local_zone;
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

/// The section's state: `useUsage`'s reads and the reports it holds, and the limit cards' refresh.
struct Usage {
    range: RangeKind,
    by: By,
    held: Option<Held<UsageReport>>,
    failed: Option<Held<String>>,
    /// The spec's key and the change count last read; `None` while offline, so the next connection reads again.
    asked: Option<(String, u64)>,
    generation: Generation,
    /// The bursts of `usage.changed` so far: each reads the report again.
    changes: u64,
    /// A limits check is running: the refreshes are off until it answers.
    checking: bool,
    check_problem: Option<String>,
    _events: Task<()>,
    _clock: Task<()>,
}

impl Usage {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut feed = store::events(cx, |event| matches!(event, protocol::Event::UsageChanged(_)));
        let events = cx.spawn(async move |this, cx| {
            guarded_async("follow the usage changes", async move {
                // `Throttle`'s rule: one read REFETCH_MS after the first change, however many come meanwhile.
                while feed.arrivals.next().await.is_some() {
                    cx.background_executor().timer(Duration::from_millis(REFETCH_MS)).await;
                    while let Ok(()) = feed.arrivals.try_recv() {}
                    let counted = this.update(cx, |usage, cx| {
                        usage.changes += 1;
                        cx.notify();
                    });
                    if counted.is_err() {
                        break;
                    }
                }
            })
            .await;
        });
        // The hook's clock: draws again at the next local midnight (a new day is a new report) and every minute between,
        // as the limits' resets are told against the time now.
        // ponytail: no redraw when the window comes back into focus, as the hook's resume has; the minute tick catches up.
        let clock = cx.spawn(async move |this, cx| {
            guarded_async("keep the usage clock", async move {
                loop {
                    let wait = clock_tick_ms(&Zoned::now(), &local_zone()).unwrap_or(TICK_CAP_MS);
                    let wait = Duration::from_millis(u64::try_from(wait).unwrap_or(1));
                    cx.background_executor().timer(wait).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
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
            generation: Generation::default(),
            changes: 0,
            checking: false,
            check_problem: None,
            _events: events,
            _clock: clock,
        }
    }
}

/// Reads the report when the spec, the day or a change asks for another (the hook's effect).
fn read(usage: &Entity<Usage>, key: &str, now: &Zoned, zone: &str, cx: &mut App) {
    if store::state(cx).connection != Connection::Ready {
        if usage.read(cx).asked.is_some() {
            usage.update(cx, |usage, _| usage.asked = None);
        }
        return;
    }
    let (range, changes) = (usage.read(cx).range, usage.read(cx).changes);
    let wanted = (key.to_owned(), changes);
    if usage.read(cx).asked.as_ref() == Some(&wanted) {
        return;
    }
    let Some(query) = usage_query(range, &UsageFilters::default(), now, zone) else {
        return;
    };
    let reading = usage.update(cx, |usage, _| {
        usage.asked = Some(wanted);
        usage.generation.start()
    });
    let call = store::request::<method::UsageQuery>(cx, query);
    let (usage, key) = (usage.downgrade(), key.to_owned());
    cx.spawn(async move |cx| {
        guarded_async("read the usage", async move {
            let answer = call.await;
            cx.update(|cx| {
                // An answer to an older read, or after Settings closed, changes nothing.
                let _ = usage.update(cx, |usage, cx| {
                    if !usage.generation.applies(reading, true) {
                        return;
                    }
                    match answer {
                        Ok(report) => {
                            usage.held = Some(Held { key, value: report });
                            usage.failed = None;
                        }
                        Err(error) => usage.failed = Some(Held { key, value: error_text(&error) }),
                    }
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// The provider's own free limits check, for one account or all of them (`accounts.check`). The daemon's
/// `accounts.changed` brings the new readings, so the answer itself is left alone.
fn refresh(usage: &Entity<Usage>, id: Option<String>, cx: &mut App) {
    usage.update(cx, |usage, cx| {
        usage.checking = true;
        usage.check_problem = None;
        cx.notify();
    });
    let call = store::request::<method::AccountsCheck>(cx, AccountsCheck { id });
    let usage = usage.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("check the subscriptions' limits", async move {
            let checked = call.await;
            cx.update(|cx| {
                let _ = usage.update(cx, |usage, cx| {
                    usage.checking = false;
                    usage.check_problem = checked.err().map(|error| error_text(&error));
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// A refresh (`IconButton`, `RefreshCw`), off while a check runs.
fn refresh_button(id: SharedString, label: String, account: Option<String>, checking: bool, usage: &Entity<Usage>) -> AnyElement {
    let button = IconButton::new(id, "rotate-cw", label);
    if checking {
        return div().opacity(0.38).child(button).into_any_element();
    }
    let usage = usage.clone();
    button.on_press(move |_, cx| refresh(&usage, account.clone(), cx)).into_any_element()
}

/// A bar for something that fills up (`Meter`): a track, with the share used in the body colour and in the accent once
/// it's near full (never red: a limit is a pause, not an error). No reading is an empty track with no value. Usage's
/// healthy fill is `bg-working` in the TypeScript; Aegis gives working no colour of its own, so it's the body's here too.
pub(super) fn meter(id: impl Into<SharedString>, label: &str, value_text: &str, share: Option<f64>, near: bool, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let percent = share.map(|share| (share * 100.0).round().clamp(0.0, 100.0));
    let track = div()
        .semantic(id, Role::Meter)
        .aria_label(label.to_owned())
        .aria_value(value_text.to_owned())
        .aria_min_numeric_value(0.0)
        .aria_max_numeric_value(100.0)
        .h(r(6.))
        .overflow_hidden()
        .rounded_full()
        .bg(hsla(colors.surface.sunk))
        .child(
            div()
                .h_full()
                .w(relative(percent.unwrap_or(0.0) as f32 / 100.0))
                .rounded_full()
                // The accent's marker, not its base: the base on the track is under 3:1 in light mode.
                .bg(hsla(if near { colors.accent.marker } else { colors.ink.mid })),
        );
    let track = match percent {
        Some(percent) => track.aria_numeric_value(percent),
        None => track,
    };
    div()
        .flex()
        .flex_col()
        .gap(r(theme.space.s4))
        .child(
            div()
                .flex()
                .justify_between()
                .gap(r(theme.space.s12))
                .type_style(&theme.typography.desktop.small)
                .child(div().flex_none().text_color(hsla(colors.text.secondary)).child(label.to_owned()))
                .child(div().min_w_0().text_right().text_color(hsla(colors.text.primary)).child(value_text.to_owned())),
        )
        .child(track)
        .into_any_element()
}

/// A subscription's limits card (`ProviderCard`): the provider, the plan if known, each limit window with its use and
/// reset, Extra usage and Credits only as the provider said them, and who is signed in.
fn provider_card(account: &AccountInfo, now: &Zoned, checking: bool, usage: &Entity<Usage>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let id = account.id.as_str();
    let ProviderView { plan, rows, stale, problem: failure, extras } = provider_view(account, now);
    let mut body: Vec<AnyElement> = Vec::new();
    match quota_card(account) {
        QuotaCard::Limits => {
            if rows.is_empty() {
                let none = "Limits aren't available for this account yet.";
                body.push(line(format!("settings.usage.{id}.none"), Role::Paragraph, none, cx));
            } else {
                let meters = rows.iter().map(|row| {
                    meter(format!("settings.usage.{id}.limit.{}", row.key), &row.label, &row.value, row.share, row.near, cx)
                });
                body.push(div().flex().flex_col().gap(r(theme.space.s10)).children(meters).into_any_element());
            }
            body.extend(extras.into_iter().enumerate().map(|(index, extra)| {
                div()
                    .semantic(format!("settings.usage.{id}.extra.{index}"), Role::Paragraph)
                    .aria_label(format!("{}: {}", extra.label, extra.value))
                    .flex()
                    .justify_between()
                    .gap(r(theme.space.s12))
                    .type_style(&theme.typography.desktop.small)
                    .child(div().text_color(hsla(colors.text.secondary)).child(extra.label))
                    .child(div().text_color(hsla(colors.text.primary)).child(extra.value))
                    .into_any_element()
            }));
            body.extend(stale.map(|stale| line(format!("settings.usage.{id}.stale"), Role::Paragraph, stale, cx)));
            body.extend(failure.map(|said| problem(format!("settings.usage.{id}.problem"), said, cx)));
        }
        QuotaCard::Unavailable => {
            let none = "Limits aren't available for this account yet. They show after its first message, or when you refresh.";
            body.push(line(format!("settings.usage.{id}.none"), Role::Paragraph, none, cx));
        }
        QuotaCard::Checking => body.push(line(format!("settings.usage.{id}.checking"), Role::Paragraph, "Checking", cx)),
        QuotaCard::SignedOut => {
            let out = "Not signed in, so there are no limits to show.";
            body.push(line(format!("settings.usage.{id}.signed_out"), Role::Paragraph, out, cx));
        }
        QuotaCard::Problem { text } => body.push(problem(format!("settings.usage.{id}.problem"), text, cx)),
    }
    let who = [Some(account.label.as_str()), account.setup.as_ref().and_then(|setup| report_of(setup).account.as_deref())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let refresh = refresh_button(
        format!("settings.usage.{id}.refresh").into(),
        format!("Refresh {}", account.label),
        Some(account.id.clone()),
        checking,
        usage,
    );
    super::card(cx)
        .mt_0()
        .py(r(theme.space.s16))
        .gap(r(theme.space.s12))
        .semantic(format!("settings.usage.{id}"), Role::Region)
        .aria_label(format!("{}, {}", provider_name(account.provider), account.label))
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s8))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .type_style(&theme.typography.desktop.body_strong)
                        .text_color(hsla(colors.text.primary))
                        .child(provider_name(account.provider)),
                )
                // The plan can be long (up to 120 characters), so it's cut short and the refresh stays inside the card.
                .children(plan.map(|plan| {
                    div()
                        .semantic(format!("settings.usage.{id}.plan"), Role::Paragraph)
                        .aria_label(plan.clone())
                        .min_w_0()
                        .max_w(relative(0.45))
                        .h(r(22.))
                        .flex()
                        .items_center()
                        .px(r(theme.space.s8))
                        .rounded_full()
                        .bg(hsla(colors.surface.sunk))
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.primary))
                        .child(div().min_w_0().truncate().child(plan))
                }))
                .child(div().flex_1())
                .child(refresh),
        )
        .children(body)
        .child(div().min_w_0().truncate().child(line(format!("settings.usage.{id}.who"), Role::Paragraph, who, cx)))
        .into_any_element()
}

/// The subscriptions' limit cards at the top of Usage (`ProviderUsage`); API keys have no subscription quota.
fn providers(accounts: &[AccountInfo], now: &Zoned, usage: &Entity<Usage>, cx: &App) -> Option<AnyElement> {
    let subscriptions: Vec<&AccountInfo> = accounts
        .iter()
        .filter(|account| matches!(account.kind, AccountKind::Subscription))
        .collect();
    if subscriptions.is_empty() {
        return None;
    }
    let theme = theme(cx);
    let (checking, said) = (usage.read(cx).checking, usage.read(cx).check_problem.clone());
    let all = refresh_button("settings.usage.refresh_all".into(), "Refresh all limits".to_owned(), None, checking, usage);
    Some(
        div()
            .semantic("settings.usage.providers", Role::Region)
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
                    .gap(r(theme.space.s12))
                    .child(super::heading("Subscriptions", cx).mt_0())
                    .child(all),
            )
            .children(said.map(|said| problem("settings.usage.check_problem", said, cx)))
            .children(subscriptions.into_iter().map(|account| provider_card(account, now, checking, usage, cx)))
            .into_any_element(),
    )
}

/// A tile (`Tile`): a label, a figure and maybe a note.
fn tile(id: &'static str, label: &'static str, figure: String, note: Option<String>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let spoken = match &note {
        Some(note) => format!("{label}: {figure}. {note}"),
        None => format!("{label}: {figure}"),
    };
    super::card(cx)
        .mt_0()
        .py(r(theme.space.s16))
        .min_w(r(150.))
        .flex_1()
        .semantic(id, Role::Group)
        .aria_label(spoken)
        .child(div().type_style(&theme.typography.desktop.small).text_color(hsla(colors.text.secondary)).child(label))
        .child(
            div()
                .mt(r(theme.space.s2))
                .type_style(&theme.typography.desktop.display_s)
                .text_color(hsla(colors.text.primary))
                .child(figure),
        )
        .children(note.map(|note| {
            div()
                .mt(r(theme.space.s2))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.secondary))
                .child(note)
        }))
        .into_any_element()
}

fn cost_tile(id: &'static str, label: &'static str, cost: &UsageCost, cx: &App) -> AnyElement {
    let text = cost_text(cost);
    tile(id, label, text.text, text.note, cx)
}

/// Tokens per day (or month) as plain bars, a slot for every day of the range and the busiest full height, with a few
/// dates under them and the zone the days are in. No chart library: a picture, with a sentence for anyone who can't see it.
fn bars(report: &UsageReport, range: RangeKind, cx: &App) -> Option<AnyElement> {
    let chart = chart_data(report, range);
    if chart.bars.is_empty() {
        return None;
    }
    let theme = theme(cx);
    let colors = palette(cx);
    let months = matches!(chart.unit, Unit::Month);
    let count = chart.bars.len() as f32;
    let columns = chart.bars.iter().map(|bar| {
        let height = if bar.share > 0.0 { (bar.share * 100.0).max(4.0) } else { 1.0 };
        div()
            .h_full()
            .min_w_0()
            .flex_1()
            .flex()
            .items_end()
            .px(px(if months { 2. } else { 1. }))
            // The field edge's grey: quiet, and 3:1 against the card in both modes (line.soft isn't).
            .child(div().w_full().h(relative(height as f32 / 100.0)).rounded_t(px(3.)).bg(hsla(colors.line.field)))
    });
    let last = chart.ticks.len().saturating_sub(1);
    // Each date centred under its bar, the first starting there and the last ending there, so neither leaves the card:
    // a zero-wide anchor at the bar's middle, its date overflowing it to one side or both.
    let ticks = chart.ticks.iter().enumerate().map(|(index, tick)| {
        let anchor = div().absolute().top_0().left(relative((tick.index as f32 + 0.5) / count)).w(px(0.)).flex();
        let anchor = match index {
            0 => anchor.justify_start(),
            index if index == last => anchor.justify_end(),
            _ => anchor.justify_center(),
        };
        anchor.child(div().flex_none().whitespace_nowrap().child(tick.label.clone()))
    });
    let zone = format!("{} in {}", if months { "Months" } else { "Days" }, report.time_zone.as_str());
    Some(
        super::card(cx)
            .mt(r(theme.space.s20))
            .py(r(theme.space.s16))
            .child(
                div()
                    .semantic("settings.usage.chart", Role::Image)
                    .aria_label(chart.label.clone())
                    .h(r(96.))
                    .flex()
                    .items_end()
                    .children(columns),
            )
            .child(
                div()
                    .relative()
                    .mt(r(theme.space.s6))
                    .h(r(16.))
                    .type_style(&theme.typography.desktop.meta)
                    .text_color(hsla(colors.text.secondary))
                    .children(ticks),
            )
            .child(
                div()
                    .semantic("settings.usage.zone", Role::Paragraph)
                    .aria_label(zone.clone())
                    .mt(r(theme.space.s4))
                    .type_style(&theme.typography.desktop.meta)
                    .text_color(hsla(colors.text.secondary))
                    .child(zone),
            )
            .into_any_element(),
    )
}

/// A breakdown row: its name and tokens, and its cost lines under them.
fn breakdown_row(row: &Row<'_>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let tokens = format!("{} tokens", tokens_text(row.aggregate.tokens.total));
    let lines: Vec<String> = cost_lines(row.aggregate)
        .into_iter()
        .map(|line| {
            let text = labelled(line.label, &line.text);
            match line.note {
                Some(note) => format!("{text} · {note}"),
                None => text,
            }
        })
        .collect();
    let spoken = [format!("{}: {tokens}", row.name)].into_iter().chain(lines.iter().cloned()).collect::<Vec<_>>().join(". ");
    super::card(cx)
        .mt_0()
        .py(r(theme.space.s12))
        .semantic(format!("settings.usage.row.{}", row.key), Role::ListItem)
        .aria_label(spoken)
        .child(
            div()
                .flex()
                .justify_between()
                .gap(r(theme.space.s12))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .type_style(&theme.typography.desktop.body)
                        .text_color(hsla(colors.text.primary))
                        .child(row.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.secondary))
                        .child(tokens),
                ),
        )
        .children((!lines.is_empty()).then(|| {
            div()
                .mt(r(theme.space.s2))
                .flex()
                .flex_col()
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.secondary))
                .children(lines)
        }))
        .into_any_element()
}

/// The report (`UsageReportView`): the tiles, the bars past a single day, and the breakdown.
fn report_view(report: &UsageReport, range: RangeKind, by: By, usage: &Entity<Usage>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    if empty_usage(report) {
        let none = "No usage yet. It shows here after your agents' first messages.";
        return div().mt(r(theme.space.s32)).child(line("settings.usage.empty", Role::Paragraph, none, cx)).into_any_element();
    }
    let costs = &report.summary.costs;
    let combined = cost_text(&combined_cost(costs));
    let combined_note = [combined.note.as_deref(), Some("API-equivalent value, not your subscription bill")]
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
    let state = store::state(cx);
    let rows: Vec<AnyElement> = breakdown_rows(report, by, &state.bots, state.accounts.as_deref().unwrap_or_default())
        .iter()
        .map(|row| breakdown_row(row, cx))
        .collect();
    let breakdown = BREAKDOWNS
        .iter()
        .fold(RadioGroup::new("settings.usage.by", "Breakdown", by).look(Look::Segmented), |group, (value, key, label)| {
            group.choice(*value, *key, *label)
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
                .child(super::heading("Breakdown", cx).mt_0())
                .child(breakdown),
        )
        .child(
            div()
                .semantic("settings.usage.rows", Role::List)
                .aria_label("Breakdown")
                .mt(r(theme.space.s12))
                .flex()
                .flex_col()
                .gap(r(theme.space.s8))
                .children(rows),
        )
        .into_any_element()
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let usage = window.use_keyed_state("settings.usage.state", cx, |_, cx| Usage::new(cx));
    let (now, zone) = (Zoned::now(), local_zone());
    // The spec's key, as the hook's: the range and the day, so a new day reads again.
    let key = format!("{:?}|{}", usage.read(cx).range, usage_day_key(&now, &zone).unwrap_or_default());
    read(&usage, &key, &now, &zone, cx);
    let cx = &*cx;
    let theme = theme(cx);
    let state = usage.read(cx);
    let ranges = RANGES
        .iter()
        .fold(RadioGroup::new("settings.usage.range", "Range", state.range).look(Look::Segmented), |group, (value, key, label)| {
            group.choice(*value, *key, *label)
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
    let report = matching(state.held.as_ref(), Some(key.as_str()));
    let failed = matching(state.failed.as_ref(), Some(key.as_str()));
    let body = match (report, failed) {
        (Some(report), _) => Some(report_view(report, state.range, state.by, &usage, cx)),
        (None, Some(_)) => None,
        (None, None) => Some(
            div()
                .mt(r(theme.space.s32))
                .child(line("settings.usage.loading", Role::Paragraph, "Loading usage", cx))
                .into_any_element(),
        ),
    };
    super::frame("usage", "Usage", "", cx)
        .child(div().mt(r(theme.space.s20)).child(ranges))
        .children(store::state(cx).accounts.as_deref().and_then(|accounts| providers(accounts, &now, &usage, cx)))
        .children(failed.map(|said| div().mt(r(theme.space.s12)).child(problem("settings.usage.problem", said.clone(), cx))))
        .children(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use client::RpcError;
    use gpui::TestAppContext;
    use protocol::accounts::{AccountInfo, UsageReport};
    use protocol::model::BotState;
    use protocol::wire::method;
    use serde_json::{Value, json};

    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::model::usage::REFETCH_MS;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, bot, click, draw, ids, node, state, text};

    /// `{ ...base, ...patch }`: the patch's keys replace the base's.
    fn with(mut base: Value, patch: Value) -> Value {
        if let (Some(fields), Value::Object(patch)) = (base.as_object_mut(), patch) {
            fields.extend(patch);
        }
        base
    }

    fn cost(patch: Value) -> Value {
        let empty = json!({ "usd": 0, "knownUsd": 0, "knownRecords": 0, "unknownRecords": 0, "reportedUsd": null, "estimatedUsd": null });
        with(empty, patch)
    }

    fn aggregate(total: u64, costs: Value) -> Value {
        json!({
            "records": u64::from(total > 0),
            "tokens": { "input": total, "cachedInput": 0, "output": 0, "reasoning": 0, "total": total },
            "costs": with(json!({ "apiSpend": cost(json!({})), "apiValue": cost(json!({})), "unclassified": cost(json!({})) }), costs)
        })
    }

    /// usage-section.test.tsx's report: 19 to 22 October in UTC, two busy days, and all of it agent b1's.
    fn report(summary: Value, extra: Value) -> UsageReport {
        let base = json!({
            "from": "2026-10-19T00:00:00.000Z", "to": "2026-10-22T10:00:00.000Z", "timeZone": "UTC",
            "summary": summary.clone(),
            "daily": [
                with(aggregate(2000, json!({})), json!({ "day": "2026-10-21" })),
                with(aggregate(500, json!({})), json!({ "day": "2026-10-22" }))
            ],
            "byBot": [with(summary, json!({ "botId": "b1" }))],
            "byAccount": [], "byModel": []
        });
        serde_json::from_value(with(base, extra)).unwrap()
    }

    fn rpc(answer: UsageReport) -> FakeRpc {
        FakeRpc::new().on::<method::UsageQuery>(move |_| Ok(answer.clone()))
    }

    fn reads(rpc: &FakeRpc) -> usize {
        rpc.calls().iter().filter(|(name, _)| *name == "usage.query").count()
    }

    /// Connected, with agent b1, Scout.
    fn scout() -> AppState {
        state(vec![bot("b1", "Scout", BotState::Idle)], vec![], vec![])
    }

    fn with_accounts(accounts: Vec<AccountInfo>) -> AppState {
        AppState { accounts: Some(accounts), ..scout() }
    }

    fn setup(patch: Value) -> Value {
        let signed_in = json!({ "provider": "claude", "installed": true, "version": "2.0.0", "signedIn": true, "account": "Sample Account", "plan": "Max 20x", "error": null });
        with(signed_in, patch)
    }

    /// provider-usage.test.tsx's subscription, with its two windows (8% and 5% used) resetting long after any test runs.
    fn subscription(patch: Value) -> AccountInfo {
        let window = |limit: &str, key: &str, minutes: u32, used: u32| {
            json!({
                "accountId": "a1", "limitId": limit, "windowKey": key, "durationMinutes": minutes, "usedPercent": used,
                "resetsAt": "2099-01-01T00:00:00.000Z", "state": "ok", "observedAt": "2026-10-22T10:00:00.000Z"
            })
        };
        let base = json!({
            "id": "a1", "provider": "claude", "label": "Claude (Personal)", "kind": "subscription", "isDefault": true,
            "isFallback": false, "spendingCapUsd": null, "createdAt": "2026-10-01T00:00:00.000Z", "hasCredential": false,
            "setup": setup(json!({})), "limits": [window("s", "primary", 300, 8), window("w", "secondary", 10_080, 5)],
            "signIn": null, "spending": { "knownUsd": 0, "unknownRows": 0, "capUsd": null, "blocked": false }
        });
        serde_json::from_value(with(base, patch)).unwrap()
    }

    /// usage-section.test.tsx: "keeps tokens, API spend and API value in tiles of their own", and "adds a combined cost
    /// estimate beside the separate tiles, labelled as API-equivalent value and not a bill".
    #[gpui::test]
    fn keeps_each_cost_in_a_tile_of_its_own_beside_the_labelled_estimate(cx: &mut TestAppContext) {
        let summary = aggregate(
            2500,
            json!({
                "apiSpend": cost(json!({ "usd": 3, "knownUsd": 3, "knownRecords": 1 })),
                "apiValue": cost(json!({ "usd": 8, "knownUsd": 8, "knownRecords": 2 })),
                "unclassified": cost(json!({ "usd": 1, "knownUsd": 1, "knownRecords": 1 }))
            }),
        );
        let (_, calls) = app(cx, Route::Settings(Section::Usage), scout(), rpc(report(summary, json!({}))));
        assert_eq!(text("settings.usage.tokens"), "Tokens: 2.5K");
        assert_eq!(text("settings.usage.spent"), "Spent (API keys): $3.00");
        assert_eq!(text("settings.usage.value"), "API value (subscriptions): $8.00");
        assert_eq!(text("settings.usage.other"), "Other: $1.00");
        assert_eq!(
            text("settings.usage.combined"),
            "Combined cost estimate: $12.00. API-equivalent value, not your subscription bill"
        );
        assert_eq!(reads(&calls), 1);
    }

    /// usage-section.test.tsx: "draws the days as bars with a sentence for them, but not for a single day", and Other
    /// only when there's something in it. Another range is another read.
    #[gpui::test]
    fn draws_the_days_as_bars_but_not_for_today(cx: &mut TestAppContext) {
        let (cx, calls) = app(cx, Route::Settings(Section::Usage), scout(), rpc(report(aggregate(2500, json!({})), json!({}))));
        assert_eq!(text("settings.usage.chart"), "Tokens per day, highest 2K on 2026-10-21");
        assert_eq!(text("settings.usage.zone"), "Days in UTC");
        assert!(node("settings.usage.other").is_none());
        click(cx, "settings.usage.range.today");
        draw(cx);
        assert_eq!(reads(&calls), 2);
        assert!(node("settings.usage.chart").is_none());
        assert_eq!(text("settings.usage.tokens"), "Tokens: 2.5K");
    }

    /// usage-section.test.tsx: "draws a year as twelve month bars, not hundreds of day bars".
    #[gpui::test]
    fn draws_a_year_as_months(cx: &mut TestAppContext) {
        let year = report(
            aggregate(2500, json!({})),
            json!({
                "from": "2026-01-01T00:00:00.000Z", "to": "2026-12-31T23:00:00.000Z",
                "daily": [with(aggregate(2000, json!({})), json!({ "day": "2026-10-21" }))]
            }),
        );
        let (cx, _) = app(cx, Route::Settings(Section::Usage), scout(), rpc(year));
        click(cx, "settings.usage.range.year");
        draw(cx);
        assert_eq!(text("settings.usage.chart"), "Tokens per month, highest 2K in Oct");
        assert_eq!(text("settings.usage.zone"), "Months in UTC");
    }

    /// usage-section.test.tsx: "lists the breakdown by bot with its name and cost lines, and names a deleted bot".
    #[gpui::test]
    fn lists_the_breakdown_and_names_a_deleted_agent(cx: &mut TestAppContext) {
        let spent = json!({ "apiSpend": cost(json!({ "usd": 3, "knownUsd": 3, "knownRecords": 1 })) });
        let usage = report(
            aggregate(2500, spent.clone()),
            json!({ "byBot": [
                with(aggregate(2000, json!({})), json!({ "botId": "b1" })),
                with(aggregate(500, spent), json!({ "botId": "gone" }))
            ] }),
        );
        let (cx, _) = app(cx, Route::Settings(Section::Usage), scout(), rpc(usage));
        assert!(text("settings.usage.row.b1").starts_with("Scout: 2K tokens"));
        assert!(text("settings.usage.row.gone").starts_with("Deleted agent: 500 tokens"));
        assert!(text("settings.usage.row.gone").contains("Spent $3.00"));
        click(cx, "settings.usage.by.account");
        assert!(ids("settings.usage.row.").is_empty(), "this report has no accounts");
    }

    /// usage-section.test.tsx: "says there is no usage yet when there are no records".
    #[gpui::test]
    fn says_there_is_no_usage_yet(cx: &mut TestAppContext) {
        app(cx, Route::Settings(Section::Usage), scout(), rpc(report(aggregate(0, json!({})), json!({}))));
        assert_eq!(text("settings.usage.empty"), "No usage yet. It shows here after your agents' first messages.");
        assert!(node("settings.usage.tokens").is_none());
    }

    /// use-usage.ts: a burst of `usage.changed` reads the report again, once, a second after it starts.
    #[gpui::test]
    fn a_burst_of_usage_changes_reads_once(cx: &mut TestAppContext) {
        let (cx, calls) = app(cx, Route::Settings(Section::Usage), scout(), rpc(report(aggregate(2500, json!({})), json!({}))));
        assert_eq!(reads(&calls), 1);
        cx.update(|_, cx| {
            for _ in 0..3 {
                let changed = serde_json::from_value(json!({ "accountId": "a1", "botId": "b1" })).unwrap();
                crate::ui::store::fake_event(cx, &protocol::Event::UsageChanged(changed));
            }
        });
        draw(cx);
        assert_eq!(reads(&calls), 1, "not before the second is up");
        cx.executor().advance_clock(std::time::Duration::from_millis(REFETCH_MS));
        draw(cx);
        draw(cx);
        assert_eq!(reads(&calls), 2);
    }

    /// use-usage.ts: a read that fails says why, with no report and no Loading line.
    #[gpui::test]
    fn a_failed_read_says_why(cx: &mut TestAppContext) {
        let rpc = FakeRpc::new().on::<method::UsageQuery>(|_| Err(RpcError::BadResult("No answer".to_owned())));
        app(cx, Route::Settings(Section::Usage), scout(), rpc);
        assert!(text("settings.usage.problem").contains("No answer"));
        assert!(node("settings.usage.loading").is_none());
    }

    /// provider-usage.test.tsx: "shows a subscription's provider, plan, each limit window with its use, and who is signed
    /// in", and "has a refresh for each account and for all of them, which only ask for a check".
    #[gpui::test]
    fn shows_each_subscriptions_limits_and_its_refreshes_only_ask_for_a_check(cx: &mut TestAppContext) {
        let work = subscription(json!({
            "id": "a2", "label": "Work", "provider": "codex", "limits": [],
            "setup": setup(json!({ "provider": "codex", "sandbox": "ready", "plan": "pro" }))
        }));
        let rpc = rpc(report(aggregate(0, json!({})), json!({}))).on::<method::AccountsCheck>(|_| Ok(Vec::new()));
        let (cx, calls) = app(cx, Route::Settings(Section::Usage), with_accounts(vec![subscription(json!({})), work]), rpc);
        assert_eq!(text("settings.usage.a1"), "Claude Code, Claude (Personal)");
        assert_eq!(text("settings.usage.a1.plan"), "Max 20x");
        assert_eq!(ids("settings.usage.a1.limit.").len(), 2);
        assert_eq!(text("settings.usage.a1.who"), "Claude (Personal) · Sample Account");
        assert_eq!(text("settings.usage.a2.plan"), "pro");
        assert_eq!(text("settings.usage.refresh_all"), "Refresh all limits");
        assert_eq!(text("settings.usage.a2.refresh"), "Refresh Work");
        click(cx, "settings.usage.refresh_all");
        draw(cx);
        click(cx, "settings.usage.a1.refresh");
        draw(cx);
        let checks: Vec<Value> =
            calls.calls().into_iter().filter(|(name, _)| *name == "accounts.check").map(|(_, params)| params).collect();
        assert_eq!(checks, [json!({}), json!({ "id": "a1" })]);
    }

    /// provider-usage.test.tsx: "says honestly when there are no limits", "shows checking, signed out and a problem as
    /// themselves, and a refresh problem above the cards", and "ignores API keys, which have no subscription quota".
    #[gpui::test]
    fn says_what_each_card_knows_and_a_failed_check_above_them(cx: &mut TestAppContext) {
        let key = subscription(json!({
            "id": "k1", "label": "Key", "kind": "api_key", "hasCredential": true, "setup": null, "limits": []
        }));
        let accounts = vec![
            subscription(json!({ "limits": [] })),
            subscription(json!({ "id": "a2", "setup": null, "limits": [] })),
            subscription(json!({ "id": "a3", "setup": setup(json!({ "signedIn": false })), "limits": [] })),
            subscription(json!({ "id": "a4", "setup": setup(json!({ "error": "CLI failed" })), "limits": [] })),
            key,
        ];
        let rpc = rpc(report(aggregate(0, json!({})), json!({})))
            .on::<method::AccountsCheck>(|_| Err(RpcError::BadResult("Offline".to_owned())));
        let (cx, _) = app(cx, Route::Settings(Section::Usage), with_accounts(accounts), rpc);
        assert!(text("settings.usage.a1.none").starts_with("Limits aren't available for this account yet."));
        assert_eq!(text("settings.usage.a2.checking"), "Checking");
        assert_eq!(text("settings.usage.a3.signed_out"), "Not signed in, so there are no limits to show.");
        assert!(text("settings.usage.a4.problem").contains("CLI failed"));
        assert!(node("settings.usage.k1").is_none());
        click(cx, "settings.usage.refresh_all");
        draw(cx);
        assert!(text("settings.usage.check_problem").contains("Offline"));
    }

    /// provider-usage.test.tsx: an API key alone has no limit cards at all.
    #[gpui::test]
    fn api_keys_alone_have_no_limit_cards(cx: &mut TestAppContext) {
        let key = subscription(json!({ "id": "k1", "label": "Key", "kind": "api_key", "setup": null, "limits": [] }));
        app(cx, Route::Settings(Section::Usage), with_accounts(vec![key]), rpc(report(aggregate(0, json!({})), json!({}))));
        assert!(node("settings.usage.providers").is_none());
    }
}
