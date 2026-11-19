//! The first-run setup (Phase 3 plan T5.5, M5 review M6; `components/setup/provider-cards.tsx` and the first run of
//! `app/page.tsx`): Claude Code's and Codex's cards side by side, each saying what's missing and offering the one thing
//! that fixes it, then Create your first agent. The Agents screen shows it while there are no agents and nobody is
//! signed in (`wanted`).

use std::borrow::Cow;

use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use protocol::model::{Provider, ProviderSetup};
use protocol::wire::{Empty, ProviderParams, method};

use crate::guard::guarded_async;
use crate::model::labels::error_text;
use crate::model::setup::{
    CardView, LocalState, SandboxView, SignIn, SignIns, card_view, live_sign_ins, plan_label,
    provider_name, report_of,
};
use crate::model::state::AppState;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::Link;
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];

const CLAUDE_CODE: &str = "https://claude.com/claude-code";
const NPM: &str = "npm install -g @openai/codex";

/// What this window is doing for the cards (`ProviderCards`' state).
#[derive(Default)]
struct Cards {
    /// Each provider's sign-in in progress.
    signing: SignIns,
    /// The providers whose `setup.signIn` hasn't answered yet: their Sign in waits, so a second click can't open a
    /// second sign-in.
    pending: Vec<Provider>,
    preparing: bool,
    /// `setup.status` checks both providers, so one check runs for both cards.
    checking: bool,
    /// The last failed request, on its provider's card.
    problem: Option<(Provider, String)>,
}

impl Cards {
    /// A new try on `provider`'s card clears its last problem, and only its own.
    fn clear_problem(&mut self, provider: Provider) {
        if self.problem.as_ref().is_some_and(|(at, _)| *at == provider) {
            self.problem = None;
        }
    }
}

