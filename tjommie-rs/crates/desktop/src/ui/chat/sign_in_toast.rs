//! Account notices (`sign-in-toast.tsx`): an account or provider that isn't signed in, or couldn't be checked, says so
//! in one dismissible notice under the chat's header, with the way to fix it (Sign in, or Settings), instead of taking
//! the chat's space. The chat raises it and the shell draws it, as the app-level toast viewport did, so it stays
//! through a navigation. Each is shown once per PC: a dismissed one stays closed through status refreshes and revisits,
//! and comes back only after that account or provider has signed in and signed out again. It closes by itself after
//! 8 s.

use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    AnyElement, App, Global, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, svg,
};
use protocol::model::{Provider, ProviderSetup, SetupFields};

use crate::guard::guarded_async;
use crate::model::state::AppState;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::Link;
use crate::ui::primitives::button::IconButton;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// How long a notice stays before it closes by itself.
const SHOWN_FOR: Duration = Duration::from_secs(8);

/// What the notice's button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    SignIn,
    Settings,
}

impl Fix {
    fn label(self) -> &'static str {
        match self {
            Fix::SignIn => "Sign in",
            Fix::Settings => "Settings",
        }
    }
}

#[derive(Clone)]
struct Shown {
    serial: u64,
    title: SharedString,
    fix: Fix,
    route: Route,
}

/// The notices this PC has shown (by host and subject), and the one on screen.
#[derive(Default)]
pub struct SignInNotices {
    shown: HashSet<(Option<String>, String)>,
    current: Option<Shown>,
    serial: u64,
}

impl Global for SignInNotices {}

/// An account's subject.
pub fn account_subject(id: &str) -> String {
    format!("account:{id}")
}

/// A provider's subject.
pub fn provider_subject(provider: Provider) -> String {
    format!("provider:{}", format!("{provider:?}").to_lowercase())
}

/// Shows the notice for `subject` unless this PC already has. It closes itself after 8 s (a GPUI timer: real time in
/// the app, the test's clock in a test), unless a newer notice has taken its place. The banner asks on every frame it
/// draws, so a notice already shown is found without a copy (M4 review A9).
pub fn raise(subject: &str, title: String, fix: Fix, route: Route, cx: &mut App) {
    let host = store::state(cx).host.as_deref();
    let known = cx.try_global::<SignInNotices>().is_some_and(|notices| {
        notices
            .shown
            .iter()
            .any(|(at, shown)| at.as_deref() == host && shown == subject)
    });
    if known {
        return;
    }
    let host = store::state(cx).host.clone();
    let notices = cx.default_global::<SignInNotices>();
    notices.shown.insert((host, subject.to_owned()));
    // Already on screen (forgotten by a sign-in the banner hasn't caught up with yet): marked again, nothing more. A new
    // notice here would redraw the window, and the next frame would forget and raise it again, without end.
    if notices
        .current
        .as_ref()
        .is_some_and(|shown| shown.title.as_ref() == title)
    {
        return;
    }
    notices.serial += 1;
    let serial = notices.serial;
    notices.current = Some(Shown {
        serial,
        title: title.into(),
        fix,
        route,
    });
    // The shell has already drawn this frame's notice (none): the next frame shows it.
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        guarded_async("close an account notice", async move {
            cx.background_executor().timer(SHOWN_FOR).await;
            cx.update(|cx| {
                let notices = cx.default_global::<SignInNotices>();
                if notices
                    .current
                    .as_ref()
                    .is_some_and(|shown| shown.serial == serial)
                {
                    notices.current = None;
                    cx.refresh_windows();
                }
            });
        })
        .await;
    })
    .detach();
}

fn fields(setup: &ProviderSetup) -> (Provider, &SetupFields) {
    match setup {
        ProviderSetup::Claude(fields) => (Provider::Claude, fields),
        ProviderSetup::Codex(codex) => (Provider::Codex, &codex.setup),
    }
}

/// A resolved sign-in can notify again if that account or provider later signs out.
pub fn forget_resolved(cx: &mut App) {
    let (host, signed_in) = signed_in(store::state(cx));
    if let Some(notices) = cx.try_global::<SignInNotices>()
        && notices
            .shown
            .iter()
            .any(|(at, subject)| *at == host && signed_in.contains(subject))
    {
        cx.default_global::<SignInNotices>()
            .shown
            .retain(|(at, subject)| !(*at == host && signed_in.contains(subject)));
    }
}

