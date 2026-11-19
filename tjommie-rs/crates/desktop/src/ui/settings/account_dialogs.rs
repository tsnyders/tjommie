//! The account dialogs (`accounts/account-dialogs.tsx`): add a subscription, add an API key, replace a key, the
//! spending cap, rename, and remove. Each keeps what's typed in its own state, so closing it by any route throws that
//! away. The API key lives only in its field until it's sent: `take_key` empties the field before the request goes, so
//! it's never shown again. A dialog's save closes only that dialog (`Accounts::epoch`, as `closeEpoch`).

use client::ClientMethod;
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Entity, IntoElement, ParentElement as _, Role,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, px,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::InputState;
use protocol::accounts::{AccountInfo, AccountInput, AccountKind, AccountPatch, AccountsUpdate, ApiKey};
use protocol::model::{Bot, Provider};
use protocol::wire::{Id, method};

use super::accounts::{Accounts, short_name};
use crate::guard::{guarded, guarded_async};
use crate::model::accounts::{bots_on, key_ready, money, parse_cap, remember_url, take_key};
use crate::model::labels::error_text;
use crate::model::setup::provider_name;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Link, TextField, label, problem};
use crate::ui::primitives::button::{IconButton, Pill, PillVariant};
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::primitives::toggle::Toggle;
use crate::ui::projects::line;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

const CAP_HELP: &str = "Agents stop using the key once its total spend reaches this. It's an estimate from the prices \
                        Tjommie knows, not an invoice. Your provider bill may be higher.";

/// What each form keeps besides its fields (`Form`): busy while its request runs, and why it failed.
#[derive(Default)]
struct Form {
    busy: bool,
    problem: Option<String>,
}

/// The next dialog's epoch: a save closes its dialog only while no other has opened since.
fn open_epoch(accounts: &Entity<Accounts>, cx: &mut App) -> u64 {
    accounts.update(cx, |accounts, _| {
        accounts.epoch += 1;
        accounts.epoch
    })
}

/// Closes the dialog whose save finished, if it's still the latest one opened (`closeEpoch`).
fn close(accounts: &WeakEntity<Accounts>, epoch: u64, window: AnyWindowHandle, cx: &mut App) {
    if accounts.read_with(cx, |accounts, _| accounts.epoch == epoch).unwrap_or(false) {
        let _ = window.update(cx, |_, window, cx| window.close_dialog(cx));
    }
}

/// The form is sending.
fn busy<T: 'static>(dialog: &Entity<T>, form: fn(&mut T) -> &mut Form, cx: &mut App) {
    dialog.update(cx, |dialog, cx| {
        let form = form(dialog);
        form.busy = true;
        form.problem = None;
        cx.notify();
    });
}

/// The form's request failed (or was refused before it went): it says why and keeps what was typed.
fn failed<T: 'static>(dialog: &WeakEntity<T>, form: fn(&mut T) -> &mut Form, said: String, cx: &mut App) {
    let _ = dialog.update(cx, |dialog, cx| {
        let form = form(dialog);
        form.busy = false;
        form.problem = Some(said);
        cx.notify();
    });
}

/// Sends the form's one request: done, the dialog closes (while it's still the latest), else it says why.
fn send<T: 'static, M: ClientMethod>(
    dialog: &Entity<T>,
    form: fn(&mut T) -> &mut Form,
    params: M::Params,
    accounts: &Entity<Accounts>,
    epoch: u64,
    window: AnyWindowHandle,
    cx: &mut App,
) {
    busy(dialog, form, cx);
    let call = store::request::<M>(cx, params);
    let (dialog, accounts) = (dialog.downgrade(), accounts.downgrade());
    cx.spawn(async move |cx| {
        guarded_async("save an account", async move {
            let done = call.await;
            cx.update(|cx| match done {
                Ok(_) => close(&accounts, epoch, window, cx),
                Err(error) => failed(&dialog, form, error_text(&error), cx),
            });
        })
        .await;
    })
    .detach();
}

