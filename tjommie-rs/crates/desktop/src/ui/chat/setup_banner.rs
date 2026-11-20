//! The chat's setup and account banner (`setup-banner.tsx`): what stands between this agent and its work. Account
//! reminders (not signed in, couldn't be checked) are notices (`sign_in_toast`) and take no chat space; a spending cap,
//! a usage limit, a fallback account and the host's own setup stay beside the chat. Its links go to the account's card
//! (`Route::Account`), which they focus and never act on.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use jiff::Zoned;
use protocol::model::{Bot, BotPatch};
use protocol::wire::{BotsUpdate, Empty, method};

use super::sign_in_toast::{self, Fix};
use crate::guard::guarded_async;
use crate::model::accounts::{Banner, banner_for, money, when};
use crate::model::labels::error_text;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::Link;
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::shell::route::{AccountAction, Navigate as _, Route, Section};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// What a banner's button does.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Goes to a route: "Raise cap", "Add an account" (a pill when `primary`) or "Manage account" (a link).
    Go {
        label: String,
        route: Route,
        primary: bool,
    },
    /// Moves the agent to another account: one pill for one, a menu for several.
    Switch(Vec<(String, String)>),
    /// Prepares Codex's sandbox (`setup.prepareCodexSandbox`).
    SetUp,
}

/// A banner beside the chat.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Something waits on you (ochre), or it only explains (quiet).
    pub attention: bool,
    pub icon: &'static str,
    pub text: String,
    pub acts: Vec<Act>,
    pub note: Option<String>,
}

/// What a banner becomes: a notice (no chat space) or a line beside the chat.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Notice {
        subject: String,
        title: String,
        fix: Fix,
        route: Route,
    },
    Line(Line),
}

fn account_route(account: &str, action: Option<AccountAction>) -> Route {
    Route::Account {
        account: account.to_owned(),
        action,
    }
}

/// What `banner` shows for `bot` (`BannerView`).
pub fn shown_of(bot: &Bot, banner: &Banner, now: &Zoned) -> Shown {
    match banner {
        Banner::Setup {
            text,
            sign_in: true,
            ..
        } => Shown::Notice {
            subject: sign_in_toast::provider_subject(bot.provider),
            title: text.clone(),
            fix: Fix::SignIn,
            route: Route::Settings(Section::Accounts),
        },
        Banner::Setup { text, sandbox, .. } => Shown::Line(Line {
            attention: true,
            icon: "plug",
            text: text.clone(),
            acts: if *sandbox {
                vec![Act::SetUp]
            } else {
                Vec::new()
            },
            note: None,
        }),
        Banner::SignIn { account } => Shown::Notice {
            subject: sign_in_toast::account_subject(&account.id),
            title: format!("{} isn't signed in.", account.label),
            fix: Fix::SignIn,
            route: account_route(&account.id, Some(AccountAction::SignIn)),
        },
        Banner::AccountCheck { account, text } => Shown::Notice {
            subject: sign_in_toast::account_subject(&account.id),
            title: text.clone(),
            fix: Fix::Settings,
            route: account_route(&account.id, None),
        },
        Banner::Cap {
            account,
            unknown_prices,
        } if *unknown_prices > 0 => Shown::Line(Line {
            attention: true,
            icon: "plug",
            text: format!(
                "Stopped: {unknown_prices} {} no price, so {}'s cap can't be checked.",
                if *unknown_prices == 1 {
                    "turn has"
                } else {
                    "turns have"
                },
                account.label
            ),
            acts: Vec::new(),
            note: None,
        }),
        Banner::Cap { account, .. } => Shown::Line(Line {
            attention: true,
            icon: "plug",
            text: format!(
                "{} reached its {} cap.",
                account.label,
                money(account.spending.cap_usd.unwrap_or(0.))
            ),
            acts: vec![Act::Go {
                label: "Raise cap".into(),
                route: account_route(&account.id, Some(AccountAction::Cap)),
                primary: true,
            }],
            note: None,
        }),
        Banner::Limited {
            account,
            window,
            until,
            alternatives,
        } => {
            let resumes = match until {
                Some(until) => format!("at {}", when(until, now)),
                None => "when it resets".into(),
            };
            let mut acts = Vec::new();
            if alternatives.is_empty() {
                acts.push(Act::Go {
                    label: "Add an account".into(),
                    route: Route::Settings(Section::Accounts),
                    primary: true,
                });
            } else {
                acts.push(Act::Switch(
                    alternatives
                        .iter()
                        .map(|other| (other.id.clone(), other.label.clone()))
                        .collect(),
                ));
            }
            if until.is_none() {
                acts.push(Act::Go {
                    label: "Manage account".into(),
                    route: account_route(&account.id, Some(AccountAction::Retry)),
                    primary: false,
                });
            }
            Shown::Line(Line {
                attention: true,
                icon: "hourglass",
                text: format!(
                    "{} hit its {window}. {} resumes by itself {resumes}.",
                    account.label, bot.name
                ),
                acts,
                note: (!alternatives.is_empty())
                    .then(|| "Switching starts a fresh session. The chat stays.".into()),
            })
        }
        Banner::Fallback {
            pinned,
            using,
            until,
        } => Shown::Line(Line {
            attention: false,
            icon: "circle-alert",
            text: format!(
                "{} {}. {} is using {} until then.",
                pinned.label,
                match until {
                    Some(until) => format!("is limited until {}", when(until, now)),
                    None => "is limited, and its reset time isn't known".into(),
                },
                bot.name,
                using.label
            ),
            acts: Vec::new(),
            note: None,
        }),
    }
}

