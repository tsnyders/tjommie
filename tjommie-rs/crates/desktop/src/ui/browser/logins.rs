//! The logins menu (Phase 3 plan T3.6/T3.7; browser-logins.tsx; Aegis browser-panel.html "Logins menu"; spike step 8):
//! the key beside the address opens it over the page (an open menu raises the shield). It names the page's site, offers
//! to fill a saved login (for the person in control of an HTTPS page), the saved logins, and Manage logins. Only names and
//! sites are shown; the host's vault confirms the exact HTTPS site in its own trusted window before it fills anything.
//!
//! The vault is reached through `BrowserData::on_logins` with the host actions (`VaultHostAction`), as T1.4's logins
//! client sends them, and what it answers comes back as `BrowserData::logins`.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, Role, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use gpui_kit::component::WindowExt as _;
use protocol::browser::{BrowserControl, BrowserState};
use protocol::vault::{
    LoginEntry, VaultFill, VaultFillTarget, VaultHostAction, VaultList, VaultState, VaultStatus,
};
use url::Url;

use super::BrowserData;
use crate::guard::guarded;
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::{IconButton, Pill, PillVariant};
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::shell::route::{Navigate as _, Route, Section};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// Takes a logins action for the host's vault (T1.4's `LoginsClient::send`).
pub type OnLogins = Rc<dyn Fn(VaultHostAction, &mut Window, &mut App)>;

/// What the vault answered for a bot (`vault.status` and `vault.list`), for the Saved logins dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoginsView {
    pub status: Option<VaultStatus>,
    pub entries: Vec<LoginEntry>,
    /// The vault could not be asked: `LOGINS_PROBLEM` shows.
    pub failed: bool,
}

/// browser-logins.tsx's `LOGINS_PROBLEM`.
pub const LOGINS_PROBLEM: &str = "Logins could not complete the request safely. Check the selected host. Any private browser remains private until its fields are cleared or it is closed.";

/// The page's site when a saved login can go there: an HTTPS address with no user name or password in it.
pub fn fill_origin(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none())
        .then(|| url.origin().ascii_serialization())
}

/// What a fill is for: the person in control of a page, out of a private interaction (`browserLoginTarget`).
pub fn fill_target(state: &BrowserState) -> Option<VaultFillTarget> {
    if state.control != BrowserControl::Human
        || state.can_control != Some(true)
        || state.private_active == Some(true)
    {
        return None;
    }
    Some(VaultFillTarget {
        bot_id: state.bot_id.clone(),
        generation: state.generation.clone(),
        page_id: state.active_page_id.clone()?,
        control_epoch: state.control_epoch?,
    })
}

fn send(action: VaultHostAction, window: &mut Window, cx: &mut App) {
    match cx
        .try_global::<BrowserData>()
        .and_then(|data| data.on_logins.clone())
    {
        Some(on_logins) => on_logins(action, window, cx),
        None => log::info!("No logins client yet for {action:?}"),
    }
}

/// The key button and its menu, for the bot's page at `state`.
pub fn menu(state: &BrowserState, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let url = super::chrome::active_tab(state).map_or(String::new(), |tab| tab.url.clone());
    let origin = fill_origin(&url);
    let site = Url::parse(&url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "this page".into());
    let target = fill_target(state);
    let bot_id = state.bot_id.clone();
    let saved = {
        let (bot_id, origin, target) = (bot_id.clone(), origin.clone(), target.clone());
        move |window: &mut Window, cx: &mut App| {
            open_saved(bot_id.clone(), origin.clone(), target.clone(), window, cx)
        }
    };
    let fill =
        MenuItem::new("browser.logins.fill", "Fill a saved login", saved.clone()).icon("log-in");
    let fill = if target.is_some() {
        fill
    } else {
        fill.end("Take control first")
    };
    let _ = cx;
    Menu::new("browser.logins.menu", |open, _, _| {
        div()
            .when(open, |trigger| trigger.rounded_full())
            .child(IconButton::new("browser.logins", "key-round", "Logins").expanded(open))
            .into_any_element()
    })
    .anchor(gpui::Anchor::TopRight)
    .offset(px(6.))
    .note(format!("Logins for {site}"))
    .item(fill)
    .item(MenuItem::new("browser.logins.saved", "Saved logins", saved).icon("key-round"))
    .item(
        MenuItem::new("browser.logins.manage", "Manage logins", |_, cx| {
            cx.navigate(Route::Settings(Section::Logins))
        })
        .icon("settings"),
    )
    .rule()
    .note("Only names and sites are shown here. This PC confirms the exact HTTPS site before it fills anything.")
    .into_any_element()
}

