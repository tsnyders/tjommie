//! General in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx (GeneralSection)` and `components/general/desktop-app.tsx`).
//! The window closes to the tray. This device's own app controls, start at sign-in and the updater, come through
//! `app_controls`; where there are none, the row says they're unavailable, as the TypeScript does for a desktop without them.

use std::sync::Arc;

use desktop_host::updater::UpdateState;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::app_controls::{self, DesktopApp};
use crate::guard::guarded;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::problem;
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::primitives::toggle::Toggle;

const LOGIN: &str = "Start Tjommie when you sign in to Windows";

/// The start-at-sign-in switch: what the app last confirmed (`None` until General first shows, when it's read), and the
/// line that says why the last change failed.
#[derive(Default)]
struct LoginState {
    on: Option<bool>,
    problem: Option<SharedString>,
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let card = super::card(cx).child(SettingsRow::new("Closing the desktop window").description(
        "Tjommie keeps running in this device's tray. Choose Quit from the tray to stop it.",
    ));
    let card = match app_controls::controls(cx) {
        Some(app) => card
            .child(login_item(window, Arc::clone(&app), cx))
            .child(updates(app)),
        None => card.child(unavailable()),
    };
    super::frame(
        "general",
        "General",
        "These app controls belong to the device you are using, even when you connect to another PC.",
        cx,
    )
    .child(
        card.child(
            SettingsRow::new("Connected host settings")
                .description(
                    "Agents, Accounts, Usage and Devices refer to the selected host PC. Switching PCs changes those sections.",
                )
                .ruled(),
        ),
    )
    .into_any_element()
}

/// Without the app's controls: the row says they aren't available in this app version.
fn unavailable() -> AnyElement {
    div()
        .semantic("settings.general.updates", Role::Group)
        .aria_label("Unavailable in this app version.")
        .child(
            SettingsRow::new("Startup and update controls")
                .description("Unavailable in this app version.")
                .ruled(),
        )
        .into_any_element()
}

/// Start at sign-in. The switch shows what the app last confirmed. A change goes to the app, and then the switch shows
/// what the app now says; a refusal says why, under the row.
fn login_item(window: &mut Window, app: Arc<dyn DesktopApp>, cx: &mut App) -> AnyElement {
    let login = window.use_keyed_state("settings.general.login", cx, |_, _| LoginState::default());
    let known = login.read(cx).on;
    let on = match known {
        Some(on) => on,
        None => {
            let on = app.login_item();
            login.update(cx, |login, _| login.on = Some(on));
            on
        }
    };
    let said = login.read(cx).problem.clone();
    let change = login.clone();
    let switch =
        Toggle::new("settings.general.login_item", LOGIN, on).on_toggle(move |next, _, cx| {
            guarded("change starting at sign-in", || {
                set_login(&change, app.as_ref(), next, cx)
            });
        });
    div()
        .flex()
        .flex_col()
        .child(SettingsRow::new(LOGIN).control(switch).ruled())
        .children(said.map(|said| problem("settings.general.login_problem", said, cx)))
        .into_any_element()
}

/// Turns starting at sign-in on or off through the app, then keeps what the app now says. A refused change leaves the
/// switch where the app has it, and says why.
fn set_login(login: &Entity<LoginState>, app: &dyn DesktopApp, on: bool, cx: &mut App) {
    let outcome = app.set_login_item(on);
    let confirmed = app.login_item();
    login.update(cx, |login, cx| {
        login.on = Some(confirmed);
        login.problem = outcome
            .err()
            .map(|text| SharedString::from(format!("Couldn't change this setting: {text}")));
        cx.notify();
    });
}

/// The updater's state, read each time General draws (the app redraws the window when the updater reports a change),
/// and the one control it offers: Restart once an update is ready, Check now otherwise, none where the build can't
/// update itself.
fn updates(app: Arc<dyn DesktopApp>) -> AnyElement {
    let update = app.update_state();
    let text = update_text(&update);
    let busy = matches!(
        update,
        UpdateState::Checking | UpdateState::Downloading { .. }
    );
    let row = SettingsRow::new("Updates")
        .description(text.clone())
        .ruled();
    let row = match update {
        UpdateState::Ready { .. } => row.control(
            Pill::new("settings.general.restart", "Restart")
                .variant(PillVariant::Primary)
                .on_press(move |_, _| {
                    guarded("restart to update", || app.restart_to_update());
                }),
        ),
        UpdateState::Unsupported => row,
        _ => row.control(
            Pill::new("settings.general.check", "Check now")
                .disabled(busy)
                .on_press(move |_, _| {
                    guarded("check for updates", || app.check_for_updates());
                }),
        ),
    };
    div()
        .semantic("settings.general.update_status", Role::Group)
        .aria_label(text)
        .child(row)
        .into_any_element()
}