fn key_of(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

/// `provider`'s report, if the check has answered.
fn entry_for(setup: Option<&[ProviderSetup]>, provider: Provider) -> Option<&ProviderSetup> {
    setup?.iter().find(|entry| {
        matches!(
            (entry, provider),
            (ProviderSetup::Claude(_), Provider::Claude) | (ProviderSetup::Codex(_), Provider::Codex)
        )
    })
}

/// Whether the Agents screen shows the first run: no agents yet, and nobody signed in to either provider.
pub fn wanted(state: &AppState) -> bool {
    state.bots.is_empty()
        && !PROVIDERS.iter().any(|provider| {
            matches!(
                card_view(entry_for(state.setup.as_deref(), *provider), LocalState::default()),
                CardView::SignedIn { .. }
            )
        })
}

/// The first run, in the Agents screen's scroll area: what Tjommie needs, the two cards, and Create your first agent.
pub fn first_run(window: &mut Window, cx: &mut App) -> AnyElement {
    let cards = provider_cards(window, cx);
    let theme = theme(cx);
    let colors = palette(cx);
    let paragraph = |id: &'static str, text: &'static str, small: bool| {
        div()
            .semantic(id, Role::Paragraph)
            .aria_label(text)
            .type_style(if small {
                &theme.typography.desktop.small
            } else {
                &theme.typography.desktop.ui
            })
            .text_color(hsla(colors.text.secondary))
            .child(text)
    };
    div()
        .mx_auto()
        .w_full()
        .max_w(r(808.))
        .pt(r(theme.space.s24))
        .flex()
        .flex_col()
        .child(
            div()
                .semantic("setup.title", Role::Heading)
                .aria_label("Set up Tjommie")
                .type_style(&theme.typography.desktop.display_m)
                .text_color(hsla(colors.text.primary))
                .child("Set up Tjommie"),
        )
        .child(
            paragraph(
                "setup.intro",
                "Tjommie runs your agents with Claude Code and Codex on this PC. Sign in to each one once. Tjommie keeps \
                 its own sign-in, separate from how you use these tools yourself, and never sees your password.",
                false,
            )
            .mt(r(theme.space.s8)),
        )
        .child(div().mt(r(theme.space.s32)).child(cards))
        .child(
            div().mt(r(theme.space.s40)).flex().child(
                Pill::new("setup.create", "Create your first agent")
                    .variant(PillVariant::Primary)
                    .on_press(|_, cx| cx.navigate(Route::NewBot)),
            ),
        )
        .child(
            paragraph("setup.later", "You can sign in to the other one later in Settings.", true)
                .mt(r(theme.space.s12)),
        )
        .into_any_element()
}

/// Claude Code and Codex side by side (`ProviderCards`).
pub fn provider_cards(window: &mut Window, cx: &mut App) -> AnyElement {
    let cards = window.use_keyed_state("setup.cards", cx, |_, _| Cards::default());
    let (setup, ended) = {
        let state = store::state(cx);
        (state.setup.clone(), state.sign_ins_ended.clone())
    };
    // Signing in lasts until the daemon reports the provider signed in (or as another account), or says the sign-in
    // ended: a closed window, the time running out or a failed login change nothing else. Once over it stays over.
    let over = match live_sign_ins(&cards.read(cx).signing, &ended, setup.as_deref()) {
        Cow::Owned(live) => Some(live),
        Cow::Borrowed(_) => None,
    };
    if let Some(live) = over {
        cards.update(cx, |cards, _| cards.signing = live);
    }
    let shown: Vec<AnyElement> = PROVIDERS
        .iter()
        .map(|provider| card(&cards, *provider, setup.as_deref(), cx))
        .collect();
    div()
        .flex()
        .flex_wrap()
        .gap(r(theme(cx).space.s20))
        .children(shown)
        .into_any_element()
}

/// One provider's card: its name and version, what it says, what it offers, and a failed request's reason.
fn card(cards: &Entity<Cards>, provider: Provider, setup: Option<&[ProviderSetup]>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let key = key_of(provider);
    let entry = entry_for(setup, provider);
    let local = cards.read(cx);
    let view = card_view(
        entry,
        LocalState {
            signing_in: local.signing.contains_key(&provider),
            preparing: provider == Provider::Codex && local.preparing,
        },
    );
    let (body, actions) = content(cards, provider, &view, local.checking, local.pending.contains(&provider), cx);
    let problem = local
        .problem
        .as_ref()
        .filter(|(at, _)| *at == provider)
        .map(|(_, text)| {
            div()
                .semantic(format!("setup.{key}.problem"), Role::Alert)
                .aria_label(text.clone())
                .mt(r(theme.space.s8))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.danger.text))
                .child(text.clone())
        });
    let version = entry
        .and_then(|entry| report_of(entry).version.clone())
        .filter(|version| !version.is_empty())
        .map(|version| format!("Version {version}"))
        .or_else(|| (view == CardView::NotInstalled).then(|| "Not installed".to_owned()));
    let small = &theme.typography.desktop.small;
    let version: AnyElement = match version {
        Some(text) => div()
            .semantic(format!("setup.{key}.version"), Role::Paragraph)
            .aria_label(text.clone())
            .type_style(small)
            .text_color(hsla(colors.text.secondary))
            .child(text)
            .into_any_element(),
        // As tall as the line, so both cards' text starts at the same height.
        None => div().h(r(small.line_height)).into_any_element(),
    };
    let name = provider_name(provider);
    let header = div()
        .flex()
        .items_center()
        .gap(r(theme.space.s16))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(r(36.))
                .rounded(r(theme.radius.md))
                .bg(hsla(colors.surface.sunk))
                .child(
                    svg()
                        .path(match provider {
                            Provider::Claude => "icons/i/175/sparkles.svg",
                            Provider::Codex => "icons/i/175/terminal.svg",
                        })
                        .size(r(18.))
                        .text_color(hsla(colors.icon.base)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .semantic(format!("setup.{key}.name"), Role::Heading)
                        .aria_label(name)
                        .type_style(&theme.typography.desktop.ui_strong)
                        .text_color(hsla(colors.text.primary))
                        .child(name),
                )
                .child(version),
        );
    div()
        .semantic(format!("setup.{key}"), Role::Group)
        .aria_label(name)
        .flex()
        .flex_col()
        .flex_grow_1()
        .flex_basis(r(300.))
        .min_h(r(200.))
        .p(r(theme.space.s20))
        .bg(hsla(colors.surface.raised))
        .rounded(r(theme.radius.card))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .shadow(shadows(&theme.shadow.light.card))
        .child(header)
        .child(
            div()
                .mt(r(theme.space.s28))
                .flex_1()
                .flex()
                .flex_col()
                .gap(r(theme.space.s4))
                .children(body),
        )
        // As tall as a pill even when it holds only a link, so both cards' last line sits in the same place.
        .children((!actions.is_empty()).then(|| {
            div()
                .mt(r(theme.space.s16))
                .min_h(r(32.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(r(theme.space.s12))
                .children(actions)
        }))
        .children(problem)
        .into_any_element()
}

/// What a card says and offers for its view (`content`). `checking`: a check is running; `requesting`: this card's
/// Sign in hasn't been answered yet.
fn content(
    cards: &Entity<Cards>,
    provider: Provider,
    view: &CardView,
    checking: bool,
    requesting: bool,
    cx: &App,
) -> (Vec<AnyElement>, Vec<AnyElement>) {
    let key = key_of(provider);
    let claude = provider == Provider::Claude;
    let body = |text: String, icon: Option<&'static str>, helper: bool| line(format!("setup.{key}.body"), icon, text, helper, cx);
    let note = |text: String| {
        div()
            .pl(r(theme(cx).space.s24))
            .child(line(format!("setup.{key}.note"), None, text, true, cx))
            .into_any_element()
    };
    let check_again = || {
        let cards = cards.clone();
        Pill::new(format!("setup.{key}.check"), "Check again")
            .disabled(checking)
            .on_press(move |_, cx| check(&cards, provider, cx))
            .into_any_element()
    };
    let sign_in = |label: &'static str, primary: bool| {
        let cards = cards.clone();
        let pill = Pill::new(format!("setup.{key}.sign_in"), label).on_press(move |_, cx| start_sign_in(&cards, provider, cx));
        if primary {
            pill.variant(PillVariant::Primary).disabled(requesting).into_any_element()
        } else {
            pill.into_any_element()
        }
    };
    match view {
        CardView::Checking => (vec![body("Checking".into(), Some("loader-circle"), false)], Vec::new()),
        CardView::NotInstalled if claude => (
            vec![body(
                "Install Claude Code from claude.com/claude-code, then check again.".into(),
                None,
                true,
            )],
            vec![
                check_again(),
                Link::new("setup.claude.install", "claude.com/claude-code")
                    .quiet()
                    .on_press(|_, _| open(CLAUDE_CODE))
                    .into_any_element(),
            ],
        ),
        CardView::NotInstalled => (vec![install_codex(cx)], vec![check_again()]),
        CardView::Problem { reason } => (vec![body(reason.clone(), None, true)], vec![check_again()]),
        CardView::SignedOut => (
            vec![body(
                if claude {
                    "Opens Claude's sign-in page in your browser."
                } else {
                    "Opens OpenAI's sign-in page in your browser."
                }
                .into(),
                None,
                true,
            )],
            vec![sign_in(if claude { "Sign in to Claude" } else { "Sign in to Codex" }, true)],
        ),
        CardView::SigningIn => {
            let cards = cards.clone();
            (
                vec![
                    body("Finish signing in in your browser".into(), Some("loader-circle"), false),
                    note("Tjommie notices as soon as you're done.".into()),
                ],
                vec![
                    sign_in(
                        if claude {
                            "Open the sign-in window again"
                        } else {
                            "Open the sign-in page again"
                        },
                        false,
                    ),
                    Link::new(format!("setup.{key}.cancel"), "Cancel")
                        .quiet()
                        .on_press(move |_, cx| {
                            cards.update(cx, |cards, cx| {
                                cards.signing.remove(&provider);
                                cx.notify();
                            });
                        })
                        .into_any_element(),
                ],
            )
        }
        CardView::SignedIn { account, plan, sandbox } => {
            let mut said = vec![body(
                account.as_ref().map_or_else(|| "Signed in".to_owned(), |account| format!("Signed in as {account}")),
                Some("check"),
                false,
            )];
            said.extend(plan_label(plan.as_deref()).map(note));
            let sandbox_line = |text: &str, icon: Option<&'static str>, helper: bool| {
                div()
                    .mt(r(theme(cx).space.s8))
                    .child(line(format!("setup.{key}.sandbox"), icon, text.to_owned(), helper, cx))
                    .into_any_element()
            };
            let mut offered = Vec::new();
            match sandbox {
                Some(SandboxView::Needed) => {
                    said.push(sandbox_line(
                        "Codex's sandbox isn't set up yet. Codex agents at Own folder need it, and Windows asks for \
                         admin permission once.",
                        None,
                        true,
                    ));
                    let cards = cards.clone();
                    offered.push(
                        Pill::new("setup.codex.prepare", "Set up Codex's sandbox")
                            .variant(PillVariant::Primary)
                            .on_press(move |_, cx| prepare(&cards, cx))
                            .into_any_element(),
                    );
                }
                Some(SandboxView::Preparing) => said.push(sandbox_line(
                    "Setting up Codex's sandbox. Answer the Windows prompt.",
                    Some("loader-circle"),
                    false,
                )),
                Some(SandboxView::Unavailable) => {
                    said.push(sandbox_line(
                        "Tjommie couldn't check Codex's sandbox, so Codex agents at Own folder can't run yet.",
                        None,
                        true,
                    ));
                    offered.push(check_again());
                }
                Some(SandboxView::Ready) | None => {}
            }
            // ponytail: hidden while a sign-in request is on its way (Link has no unavailable state); it lasts only
            // until the daemon answers, and then the card is signing in.
            if !requesting {
                let cards = cards.clone();
                offered.push(
                    Link::new(format!("setup.{key}.other_account"), "Use a different account")
                        .on_press(move |_, cx| start_sign_in(&cards, provider, cx))
                        .into_any_element(),
                );
            }
            (said, offered)
        }
    }
}

