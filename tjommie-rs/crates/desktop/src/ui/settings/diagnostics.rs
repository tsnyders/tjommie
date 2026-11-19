//! Diagnostics in Settings (Phase 3 plan T5.5; `settings/diagnostics-section.tsx`'s `DiagnosticsSection`, row 18):
//! read-only checks of this device and the selected host, and the report with Refresh checks, Copy report and Save
//! report. The report holds versions and fixed health categories only (`model::diagnostics`); the host's name shows
//! here and never goes in it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use desktop_host::diagnostics::{REPORT_FILE_NAME, ReportSaver, SaveOutcome};
use gpui::{
    AnyElement, App, AsyncApp, ClipboardItem, Entity, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
};
use protocol::diagnostics::HostDiagnostics;
use protocol::wire::{Empty, method};
use serde_json::{Value, json};

use crate::guard::guarded_async;
use crate::model::diagnostics::{ReportInput, diagnostic_report};
use crate::model::state::{Connection, OfflineReason};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::problem;
use crate::ui::primitives::button::Pill;
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::shell::data::ShellData;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// This app's version: this device's, as against the host's.
pub(super) const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One report save at a time, for the whole app (the second would open a second dialog over the first). It's the app's,
/// not the process's, so view tests running side by side don't share it.
#[derive(Default)]
struct Saver(Arc<ReportSaver>);

impl gpui::Global for Saver {}

/// What a settings section knows of this device and the host (`useSystemInfo`): the host's `diagnostics.get`, read
/// again when the PC, the connection or a refresh changes, and never shown for another PC than the one it was read for.
#[derive(Default)]
pub(super) struct SystemInfo {
    pub(super) host: Option<HostDiagnostics>,
    pub(super) host_unavailable: bool,
    pub(super) loading: bool,
    /// The PC, whether connected, and the refresh it was read for.
    read_for: Option<(Option<String>, bool, u64)>,
    revision: u64,
}

/// The section's system info, read when what it was read for has changed.
pub(super) fn system_info(window: &mut Window, cx: &mut App) -> Entity<SystemInfo> {
    let info = window.use_keyed_state("settings.system_info", cx, |_, _| SystemInfo::default());
    let state = store::state(cx);
    let wanted = (
        state.host.clone(),
        state.connection == Connection::Ready,
        info.read(cx).revision,
    );
    if info.read(cx).read_for.as_ref() != Some(&wanted) {
        read(&info, wanted, cx);
    }
    info
}