/// Focuses the dialog's first field once it's drawn.
fn focus_soon(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let input = input.clone();
    window.defer(cx, move |window, cx| {
        guarded("focus the account form's first field", || input.update(cx, |input, cx| input.focus(window, cx)));
    });
}

/// A labelled field, with a line of help under it (`Field`).
fn field(text: &'static str, input: impl IntoElement, help: Option<AnyElement>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .mt(r(theme.space.s16))
        .flex()
        .flex_col()
        .child(label(text, cx))
        .child(input)
        .children(help.map(|help| div().mt(r(theme.space.s6)).child(help)))
        .into_any_element()
}

fn name_field(name: &Entity<InputState>, cx: &App) -> AnyElement {
    field("Name", TextField::new("account_form.name", "Name", name), None, cx)
}

/// The key (`KeyField`): masked, with Show key, and the line saying where it's kept.
fn key_field(provider: Provider, key: &Entity<InputState>, shown: bool, on_show: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    let (icon, toggle) = if shown { ("eye-off", "Hide key") } else { ("eye", "Show key") };
    let help = format!("Stored in Windows Credential Manager and given only to the {} process.", provider_name(provider));
    let input = div()
        .relative()
        .child(TextField::new("account_form.key", "API key", key))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .right(px(2.))
                .child(IconButton::new("account_form.show_key", icon, toggle).on_press(on_show)),
        );
    field("API key", input, Some(line("account_form.key_help", Role::Paragraph, help, cx)), cx)
}

/// The cap in dollars (`CapField`), with what it means, or a warning when it's below what's already spent.
fn cap_field(cap: &Entity<InputState>, below: Option<String>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let input = div()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .child(div().type_style(&theme.typography.desktop.ui).text_color(hsla(palette(cx).text.secondary)).child("$"))
        .child(div().flex_1().child(TextField::new("account_form.cap", "Spending cap in dollars", cap)));
    let help = below.unwrap_or_else(|| CAP_HELP.to_owned());
    field("Spending cap", input, Some(line("account_form.cap_help", Role::Paragraph, help, cx)), cx)
}

/// Cancel and the form's own button, off until the form can go (`Form`'s buttons).
fn buttons(submit: &'static str, danger: bool, ready: bool, on_submit: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let pill = Pill::new("account_form.save", submit)
        .variant(if danger { PillVariant::Danger } else { PillVariant::Primary })
        .disabled(!ready)
        .on_press(on_submit)
        .into_any_element();
    div()
        .mt(r(theme.space.s20))
        .flex()
        .justify_end()
        .gap(r(theme.space.s8))
        .child(Pill::new("account_form.cancel", "Cancel").on_press(|window, cx| window.close_dialog(cx)))
        .child(pill)
        .into_any_element()
}

/// Show key and Hide key: the field unmasks, or masks again.
fn toggle_shown<T: 'static>(dialog: &Entity<T>, parts: fn(&mut T) -> (&mut bool, Entity<InputState>), window: &mut Window, cx: &mut App) {
    dialog.update(cx, |dialog, cx| {
        let (shown, key) = parts(dialog);
        *shown = !*shown;
        let masked = !*shown;
        key.update(cx, |key, cx| key.set_masked(masked, window, cx));
        cx.notify();
    });
}

/// Add subscription (`AddSubscriptionDialog`): a name, then the provider's own sign-in page.
struct NewSubscription {
    name: Entity<InputState>,
    form: Form,
}