/// One line of a card, read as `id`, after `icon` when it has one (`Line`); a helper line is smaller and quieter.
fn line(id: String, icon: Option<&'static str>, text: String, helper: bool, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let (style, color) = if helper {
        (&theme.typography.desktop.small, colors.text.secondary)
    } else {
        (&theme.typography.desktop.ui, colors.text.primary)
    };
    let text = SharedString::from(text);
    div()
        .semantic(id, Role::Paragraph)
        .aria_label(text.clone())
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .type_style(style)
        .text_color(hsla(color))
        // ponytail: the loader stands still, as Accounts' does; spinners are kept for tools and connections.
        .children(icon.map(|icon| {
            svg()
                .path(format!("icons/i/175/{icon}.svg"))
                .size(r(16.))
                .flex_none()
                .text_color(hsla(colors.icon.base))
        }))
        .child(div().flex_1().min_w_0().child(text))
        .into_any_element()
}

/// Codex's install line, with its command in the code face.
fn install_codex(cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic("setup.codex.body", Role::Paragraph)
        .aria_label(format!("Install Codex with {NPM}, then check again."))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(r(theme.space.s4))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(colors.text.secondary))
        .child("Install Codex with")
        .child(
            div()
                .px(r(theme.space.s4))
                .rounded(r(theme.radius.xs))
                .bg(hsla(colors.surface.sunk))
                .type_style(&theme.typography.desktop.code)
                .text_color(hsla(colors.text.primary))
                .child(NPM),
        )
        .child(", then check again.")
        .into_any_element()
}

