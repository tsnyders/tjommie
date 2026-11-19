//! Accounts in Settings (Phase 3 plan T5.5; `accounts/accounts-section.tsx`, `account-card.tsx`, `limit-meter.tsx`;
//! row 6). Agents run Claude Code and Codex on these accounts: a group for each provider with its Add (a subscription or
//! an API key), and a card for each account with its one status line and what fixes it, its limits or its spending, what
//! it used today and this month, and its menu. The dialogs are `account_dialogs`.
//!
//! A link (`Route::Account`: the chat's setup banner's Sign in, a limit notice's Raise cap or Retry) points at one card.
//! Once the control it names is on the card, that control takes the focus, once. A link never starts a sign-in or a
//! retry by itself.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use client::ClientMethod;
use futures::StreamExt as _;
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px, svg,
};
use jiff::Zoned;
use protocol::accounts::{
    AccountAttempt, AccountInfo, AccountKind, AccountPatch, AccountsCheck, AccountsUpdate, UsageAggregate,
};
use protocol::model::{Bot, CodexSandboxReadiness, Provider, ProviderSetup};
use protocol::wire::{Id, method};

use super::account_dialogs;
use super::usage::meter;
use crate::guard::{guarded, guarded_async};
use crate::model::accounts::{
    Focus, RETRY_CAPTION, StatusAction, StatusFlags, Tone, accounts_of, bots_on, focus_ready, limit_label, limit_near,
    limit_value, remember_url, spending_meter, status_line, subscription_label,
};
use crate::model::labels::error_text;
use crate::model::setup::{provider_name, report_of};
use crate::model::state::Connection;
use crate::model::usage::{
    Held, REFETCH_MS, RangeKind, UsageFilters, account_usage_text, concerns, matching, usage_day_key, usage_query,
};
use crate::semantic::SemanticExt as _;
use crate::ui::agents::cards::more_trigger;
use crate::ui::fields::{Link, problem};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::primitives::{focus_by_id, focus_handle};
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::primitives::toggle::Toggle;
use crate::ui::projects::line;
use crate::ui::routines::editor::local_zone;
use crate::ui::shell::route::{self, AccountAction, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

const NOTE: &str = "Agents run Claude Code and Codex on these accounts. Subscriptions sign in on the provider's own page; \
                    Tjommie never sees your password. API keys stay in Windows Credential Manager.";
const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];
/// A subscription still unchecked this long offers Check again instead of spinning on (`STUCK_MS`).
const STUCK: Duration = Duration::from_secs(8);
/// The cards' clock: limits and their resets are told against the time now.
const MINUTE: Duration = Duration::from_secs(60);

/// The provider's short name, as the cards and dialogs say it ("Claude", "Codex").
pub(super) fn short_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "Claude",
        Provider::Codex => "Codex",
    }
}