pub(super) fn open_add_subscription(provider: Provider, accounts: Entity<Accounts>, window: &mut Window, cx: &mut App) {
    let placeholder = format!("{} (Personal)", short_name(provider));
    let name = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    let dialog = cx.new(|_| NewSubscription { name: name.clone(), form: Form::default() });
    let epoch = open_epoch(&accounts, cx);
    let handle = window.window_handle();
    open_dialog(window, cx, move |surface, window, cx| {
        let state = dialog.read(cx);
        let ready = !state.name.read(cx).value().trim().is_empty() && !state.form.busy;
        let note = "Signing in opens the provider's own page in your browser. Tjommie never sees your password.";
        let save = {
            let (dialog, accounts) = (dialog.clone(), accounts.clone());
            move |_: &mut Window, cx: &mut App| add_subscription(&dialog, &accounts, provider, epoch, handle, cx)
        };
        surface
            .w(window.rem_size() * 26.25)
            .title(title(format!("Add {} subscription", provider_name(provider)), cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(name_field(&state.name, cx))
                    .child(div().mt(r(theme(cx).space.s12)).child(line("account_form.note", Role::Paragraph, note, cx)))
                    .children(state.form.problem.clone().map(|said| problem("account_form.problem", said, cx)))
                    .child(buttons("Add and sign in", false, ready, save, cx)),
            )
    });
    focus_soon(&name, window, cx);
}

/// Creates the subscription, then starts its sign-in (`AccountsSection`'s onSubmit). The account exists once it's
/// created: if its sign-in can't start, the section says so and the card offers Try again; creating it again would only
/// collide.
fn add_subscription(
    dialog: &Entity<NewSubscription>,
    accounts: &Entity<Accounts>,
    provider: Provider,
    epoch: u64,
    window: AnyWindowHandle,
    cx: &mut App,
) {
    let label = dialog.read(cx).name.read(cx).value().trim().to_owned();
    let form: fn(&mut NewSubscription) -> &mut Form = |dialog| &mut dialog.form;
    busy(dialog, form, cx);
    let create = store::request::<method::AccountsCreate>(cx, AccountInput::Subscription { provider, label });
    let (dialog, accounts) = (dialog.downgrade(), accounts.downgrade());
    cx.spawn(async move |cx| {
        guarded_async("add a subscription", async move {
            let created = match create.await {
                Ok(created) => created,
                Err(error) => {
                    cx.update(|cx| failed(&dialog, form, error_text(&error), cx));
                    return;
                }
            };
            let start = cx.update(|cx| store::request::<method::AccountsSignIn>(cx, Id { id: created.id.clone() }));
            let started = start.await;
            cx.update(|cx| {
                let _ = accounts.update(cx, |accounts, cx| {
                    match started {
                        Ok(started) => accounts.urls = remember_url(&accounts.urls, &created.id, started.url.as_deref()),
                        Err(error) => accounts.problem = Some(error_text(&error)),
                    }
                    cx.notify();
                });
                close(&accounts, epoch, window, cx);
            });
        })
        .await;
    })
    .detach();
}

/// Add API key (`AddKeyDialog`): a name, the key, its cap, and whether it's the provider's fallback.
struct NewKey {
    name: Entity<InputState>,
    key: Entity<InputState>,
    shown: bool,
    cap: Entity<InputState>,
    fallback: bool,
    form: Form,
}

pub(super) fn open_add_key(provider: Provider, accounts: Entity<Accounts>, window: &mut Window, cx: &mut App) {
    let placeholder = format!("{} (API key)", short_name(provider));
    let name = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    let key = cx.new(|cx| InputState::new(window, cx).masked(true));
    let cap = cx.new(|cx| InputState::new(window, cx));
    let dialog = cx.new(|_| NewKey { name: name.clone(), key, shown: false, cap, fallback: false, form: Form::default() });
    let epoch = open_epoch(&accounts, cx);
    let handle = window.window_handle();
    open_dialog(window, cx, move |surface, window, cx| {
        let state = dialog.read(cx);
        let ready = !state.name.read(cx).value().trim().is_empty()
            && key_ready(&state.key.read(cx).value())
            && parse_cap(&state.cap.read(cx).value()).is_some()
            && !state.form.busy;
        let short = short_name(provider);
        let show = {
            let dialog = dialog.clone();
            move |window: &mut Window, cx: &mut App| {
                toggle_shown(&dialog, |dialog: &mut NewKey| (&mut dialog.shown, dialog.key.clone()), window, cx);
            }
        };
        let flip = {
            let dialog = dialog.clone();
            move |on: bool, _: &mut Window, cx: &mut App| {
                dialog.update(cx, |dialog, cx| {
                    dialog.fallback = on;
                    cx.notify();
                });
            }
        };
        let save = {
            let (dialog, accounts) = (dialog.clone(), accounts.clone());
            move |window: &mut Window, cx: &mut App| add_key(&dialog, &accounts, provider, epoch, handle, window, cx)
        };
        let theme = theme(cx);
        let fallback = div()
            .mt(r(theme.space.s16))
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
                            .type_style(&theme.typography.desktop.body)
                            .text_color(hsla(palette(cx).text.primary))
                            .child(format!("Fallback for {short}")),
                    )
                    .child(line(
                        "account_form.fallback_note",
                        Role::Paragraph,
                        format!("When a {short} subscription account hits its limit, agents carry on with this key until it resets."),
                        cx,
                    )),
            )
            .child(Toggle::new("account_form.fallback", format!("Fallback for {short}"), state.fallback).on_toggle(flip));
        surface
            .w(window.rem_size() * 26.25)
            .title(title(format!("Add {} API key", provider_name(provider)), cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(name_field(&state.name, cx))
                    .child(key_field(provider, &state.key, state.shown, show, cx))
                    .child(cap_field(&state.cap, None, cx))
                    .child(fallback)
                    .children(state.form.problem.clone().map(|said| problem("account_form.problem", said, cx)))
                    .child(buttons("Add key", false, ready, save, cx)),
            )
    });
    focus_soon(&name, window, cx);
}