/// Opens a web page in the default browser; one that can't open is only logged, as Settings' About does.
fn open(url: &str) {
    if let Err(why) = desktop_host::links::open_external(url) {
        log::warn!("Setup: a link did not open ({why})");
    }
}

/// Starts `provider`'s sign-in (`signIn`): the daemon opens its page in the browser. The daemon can take seconds to
/// answer (Codex starts its app server first); until then this card's Sign in waits.
fn start_sign_in(cards: &Entity<Cards>, provider: Provider, cx: &mut App) {
    let since = entry_for(store::state(cx).setup.as_deref(), provider).cloned();
    cards.update(cx, |cards, cx| {
        cards.pending.push(provider);
        cards.clear_problem(provider);
        cx.notify();
    });
    let call = store::request::<method::SetupSignIn>(cx, ProviderParams { provider });
    after(
        cards,
        "start a provider's sign-in",
        call,
        move |cards, done| {
            cards.pending.retain(|each| *each != provider);
            match done {
                Ok(started) => {
                    cards.signing.insert(provider, SignIn { since, attempt: started.attempt });
                }
                Err(why) => cards.problem = Some((provider, why)),
            }
        },
        cx,
    );
}

/// Sets up Codex's sandbox (`prepare`): Windows asks for admin permission once.
fn prepare(cards: &Entity<Cards>, cx: &mut App) {
    cards.update(cx, |cards, cx| {
        cards.preparing = true;
        cards.clear_problem(Provider::Codex);
        cx.notify();
    });
    let call = store::request::<method::SetupPrepareCodexSandbox>(cx, Empty {});
    after(
        cards,
        "set up Codex's sandbox",
        call,
        |cards, done| {
            cards.preparing = false;
            if let Err(why) = done {
                cards.problem = Some((Provider::Codex, why));
            }
        },
        cx,
    );
}