/// What a banner's action last said.
#[derive(Default)]
struct Problem(Option<SharedString>);

/// The banner for `bot`, if one shows beside the chat. A notice it calls for is raised once per PC.
pub fn banner(bot: &Bot, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let shown = {
        let state = store::state(cx);
        let now = Zoned::now();
        banner_for(bot, state.accounts.as_deref(), &now).map(|banner| shown_of(bot, &banner, &now))
    }?;
    let line = match shown {
        Shown::Notice {
            subject,
            title,
            fix,
            route,
        } => {
            sign_in_toast::raise(&subject, title, fix, route, cx);
            return None;
        }
        Shown::Line(line) => line,
    };
    let problem: Entity<Problem> = window.use_keyed_state(
        SharedString::from(format!("chat.{}.banner.problem", bot.id)),
        cx,
        |_, _| Problem::default(),
    );
    let said = problem.read(cx).0.clone();
    let theme = theme(cx);
    let colors = palette(cx);
    let id = bot.id.clone();
    let acts: Vec<AnyElement> = line
        .acts
        .iter()
        .enumerate()
        .map(|(index, act)| act_view(&id, index, act, &problem))
        .collect();
    let text = SharedString::from(line.text.clone());
    Some(
        div()
            .semantic(format!("chat.{id}.banner"), Role::Status)
            .aria_label(text.clone())
            .mx(r(theme.space.s28))
            .mt(r(theme.space.s8))
            .max_w(r(600.))
            .flex()
            .flex_col()
            .gap(r(theme.space.s8))
            .px(r(theme.space.s16))
            .py(r(theme.space.s12))
            .rounded(r(theme.radius.md))
            .when(line.attention, |frame| frame.bg(hsla(colors.accent.tint)))
            .when(!line.attention, |frame| {
                frame
                    .bg(hsla(colors.surface.raised))
                    .shadow(shadows(&theme.shadow.light.card))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s12))
                    .child(
                        svg()
                            .path(format!("icons/i/175/{}.svg", line.icon))
                            .size(r(16.))
                            .flex_none()
                            .text_color(hsla(if line.attention {
                                colors.accent.base
                            } else {
                                colors.icon.base
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .type_style(&theme.typography.desktop.ui)
                            .text_color(hsla(colors.text.primary))
                            .child(text),
                    )
                    .children(acts),
            )
            .when_some(line.note, |frame, note| {
                frame.child(
                    div()
                        .ml(r(28.))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.secondary))
                        .child(note),
                )
            })
            .when_some(said, |frame, said| {
                frame.child(
                    div()
                        .semantic(format!("chat.{id}.banner.problem"), Role::Alert)
                        .aria_label(said.clone())
                        .ml(r(28.))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.danger.text))
                        .child(said),
                )
            })
            .into_any_element(),
    )
}