/// Takes the key out of its field (emptied before anything is sent) as the wire's `ApiKey`, or says why it can't go.
fn take<T: 'static>(dialog: &Entity<T>, form: fn(&mut T) -> &mut Form, key: &Entity<InputState>, window: &mut Window, cx: &mut App) -> Option<ApiKey> {
    let typed = key.read(cx).value().to_string();
    let secret = take_key(&typed, || key.update(cx, |key, cx| key.set_value("", window, cx)));
    match ApiKey::new(&secret) {
        Ok(key) => Some(key),
        Err(invalid) => {
            failed(&dialog.downgrade(), form, invalid.0, cx);
            None
        }
    }
}

fn add_key(
    dialog: &Entity<NewKey>,
    accounts: &Entity<Accounts>,
    provider: Provider,
    epoch: u64,
    handle: AnyWindowHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let (label, cap, fallback, key) = {
        let state = dialog.read(cx);
        let label = state.name.read(cx).value().trim().to_owned();
        (label, parse_cap(&state.cap.read(cx).value()), state.fallback, state.key.clone())
    };
    let Some(cap) = cap else {
        return;
    };
    let form: fn(&mut NewKey) -> &mut Form = |dialog| &mut dialog.form;
    let Some(api_key) = take(dialog, form, &key, window, cx) else {
        return;
    };
    let input = AccountInput::ApiKey { provider, label, api_key, spending_cap_usd: cap, is_fallback: Some(fallback) };
    send::<NewKey, method::AccountsCreate>(dialog, form, input, accounts, epoch, handle, cx);
}

/// Replace key (`ReplaceKeyDialog`): the key goes alone, never with any other change to the account.
struct ReplaceKey {
    key: Entity<InputState>,
    shown: bool,
    form: Form,
}