/// Check again (`checkAgain`): `setup.status` runs both CLIs' checks, so only this button asks for it. A failure shows
/// on the card that asked.
fn check(cards: &Entity<Cards>, provider: Provider, cx: &mut App) {
    cards.update(cx, |cards, cx| {
        cards.checking = true;
        cards.clear_problem(provider);
        cx.notify();
    });
    let call = store::request::<method::SetupStatus>(cx, Empty {});
    after(
        cards,
        "check the providers again",
        call,
        move |cards, done| {
            cards.checking = false;
            if let Err(why) = done {
                cards.problem = Some((provider, why));
            }
        },
        cx,
    );
}

/// Waits for `call`, then hands `then` its answer (a failure as words) and redraws the cards.
fn after<T: 'static, E: std::fmt::Display + 'static>(
    cards: &Entity<Cards>,
    what: &'static str,
    call: impl Future<Output = Result<T, E>> + 'static,
    then: impl FnOnce(&mut Cards, Result<T, String>) + 'static,
    cx: &mut App,
) {
    let cards = cards.downgrade();
    cx.spawn(async move |cx| {
        guarded_async(what, async move {
            let done = call.await.map_err(|error| error_text(&error));
            cx.update(|cx| {
                let _ = cards.update(cx, |cards, cx| {
                    then(cards, done);
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        App, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point,
        TestAppContext, VisualTestContext, Window,
    };
    use protocol::accounts::SignInStart;
    use protocol::model::{CodexSandboxReadiness, CodexSetup, ProviderSetup, SetupFields};
    use protocol::wire::method;
    use serde_json::{Value, json};

    use crate::model::testing::FakeRpc;
    use crate::ui::testing::{click, draw, node, state, text};

    /// An installed, signed-out report, then `edit`ed. Invented versions and accounts.
    fn fields(version: &str, edit: impl FnOnce(&mut SetupFields)) -> SetupFields {
        let mut fields = SetupFields {
            installed: true,
            version: Some(version.to_owned()),
            signed_in: false,
            account: None,
            plan: None,
            error: None,
        };
        edit(&mut fields);
        fields
    }

    fn claude(edit: impl FnOnce(&mut SetupFields)) -> ProviderSetup {
        ProviderSetup::Claude(fields("2.1.280", edit))
    }

    fn codex(sandbox: CodexSandboxReadiness, edit: impl FnOnce(&mut SetupFields)) -> ProviderSetup {
        ProviderSetup::Codex(CodexSetup {
            setup: fields("0.157.1", edit),
            sandbox,
        })
    }

    fn missing(fields: &mut SetupFields) {
        fields.installed = false;
        fields.version = None;
    }

    /// The two cards alone, over the store's fake with `setup`. Returns the window and the calls made.
    fn cards(cx: &mut TestAppContext, setup: Vec<ProviderSetup>, rpc: FakeRpc) -> (&mut VisualTestContext, Arc<FakeRpc>) {
        let mut connected = state(vec![], vec![], vec![]);
        connected.setup = Some(setup);
        let calls = cx.update(|cx| crate::ui::store::fake(cx, connected, rpc));
        let cx = crate::ui::primitives::test_window(cx, super::provider_cards);
        draw(cx);
        (cx, calls)
    }

    fn asked(calls: &FakeRpc, method: &str) -> Vec<Value> {
        calls
            .calls()
            .into_iter()
            .filter(|(name, _)| *name == method)
            .map(|(_, params)| params)
            .collect()
    }

    fn started() -> FakeRpc {
        FakeRpc::new().on::<method::SetupSignIn>(|_| {
            Ok(SignInStart {
                url: None,
                attempt: "attempt-1".into(),
            })
        })
    }

    /// A click as the platform delivers it, without running what it starts: the frame after it is drawn before any
    /// answer comes back.
    fn press_at(position: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let (modifiers, button) = (Modifiers::default(), MouseButton::Left);
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
    }

    /// `provider-cards`: a signed-out provider says where Sign in goes, a missing one how to install it; Sign in asks
    /// the daemon for that provider's sign-in.
    #[gpui::test]
    fn a_signed_out_and_a_missing_provider_say_so_and_sign_in_asks_for_that_one(cx: &mut TestAppContext) {
        let (cx, calls) = cards(cx, vec![claude(|_| {}), codex(CodexSandboxReadiness::NotConfigured, missing)], started());
        assert_eq!(text("setup.claude.version"), "Version 2.1.280");
        assert_eq!(text("setup.claude.body"), "Opens Claude's sign-in page in your browser.");
        assert_eq!(text("setup.claude.sign_in"), "Sign in to Claude");
        assert_eq!(text("setup.codex.version"), "Not installed");
        assert!(text("setup.codex.body").contains("npm install -g"), "{}", text("setup.codex.body"));
        assert!(node("setup.codex.check").is_some(), "Check again");
        assert!(node("setup.codex.sign_in").is_none(), "nothing to sign in to yet");
        click(cx, "setup.claude.sign_in");
        assert_eq!(asked(&calls, "setup.signIn"), [json!({ "provider": "claude" })]);
    }

    /// While signing in, the card says to finish in the browser and offers the page again; Cancel puts it back.
    #[gpui::test]
    fn signing_in_says_to_finish_in_the_browser_and_cancel_puts_it_back(cx: &mut TestAppContext) {
        let (cx, _) = cards(cx, vec![claude(|_| {}), codex(CodexSandboxReadiness::Ready, |_| {})], started());
        click(cx, "setup.claude.sign_in");
        draw(cx);
        assert_eq!(text("setup.claude.body"), "Finish signing in in your browser");
        assert_eq!(text("setup.claude.note"), "Tjommie notices as soon as you're done.");
        assert_eq!(text("setup.claude.sign_in"), "Open the sign-in window again");
        assert_eq!(
            text("setup.codex.body"),
            "Opens OpenAI's sign-in page in your browser.",
            "the other card is untouched"
        );
        click(cx, "setup.claude.cancel");
        assert_eq!(text("setup.claude.body"), "Opens Claude's sign-in page in your browser.");
        assert_eq!(text("setup.claude.sign_in"), "Sign in to Claude");
    }

    /// A request that fails says why on its own card only, and the card can be tried again.
    #[gpui::test]
    fn a_failed_request_says_why_on_that_card(cx: &mut TestAppContext) {
        let rpc = FakeRpc::new().on::<method::SetupSignIn>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "The sign-in could not start".into(),
            })
        });
        let (cx, _) = cards(cx, vec![claude(|_| {}), codex(CodexSandboxReadiness::Ready, |_| {})], rpc);
        click(cx, "setup.claude.sign_in");
        assert_eq!(text("setup.claude.problem"), "The sign-in could not start");
        assert!(node("setup.codex.problem").is_none(), "only on the card that asked");
        assert_eq!(text("setup.claude.body"), "Opens Claude's sign-in page in your browser.", "still signed out");
        assert!(node("setup.claude.sign_in").is_some_and(|node| node.enabled), "Sign in can be tried again");
    }

    /// Signed in says as whom and on which plan. Codex's sandbox that isn't set up offers Set up, which asks the daemon.
    #[gpui::test]
    fn signed_in_says_as_whom_and_codex_offers_its_sandbox(cx: &mut TestAppContext) {
        let signed_in = |fields: &mut SetupFields| {
            fields.signed_in = true;
            fields.account = Some("fixture-codex".into());
        };
        let ready = codex(CodexSandboxReadiness::Ready, signed_in);
        let rpc = FakeRpc::new().on::<method::SetupPrepareCodexSandbox>(move |_| Ok(ready.clone()));
        let claude_in = claude(|fields| {
            fields.signed_in = true;
            fields.account = Some("fixture-account".into());
            fields.plan = Some("max".into());
        });
        let (cx, calls) = cards(cx, vec![claude_in, codex(CodexSandboxReadiness::NotConfigured, signed_in)], rpc);
        assert_eq!(text("setup.claude.body"), "Signed in as fixture-account");
        assert_eq!(text("setup.claude.note"), "Max plan");
        assert!(node("setup.claude.other_account").is_some(), "Use a different account");
        assert!(node("setup.claude.sandbox").is_none(), "Claude has no sandbox");
        assert!(text("setup.codex.sandbox").contains("sandbox isn't set up yet"));
        click(cx, "setup.codex.prepare");
        assert_eq!(asked(&calls, "setup.prepareCodexSandbox").len(), 1);
        assert!(node("setup.codex.problem").is_none());
    }

    /// Check again asks for a fresh check, and it's unavailable on both cards until the check answers.
    #[gpui::test]
    fn check_again_asks_for_a_check_and_waits_for_it(cx: &mut TestAppContext) {
        let rpc = FakeRpc::new().on::<method::SetupStatus>(|_| Ok(Vec::new()));
        let failed = claude(|fields| fields.error = Some("The check failed".into()));
        let (cx, calls) = cards(cx, vec![failed, codex(CodexSandboxReadiness::NotConfigured, missing)], rpc);
        assert_eq!(text("setup.claude.body"), "The check failed");
        // The click and the frame after it, before the daemon's answer is read.
        let at = cx.debug_bounds("setup.claude.check").expect("Check again").center();
        cx.update(|window, cx| {
            press_at(at, window, cx);
            window.draw(cx).clear(cx);
        });
        assert!(!node("setup.claude.check").unwrap().enabled, "unavailable while it runs");
        assert!(!node("setup.codex.check").unwrap().enabled, "on both cards: one check runs both");
        draw(cx);
        assert_eq!(asked(&calls, "setup.status").len(), 1);
        assert!(node("setup.claude.check").unwrap().enabled, "available again once it answered");
    }
}