fn act_view(bot_id: &str, index: usize, act: &Act, problem: &Entity<Problem>) -> AnyElement {
    let id = format!("chat.{bot_id}.banner.{index}");
    match act {
        Act::Go {
            label,
            route,
            primary: true,
        } => {
            let route = route.clone();
            Pill::new(id, label.clone())
                .variant(PillVariant::Primary)
                .small()
                .on_press(move |_, cx| cx.navigate(route.clone()))
                .into_any_element()
        }
        Act::Go { label, route, .. } => {
            let route = route.clone();
            Link::new(id, label.clone())
                .on_press(move |_, cx| cx.navigate(route.clone()))
                .into_any_element()
        }
        Act::SetUp => {
            let problem = problem.clone();
            Pill::new(id, "Set up")
                .variant(PillVariant::Primary)
                .small()
                .on_press(move |_, cx| {
                    let call = store::request::<method::SetupPrepareCodexSandbox>(cx, Empty {});
                    report(&problem, call, cx);
                })
                .into_any_element()
        }
        Act::Switch(others) => {
            let move_to = |account: String, problem: Entity<Problem>, bot: String| {
                move |_: &mut Window, cx: &mut App| {
                    let call = store::request::<method::BotsUpdate>(
                        cx,
                        BotsUpdate {
                            id: bot.clone(),
                            patch: BotPatch {
                                account_id: Some(Some(account.clone())),
                                ..BotPatch::default()
                            },
                        },
                    );
                    report(&problem, call, cx);
                }
            };
            match others.as_slice() {
                [(account, label)] => Pill::new(id, format!("Switch to {label}"))
                    .variant(PillVariant::Primary)
                    .small()
                    .on_press(move_to(account.clone(), problem.clone(), bot_id.to_owned()))
                    .into_any_element(),
                _ => {
                    let trigger = format!("{id}.open");
                    let mut menu = Menu::new(SharedString::from(id.clone()), move |_, _, _| {
                        Pill::new(trigger, "Switch account")
                            .variant(PillVariant::Primary)
                            .small()
                            .into_any_element()
                    })
                    .anchor(gpui::Anchor::TopRight)
                    .offset(gpui::px(6.));
                    for (account, label) in others {
                        menu = menu.item(MenuItem::new(
                            format!("{id}.{account}"),
                            label.clone(),
                            move_to(account.clone(), problem.clone(), bot_id.to_owned()),
                        ));
                    }
                    menu.into_any_element()
                }
            }
        }
    }
}

