//! About in Settings (Phase 3 plan T5.5; `settings/diagnostics-section.tsx`'s `AboutSection`): this device's version
//! as against the host's and its daemon's, the release, source and license links, and the third-party notices in a
//! dialog. The notices are `third-party-notices.txt` beside the app's exe, as the release bundles them; without the
//! file the dialog says they are unavailable, as the TypeScript does when its notices can't be read.

use gpui::{
    AnyElement, App, AppContext as _, Entity, IntoElement, ParentElement as _, Role,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use serde_json::Value;

use super::diagnostics::{APP_VERSION, system_info, value};
use crate::guard::guarded_async;
use crate::model::diagnostics::diagnostic_version;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Link, problem};
use crate::ui::primitives::button::Pill;
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

const RELEASES: &str = "https://github.com/tsnyders/tjommie/releases/latest";
const PROJECT: &str = "https://github.com/tsnyders/tjommie";
const LICENSE: &str = "https://github.com/tsnyders/tjommie/blob/main/LICENSE";

/// The notices' largest size (256 KiB, as the TypeScript allows).
const NOTICES_LIMIT: u64 = 262_144;

/// Opens a link in the default browser; one that can't open is only logged, as the TypeScript's did nothing.
fn open(url: &str) {
    if let Err(why) = desktop_host::links::open_external(url) {
        log::warn!("About: a link did not open ({why})");
    }
}

/// A version from the host's facts, or "Unknown".
fn version_of(value: Option<&String>) -> String {
    diagnostic_version(value.map(|version| Value::from(version.as_str())).as_ref())
        .unwrap_or_else(|| "Unknown".to_owned())
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let info = system_info(window, cx);
    let paired = store::state(cx).host.is_some();
    let (host_version, daemon_version) = {
        let host = info.read(cx).host.as_ref();
        (
            version_of(host.and_then(|host| host.app_version.as_ref())),
            version_of(host.and_then(|host| host.daemon_version.as_ref())),
        )
    };
    let device_version = diagnostic_version(Some(&Value::from(APP_VERSION)))
        .unwrap_or_else(|| "Unknown in this app".to_owned());
    let theme = theme(cx);
    super::frame(
        "about",
        "About",
        "Tjommie connects this device to agents running on the selected host PC.",
        cx,
    )
    .child(
        super::card(cx)
            .child(
                SettingsRow::new("This device's app version")
                    .control(value("settings.about.device_version", device_version, cx)),
            )
            .child(
                SettingsRow::new("Connected host's app version")
                    .description(if paired {
                        "The paired PC, which may have a different version."
                    } else {
                        "The host running on this device."
                    })
                    .control(value("settings.about.host_version", host_version, cx))
                    .ruled(),
            )
            .child(
                SettingsRow::new("Host daemon version")
                    .control(value("settings.about.daemon_version", daemon_version, cx))
                    .ruled(),
            )
            .child(
                SettingsRow::new("Updates")
                    .description("This device's automatic update state is in General. Update the host on that PC.")
                    .control(Link::new("settings.about.releases", "Latest release").on_press(|_, _| open(RELEASES)))
                    .ruled(),
            )
            .child(
                SettingsRow::new("Source code")
                    .control(Link::new("settings.about.source", "GitHub project").on_press(|_, _| open(PROJECT)))
                    .ruled(),
            )
            .child(
                SettingsRow::new("License")
                    .control(
                        Link::new("settings.about.license", "Apache License 2.0").on_press(|_, _| open(LICENSE)),
                    )
                    .ruled(),
            ),
    )
    .child(
        div().mt(r(theme.space.s16)).child(
            Pill::new("settings.about.notices", "Third-party notices")
                .on_press(open_notices),
        ),
    )
    .into_any_element()
}

/// The notices as read: `None` while reading, then their text or `Err` when unavailable.
type Notices = Option<Result<String, ()>>;