/// The host, and the subjects that are signed in on it.
fn signed_in(state: &AppState) -> (Option<String>, Vec<String>) {
    let host = state.host.clone();
    let mut signed_in: Vec<String> = state
        .accounts
        .iter()
        .flatten()
        .filter(|account| {
            account
                .setup
                .as_ref()
                .is_some_and(|setup| fields(setup).1.signed_in)
        })
        .map(|account| account_subject(&account.id))
        .collect();
    signed_in.extend(
        state
            .setup
            .iter()
            .flatten()
            .map(fields)
            .filter(|(_, fields)| fields.signed_in)
            .map(|(provider, _)| provider_subject(provider)),
    );
    (host, signed_in)
}

/// The notice on screen, if any, for the shell to draw; it closes after 8 s, by its x, or by its button.
pub fn view(cx: &App) -> Option<AnyElement> {
    let shown = cx.try_global::<SignInNotices>()?.current.clone()?;
    let theme = theme(cx);
    let colors = palette(cx);
    let route = shown.route.clone();
    Some(
        div()
            .semantic("chat.notice", Role::Status)
            .aria_label(shown.title.clone())
            .w(r(360.))
            .max_w_full()
            .flex()
            .items_center()
            .gap(r(theme.space.s8))
            .p(r(theme.space.s8))
            .pl(r(theme.space.s12))
            .bg(hsla(colors.surface.raised))
            .rounded(r(theme.radius.md))
            .border_t_1()
            .border_color(hsla(colors.overlay.highlight))
            .shadow(shadows(&theme.shadow.light.raised))
            .child(
                svg()
                    .path("icons/i/175/plug.svg")
                    .size(r(16.))
                    .flex_none()
                    .text_color(hsla(colors.icon.base)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(colors.text.primary))
                    .child(shown.title.clone()),
            )
            .child(
                Link::new("chat.notice.fix", shown.fix.label()).on_press(move |_, cx| {
                    cx.default_global::<SignInNotices>().current = None;
                    cx.navigate(route.clone());
                }),
            )
            .child(
                IconButton::new("chat.notice.dismiss", "x", "Dismiss account notification")
                    .on_press(|_, cx| {
                        cx.default_global::<SignInNotices>().current = None;
                        cx.refresh_windows();
                    }),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui::TestAppContext;
    use protocol::model::{BotState, SETUP_SIGN_IN_PREFIX};
    use serde_json::{Value, json};

    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{self, AccountAction, Navigate as _, Route, Section};
    use crate::ui::store;
    use crate::ui::testing::{app, bot, click, draw, node, state};

    fn account(setup: Value) -> protocol::accounts::AccountInfo {
        serde_json::from_value(json!({
            "id": "personal", "provider": "claude", "label": "Claude (Personal)", "kind": "subscription",
            "isDefault": true, "isFallback": false, "spendingCapUsd": null, "createdAt": "2026-11-01T09:00:00.000Z",
            "hasCredential": false, "setup": setup, "limits": [], "signIn": null,
            "spending": { "knownUsd": 0, "unknownRows": 0, "capUsd": null, "blocked": false }
        }))
        .unwrap()
    }

    fn claude(signed_in: bool, error: Option<&str>) -> Value {
        json!({ "provider": "claude", "installed": true, "version": "2.0.0", "signedIn": signed_in, "account": null, "error": error })
    }

    /// Forge needs setup for `reason`, with these accounts.
    fn needs(reason: &str, accounts: Option<Vec<protocol::accounts::AccountInfo>>) -> AppState {
        let mut forge = bot("b1", "Forge", BotState::NeedsSetup);
        forge.setup_reason = Some(reason.into());
        AppState {
            accounts,
            ..state(vec![forge], vec![], vec![])
        }
    }

    fn sign_in_reason() -> String {
        format!("{SETUP_SIGN_IN_PREFIX} Claude Code.")
    }

    #[gpui::test]
    fn shows_one_dismissible_notice_with_the_accounts_sign_in(cx: &mut TestAppContext) {
        let accounts = Some(vec![account(claude(false, None))]);
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            needs(&sign_in_reason(), accounts),
            FakeRpc::new(),
        );
        draw(cx);
        assert_eq!(
            node("chat.notice").unwrap().text,
            "Claude (Personal) isn't signed in."
        );
        assert!(node("chat.b1.banner").is_none(), "it takes no chat space");
        assert!(node("chat.notice.dismiss").is_some());
        assert_eq!(node("chat.notice.fix").unwrap().text, "Sign in");
        click(cx, "chat.notice.fix");
        assert_eq!(
            cx.update(|_, cx| route::current(cx)),
            Route::Account {
                account: "personal".into(),
                action: Some(AccountAction::SignIn)
            }
        );
    }

    #[gpui::test]
    fn a_dismissed_notice_stays_closed_across_refreshes_and_revisits(cx: &mut TestAppContext) {
        let accounts = Some(vec![account(claude(false, None))]);
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            needs(&sign_in_reason(), accounts),
            FakeRpc::new(),
        );
        draw(cx);
        click(cx, "chat.notice.dismiss");
        assert!(node("chat.notice").is_none());
        cx.update(|_, cx| store::update(cx, |_| true));
        draw(cx);
        cx.update(|_, cx| cx.navigate(Route::Agents));
        draw(cx);
        cx.update(|_, cx| cx.navigate(Route::Chat("b1".into())));
        draw(cx);
        assert!(
            node("chat.notice").is_none(),
            "a refresh or a revisit doesn't bring it back"
        );
    }

    #[gpui::test]
    fn a_new_failure_after_signing_in_shows_again_and_pcs_are_separate(cx: &mut TestAppContext) {
        let accounts = Some(vec![account(claude(false, None))]);
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            needs(&sign_in_reason(), accounts),
            FakeRpc::new(),
        );
        draw(cx);
        click(cx, "chat.notice.dismiss");
        let set = |signed_in: bool| {
            move |state: &mut AppState| {
                state.accounts = Some(vec![account(claude(signed_in, None))]);
                true
            }
        };
        cx.update(|_, cx| store::update(cx, set(true)));
        draw(cx);
        cx.update(|_, cx| store::update(cx, set(false)));
        // The chat raises it as it draws; the shell, which drew first, shows it on the next frame.
        draw(cx);
        draw(cx);
        assert!(
            node("chat.notice").is_some(),
            "signed in, then out again: it shows"
        );
        click(cx, "chat.notice.dismiss");
        cx.update(|_, cx| {
            store::update(cx, |state| {
                state.host = Some("pc-2".into());
                true
            })
        });
        draw(cx);
        draw(cx);
        assert!(
            node("chat.notice").is_some(),
            "another PC keeps its own notices"
        );
    }

    /// M4 review A9: the notice is the app's, not the chat's.
    #[gpui::test]
    fn the_notice_stays_through_a_navigation(cx: &mut TestAppContext) {
        let accounts = Some(vec![account(claude(false, None))]);
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            needs(&sign_in_reason(), accounts),
            FakeRpc::new(),
        );
        draw(cx);
        assert!(node("chat.notice").is_some());
        cx.update(|_, cx| cx.navigate(Route::Agents));
        draw(cx);
        assert!(node("screen.chat").is_none(), "the chat is gone");
        assert_eq!(
            node("chat.notice").unwrap().text,
            "Claude (Personal) isn't signed in.",
            "and its notice stays for its 8 s"
        );
    }

    #[gpui::test]
    fn turns_a_providers_sign_in_into_a_notice(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            needs(&sign_in_reason(), None),
            FakeRpc::new(),
        );
        draw(cx);
        assert!(node("chat.notice").is_some());
        assert!(node("chat.b1.banner").is_none());
        click(cx, "chat.notice.fix");
        assert_eq!(
            cx.update(|_, cx| route::current(cx)),
            Route::Settings(Section::Accounts)
        );
    }

    #[gpui::test]
    fn offers_settings_for_an_account_that_couldnt_be_checked(cx: &mut TestAppContext) {
        let check = "Check or sign in to this account in Accounts.";
        let accounts = Some(vec![account(claude(false, Some("The CLI timed out.")))]);
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            needs(check, accounts),
            FakeRpc::new(),
        );
        draw(cx);
        assert_eq!(
            node("chat.notice").unwrap().text,
            "Claude (Personal) could not be checked."
        );
        assert_eq!(node("chat.notice.fix").unwrap().text, "Settings");
        click(cx, "chat.notice.fix");
        assert_eq!(
            cx.update(|_, cx| route::current(cx)),
            Route::Account {
                account: "personal".into(),
                action: None
            }
        );
    }

    #[gpui::test]
    fn closes_by_itself_after_eight_seconds(cx: &mut TestAppContext) {
        let accounts = Some(vec![account(claude(false, None))]);
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            needs(&sign_in_reason(), accounts),
            FakeRpc::new(),
        );
        draw(cx);
        assert!(node("chat.notice").is_some());
        cx.executor().advance_clock(Duration::from_secs(9));
        draw(cx);
        assert!(node("chat.notice").is_none());
    }
}