/// Says why an action failed, under the banner.
fn report<T: 'static>(
    problem: &Entity<Problem>,
    call: futures::future::BoxFuture<'static, Result<T, crate::model::connection::RequestError>>,
    cx: &mut App,
) {
    problem.update(cx, |problem, cx| {
        problem.0 = None;
        cx.notify();
    });
    let problem = problem.clone();
    cx.spawn(async move |cx| {
        guarded_async("run a banner action", async move {
            if let Err(error) = call.await {
                problem.update(cx, |problem, cx| {
                    problem.0 = Some(error_text(&error).into());
                    cx.notify();
                });
            }
        })
        .await;
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::accounts::AccountInfo;
    use serde_json::{Value, json};

    fn account(patch: Value) -> AccountInfo {
        let mut base = json!({
            "id": "a1", "provider": "claude", "label": "Claude (Personal)", "kind": "subscription",
            "isDefault": true, "isFallback": false, "spendingCapUsd": null, "createdAt": "2026-11-01T09:00:00.000Z",
            "hasCredential": false, "setup": null, "limits": [], "signIn": null,
            "spending": { "knownUsd": 0, "unknownRows": 0, "capUsd": null, "blocked": false }
        });
        if let (Some(base), Some(patch)) = (base.as_object_mut(), patch.as_object()) {
            base.extend(patch.clone());
        }
        serde_json::from_value(base).unwrap()
    }

    fn scout() -> Bot {
        crate::ui::testing::bot("b1", "Scout", protocol::model::BotState::Limited)
    }

    fn line(shown: Shown) -> Line {
        match shown {
            Shown::Line(line) => line,
            Shown::Notice { .. } => panic!("a notice, not a line"),
        }
    }

    const UNTIL: &str = "2026-10-22T14:00:00.000Z";

    #[test]
    fn reserves_no_chat_space_for_a_sign_in_or_an_account_check() {
        let now = Zoned::now();
        let personal = account(json!({}));
        match shown_of(&scout(), &Banner::SignIn { account: &personal }, &now) {
            Shown::Notice {
                title, fix, route, ..
            } => {
                assert_eq!(title, "Claude (Personal) isn't signed in.");
                assert_eq!(fix, Fix::SignIn);
                assert_eq!(route, account_route("a1", Some(AccountAction::SignIn)));
            }
            Shown::Line(line) => panic!("chat space taken: {line:?}"),
        }
        let check = Banner::AccountCheck {
            account: &personal,
            text: "Claude could not be checked.".into(),
        };
        match shown_of(&scout(), &check, &now) {
            Shown::Notice {
                title, fix, route, ..
            } => {
                assert_eq!(title, "Claude could not be checked.");
                assert_eq!(fix, Fix::Settings, "it offers Settings, not a sign-in");
                assert_eq!(route, account_route("a1", None));
            }
            Shown::Line(line) => panic!("chat space taken: {line:?}"),
        }
    }

    #[test]
    fn raises_a_cap_through_the_account_card_and_has_no_button_when_the_cap_cant_be_checked() {
        let now = Zoned::now();
        let capped = account(
            json!({ "kind": "api_key", "spending": { "knownUsd": 20, "unknownRows": 0, "capUsd": 20, "blocked": true } }),
        );
        let raise = line(shown_of(
            &scout(),
            &Banner::Cap {
                account: &capped,
                unknown_prices: 0,
            },
            &now,
        ));
        assert!(
            raise.text.ends_with("reached its $20.00 cap."),
            "{}",
            raise.text
        );
        assert_eq!(
            raise.acts,
            [Act::Go {
                label: "Raise cap".into(),
                route: account_route("a1", Some(AccountAction::Cap)),
                primary: true
            }]
        );
        let unpriced = line(shown_of(
            &scout(),
            &Banner::Cap {
                account: &capped,
                unknown_prices: 2,
            },
            &now,
        ));
        assert!(
            unpriced.text.contains("2 turns have no price"),
            "{}",
            unpriced.text
        );
        assert!(unpriced.acts.is_empty());
    }

    #[test]
    fn offers_one_account_a_menu_for_several_and_a_way_to_add_one_with_no_automatic_choice() {
        let now = Zoned::now();
        let personal = account(json!({}));
        let work = account(json!({ "id": "a2", "label": "Work", "isDefault": false }));
        let spare = account(json!({ "id": "a3", "label": "Spare", "isDefault": false }));
        let limited = |alternatives: Vec<&AccountInfo>| {
            line(shown_of(
                &scout(),
                &Banner::Limited {
                    account: &personal,
                    window: "5-hour limit".into(),
                    until: Some(UNTIL.into()),
                    alternatives,
                },
                &now,
            ))
        };
        let one = limited(vec![&work]);
        assert!(one.text.contains("hit its 5-hour limit"), "{}", one.text);
        assert!(one.text.contains("Scout resumes by itself"), "{}", one.text);
        assert_eq!(one.acts, [Act::Switch(vec![("a2".into(), "Work".into())])]);
        assert_eq!(
            one.note.as_deref(),
            Some("Switching starts a fresh session. The chat stays.")
        );
        let several = limited(vec![&work, &spare]);
        assert!(matches!(several.acts.as_slice(), [Act::Switch(others)] if others.len() == 2));
        let none = limited(vec![]);
        assert_eq!(
            none.acts,
            [Act::Go {
                label: "Add an account".into(),
                route: Route::Settings(Section::Accounts),
                primary: true
            }]
        );
        assert_eq!(none.note, None);
        for line in [one, several, none] {
            assert!(!format!("{line:?}").to_lowercase().contains("always"));
        }
    }

    #[test]
    fn says_a_limited_agent_resumes_when_it_resets_when_no_time_is_known() {
        let now = Zoned::now();
        let personal = account(json!({}));
        let unknown = line(shown_of(
            &scout(),
            &Banner::Limited {
                account: &personal,
                window: "5-hour limit".into(),
                until: None,
                alternatives: vec![],
            },
            &now,
        ));
        assert!(
            unknown
                .text
                .ends_with("Scout resumes by itself when it resets."),
            "{}",
            unknown.text
        );
        assert!(unknown.acts.contains(&Act::Go {
            label: "Manage account".into(),
            route: account_route("a1", Some(AccountAction::Retry)),
            primary: false
        }));
        let known = line(shown_of(
            &scout(),
            &Banner::Limited {
                account: &personal,
                window: "5-hour limit".into(),
                until: Some(UNTIL.into()),
                alternatives: vec![],
            },
            &now,
        ));
        assert!(!format!("{:?}", known.acts).contains("Manage account"));
        let key = account(json!({ "id": "k1", "label": "Claude key" }));
        let fallback = line(shown_of(
            &scout(),
            &Banner::Fallback {
                pinned: &personal,
                using: &key,
                until: None,
            },
            &now,
        ));
        assert!(
            fallback
                .text
                .contains("is limited, and its reset time isn't known."),
            "{}",
            fallback.text
        );
    }

    #[test]
    fn explains_a_fallback_with_nothing_to_press() {
        let now = Zoned::now();
        let personal = account(json!({}));
        let key = account(json!({ "id": "k1", "label": "Claude key" }));
        let fallback = line(shown_of(
            &scout(),
            &Banner::Fallback {
                pinned: &personal,
                using: &key,
                until: Some(UNTIL.into()),
            },
            &now,
        ));
        assert!(
            fallback
                .text
                .starts_with("Claude (Personal) is limited until"),
            "{}",
            fallback.text
        );
        assert!(
            fallback
                .text
                .ends_with("Scout is using Claude key until then."),
            "{}",
            fallback.text
        );
        assert!(fallback.acts.is_empty() && !fallback.attention);
    }

    #[test]
    fn keeps_the_sandbox_banner() {
        let now = Zoned::now();
        let text = "Codex's sandbox isn't set up yet. Windows asks for admin permission once.";
        let sandbox = line(shown_of(
            &scout(),
            &Banner::Setup {
                text: text.into(),
                sandbox: true,
                sign_in: false,
            },
            &now,
        ));
        assert_eq!(sandbox.text, text);
        assert_eq!(sandbox.acts, [Act::SetUp]);
    }
}