fn read(info: &Entity<SystemInfo>, wanted: (Option<String>, bool, u64), cx: &mut App) {
    info.update(cx, |info, _| {
        info.read_for = Some(wanted.clone());
        info.loading = true;
        // Never another PC's result: what was read before goes now.
        info.host = None;
        info.host_unavailable = false;
    });
    let call = wanted
        .1
        .then(|| store::request::<method::DiagnosticsGet>(cx, Empty {}));
    let info = info.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("read the diagnostics", async move {
            let host = match call {
                Some(call) => Some(call.await),
                None => None,
            };
            let _ = cx.update(|cx| {
                info.update(cx, |info, cx| {
                    if info.read_for.as_ref() != Some(&wanted) {
                        return;
                    }
                    info.loading = false;
                    match host {
                        Some(Ok(host)) => info.host = Some(host),
                        Some(Err(_)) => info.host_unavailable = true,
                        None => {}
                    }
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

/// The connection as the report reads it: `{ status, reason }`.
pub(super) fn connection_value(connection: &Connection) -> Value {
    let (status, reason) = match connection {
        Connection::Connecting => ("connecting", None),
        Connection::Ready => ("ready", None),
        Connection::Reconnecting => ("reconnecting", None),
        Connection::Offline { reason } => (
            "offline",
            Some(match reason {
                OfflineReason::HostOffline => "host_offline",
                OfflineReason::Unreachable => "unreachable",
            }),
        ),
        Connection::Removed => ("removed", None),
        Connection::Update => ("update", None),
        Connection::Failed { .. } => ("failed", None),
        Connection::Outside => ("outside", None),
    };
    json!({ "status": status, "reason": reason })
}

/// The report for what the section knows now.
pub(super) fn report(info: &SystemInfo, cx: &App) -> String {
    let state = store::state(cx);
    diagnostic_report(&ReportInput {
        device: json!({ "appVersion": APP_VERSION, "platform": "windows" }),
        host: info
            .host
            .as_ref()
            .and_then(|host| serde_json::to_value(host).ok())
            .unwrap_or(Value::Null),
        speech: crate::speech::status_value(cx),
        connection: connection_value(&state.connection),
        paired: state.host.is_some(),
        platform: "windows",
        // No file session in this app yet (Settings, Files).
        attachments: false,
    })
}

/// A value at a row's right, in secondary text.
pub(super) fn value(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    cx: &App,
) -> AnyElement {
    let text = text.into();
    div()
        .semantic(id, Role::Paragraph)
        .aria_label(text.clone())
        .type_style(&theme(cx).typography.desktop.ui)
        .text_color(hsla(palette(cx).text.secondary))
        .child(text)
        .into_any_element()
}

/// A daemon enum as its wire name ("online", "ok").
fn named(value: &impl serde::Serialize) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
}

#[derive(Clone, Copy, PartialEq)]
enum Export {
    Copy,
    Save,
}

/// The section's exports: the one under way, and what the last one said.
struct Exports {
    action: Option<Export>,
    notice: Option<SharedString>,
    problem: Option<SharedString>,
    /// Set when the section goes, which cancels a save not yet finished.
    gone: Arc<AtomicBool>,
}

impl Drop for Exports {
    fn drop(&mut self) {
        self.gone.store(true, Ordering::SeqCst);
    }
}

/// Clears the export under way when its task ends, however it ends (a panic the guard caught included), so a failed
/// save never leaves Refresh, Copy and Save dimmed.
struct ClearAction {
    exports: WeakEntity<Exports>,
    cx: AsyncApp,
}

impl Drop for ClearAction {
    fn drop(&mut self) {
        let exports = self.exports.clone();
        self.cx.update(|cx| {
            let _ = exports.update(cx, |exports, cx| {
                if exports.action.take().is_some() {
                    cx.notify();
                }
            });
        });
    }
}

fn save(exports: &Entity<Exports>, report: String, cx: &mut App) {
    let gone = Arc::clone(&exports.read(cx).gone);
    exports.update(cx, |exports, cx| {
        exports.action = Some(Export::Save);
        exports.notice = None;
        exports.problem = None;
        cx.notify();
    });
    let exports = exports.downgrade();
    let directory = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Documents");
    // The save writes on Tokio's blocking pool while its picker is GPUI's: it's polled here with the live runtime
    // entered (M5 review C1).
    let tokio = cx
        .try_global::<crate::live::Tokio>()
        .map(|tokio| tokio.0.clone());
    let saver = Arc::clone(&cx.default_global::<Saver>().0);
    cx.spawn(async move |cx| {
        guarded_async("save the diagnostic report", async move {
            let _clear = ClearAction {
                exports: exports.clone(),
                cx: cx.clone(),
            };
            let picker = cx.clone();
            let saving = saver.save_report(
                &report,
                move || async move {
                    let picked = picker
                        .update(|cx| cx.prompt_for_new_path(&directory, Some(REPORT_FILE_NAME)));
                    picked.await.ok().and_then(Result::ok).flatten()
                },
                move || gone.load(Ordering::SeqCst),
            );
            let outcome = crate::live::in_runtime(tokio, saving).await;
            let _ = cx.update(|cx| {
                exports.update(cx, |exports, cx| {
                    exports.action = None;
                    match outcome {
                        Ok(SaveOutcome::Saved) => {
                            exports.notice = Some("Report saved on this device.".into())
                        }
                        Ok(SaveOutcome::Cancelled) => {
                            exports.notice = Some("Save cancelled.".into())
                        }
                        Err(_) => {
                            exports.problem = Some(
                                "The report could not be saved. Try again or copy the preview."
                                    .into(),
                            )
                        }
                    }
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let info = system_info(window, cx);
    let exports = window.use_keyed_state("settings.diagnostics.exports", cx, |_, _| Exports {
        action: None,
        notice: None,
        problem: None,
        gone: Arc::new(AtomicBool::new(false)),
    });
    let report = report(info.read(cx), cx);
    let state = store::state(cx);
    let host_name: SharedString = match &state.host {
        None => "This device's host".into(),
        Some(id) => cx
            .try_global::<ShellData>()
            .and_then(|data| {
                data.saved
                    .iter()
                    .find(|saved| saved.id.as_ref() == id.as_str())
                    .map(|saved| saved.name.clone())
            })
            .unwrap_or_else(|| "Paired PC".into()),
    };
    let status = connection_value(&state.connection)
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let (storage, relay, unavailable, loading) = {
        let info = info.read(cx);
        (
            info.host.as_ref().and_then(|host| named(&host.storage)),
            info.host.as_ref().and_then(|host| named(&host.relay)),
            info.host_unavailable,
            info.loading,
        )
    };
    let (action, notice, failed) = {
        let exports = exports.read(cx);
        (
            exports.action,
            exports.notice.clone(),
            exports.problem.clone(),
        )
    };
    let theme = theme(cx);
    let colors = palette(cx);
    let refresh = {
        let (info, exports) = (info.clone(), exports.clone());
        let pill = Pill::new(
            "settings.diagnostics.refresh",
            if loading {
                "Checking…"
            } else {
                "Refresh checks"
            },
        );
        pill.disabled(loading || action.is_some())
            .on_press(move |_, cx| {
                exports.update(cx, |exports, cx| {
                    exports.notice = None;
                    cx.notify();
                });
                info.update(cx, |info, cx| {
                    info.revision += 1;
                    cx.notify();
                });
            })
    };
    let copy = {
        let (exports, report) = (exports.clone(), report.clone());
        let pill = Pill::new(
            "settings.diagnostics.copy",
            if action == Some(Export::Copy) {
                "Copying…"
            } else {
                "Copy report"
            },
        );
        pill.disabled(action.is_some()).on_press(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(report.clone()));
            exports.update(cx, |exports, cx| {
                exports.problem = None;
                exports.notice = Some("Report copied.".into());
                cx.notify();
            });
        })
    };
    let saving = {
        let (exports, report) = (exports.clone(), report.clone());
        let pill = Pill::new(
            "settings.diagnostics.save",
            if action == Some(Export::Save) {
                "Saving…"
            } else {
                "Save report"
            },
        );
        pill.disabled(action.is_some())
            .on_press(move |_, cx| save(&exports, report.clone(), cx))
    };
    super::frame(
        "diagnostics",
        "Diagnostics",
        "Read-only checks. No model turn, upload or chat attachment is created.",
        cx,
    )
    .child(
        super::card(cx)
            .child(
                SettingsRow::new("Connected host")
                    .description("The host name is shown here but excluded from the report.")
                    .control(value("settings.diagnostics.host", host_name, cx)),
            )
            .child(
                SettingsRow::new("Connection")
                    .control(value("settings.diagnostics.connection", status, cx))
                    .ruled(),
            )
            .child(
                SettingsRow::new("Host storage")
                    .control(value(
                        "settings.diagnostics.storage",
                        storage.unwrap_or_else(|| "Unknown".to_owned()),
                        cx,
                    ))
                    .ruled(),
            )
            .child(
                SettingsRow::new("Relay")
                    .description(
                        "A connected relay does not tell us which route this device is using. Direct/relay route \
                         and historical errors are unavailable.",
                    )
                    .control(value(
                        "settings.diagnostics.relay",
                        relay.unwrap_or_else(|| "Unknown".to_owned()),
                        cx,
                    ))
                    .ruled(),
            )
            .child(
                SettingsRow::new("Last connection error")
                    .description(
                        "Historical errors are not retained. The preview includes only a fixed category for the \
                         current connection issue.",
                    )
                    .control(value("settings.diagnostics.last_error", "Unavailable", cx))
                    .ruled(),
            ),
    )
    .children(unavailable.then(|| {
        problem(
            "settings.diagnostics.unavailable",
            "The host diagnostic check is unavailable. Unknown values stay in the report.",
            cx,
        )
    }))
    .child(super::heading("Report preview", cx))
    .child(
        div()
            .mt(r(theme.space.s10))
            .type_style(&theme.typography.desktop.small)
            .text_color(hsla(colors.text.secondary))
            .child(
                "Only versions and fixed health categories are included. Names, account identifiers, paths, URLs, \
                 tokens, pairing codes, logs, audio, transcripts, messages and file contents are excluded.",
            ),
    )
    .child(
        div()
            .semantic("settings.diagnostics.preview", Role::Code)
            .aria_label("Diagnostic report preview")
            .mt(r(theme.space.s12))
            .min_h(r(288.))
            .max_h(r(420.))
            .overflow_y_scroll()
            .p(r(theme.space.s12))
            .rounded(r(theme.radius.field))
            .border_1()
            .border_color(hsla(colors.line.field))
            .bg(hsla(colors.surface.raised))
            .type_style(&theme.typography.desktop.meta)
            .font_family(crate::assets::MONO_FAMILY)
            .text_color(hsla(colors.text.primary))
            .child(
                div()
                    .semantic("settings.diagnostics.preview.body", Role::Paragraph)
                    .aria_label(report.clone())
                    .child(report),
            ),
    )
    .child(
        div()
            .mt(r(theme.space.s12))
            .flex()
            .flex_wrap()
            .gap(r(theme.space.s8))
            .child(refresh)
            .child(copy)
            .child(saving),
    )
    .children(notice.map(|notice| {
        div()
            .mt(r(theme.space.s12))
            .child(value("settings.diagnostics.notice", notice, cx))
    }))
    .children(failed.map(|text| problem("settings.diagnostics.problem", text, cx)))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::TestAppContext;
    use protocol::diagnostics::HostDiagnostics;
    use protocol::wire::method;
    use serde_json::{Value, json};

    use super::APP_VERSION;
    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::data::{SavedHost, ShellData};
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, click, draw, node, state, text};

    fn host(version: &str) -> HostDiagnostics {
        serde_json::from_value(json!({
            "appVersion": version, "daemonVersion": version,
            "providers": [{ "provider": "claude", "version": "2.1.3" }], "relay": "online", "storage": "ok"
        }))
        .unwrap()
    }

    fn report() -> Value {
        serde_json::from_str(&text("settings.diagnostics.preview.body")).unwrap()
    }

    fn on_a_paired_pc() -> AppState {
        AppState {
            host: Some("pc-1".into()),
            ..state(vec![], vec![], vec![])
        }
    }

    fn name_the_pc(cx: &mut gpui::VisualTestContext) {
        cx.update(|_, cx| {
            let mut data = cx.global::<ShellData>().clone();
            data.saved = vec![SavedHost {
                id: "pc-1".into(),
                name: "Private host name".into(),
            }];
            cx.set_global(data);
        });
        draw(cx);
    }

    /// diagnostics-section.test.tsx: "distinguishes the current device version from the selected host, and excludes its
    /// display name".
    #[gpui::test]
    fn shows_the_hosts_name_keeps_it_out_of_the_report_and_tells_the_versions_apart(
        cx: &mut TestAppContext,
    ) {
        let rpc = FakeRpc::new().on::<method::DiagnosticsGet>(|_| Ok(host("0.3.2")));
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Diagnostics),
            on_a_paired_pc(),
            rpc,
        );
        name_the_pc(cx);
        assert_eq!(text("settings.diagnostics.host"), "Private host name");
        assert!(!text("settings.diagnostics.preview.body").contains("Private host name"));
        let report = report();
        assert_eq!(report["device"]["appVersion"], APP_VERSION);
        assert_eq!(report["host"]["appVersion"], "0.3.2");
        assert_eq!(report["host"]["selection"], "paired");
        assert_eq!(text("settings.diagnostics.storage"), "ok");
        assert_eq!(text("settings.diagnostics.relay"), "online");
    }

    /// diagnostics-section.test.tsx: "never reuses a previous host's diagnostic result after switching PCs".
    #[gpui::test]
    fn reads_the_host_again_after_switching_pcs(cx: &mut TestAppContext) {
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        let rpc = FakeRpc::new().on::<method::DiagnosticsGet>(move |_| {
            Ok(host(if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                "9.9.9"
            } else {
                "0.3.2"
            }))
        });
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Diagnostics),
            on_a_paired_pc(),
            rpc,
        );
        assert_eq!(report()["host"]["appVersion"], "9.9.9");
        cx.update(|_, cx| {
            crate::ui::store::update(cx, |state| {
                state.host = Some("pc-2".into());
                true
            })
        });
        draw(cx);
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "read again for the other PC"
        );
        assert_eq!(report()["host"]["appVersion"], "0.3.2");
        assert!(!text("settings.diagnostics.preview.body").contains("9.9.9"));
    }

    /// diagnostics-section.test.tsx: "copies exactly the preview and reports save cancellation without claiming
    /// success".
    #[gpui::test]
    fn copies_exactly_the_preview_and_says_a_cancelled_save(cx: &mut TestAppContext) {
        let rpc = FakeRpc::new().on::<method::DiagnosticsGet>(|_| Ok(host("0.3.2")));
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Diagnostics),
            state(vec![], vec![], vec![]),
            rpc,
        );
        let preview = text("settings.diagnostics.preview.body");
        click(cx, "settings.diagnostics.copy");
        let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(copied.as_deref(), Some(preview.as_str()));
        assert_eq!(text("settings.diagnostics.notice"), "Report copied.");
        click(cx, "settings.diagnostics.save");
        cx.simulate_new_path_selection(|_| None);
        draw(cx);
        assert_eq!(text("settings.diagnostics.notice"), "Save cancelled.");
        assert!(
            node("settings.diagnostics.problem").is_none(),
            "no claim of success or failure"
        );
    }

    /// M5 review C1: Save report writes the report where the picker says. Its write runs on Tokio's blocking pool, so the
    /// app's runtime is installed here as `live::start` installs it; before the fix the save panicked with no runtime.
    #[gpui::test]
    fn saves_the_report_where_the_picker_says(cx: &mut TestAppContext) {
        // The write is a real Tokio thread, so its wake-ups aren't the test scheduler's own.
        cx.executor().allow_parking();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let root = desktop_host::owner_safety::test_root().unwrap();
        let path = root.path().join("tjommie-diagnostics.json");
        let rpc = FakeRpc::new().on::<method::DiagnosticsGet>(|_| Ok(host("0.3.2")));
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Diagnostics),
            state(vec![], vec![], vec![]),
            rpc,
        );
        cx.update(|_, cx| cx.set_global(crate::live::Tokio(runtime.handle().clone())));
        let preview = text("settings.diagnostics.preview.body");
        click(cx, "settings.diagnostics.save");
        let chosen = path.clone();
        cx.simulate_new_path_selection(move |_| Some(chosen.clone()));
        // The write finishes on the runtime's own thread: draw until its answer has come back.
        for _ in 0..300 {
            draw(cx);
            if node("settings.diagnostics.notice")
                .is_some_and(|node| node.text == "Report saved on this device.")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            text("settings.diagnostics.notice"),
            "Report saved on this device."
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, serde_json::from_str::<Value>(&preview).unwrap());
        assert!(
            node("settings.diagnostics.save").is_some(),
            "Save is back, not stranded"
        );
    }

    /// diagnostics-section.test.tsx: "shows unknown versions and safe errors when a bridge/check is unavailable".
    #[gpui::test]
    fn says_when_the_host_check_is_unavailable_and_keeps_unknowns_in_the_report(
        cx: &mut TestAppContext,
    ) {
        let rpc = FakeRpc::new().on::<method::DiagnosticsGet>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "SYNTHETIC_SECRET at C:/private".into(),
            })
        });
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Diagnostics),
            state(vec![], vec![], vec![]),
            rpc,
        );
        draw(cx);
        assert!(text("settings.diagnostics.unavailable").contains("unavailable"));
        let report = report();
        assert_eq!(report["host"]["appVersion"], Value::Null);
        assert_eq!(report["host"]["storage"], "unknown");
        assert!(!text("settings.diagnostics.preview.body").contains("SYNTHETIC_SECRET"));
        assert_eq!(text("settings.diagnostics.storage"), "Unknown");
    }
}