/// The Saved logins dialog for the page's site: choose one and fill it on the host, or unlock the vault there first.
fn open_saved(
    bot_id: String,
    origin: Option<String>,
    target: Option<VaultFillTarget>,
    window: &mut Window,
    cx: &mut App,
) {
    send(VaultHostAction::Status, window, cx);
    send(
        VaultHostAction::List(VaultList {
            bot_id: Some(bot_id.clone()),
        }),
        window,
        cx,
    );
    let chosen = Rc::new(std::cell::RefCell::new(None::<String>));
    open_dialog(window, cx, move |dialog, window, cx| {
        let theme = theme(cx);
        let colors = palette(cx);
        let view = cx
            .try_global::<BrowserData>()
            .and_then(|data| data.logins.get(&bot_id).cloned())
            .unwrap_or_default();
        let choices: Vec<LoginEntry> = view
            .entries
            .iter()
            .filter(|entry| {
                entry.bot_id == bot_id && Some(entry.origin.as_str()) == origin.as_deref()
            })
            .cloned()
            .collect();
        let entry = chosen
            .borrow()
            .as_ref()
            .and_then(|id| choices.iter().find(|entry| &entry.id == id).cloned());
        let state = view.status.as_ref().map(|status| status.state);
        let words = match (&view.status, view.failed) {
            (Some(status), _) => format!(
                "Logins {}. {}",
                match status.state {
                    VaultState::Locked => "locked",
                    VaultState::Unlocked => "unlocked",
                    VaultState::Unavailable => "unavailable",
                },
                status.reason.clone().unwrap_or_default()
            ),
            (None, true) => "Logins unavailable.".into(),
            (None, false) => "Checking host…".into(),
        };
        let rows = choices.iter().map(|choice| {
            let (id, chosen) = (choice.id.clone(), chosen.clone());
            let on = entry.as_ref().is_some_and(|entry| entry.id == choice.id);
            div()
                .semantic(format!("logins.entry.{}", choice.id), Role::RadioButton)
                .aria_label(choice.name.clone())
                .aria_selected(on)
                .flex()
                .items_center()
                .min_h(r(36.))
                .px(r(theme.space.s10))
                .rounded(r(theme.radius.sm))
                .cursor_pointer()
                .when(on, |row| row.bg(hsla(colors.state.hover)))
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.primary))
                .on_click(move |_, window, _| {
                    guarded("choose a saved login", || {
                        *chosen.borrow_mut() = Some(id.clone());
                        window.refresh();
                    });
                })
                .child(choice.name.clone())
        });
        let fill = target
            .clone()
            .zip(entry.clone())
            .filter(|_| state == Some(VaultState::Unlocked));
        dialog
            .w(window.rem_size() * 25.0)
            .title(title("Saved logins", cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s8))
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(colors.text.secondary))
                    .child(format!(
                        "Only metadata is shown here. The host confirms its exact HTTPS destination before filling. {words}"
                    ))
                    .map(|body| {
                        if choices.is_empty() {
                            body.child(format!(
                                "No saved logins for {}.",
                                origin.as_deref().unwrap_or("this page")
                            ))
                        } else {
                            body.child(
                                div()
                                    .semantic("logins.entries", Role::RadioGroup)
                                    .aria_label("Saved login for this origin")
                                    .flex()
                                    .flex_col()
                                    .children(rows),
                            )
                        }
                    })
                    .when(target.is_none(), |body| {
                        body.child("Take control of the current browser page before filling a login.")
                    })
                    .when(view.failed, |body| body.child(LOGINS_PROBLEM))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(r(theme.space.s8))
                            .when_some(fill, |buttons, (target, entry)| {
                                buttons.child(
                                    Pill::new("logins.fill", "Fill on host")
                                        .variant(PillVariant::Primary)
                                        .small()
                                        .on_press(move |window, cx| {
                                            send(
                                                VaultHostAction::Fill(VaultFill {
                                                    bot_id: target.bot_id.clone(),
                                                    generation: target.generation.clone(),
                                                    page_id: target.page_id.clone(),
                                                    control_epoch: target.control_epoch,
                                                    id: entry.id.clone(),
                                                    revision: entry.revision,
                                                }),
                                                window,
                                                cx,
                                            );
                                            window.close_dialog(cx);
                                        }),
                                )
                            })
                            .when(state == Some(VaultState::Locked), |buttons| {
                                buttons.child(
                                    Pill::new("logins.unlock", "Unlock on host")
                                        .small()
                                        .on_press(|window, cx| send(VaultHostAction::Unlock, window, cx)),
                                )
                            })
                            .child(Pill::new("logins.manage", "Manage logins").small().on_press(|window, cx| {
                                window.close_dialog(cx);
                                cx.navigate(Route::Settings(Section::Logins));
                            })),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_https_page_can_take_a_saved_login() {
        assert_eq!(
            fill_origin("https://docs.rs/rusqlite/latest/").as_deref(),
            Some("https://docs.rs")
        );
        assert_eq!(fill_origin("http://example.com/"), None, "not HTTPS");
        assert_eq!(
            fill_origin("https://user:secret@example.com/"),
            None,
            "credentials in it"
        );
        assert_eq!(fill_origin("about:blank"), None);
        assert_eq!(fill_origin(""), None);
    }

    #[test]
    fn a_fill_is_for_the_person_in_control_of_an_open_page() {
        let mut state = BrowserState {
            bot_id: "b1".into(),
            generation: "g1".into(),
            control: BrowserControl::Human,
            control_epoch: Some(3),
            private_epoch: None,
            private_active: None,
            can_control: Some(true),
            revision: None,
            viewport_revision: None,
            busy: None,
            active_page_id: Some("p1".into()),
            tabs: Vec::new(),
            dialog: None,
        };
        assert_eq!(
            fill_target(&state),
            Some(VaultFillTarget {
                bot_id: "b1".into(),
                generation: "g1".into(),
                page_id: "p1".into(),
                control_epoch: 3
            })
        );
        state.private_active = Some(true);
        assert_eq!(fill_target(&state), None, "private");
        state.private_active = None;
        state.control = BrowserControl::Bot;
        assert_eq!(fill_target(&state), None, "the bot drives");
        state.control = BrowserControl::Human;
        state.can_control = Some(false);
        assert_eq!(fill_target(&state), None, "another device holds it");
    }
}