/// The updater's state in words (`updateText`). A failed check shows the first line of its message, trimmed, at most
/// 120 characters.
fn update_text(update: &UpdateState) -> String {
    match update {
        UpdateState::Idle => "Not checked yet".to_owned(),
        UpdateState::Checking => "Checking".to_owned(),
        UpdateState::None => "Up to date".to_owned(),
        UpdateState::Downloading { percent } => format!("Downloading {}%", percent.round()),
        UpdateState::Ready { version } => format!("Ready: Restart to update to v{version}"),
        UpdateState::Error { message } => {
            let first = message
                .trim()
                .split(['\r', '\n'])
                .next()
                .unwrap_or_default();
            format!(
                "Couldn't check: {}",
                first.chars().take(120).collect::<String>()
            )
        }
        UpdateState::Unsupported => "This build doesn't update itself.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use desktop_host::updater::UpdateState;
    use gpui::{TestAppContext, VisualTestContext};

    use super::LOGIN;
    use crate::app_controls::{AppControls, DesktopApp};
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, click, draw, node, state, text};

    const SWITCH: &str = "settings.general.login_item";
    const PROBLEM: &str = "settings.general.login_problem";
    const STATUS: &str = "settings.general.update_status";
    const CHECK: &str = "settings.general.check";
    const RESTART: &str = "settings.general.restart";

    /// A call the fake desktop app was asked to make.
    #[derive(Clone, Debug, PartialEq)]
    enum Call {
        SetLoginItem(bool),
        CheckForUpdates,
        RestartToUpdate,
    }

    /// The desktop app as a test sets it: the login item and the updater answer from fields the test changes, a change
    /// the app refuses is in `refuses`, and every call is recorded.
    struct FakeApp {
        login: Mutex<bool>,
        refuses: Mutex<Option<String>>,
        update: Mutex<UpdateState>,
        calls: Mutex<Vec<Call>>,
    }

    impl FakeApp {
        fn new(login: bool, update: UpdateState) -> Arc<Self> {
            Arc::new(Self {
                login: Mutex::new(login),
                refuses: Mutex::new(None),
                update: Mutex::new(update),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, call: Call) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl DesktopApp for FakeApp {
        fn login_item(&self) -> bool {
            *self.login.lock().unwrap()
        }

        fn set_login_item(&self, on: bool) -> Result<(), String> {
            self.record(Call::SetLoginItem(on));
            if let Some(refusal) = self.refuses.lock().unwrap().clone() {
                return Err(refusal);
            }
            *self.login.lock().unwrap() = on;
            Ok(())
        }

        fn update_state(&self) -> UpdateState {
            self.update.lock().unwrap().clone()
        }

        fn check_for_updates(&self) {
            self.record(Call::CheckForUpdates);
        }

        fn restart_to_update(&self) {
            self.record(Call::RestartToUpdate);
        }
    }

    /// General in the app's window, with `desktop` as the app's controls (none when `None`).
    fn general(cx: &mut TestAppContext, desktop: Option<Arc<FakeApp>>) -> &mut VisualTestContext {
        if let Some(desktop) = desktop {
            cx.update(|cx| cx.set_global(AppControls(desktop)));
        }
        app(
            cx,
            Route::Settings(Section::General),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        )
        .0
    }

    /// The updater reports a change: its state moves, and the app redraws its windows, as it does in production.
    fn report(cx: &mut VisualTestContext, desktop: &FakeApp, update: UpdateState) {
        *desktop.update.lock().unwrap() = update;
        cx.update(|_, cx| cx.refresh_windows());
        draw(cx);
    }

    #[gpui::test]
    fn without_the_app_controls_the_row_says_they_are_unavailable(cx: &mut TestAppContext) {
        general(cx, None);
        assert_eq!(
            text("settings.general.updates"),
            "Unavailable in this app version."
        );
        assert!(node(SWITCH).is_none());
        assert!(node(STATUS).is_none());
    }

    /// The switch shows the app's value when General opens, and each flip goes through the app.
    #[gpui::test]
    fn the_switch_shows_the_app_value_and_flips_it_through_the_app(cx: &mut TestAppContext) {
        let desktop = FakeApp::new(true, UpdateState::Idle);
        let cx = general(cx, Some(Arc::clone(&desktop)));
        let switch = node(SWITCH).expect("the switch is on screen");
        assert_eq!(switch.text, LOGIN);
        assert!(switch.toggled, "on, as the app says");
        click(cx, SWITCH);
        assert_eq!(desktop.calls(), [Call::SetLoginItem(false)]);
        assert!(!node(SWITCH).unwrap().toggled);
        click(cx, SWITCH);
        assert_eq!(
            desktop.calls(),
            [Call::SetLoginItem(false), Call::SetLoginItem(true)]
        );
        assert!(node(SWITCH).unwrap().toggled);
    }

    /// A change the app refuses says why, under the row, and the switch keeps what the app confirmed. The line goes once
    /// a change works.
    #[gpui::test]
    fn a_refused_change_says_why_and_the_switch_keeps_what_the_app_confirmed(
        cx: &mut TestAppContext,
    ) {
        let desktop = FakeApp::new(true, UpdateState::Idle);
        *desktop.refuses.lock().unwrap() = Some("Access is denied.".to_owned());
        let cx = general(cx, Some(Arc::clone(&desktop)));
        click(cx, SWITCH);
        assert_eq!(
            text(PROBLEM),
            "Couldn't change this setting: Access is denied."
        );
        assert!(
            node(SWITCH).unwrap().toggled,
            "still on: the app refused the change"
        );
        *desktop.refuses.lock().unwrap() = None;
        click(cx, SWITCH);
        assert!(
            node(PROBLEM).is_none(),
            "a change that works clears the line"
        );
        assert!(!node(SWITCH).unwrap().toggled);
    }

    /// Each of the updater's states, in its words.
    #[gpui::test]
    fn the_updater_says_where_it_stands_in_each_state(cx: &mut TestAppContext) {
        let desktop = FakeApp::new(true, UpdateState::Idle);
        let cx = general(cx, Some(Arc::clone(&desktop)));
        let cases = [
            (UpdateState::Idle, "Not checked yet".to_owned()),
            (UpdateState::Checking, "Checking".to_owned()),
            (UpdateState::None, "Up to date".to_owned()),
            (
                UpdateState::Downloading { percent: 42.4 },
                "Downloading 42%".to_owned(),
            ),
            (
                UpdateState::Ready {
                    version: "1.2.3".to_owned(),
                },
                "Ready: Restart to update to v1.2.3".to_owned(),
            ),
            (
                UpdateState::Error {
                    message: "Network is down.".to_owned(),
                },
                "Couldn't check: Network is down.".to_owned(),
            ),
            (
                UpdateState::Unsupported,
                "This build doesn't update itself.".to_owned(),
            ),
        ];
        for (update, said) in cases {
            report(cx, &desktop, update);
            assert_eq!(text(STATUS), said);
        }
    }

    /// A failed check shows only the first line of its message, trimmed, at most 120 characters.
    #[gpui::test]
    fn a_failed_check_shows_its_first_line_at_most_120_characters(cx: &mut TestAppContext) {
        let desktop = FakeApp::new(true, UpdateState::Idle);
        let cx = general(cx, Some(Arc::clone(&desktop)));
        let detail = format!("HTTP 404: {}", "x".repeat(140));
        report(
            cx,
            &desktop,
            UpdateState::Error {
                message: format!("  {detail}\r\nHeaders: secret  "),
            },
        );
        assert_eq!(
            text(STATUS),
            format!(
                "Couldn't check: {}",
                detail.chars().take(120).collect::<String>()
            )
        );
    }

    /// Restart shows only once an update is ready, and restarts through the app.
    #[gpui::test]
    fn restart_shows_only_once_an_update_is_ready(cx: &mut TestAppContext) {
        let desktop = FakeApp::new(
            true,
            UpdateState::Ready {
                version: "1.2.3".to_owned(),
            },
        );
        let cx = general(cx, Some(Arc::clone(&desktop)));
        assert_eq!(text(STATUS), "Ready: Restart to update to v1.2.3");
        assert!(
            node(CHECK).is_none(),
            "Restart takes the place of Check now"
        );
        click(cx, RESTART);
        assert_eq!(desktop.calls(), [Call::RestartToUpdate]);
        report(cx, &desktop, UpdateState::None);
        assert!(node(RESTART).is_none());
        assert!(node(CHECK).is_some());
    }

    /// Check now starts a check, and is unavailable while one runs.
    #[gpui::test]
    fn check_now_runs_a_check_unless_one_is_under_way(cx: &mut TestAppContext) {
        let desktop = FakeApp::new(true, UpdateState::Idle);
        let cx = general(cx, Some(Arc::clone(&desktop)));
        click(cx, CHECK);
        assert_eq!(desktop.calls(), [Call::CheckForUpdates]);
        for busy in [
            UpdateState::Checking,
            UpdateState::Downloading { percent: 10.0 },
        ] {
            report(cx, &desktop, busy);
            assert!(!node(CHECK).unwrap().enabled, "unavailable while it runs");
            click(cx, CHECK);
        }
        assert_eq!(
            desktop.calls(),
            [Call::CheckForUpdates],
            "a pressed unavailable button does nothing"
        );
    }

    /// A build that can't update itself says so, and offers no control.
    #[gpui::test]
    fn a_build_that_cannot_update_offers_no_control(cx: &mut TestAppContext) {
        let desktop = FakeApp::new(true, UpdateState::Unsupported);
        general(cx, Some(desktop));
        assert_eq!(text(STATUS), "This build doesn't update itself.");
        assert!(node(CHECK).is_none());
        assert!(node(RESTART).is_none());
    }
}