fn key_of(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

fn provider_of(setup: &ProviderSetup) -> Provider {
    match setup {
        ProviderSetup::Claude(_) => Provider::Claude,
        ProviderSetup::Codex(_) => Provider::Codex,
    }
}

/// What a card keeps: its action running, why it failed, and since when it has waited for its first check.
#[derive(Default)]
struct Card {
    busy: bool,
    problem: Option<String>,
    waiting_since: Option<Instant>,
}

/// The section's state (`AccountsSection`'s, and its cards').
pub(super) struct Accounts {
    /// The sign-in pages by account, kept so "Open the page again" has one; the daemon opens the page itself.
    pub(super) urls: HashMap<String, String>,
    pub(super) problem: Option<String>,
    /// Subscriptions that hadn't been checked were checked once.
    checked: bool,
    cards: HashMap<String, Card>,
    sandbox_busy: bool,
    sandbox_problem: Option<String>,
    /// The link's wish already met; a wish is met once.
    focused: Option<(String, AccountAction)>,
    /// The latest dialog opened: a save closes only its own dialog (`closeEpoch`).
    pub(super) epoch: u64,
    _clock: Task<()>,
}

impl Accounts {
    fn new(cx: &mut Context<Self>) -> Self {
        let clock = cx.spawn(async move |this, cx| {
            guarded_async("keep the accounts' clock", async move {
                loop {
                    cx.background_executor().timer(MINUTE).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .await;
        });
        Accounts {
            urls: HashMap::new(),
            problem: None,
            checked: false,
            cards: HashMap::new(),
            sandbox_busy: false,
            sandbox_problem: None,
            focused: None,
            epoch: 0,
            _clock: clock,
        }
    }

    fn card(&mut self, id: &str) -> &mut Card {
        self.cards.entry(id.to_owned()).or_default()
    }
}

/// A card's action (`run`): the card is busy while it runs and says why if it fails; `then` takes the answer.
fn run<M: ClientMethod>(
    accounts: &Entity<Accounts>,
    id: &str,
    params: M::Params,
    then: impl FnOnce(&mut Accounts, M::Result) + 'static,
    cx: &mut App,
) {
    accounts.update(cx, |accounts, cx| {
        let card = accounts.card(id);
        card.busy = true;
        card.problem = None;
        cx.notify();
    });
    let call = store::request::<M>(cx, params);
    let (accounts, id) = (accounts.downgrade(), id.to_owned());
    cx.spawn(async move |cx| {
        guarded_async("run an account's action", async move {
            let done = call.await;
            cx.update(|cx| {
                let _ = accounts.update(cx, |accounts, cx| {
                    accounts.card(&id).busy = false;
                    match done {
                        Ok(answer) => then(accounts, answer),
                        Err(error) => accounts.card(&id).problem = Some(error_text(&error)),
                    }
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// Starts a sign-in (`signIn`): the daemon opens the provider's page, and its address is kept for Open the page again.
fn sign_in(accounts: &Entity<Accounts>, id: &str, cx: &mut App) {
    let key = id.to_owned();
    let remember = move |accounts: &mut Accounts, started: protocol::accounts::SignInStart| {
        accounts.urls = remember_url(&accounts.urls, &key, started.url.as_deref());
    };
    run::<method::AccountsSignIn>(accounts, id, Id { id: id.to_owned() }, remember, cx);
}

fn update(accounts: &Entity<Accounts>, id: &str, patch: AccountPatch, cx: &mut App) {
    run::<method::AccountsUpdate>(accounts, id, AccountsUpdate { id: id.to_owned(), patch }, |_, _| {}, cx);
}

/// Codex's sandbox (`accounts.prepareCodexSandbox`): Windows asks for admin permission once.
fn prepare_sandbox(accounts: &Entity<Accounts>, id: String, cx: &mut App) {
    accounts.update(cx, |accounts, cx| {
        accounts.sandbox_busy = true;
        accounts.sandbox_problem = None;
        cx.notify();
    });
    let call = store::request::<method::AccountsPrepareCodexSandbox>(cx, Id { id });
    let accounts = accounts.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("set up Codex's sandbox", async move {
            let done = call.await;
            cx.update(|cx| {
                let _ = accounts.update(cx, |accounts, cx| {
                    accounts.sandbox_busy = false;
                    accounts.sandbox_problem = done.err().map(|error| error_text(&error));
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// A subscription whose first check hasn't come back is checked once when the section opens; its card offers Check
/// again if it still hasn't answered, so nothing waits for ever.
fn first_check(accounts: &Entity<Accounts>, list: &[AccountInfo], cx: &mut App) {
    let unchecked = list.iter().any(|each| each.kind == AccountKind::Subscription && each.setup.is_none());
    if accounts.read(cx).checked || !unchecked {
        return;
    }
    accounts.update(cx, |accounts, _| accounts.checked = true);
    let call = store::request::<method::AccountsCheck>(cx, AccountsCheck { id: None });
    cx.spawn(async move |_| {
        guarded_async("check the subscriptions once", async move {
            // A failure is the card's to show: it offers Check again.
            let _ = call.await;
        })
        .await;
    })
    .detach();
}

/// `useStuck` for each card: whether a subscription has waited past STUCK for its first check. A card starts its clock
/// when it starts waiting, and the section draws again when that wait is up.
fn stuck_cards(accounts: &Entity<Accounts>, list: &[AccountInfo], cx: &mut App) -> HashMap<String, bool> {
    let now = cx.background_executor().now();
    let mut started = false;
    let stuck = accounts.update(cx, |accounts, _| {
        list.iter()
            .map(|account| {
                let waiting =
                    account.kind == AccountKind::Subscription && account.setup.is_none() && account.sign_in.is_none();
                let card = accounts.card(&account.id);
                if !waiting {
                    card.waiting_since = None;
                } else if card.waiting_since.is_none() {
                    card.waiting_since = Some(now);
                    started = true;
                }
                let stuck = card.waiting_since.is_some_and(|since| now.saturating_duration_since(since) >= STUCK);
                (account.id.clone(), stuck)
            })
            .collect()
    });
    if started {
        let accounts = accounts.downgrade();
        cx.spawn(async move |cx| {
            guarded_async("notice a check that takes long", async move {
                cx.background_executor().timer(STUCK).await;
                cx.update(|cx| {
                    let _ = accounts.update(cx, |_, cx| cx.notify());
                });
            })
            .await;
        })
        .detach();
    }
    stuck
}

/// The link's wish (`Route::Account` with an action). It's forgotten once the route moves on, so the same link works again.
fn wish(accounts: &Entity<Accounts>, cx: &mut App) -> Option<(String, AccountAction)> {
    let wish = match route::current(cx) {
        Route::Account { account, action: Some(action) } => Some((account, action)),
        _ => None,
    };
    if wish.is_none() && accounts.read(cx).focused.is_some() {
        accounts.update(cx, |accounts, _| accounts.focused = None);
    }
    wish
}

fn focus_of(action: AccountAction) -> Focus {
    match action {
        AccountAction::SignIn => Focus::SignIn,
        AccountAction::Cap => Focus::Cap,
        AccountAction::Retry => Focus::Retry,
    }
}

/// The control a focus names on a card. Raise cap comes first when the cap stopped the key; Edit cap is always there.
fn target(id: &str, focus: Focus, actions: &[StatusAction]) -> String {
    let control = match focus {
        Focus::SignIn => "signin",
        Focus::Retry => "retry",
        Focus::Cap if actions.contains(&StatusAction::RaiseCap) => "raise_cap",
        Focus::Cap => "cap",
    };
    format!("settings.accounts.{id}.{control}")
}

/// What one account used today and this month (`useUsage` twice, filtered to the account): the summaries, kept by the
/// day they were read on, and read again on that account's `usage.changed`.
struct CardUsage {
    account: String,
    today: Option<Held<UsageAggregate>>,
    month: Option<Held<UsageAggregate>>,
    asked: Option<(String, u64)>,
    changes: u64,
    _events: Task<()>,
}

impl CardUsage {
    fn new(account: String, cx: &mut Context<Self>) -> Self {
        let filters = UsageFilters { account: Some(account.clone()), ..UsageFilters::default() };
        let mut feed = store::events(cx, move |event| {
            matches!(event, protocol::Event::UsageChanged(changed) if concerns(&filters, changed))
        });
        let events = cx.spawn(async move |this, cx| {
            guarded_async("follow an account's usage", async move {
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
        CardUsage {
            account,
            today: None,
            month: None,
            asked: None,
            changes: 0,
            _events: events,
        }
    }
}

/// Reads the account's day and month when the day or a change asks for them. A failed read keeps what was shown, as
/// the card's "…" does until a reading arrives.
fn read_usage(usage: &Entity<CardUsage>, day: &str, now: &Zoned, zone: &str, cx: &mut App) {
    if store::state(cx).connection != Connection::Ready {
        if usage.read(cx).asked.is_some() {
            usage.update(cx, |usage, _| usage.asked = None);
        }
        return;
    }
    let wanted = (day.to_owned(), usage.read(cx).changes);
    if usage.read(cx).asked.as_ref() == Some(&wanted) {
        return;
    }
    let filters = UsageFilters { account: Some(usage.read(cx).account.clone()), ..UsageFilters::default() };
    let (Some(today), Some(month)) =
        (usage_query(RangeKind::Today, &filters, now, zone), usage_query(RangeKind::Month, &filters, now, zone))
    else {
        return;
    };
    usage.update(cx, |usage, _| usage.asked = Some(wanted.clone()));
    let calls = (store::request::<method::UsageQuery>(cx, today), store::request::<method::UsageQuery>(cx, month));
    let usage = usage.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("read an account's usage", async move {
            let (today, month) = futures::join!(calls.0, calls.1);
            cx.update(|cx| {
                let _ = usage.update(cx, |usage, cx| {
                    // An answer to an older read changes nothing.
                    if usage.asked.as_ref() != Some(&wanted) {
                        return;
                    }
                    let (day, _) = wanted;
                    if let Ok(report) = today {
                        usage.today = Some(Held { key: day.clone(), value: report.summary });
                    }
                    if let Ok(report) = month {
                        usage.month = Some(Held { key: day, value: report.summary });
                    }
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// "Today … · This month …", with "…" until a reading arrives.
fn usage_line(usage: &Entity<CardUsage>, day: &str, kind: AccountKind, cx: &App) -> String {
    let usage = usage.read(cx);
    let text = |held: Option<&Held<UsageAggregate>>| {
        matching(held, Some(day)).map_or_else(|| "…".to_owned(), |summary| account_usage_text(summary, kind))
    };
    format!("Today {} · This month {}", text(usage.today.as_ref()), text(usage.month.as_ref()))
}

/// What a card is drawn from, gathered while `cx` could still change.
struct CardData<'a> {
    account: &'a AccountInfo,
    siblings: &'a [&'a AccountInfo],
    bots: &'a [Bot],
    now: &'a Zoned,
    busy: bool,
    problem: Option<String>,
    reopen: Option<String>,
    stuck: bool,
    usage: String,
    more: Option<FocusHandle>,
}

/// A card's button: a pill, off (dimmed, no handler) while it can't be pressed.
pub(super) fn pill(id: String, text: &'static str, primary: bool, enabled: bool, on_press: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let pill = Pill::new(id, text);
    let pill = if primary { pill.variant(PillVariant::Primary) } else { pill };
    if enabled {
        pill.on_press(on_press).into_any_element()
    } else {
        div().opacity(0.38).child(pill).into_any_element()
    }
}

/// The button for what fixes the status line (`act`). Sign in and Try again share `signin`, as only one shows at a time.
fn status_action(action: StatusAction, data: &CardData<'_>, accounts: &Entity<Accounts>) -> AnyElement {
    let account = data.account;
    let id = account.id.clone();
    let control = format!("settings.accounts.{id}");
    let accounts = accounts.clone();
    let free = !data.busy;
    match action {
        StatusAction::OpenAgain => {
            let url = data.reopen.clone().unwrap_or_default();
            Link::new(format!("{control}.open_again"), "Open the page again")
                .on_press(move |_, _| super::about::open(&url))
                .into_any_element()
        }
        StatusAction::Cancel => {
            let attempt = account.sign_in.as_ref().map(|sign_in| sign_in.attempt.clone()).unwrap_or_default();
            pill(format!("{control}.cancel"), "Cancel", false, free, move |_, cx| {
                let params = AccountAttempt { id: id.clone(), attempt: attempt.clone() };
                run::<method::AccountsCancelSignIn>(&accounts, &id, params, |_, _| {}, cx);
            })
        }
        StatusAction::TryAgain => {
            pill(format!("{control}.signin"), "Try again", true, free, move |_, cx| sign_in(&accounts, &id, cx))
        }
        StatusAction::Check => pill(format!("{control}.check"), "Check again", false, free, move |_, cx| {
            run::<method::AccountsCheck>(&accounts, &id, AccountsCheck { id: Some(id.clone()) }, |_, _| {}, cx);
        }),
        StatusAction::RetryLimit => pill(format!("{control}.retry"), "Retry queued work", true, free, move |_, cx| {
            run::<method::AccountsRetryLimit>(&accounts, &id, Id { id: id.clone() }, |_, _| {}, cx);
        }),
        StatusAction::SignIn => {
            pill(format!("{control}.signin"), "Sign in", true, free, move |_, cx| sign_in(&accounts, &id, cx))
        }
        StatusAction::AddKey => {
            let account = account.clone();
            pill(format!("{control}.add_key"), "Add key", true, true, move |window, cx| {
                account_dialogs::open_replace_key(&account, accounts.clone(), window, cx);
            })
        }
        StatusAction::RaiseCap => {
            let account = account.clone();
            pill(format!("{control}.raise_cap"), "Raise cap", true, true, move |window, cx| {
                account_dialogs::open_cap(&account, accounts.clone(), window, cx);
            })
        }
    }
}

/// The card's menu (`AccountMenu`, "More for …"). What can't run while the card is busy isn't offered then.
fn card_menu(data: &CardData<'_>, accounts: &Entity<Accounts>) -> Option<AnyElement> {
    let more = data.more.clone()?;
    let account = data.account.clone();
    let id = account.id.clone();
    let label: SharedString = format!("More for {}", account.label).into();
    let trigger = {
        let id = id.clone();
        move |open, window: &Window, cx: &App| more_trigger(format!("settings.accounts.{id}.more"), label, open, &more, window, cx)
    };
    let item = |name: &str| format!("settings.accounts.{id}.{name}");
    let mut menu = Menu::new(SharedString::from(item("menu")), trigger).anchor(gpui::Anchor::TopRight).offset(px(6.));
    menu = menu.item(MenuItem::new(item("rename"), "Rename", {
        let (account, accounts) = (account.clone(), accounts.clone());
        move |window, cx| account_dialogs::open_rename(&account, accounts.clone(), window, cx)
    }));
    if !account.is_default && !data.busy {
        let (accounts, id) = (accounts.clone(), id.clone());
        menu = menu.item(MenuItem::new(item("make_default"), "Make default", move |_, cx| {
            update(&accounts, &id, AccountPatch { is_default: Some(true), ..AccountPatch::default() }, cx);
        }));
    }
    let signed_in = account.setup.as_ref().is_some_and(|setup| report_of(setup).signed_in);
    if account.kind == AccountKind::Subscription && signed_in && !data.busy {
        let (accounts, id) = (accounts.clone(), id.clone());
        menu = menu.item(MenuItem::new(item("different"), "Use a different account", move |_, cx| {
            sign_in(&accounts, &id, cx);
        }));
    }
    if account.kind == AccountKind::ApiKey {
        let (replace, cap) = ((account.clone(), accounts.clone()), (account.clone(), accounts.clone()));
        menu = menu
            .item(MenuItem::new(item("replace_key"), "Replace key", move |window, cx| {
                account_dialogs::open_replace_key(&replace.0, replace.1.clone(), window, cx);
            }))
            .item(MenuItem::new(item("edit_cap"), "Edit cap", move |window, cx| {
                account_dialogs::open_cap(&cap.0, cap.1.clone(), window, cx);
            }));
    }
    let (bots, accounts) = (data.bots.to_vec(), accounts.clone());
    menu = menu.item(MenuItem::new(item("remove"), "Remove", move |window, cx| {
        account_dialogs::open_remove(&account, &bots, accounts.clone(), window, cx);
    }));
    Some(menu.into_any_element())
}

/// One account (`AccountCard`).
fn card(data: CardData<'_>, accounts: &Entity<Accounts>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let account = data.account;
    let id = account.id.as_str();
    let status = status_line(account, data.now, StatusFlags { stuck: data.stuck, can_reopen: data.reopen.is_some() });
    let api_key = account.kind == AccountKind::ApiKey;

    let header = div()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .child(
            div()
                .min_w_0()
                .truncate()
                .type_style(&theme.typography.desktop.body_strong)
                .text_color(hsla(colors.text.primary))
                .child(account.label.clone()),
        )
        .children(account.is_default.then(|| {
            div()
                .semantic(format!("settings.accounts.{id}.default"), Role::Paragraph)
                .aria_label("Default")
                .flex_none()
                .h(r(22.))
                .flex()
                .items_center()
                .px(r(theme.space.s8))
                .rounded_full()
                .bg(hsla(colors.surface.sunk))
                .type_style(&theme.typography.desktop.meta)
                .text_color(hsla(colors.text.primary))
                .child("Default")
        }))
        .child(div().flex_1())
        .children(card_menu(&data, accounts));

    let kind = if api_key { "API key".to_owned() } else { subscription_label(account) };
    let (role, tone) = match status.tone {
        Tone::Problem => (Role::Alert, colors.text.primary),
        Tone::Muted => (Role::Paragraph, colors.text.secondary),
        Tone::Busy | Tone::Ok | Tone::Limited => (Role::Paragraph, colors.text.primary),
    };
    let icon = match status.tone {
        Tone::Busy => Some("loader-circle"),
        Tone::Ok => Some("check"),
        Tone::Limited => Some("hourglass"),
        Tone::Muted | Tone::Problem => None,
    };
    let actions: Vec<AnyElement> = status.actions.iter().map(|action| status_action(*action, &data, accounts)).collect();
    let status_row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(r(theme.space.s12))
        .child(
            div()
                .semantic(format!("settings.accounts.{id}.status"), role)
                .aria_label(status.text.clone())
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .gap(r(theme.space.s8))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(tone))
                .children(icon.map(|icon| {
                    svg()
                        .path(format!("icons/i/175/{icon}.svg"))
                        .size(r(14.))
                        .flex_none()
                        .text_color(hsla(colors.icon.base))
                }))
                .child(status.text.clone()),
        )
        .children((!actions.is_empty()).then(|| div().flex().flex_wrap().items_center().gap(r(theme.space.s8)).children(actions)));

    let limits: Option<AnyElement> = match account.kind {
        AccountKind::Subscription if !account.limits.is_empty() => Some(
            div()
                .flex()
                .flex_col()
                .gap(r(theme.space.s10))
                .children(account.limits.iter().map(|window| {
                    let meter_id = format!("settings.accounts.{id}.limit.{}.{}", window.limit_id, window.window_key);
                    let share = window.used_percent.map(|used| used / 100.0);
                    meter(meter_id, &limit_label(window), &limit_value(window, data.now), share, limit_near(window), cx)
                }))
                .into_any_element(),
        ),
        AccountKind::Subscription if account.setup.as_ref().is_some_and(|setup| report_of(setup).signed_in) => {
            let later = "Limits show after this account's first message.";
            Some(line(format!("settings.accounts.{id}.limits_later"), Role::Paragraph, later, cx))
        }
        _ => None,
    };

    let spending = api_key.then(|| {
        let spending = spending_meter(&account.spending);
        let open = {
            let (account, accounts) = (account.clone(), accounts.clone());
            move |window: &mut Window, cx: &mut App| account_dialogs::open_cap(&account, accounts.clone(), window, cx)
        };
        div()
            .flex()
            .items_end()
            .gap(r(theme.space.s12))
            .child(div().min_w_0().flex_1().child(meter(
                format!("settings.accounts.{id}.spending"),
                "Spending",
                &spending.text,
                Some(spending.share),
                spending.near,
                cx,
            )))
            .child(Link::new(format!("settings.accounts.{id}.cap"), "Edit cap").on_press(open))
    });

    let fallback = api_key.then(|| {
        let short = short_name(account.provider);
        let replaces = data
            .siblings
            .iter()
            .find(|other| other.id != account.id && other.is_fallback)
            .filter(|_| !account.is_fallback)
            .map(|other| format!(" Replaces {}.", other.label))
            .unwrap_or_default();
        let note = format!("When a {short} subscription account hits its limit, agents carry on with this key until it resets.{replaces}");
        let toggle = Toggle::new(format!("settings.accounts.{id}.fallback"), format!("Fallback for {short}"), account.is_fallback);
        let toggle = if data.busy {
            div().opacity(0.38).child(toggle).into_any_element()
        } else {
            let (accounts, id) = (accounts.clone(), account.id.clone());
            toggle
                .on_toggle(move |on, _, cx| {
                    update(&accounts, &id, AccountPatch { is_fallback: Some(on), ..AccountPatch::default() }, cx);
                })
                .into_any_element()
        };
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(r(theme.space.s16))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .type_style(&theme.typography.desktop.small)
                            .text_color(hsla(colors.text.primary))
                            .child(format!("Fallback for {short}")),
                    )
                    .child(line(format!("settings.accounts.{id}.fallback_note"), Role::Paragraph, note, cx)),
            )
            .child(toggle)
    });

    let users = bots_on(&account.id, data.bots);
    let users = (!users.is_empty()).then(|| {
        let words = if users.len() == 1 { "1 agent".to_owned() } else { format!("{} agents", users.len()) };
        let faces = users.iter().take(5).enumerate().map(|(index, bot)| {
            div().ml(px(if index == 0 { 0. } else { -4. })).child(BotAvatar::new(bot.color, bot.shape, 18.))
        });
        div()
            .semantic(format!("settings.accounts.{id}.users"), Role::Paragraph)
            .aria_label(words.clone())
            .flex()
            .items_center()
            .gap(r(theme.space.s6))
            .type_style(&theme.typography.desktop.small)
            .text_color(hsla(colors.text.secondary))
            .child(div().flex().children(faces))
            .child(words)
    });

    super::card(cx)
        .mt_0()
        .py(r(theme.space.s16))
        .gap(r(theme.space.s12))
        .semantic(format!("settings.accounts.{id}"), Role::Region)
        .aria_label(account.label.clone())
        // The kind sits close under the name (`-mt-2`).
        .child(
            div()
                .flex()
                .flex_col()
                .gap(r(theme.space.s4))
                .child(header)
                .child(line(format!("settings.accounts.{id}.kind"), Role::Paragraph, kind, cx)),
        )
        .child(status_row)
        .children(status.actions.contains(&StatusAction::RetryLimit).then(|| {
            line(format!("settings.accounts.{id}.retry_caption"), Role::Paragraph, RETRY_CAPTION, cx)
        }))
        .children(data.problem.clone().map(|said| problem(format!("settings.accounts.{id}.problem"), said, cx)))
        .children(limits)
        .children(spending)
        .child(line(format!("settings.accounts.{id}.usage"), Role::Paragraph, data.usage.clone(), cx))
        .children(fallback)
        .children(users)
        .into_any_element()
}

/// Add's look: a stone pill with a plus.
fn add_look<E: gpui::Styled + gpui::ParentElement>(element: E, fill: gpui::Rgba, cx: &App) -> E {
    let theme = theme(cx);
    let ink = palette(cx).text.primary;
    element
        .relative()
        .h(r(32.))
        .flex()
        .items_center()
        .gap(r(theme.space.s6))
        .px(r(theme.space.s14))
        .rounded_full()
        .bg(hsla(fill))
        .type_style(&theme.typography.desktop.button)
        .text_color(hsla(ink))
        .child(svg().path("icons/i/175/plus.svg").size(r(16.)).text_color(hsla(ink)))
        .child("Add")
}

/// Add's trigger (`Menu.Trigger`, "+ Add").
fn add_trigger(id: String, label: SharedString, open: bool, focus: &FocusHandle, window: &Window, cx: &App) -> AnyElement {
    let colors = palette(cx);
    let hover = colors.state.sunk_hover;
    let ring = focus_visible(focus, window);
    let trigger = div()
        .semantic(id, Role::Button)
        .aria_label(label)
        .aria_expanded(open)
        .track_focus(focus)
        .cursor_pointer()
        .hover(move |style| style.bg(hsla(hover)));
    add_look(trigger, if open { hover } else { colors.surface.sunk }, cx)
        .children(ring.then(|| focus_ring(theme(cx).radius.pill, cx)))
        .into_any_element()
}

/// Add while the provider isn't installed (`disabled`): dimmed, with nothing to open.
fn disabled_add(id: String, label: String, cx: &App) -> AnyElement {
    let trigger = div().semantic(id, Role::Button).aria_label(label).opacity(0.5);
    add_look(trigger, palette(cx).surface.sunk, cx).into_any_element()
}

/// What a provider's group is drawn from.
struct GroupData<'a> {
    provider: Provider,
    mine: &'a [&'a AccountInfo],
    fallback: Option<&'a ProviderSetup>,
    cards: Vec<AnyElement>,
    add: Option<FocusHandle>,
    sandbox_busy: bool,
    sandbox_problem: Option<String>,
}

/// A provider's accounts (`Group`): its version and Add, what to install or set up first, and its cards.
fn group(data: GroupData<'_>, accounts: &Entity<Accounts>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let (provider, key, short) = (data.provider, key_of(data.provider), short_name(data.provider));
    let setup = data.mine.iter().find_map(|each| each.setup.as_ref()).or(data.fallback);
    let installed = setup.is_none_or(|setup| report_of(setup).installed);
    let version = setup.and_then(|setup| report_of(setup).version.clone());
    let sandbox = match setup {
        Some(ProviderSetup::Codex(codex)) => Some(codex.sandbox),
        _ => None,
    };
    let (add_id, label) = (format!("settings.accounts.{key}.add"), format!("Add {short} account"));
    let add = match data.add.filter(|_| installed) {
        Some(focus) => {
            let (subscription, api_key) = (accounts.clone(), accounts.clone());
            let label = SharedString::from(label);
            Menu::new(SharedString::from(format!("settings.accounts.{key}.add_menu")), move |open, window, cx| {
                add_trigger(add_id, label, open, &focus, window, cx)
            })
            .anchor(gpui::Anchor::TopRight)
            .offset(px(6.))
            .item(MenuItem::new(format!("settings.accounts.{key}.add.subscription"), "Subscription", move |window, cx| {
                account_dialogs::open_add_subscription(provider, subscription.clone(), window, cx);
            }))
            .item(MenuItem::new(format!("settings.accounts.{key}.add.key"), "API key", move |window, cx| {
                account_dialogs::open_add_key(provider, api_key.clone(), window, cx);
            }))
            .into_any_element()
        }
        None => disabled_add(add_id, label, cx),
    };

    let install = (!installed).then(|| {
        let (said, row) = match provider {
            Provider::Claude => (
                "Install Claude Code from claude.com/claude-code, then check again.",
                div()
                    .child("Install Claude Code from ")
                    .child(
                        Link::new(format!("settings.accounts.{key}.install_link"), "claude.com/claude-code")
                            .quiet()
                            .on_press(|_, _| super::about::open("https://claude.com/claude-code")),
                    )
                    .child(", then check again."),
            ),
            Provider::Codex => (
                "Install Codex with npm install -g @openai/codex, then check again.",
                div()
                    .child("Install Codex with ")
                    .child(
                        div()
                            .px(r(theme.space.s4))
                            .rounded(r(theme.radius.xs))
                            .bg(hsla(colors.surface.sunk))
                            .type_style(&theme.typography.desktop.code)
                            .child("npm install -g @openai/codex"),
                    )
                    .child(", then check again."),
            ),
        };
        row.semantic(format!("settings.accounts.{key}.install"), Role::Paragraph)
            .aria_label(said)
            .mt(r(theme.space.s12))
            .flex()
            .flex_wrap()
            .items_center()
            .type_style(&theme.typography.desktop.small)
            .text_color(hsla(colors.text.secondary))
    });

    let sandbox_row = (installed && sandbox.is_some_and(|sandbox| sandbox != CodexSandboxReadiness::Ready)).then(|| {
        let said = "Codex's sandbox isn't set up yet. Codex agents at Own folder need it, and Windows asks for admin \
                    permission once.";
        let set_up = data.mine.first().map(|first| {
            let (accounts, id) = (accounts.clone(), first.id.clone());
            pill(format!("settings.accounts.{key}.sandbox_setup"), "Set up Codex's sandbox", true, !data.sandbox_busy, move |_, cx| {
                prepare_sandbox(&accounts, id.clone(), cx);
            })
        });
        div()
            .mt(r(theme.space.s12))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(r(theme.space.s12))
            .child(div().min_w_0().flex_1().child(line(format!("settings.accounts.{key}.sandbox"), Role::Paragraph, said, cx)))
            .children(set_up)
    });
    let sandbox_problem = (provider == Provider::Codex).then_some(data.sandbox_problem).flatten();

    let body = if data.mine.is_empty() {
        let none = format!("No {short} accounts yet.");
        div().mt(r(theme.space.s12)).child(line(format!("settings.accounts.{key}.none"), Role::Paragraph, none, cx))
    } else {
        div().mt(r(theme.space.s12)).flex().flex_col().gap(r(theme.space.s16)).children(data.cards)
    };

    div()
        .semantic(format!("settings.accounts.{key}"), Role::Region)
        .aria_label(provider_name(provider))
        .mt(r(theme.space.s32))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .child(
                    div()
                        .type_style(&theme.typography.desktop.body_strong)
                        .text_color(hsla(colors.text.primary))
                        .child(provider_name(provider)),
                )
                .children(version.map(|version| {
                    line(format!("settings.accounts.{key}.version"), Role::Paragraph, format!("Version {version}"), cx)
                }))
                .child(div().flex_1())
                .child(add),
        )
        .children(install)
        .children(sandbox_row)
        .children(sandbox_problem.map(|said| problem(format!("settings.accounts.{key}.problem"), said, cx)))
        .child(body)
        .into_any_element()
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let accounts = window.use_keyed_state("settings.accounts.state", cx, |_, cx| Accounts::new(cx));
    let Some(list) = store::state(cx).accounts.clone() else {
        let loading = line("settings.accounts.loading", Role::Paragraph, "Loading accounts", cx);
        let space = theme(cx).space.s32;
        return super::frame("accounts", "Accounts", NOTE, cx).child(div().mt(r(space)).child(loading)).into_any_element();
    };
    first_check(&accounts, &list, cx);
    let stuck = stuck_cards(&accounts, &list, cx);
    let wish = wish(&accounts, cx);
    let (now, zone) = (Zoned::now(), local_zone());
    let day = usage_day_key(&now, &zone).unwrap_or_default();
    let (bots, setup) = (store::state(cx).bots.clone(), store::state(cx).setup.clone());

    // Each card's usage, its menu's focus handle, and Add's: gathered while `cx` can still change.
    let mut usage_lines: HashMap<String, String> = HashMap::new();
    let mut mores: HashMap<String, FocusHandle> = HashMap::new();
    for account in &list {
        let id = account.id.clone();
        let reader = SharedString::from(format!("settings.accounts.{id}.usage_state"));
        let usage = window.use_keyed_state(reader, cx, {
            let id = id.clone();
            move |_, cx| CardUsage::new(id, cx)
        });
        read_usage(&usage, &day, &now, &zone, cx);
        usage_lines.insert(id.clone(), usage_line(&usage, &day, account.kind, cx));
        mores.insert(id.clone(), focus_handle(&format!("settings.accounts.{id}.more"), window, cx));
    }
    let mut adds: HashMap<&str, FocusHandle> = HashMap::new();
    for provider in PROVIDERS {
        let key = key_of(provider);
        adds.insert(key, focus_handle(&format!("settings.accounts.{key}.add"), window, cx));
    }

    // The link's wish, met once its control is on the card.
    if let Some((wanted, action)) = &wish
        && let Some(account) = list.iter().find(|each| &each.id == wanted)
    {
        let flags = StatusFlags {
            stuck: stuck.get(&account.id).copied().unwrap_or(false),
            can_reopen: accounts.read(cx).urls.contains_key(&account.id),
        };
        let actions = status_line(account, &now, flags).actions;
        let focus = focus_of(*action);
        let met = accounts.read(cx).focused.as_ref() == Some(&(wanted.clone(), *action));
        if focus_ready(focus, account, &actions) && !met {
            accounts.update(cx, |accounts, _| accounts.focused = Some((wanted.clone(), *action)));
            let control = target(&account.id, focus, &actions);
            // After this draw, once the control has drawn its focus handle.
            window.defer(cx, move |window, cx| {
                guarded("focus the control a link points at", || {
                    focus_by_id(&control, window, cx);
                });
            });
        }
    }

    let cx = &*cx;
    let theme = theme(cx);
    let state = accounts.read(cx);
    let groups: Vec<AnyElement> = PROVIDERS
        .iter()
        .map(|provider| {
            let mine = accounts_of(&list, *provider);
            let cards = mine
                .iter()
                .map(|&account| {
                    let kept = state.cards.get(&account.id);
                    let data = CardData {
                        account,
                        siblings: &mine,
                        bots: &bots,
                        now: &now,
                        busy: kept.is_some_and(|card| card.busy),
                        problem: kept.and_then(|card| card.problem.clone()),
                        reopen: state.urls.get(&account.id).cloned(),
                        stuck: stuck.get(&account.id).copied().unwrap_or(false),
                        usage: usage_lines.get(&account.id).cloned().unwrap_or_default(),
                        more: mores.get(&account.id).cloned(),
                    };
                    card(data, &accounts, cx)
                })
                .collect();
            let fallback = setup.as_ref().and_then(|all| all.iter().find(|entry| provider_of(entry) == *provider));
            let data = GroupData {
                provider: *provider,
                mine: &mine,
                fallback,
                cards,
                add: adds.get(key_of(*provider)).cloned(),
                sandbox_busy: state.sandbox_busy,
                sandbox_problem: state.sandbox_problem.clone(),
            };
            group(data, &accounts, cx)
        })
        .collect();
    super::frame("accounts", "Accounts", NOTE, cx)
        .children(state.problem.clone().map(|said| div().mt(r(theme.space.s12)).child(problem("settings.accounts.problem", said, cx))))
        .children(groups)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{TestAppContext, VisualTestContext};
    use protocol::accounts::{AccountInfo, UsageReport};
    use protocol::model::{Bot, BotState};
    use protocol::wire::method;
    use serde_json::{Value, json};

    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{AccountAction, Route, Section};
    use crate::ui::testing::{app, bot, click, draw, ids, node, state, text};

    /// `{ ...base, ...patch }`: the patch's keys replace the base's.
    fn with(mut base: Value, patch: Value) -> Value {
        if let (Some(fields), Value::Object(patch)) = (base.as_object_mut(), patch) {
            fields.extend(patch);
        }
        base
    }

    fn setup(patch: Value) -> Value {
        let signed_in = json!({ "provider": "claude", "installed": true, "version": "2.0.0", "signedIn": true, "account": "Sample Account", "plan": "max", "error": null });
        with(signed_in, patch)
    }

    /// accounts.test.tsx's subscription: signed in on Max, its 5-hour window 42% used and resetting long after any test.
    fn sub(patch: Value) -> AccountInfo {
        let base = json!({
            "id": "a1", "provider": "claude", "label": "Claude (Personal)", "kind": "subscription", "isDefault": true,
            "isFallback": false, "spendingCapUsd": null, "createdAt": "2026-10-01T00:00:00.000Z", "hasCredential": false,
            "setup": setup(json!({})),
            "limits": [{ "accountId": "a1", "limitId": "l", "windowKey": "primary", "durationMinutes": 300, "usedPercent": 42,
                         "resetsAt": "2099-01-01T00:00:00.000Z", "state": "ok", "observedAt": "2026-10-22T10:00:00.000Z" }],
            "signIn": null, "spending": { "knownUsd": 0, "unknownRows": 0, "capUsd": null, "blocked": false }
        });
        serde_json::from_value(with(base, patch)).unwrap()
    }

    /// accounts.test.tsx's key: $5 spent of a $20 cap, and Claude's fallback.
    fn key(patch: Value) -> AccountInfo {
        let base = json!({
            "id": "k1", "label": "Claude key", "kind": "api_key", "isDefault": false, "isFallback": true, "spendingCapUsd": 20,
            "hasCredential": true, "setup": null, "limits": [],
            "spending": { "knownUsd": 5, "unknownRows": 0, "capUsd": 20, "blocked": false }
        });
        sub(with(base, patch))
    }

    fn signed_out() -> AccountInfo {
        sub(json!({ "setup": setup(json!({ "signedIn": false, "account": null, "plan": null, "version": null })) }))
    }

    /// A usage report with nothing in it, for every card's Today and This month.
    fn nothing_used() -> UsageReport {
        let cost = json!({ "usd": 0, "knownUsd": 0, "knownRecords": 0, "unknownRecords": 0, "reportedUsd": null, "estimatedUsd": null });
        serde_json::from_value(json!({
            "from": "2026-11-01T00:00:00.000Z", "to": "2026-11-19T10:00:00.000Z", "timeZone": "UTC",
            "summary": { "records": 0, "tokens": { "input": 0, "cachedInput": 0, "output": 0, "reasoning": 0, "total": 0 },
                         "costs": { "apiSpend": cost, "apiValue": cost, "unclassified": cost } },
            "daily": [], "byBot": [], "byAccount": [], "byModel": []
        }))
        .unwrap()
    }

    fn rpc() -> FakeRpc {
        FakeRpc::new().on::<method::UsageQuery>(|_| Ok(nothing_used()))
    }

    /// Scout, pinned to a1.
    fn scout() -> Bot {
        Bot { account_id: Some(Some("a1".into())), ..bot("b1", "Scout", BotState::Idle) }
    }

    fn open(cx: &mut TestAppContext, route: Route, accounts: Vec<AccountInfo>, rpc: FakeRpc) -> (&mut VisualTestContext, Arc<FakeRpc>) {
        app(cx, route, AppState { accounts: Some(accounts), ..state(vec![scout()], vec![], vec![]) }, rpc)
    }

    fn settings() -> Route {
        Route::Settings(Section::Accounts)
    }

    fn link(account: &str, action: AccountAction) -> Route {
        Route::Account { account: account.into(), action: Some(action) }
    }

    fn calls(rpc: &FakeRpc, method: &str) -> Vec<Value> {
        rpc.calls().into_iter().filter(|(name, _)| *name == method).map(|(_, params)| params).collect()
    }

    /// accounts.test.tsx: "shows a signed-in subscription: its plan, one meter per limit, and who uses it".
    #[gpui::test]
    fn shows_a_signed_in_subscription(cx: &mut TestAppContext) {
        open(cx, settings(), vec![sub(json!({}))], rpc());
        assert_eq!(text("settings.accounts.a1"), "Claude (Personal)");
        assert_eq!(text("settings.accounts.a1.default"), "Default");
        assert_eq!(text("settings.accounts.a1.kind"), "Subscription · Max plan · Sample Account");
        assert_eq!(text("settings.accounts.a1.status"), "Signed in");
        assert_eq!(ids("settings.accounts.a1.limit."), ["settings.accounts.a1.limit.l.primary"]);
        assert_eq!(text("settings.accounts.a1.limit.l.primary"), "5-hour limit");
        assert_eq!(text("settings.accounts.a1.users"), "1 agent");
        assert_eq!(text("settings.accounts.a1.more"), "More for Claude (Personal)");
        assert!(text("settings.accounts.a1.usage").starts_with("Today "));
        assert_eq!(text("settings.accounts.codex.none"), "No Codex accounts yet.");
    }

    /// accounts.test.tsx: "offers Sign in on a signed-out subscription, and Cancel and the page again while one is
    /// waiting"; the page's address is the one the sign-in gave.
    #[gpui::test]
    fn offers_sign_in_then_cancel_and_the_page_again_while_waiting(cx: &mut TestAppContext) {
        let started = json!({ "url": "https://example.test/signin", "attempt": "t1" });
        let rpc = rpc().on::<method::AccountsSignIn>(move |_| Ok(serde_json::from_value(started.clone()).unwrap()));
        let (cx, calls_made) = open(cx, settings(), vec![signed_out()], rpc);
        assert_eq!(text("settings.accounts.a1.status"), "Not signed in. Agents on it wait.");
        click(cx, "settings.accounts.a1.signin");
        draw(cx);
        assert_eq!(calls(&calls_made, "accounts.signIn"), [json!({ "id": "a1" })]);
        cx.update(|_, cx| {
            let attempt = serde_json::from_value(json!({ "attempt": "t1", "state": "waiting" })).unwrap();
            let waiting = AccountInfo { sign_in: Some(attempt), ..signed_out() };
            crate::ui::store::fake_event(cx, &protocol::Event::AccountsChanged(vec![waiting]));
        });
        draw(cx);
        assert_eq!(text("settings.accounts.a1.status"), "Finish signing in in your browser");
        assert!(node("settings.accounts.a1.cancel").is_some());
        assert!(node("settings.accounts.a1.open_again").is_some());
    }

    /// accounts.test.tsx: "says how an API key is doing: spending against its cap, the fallback switch, and what replaces".
    #[gpui::test]
    fn says_how_an_api_key_is_doing(cx: &mut TestAppContext) {
        let keys = vec![key(json!({ "isFallback": false })), key(json!({ "id": "k2", "label": "Other key", "isFallback": true }))];
        open(cx, settings(), keys, rpc());
        assert_eq!(text("settings.accounts.k1.kind"), "API key");
        assert_eq!(text("settings.accounts.k1.status"), "Key saved");
        assert_eq!(text("settings.accounts.k1.spending"), "Spending");
        assert_eq!(text("settings.accounts.k1.fallback"), "Fallback for Claude");
        assert!(text("settings.accounts.k1.fallback_note").ends_with("until it resets. Replaces Other key."));
        assert!(!text("settings.accounts.k2.fallback_note").contains("Replaces"));
        assert_eq!(text("settings.accounts.k1.cap"), "Edit cap");
    }

    /// accounts.test.tsx: "stops an API key at its cap, and at unpriced turns, with the right way forward".
    #[gpui::test]
    fn stops_an_api_key_at_its_cap_and_at_unpriced_turns(cx: &mut TestAppContext) {
        let capped = key(json!({ "spending": { "knownUsd": 20, "unknownRows": 0, "capUsd": 20, "blocked": true } }));
        let unpriced = key(json!({ "id": "k2", "label": "Other key", "spending": { "knownUsd": 3, "unknownRows": 2, "capUsd": 20, "blocked": true } }));
        open(cx, settings(), vec![capped, unpriced], rpc());
        assert_eq!(text("settings.accounts.k1.status"), "Reached its $20.00 cap. Agents stopped using it.");
        assert!(node("settings.accounts.k1.raise_cap").is_some());
        assert_eq!(text("settings.accounts.k2.status"), "Stopped: 2 turns have no price, so the cap can't be checked.");
        assert!(node("settings.accounts.k2.raise_cap").is_none());
    }

    /// accounts.test.tsx: "offers Retry queued work, with its caption, for an unknown reset, and never starts one because a
    /// link pointed at it": the link focuses it, and asks nothing of the daemon.
    #[gpui::test]
    fn a_retry_link_focuses_retry_queued_work_and_starts_nothing(cx: &mut TestAppContext) {
        let limited = sub(json!({ "limits": [{ "accountId": "a1", "limitId": "l", "windowKey": "primary", "durationMinutes": 300,
            "usedPercent": 42, "resetsAt": null, "state": "reached", "observedAt": "2026-10-22T10:00:00.000Z" }] }));
        let (cx, calls_made) = open(cx, link("a1", AccountAction::Retry), vec![limited], rpc());
        draw(cx);
        assert_eq!(text("settings.accounts.a1.status"), "Limited. Reset time unknown.");
        let caption = "Retries one queued turn; it may use subscription allowance or API credit.";
        assert_eq!(text("settings.accounts.a1.retry_caption"), caption);
        assert!(node("settings.accounts.a1.retry").unwrap().focused, "the link's control has the focus");
        assert!(calls(&calls_made, "accounts.retryLimit").is_empty());
    }

    /// accounts.test.tsx: "never starts a sign-in because a link pointed at the card"; it focuses Sign in.
    #[gpui::test]
    fn a_sign_in_link_focuses_sign_in_and_starts_nothing(cx: &mut TestAppContext) {
        let (cx, calls_made) = open(cx, link("a1", AccountAction::SignIn), vec![signed_out()], rpc());
        draw(cx);
        assert!(node("settings.accounts.a1.signin").unwrap().focused);
        assert!(calls(&calls_made, "accounts.signIn").is_empty());
        assert!(calls(&calls_made, "accounts.create").is_empty());
    }

    /// A Raise cap link focuses the key's Edit cap while the cap hasn't stopped it.
    #[gpui::test]
    fn a_cap_link_focuses_the_cap(cx: &mut TestAppContext) {
        let (cx, _) = open(cx, link("k1", AccountAction::Cap), vec![key(json!({}))], rpc());
        draw(cx);
        assert!(node("settings.accounts.k1.cap").unwrap().focused);
    }

    /// accounts.test.tsx: "holds the API key in a masked field … with a Show button" and "keeps Add key disabled until
    /// there is a name, a key and a cap"; the key leaves its field as it's sent.
    #[gpui::test]
    fn adds_a_key_only_with_a_name_a_key_and_a_cap(cx: &mut TestAppContext) {
        let rpc = rpc().on::<method::AccountsCreate>(|_| Ok(key(json!({ "id": "k9" }))));
        let (cx, calls_made) = open(cx, settings(), vec![], rpc);
        click(cx, "settings.accounts.codex.add");
        click(cx, "settings.accounts.codex.add.key");
        assert_eq!(text("dialog.title"), "Add Codex API key");
        assert_eq!(text("account_form.show_key"), "Show key");
        assert_eq!(text("account_form.key_help"), "Stored in Windows Credential Manager and given only to the Codex process.");
        assert_eq!(text("account_form.fallback"), "Fallback for Codex");
        click(cx, "account_form.save");
        assert!(calls(&calls_made, "accounts.create").is_empty(), "off until a name, a key and a cap");
        click(cx, "account_form.name");
        cx.simulate_input("Work key");
        click(cx, "account_form.key");
        cx.simulate_input("sample-key-not-real");
        click(cx, "account_form.cap");
        cx.simulate_input("25");
        draw(cx);
        click(cx, "account_form.save");
        draw(cx);
        let created = calls(&calls_made, "accounts.create");
        assert_eq!(created.len(), 1);
        assert_eq!(created[0]["kind"], "api_key");
        assert_eq!(created[0]["label"], "Work key");
        assert_eq!(created[0]["apiKey"], "sample-key-not-real");
        assert_eq!(created[0]["spendingCapUsd"], 25.0);
        assert_eq!(created[0]["isFallback"], false);
        assert!(node("account_form.save").is_none(), "added: the dialog closed");
    }

    /// accounts.test.tsx: "warns when a cap is below what is already spent" and "replaces a key with the key alone";
    /// rename starts with the name, and Save sends the change.
    #[gpui::test]
    fn the_cap_replace_and_rename_dialogs(cx: &mut TestAppContext) {
        let spent = key(json!({ "spendingCapUsd": 10, "spending": { "knownUsd": 25, "unknownRows": 0, "capUsd": 10, "blocked": true } }));
        let rpc = rpc().on::<method::AccountsUpdate>(|_| Ok(key(json!({}))));
        let (cx, calls_made) = open(cx, settings(), vec![spent], rpc);
        click(cx, "settings.accounts.k1.cap");
        assert_eq!(text("dialog.title"), "Spending cap");
        let below = "This is below the $25.00 already spent, so agents stop using the key right away.";
        assert_eq!(text("account_form.cap_help"), below);
        click(cx, "account_form.cancel");
        click(cx, "settings.accounts.k1.more");
        click(cx, "settings.accounts.k1.replace_key");
        assert_eq!(text("dialog.title"), "Replace key");
        assert!(node("account_form.key").is_some());
        assert!(node("account_form.name").is_none() && node("account_form.cap").is_none());
        click(cx, "account_form.cancel");
        click(cx, "settings.accounts.k1.more");
        click(cx, "settings.accounts.k1.rename");
        assert_eq!(text("dialog.title"), "Rename account");
        click(cx, "account_form.save");
        assert!(calls(&calls_made, "accounts.update").is_empty(), "off until the name changes");
        click(cx, "account_form.name");
        cx.simulate_input(" 2");
        draw(cx);
        click(cx, "account_form.save");
        draw(cx);
        assert_eq!(calls(&calls_made, "accounts.update"), [json!({ "id": "k1", "patch": { "label": "Claude key 2" } })]);
    }

    /// account-dialogs.tsx: an account with agents on it can't be removed, and says which to move; one without asks first.
    #[gpui::test]
    fn remove_names_the_agents_to_move_first(cx: &mut TestAppContext) {
        let rpc = rpc().on::<method::AccountsRemove>(|_| Ok(protocol::wire::Empty {}));
        let (cx, calls_made) = open(cx, settings(), vec![sub(json!({})), key(json!({}))], rpc);
        click(cx, "settings.accounts.a1.more");
        click(cx, "settings.accounts.a1.remove");
        assert_eq!(text("dialog.title"), "Can't remove Claude (Personal)");
        assert_eq!(text("account_remove.bot.b1"), "Scout");
        click(cx, "account_remove.ok");
        click(cx, "settings.accounts.k1.more");
        click(cx, "settings.accounts.k1.remove");
        assert_eq!(text("dialog.title"), "Remove Claude key?");
        assert_eq!(text("account_form.note"), "The key is removed from Windows Credential Manager. Usage history stays.");
        click(cx, "account_form.save");
        draw(cx);
        assert_eq!(calls(&calls_made, "accounts.remove"), [json!({ "id": "k1" })]);
    }
}