/// Reads the notices beside the exe, off the UI thread.
fn read_notices() -> Result<String, ()> {
    let path = std::env::current_exe()
        .map_err(|_| ())?
        .with_file_name("third-party-notices.txt");
    let size = std::fs::metadata(&path).map_err(|_| ())?.len();
    if size > NOTICES_LIMIT {
        return Err(());
    }
    std::fs::read_to_string(&path).map_err(|_| ())
}

/// The third-party notices, in the app (`ThirdPartyNotices`): loading, unavailable, or their text.
fn open_notices(window: &mut Window, cx: &mut App) {
    let notices: Entity<Notices> = cx.new(|_| None);
    let reading = notices.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("read the third-party notices", async move {
            let read = cx.background_spawn(async { read_notices() }).await;
            let _ = cx.update(|cx| {
                reading.update(cx, |notices, cx| {
                    *notices = Some(read);
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
    open_dialog(window, cx, move |dialog, window, cx| {
        let theme = theme(cx);
        let body: AnyElement = match notices.read(cx) {
            None => value("settings.about.notices.loading", "Loading notices…", cx),
            Some(Err(())) => problem(
                "settings.about.notices.problem",
                "Notices are unavailable in this app build.",
                cx,
            ),
            Some(Ok(text)) => div()
                .semantic("settings.about.notices.text", Role::Code)
                .aria_label("Third-party license texts")
                .mt(r(theme.space.s12))
                .max_h(window.viewport_size().height * 0.65)
                .overflow_y_scroll()
                .type_style(&theme.typography.desktop.meta)
                .font_family(crate::assets::MONO_FAMILY)
                .text_color(hsla(palette(cx).text.secondary))
                .child(text.clone())
                .into_any_element(),
        };
        // diagnostics-section.tsx's 960 px, as rems.
        dialog
            .w(window.rem_size() * 60.0)
            .title(title("Third-party notices", cx))
            .child(body)
    });
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use protocol::diagnostics::HostDiagnostics;
    use protocol::wire::method;
    use serde_json::json;

    use super::APP_VERSION;
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, click, draw, node, state, text};

    /// diagnostics-section.test.tsx (About's half of "distinguishes the current device version from the selected
    /// host"): this device's version, the host's and its daemon's, and the notices button.
    #[gpui::test]
    fn tells_this_devices_version_from_the_hosts(cx: &mut TestAppContext) {
        let host: HostDiagnostics = serde_json::from_value(json!({
            "appVersion": "0.3.2", "daemonVersion": "0.3.3", "providers": [], "relay": "off", "storage": "ok"
        }))
        .unwrap();
        let rpc = FakeRpc::new().on::<method::DiagnosticsGet>(move |_| Ok(host.clone()));
        let (_, _) = app(cx, Route::Settings(Section::About), state(vec![], vec![], vec![]), rpc);
        assert_eq!(text("settings.about.device_version"), APP_VERSION);
        assert_eq!(text("settings.about.host_version"), "0.3.2");
        assert_eq!(text("settings.about.daemon_version"), "0.3.3");
        assert!(node("settings.about.notices").is_some());
        assert!(node("settings.about.license").is_some());
    }

    /// diagnostics-section.test.tsx: "opens bundled notices inside the app without a blocked new window" — the test
    /// runner has no notices beside it, so the dialog says they are unavailable.
    #[gpui::test]
    fn the_notices_open_in_the_app(cx: &mut TestAppContext) {
        let rpc = FakeRpc::new().on::<method::DiagnosticsGet>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "not now".into(),
            })
        });
        let (cx, _) = app(cx, Route::Settings(Section::About), state(vec![], vec![], vec![]), rpc);
        assert_eq!(text("settings.about.host_version"), "Unknown");
        click(cx, "settings.about.notices");
        draw(cx);
        assert_eq!(text("dialog.title"), "Third-party notices");
        assert_eq!(
            text("settings.about.notices.problem"),
            "Notices are unavailable in this app build."
        );
    }
}