pub(super) fn open_replace_key(account: &AccountInfo, accounts: Entity<Accounts>, window: &mut Window, cx: &mut App) {
    let key = cx.new(|cx| InputState::new(window, cx).masked(true));
    let dialog = cx.new(|_| ReplaceKey { key: key.clone(), shown: false, form: Form::default() });
    let epoch = open_epoch(&accounts, cx);
    let handle = window.window_handle();
    let (id, provider) = (account.id.clone(), account.provider);
    open_dialog(window, cx, move |surface, window, cx| {
        let state = dialog.read(cx);
        let ready = key_ready(&state.key.read(cx).value()) && !state.form.busy;
        let show = {
            let dialog = dialog.clone();
            move |window: &mut Window, cx: &mut App| {
                toggle_shown(&dialog, |dialog: &mut ReplaceKey| (&mut dialog.shown, dialog.key.clone()), window, cx);
            }
        };
        let save = {
            let (dialog, accounts, id) = (dialog.clone(), accounts.clone(), id.clone());
            move |window: &mut Window, cx: &mut App| {
                let form: fn(&mut ReplaceKey) -> &mut Form = |dialog| &mut dialog.form;
                let key = dialog.read(cx).key.clone();
                if let Some(api_key) = take(&dialog, form, &key, window, cx) {
                    let patch = AccountPatch { api_key: Some(api_key), ..AccountPatch::default() };
                    let params = AccountsUpdate { id: id.clone(), patch };
                    send::<ReplaceKey, method::AccountsUpdate>(&dialog, form, params, &accounts, epoch, handle, cx);
                }
            }
        };
        surface
            .w(window.rem_size() * 26.25)
            .title(title("Replace key", cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(key_field(provider, &state.key, state.shown, show, cx))
                    .children(state.form.problem.clone().map(|said| problem("account_form.problem", said, cx)))
                    .child(buttons("Save key", false, ready, save, cx)),
            )
    });
    focus_soon(&key, window, cx);
}

/// Spending cap (`CapDialog`): starts at the cap, and warns when a new one is below what's already spent.
struct Cap {
    cap: Entity<InputState>,
    form: Form,
}

pub(super) fn open_cap(account: &AccountInfo, accounts: Entity<Accounts>, window: &mut Window, cx: &mut App) {
    let start = account.spending_cap_usd.map(|cap| cap.to_string()).unwrap_or_default();
    let cap = cx.new(|cx| InputState::new(window, cx).default_value(start));
    let dialog = cx.new(|_| Cap { cap: cap.clone(), form: Form::default() });
    let epoch = open_epoch(&accounts, cx);
    let handle = window.window_handle();
    let (id, spent) = (account.id.clone(), account.spending.known_usd);
    open_dialog(window, cx, move |surface, window, cx| {
        let state = dialog.read(cx);
        let amount = parse_cap(&state.cap.read(cx).value());
        let below = amount
            .filter(|amount| *amount < spent)
            .map(|_| format!("This is below the {} already spent, so agents stop using the key right away.", money(spent)));
        let ready = amount.is_some() && !state.form.busy;
        let save = {
            let (dialog, accounts, id) = (dialog.clone(), accounts.clone(), id.clone());
            move |_: &mut Window, cx: &mut App| {
                let Some(amount) = parse_cap(&dialog.read(cx).cap.read(cx).value()) else {
                    return;
                };
                let patch = AccountPatch { spending_cap_usd: Some(amount), ..AccountPatch::default() };
                let params = AccountsUpdate { id: id.clone(), patch };
                send::<Cap, method::AccountsUpdate>(&dialog, |dialog| &mut dialog.form, params, &accounts, epoch, handle, cx);
            }
        };
        surface
            .w(window.rem_size() * 20.0)
            .title(title("Spending cap", cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(cap_field(&state.cap, below, cx))
                    .children(state.form.problem.clone().map(|said| problem("account_form.problem", said, cx)))
                    .child(buttons("Save", false, ready, save, cx)),
            )
    });
    focus_soon(&cap, window, cx);
}

/// Rename (`RenameDialog`): starts with the name; Save is off until it changes.
struct Rename {
    name: Entity<InputState>,
    form: Form,
}

pub(super) fn open_rename(account: &AccountInfo, accounts: Entity<Accounts>, window: &mut Window, cx: &mut App) {
    let start = account.label.clone();
    let name = cx.new(|cx| InputState::new(window, cx).placeholder("Account name").default_value(start));
    let dialog = cx.new(|_| Rename { name: name.clone(), form: Form::default() });
    let epoch = open_epoch(&accounts, cx);
    let handle = window.window_handle();
    let (id, was) = (account.id.clone(), account.label.clone());
    open_dialog(window, cx, move |surface, window, cx| {
        let state = dialog.read(cx);
        let typed = state.name.read(cx).value().trim().to_owned();
        let ready = !typed.is_empty() && typed != was && !state.form.busy;
        let save = {
            let (dialog, accounts, id) = (dialog.clone(), accounts.clone(), id.clone());
            move |_: &mut Window, cx: &mut App| {
                let label = dialog.read(cx).name.read(cx).value().trim().to_owned();
                let params = AccountsUpdate { id: id.clone(), patch: AccountPatch { label: Some(label), ..AccountPatch::default() } };
                send::<Rename, method::AccountsUpdate>(&dialog, |dialog| &mut dialog.form, params, &accounts, epoch, handle, cx);
            }
        };
        surface
            .w(window.rem_size() * 20.0)
            .title(title("Rename account", cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(name_field(&state.name, cx))
                    .children(state.form.problem.clone().map(|said| problem("account_form.problem", said, cx)))
                    .child(buttons("Save", false, ready, save, cx)),
            )
    });
    focus_soon(&name, window, cx);
}

/// Remove (`RemoveDialog`). An account with agents on it can't be removed: the dialog says which agents to move first.
/// Otherwise it says what is and isn't removed; a sign-in the provider's own tool keeps in its folder isn't claimed to
/// be deleted.
struct Remove {
    form: Form,
}

pub(super) fn open_remove(account: &AccountInfo, bots: &[Bot], accounts: Entity<Accounts>, window: &mut Window, cx: &mut App) {
    let using: Vec<(String, String)> =
        bots_on(&account.id, bots).into_iter().map(|bot| (bot.id.clone(), bot.name.clone())).collect();
    let name = account.label.clone();
    if !using.is_empty() {
        open_dialog(window, cx, move |surface, window, cx| {
            let theme = theme(cx);
            let links = using.iter().map(|(id, name)| {
                let route = Route::BotSettings(id.clone());
                Link::new(format!("account_remove.bot.{id}"), name.clone()).on_press(move |window, cx| {
                    window.close_dialog(cx);
                    cx.navigate(route.clone());
                })
            });
            surface
                .w(window.rem_size() * 20.0)
                .title(title(format!("Can't remove {name}"), cx))
                // unguarded: returns a constant, nothing in it can panic
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(r(theme.space.s8))
                        .child(line("account_remove.note", Role::Paragraph, "Move these agents to another account first:", cx))
                        .child(
                            div()
                                .semantic("account_remove.bots", Role::List)
                                .aria_label("Agents on this account")
                                .flex()
                                .flex_col()
                                .items_start()
                                .gap(r(theme.space.s6))
                                .children(links),
                        )
                        .child(
                            div().mt(r(theme.space.s12)).flex().justify_end().child(
                                Pill::new("account_remove.ok", "OK")
                                    .variant(PillVariant::Primary)
                                    .on_press(|window, cx| window.close_dialog(cx)),
                            ),
                        ),
                )
        });
        return;
    }
    let dialog = cx.new(|_| Remove { form: Form::default() });
    let epoch = open_epoch(&accounts, cx);
    let handle = window.window_handle();
    let what = match account.kind {
        AccountKind::ApiKey => "The key is removed from Windows Credential Manager.",
        AccountKind::Subscription => "Disconnects this account from Tjommie.",
    };
    let id = account.id.clone();
    open_dialog(window, cx, move |surface, window, cx| {
        let state = dialog.read(cx);
        let save = {
            let (dialog, accounts, id) = (dialog.clone(), accounts.clone(), id.clone());
            move |_: &mut Window, cx: &mut App| {
                send::<Remove, method::AccountsRemove>(&dialog, |dialog| &mut dialog.form, Id { id: id.clone() }, &accounts, epoch, handle, cx);
            }
        };
        surface
            .w(window.rem_size() * 20.0)
            .title(title(format!("Remove {name}?"), cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(line("account_form.note", Role::Paragraph, format!("{what} Usage history stays."), cx))
                    .children(state.form.problem.clone().map(|said| problem("account_form.problem", said, cx)))
                    .child(buttons("Remove", true, !state.form.busy, save, cx)),
            )
    });
}
