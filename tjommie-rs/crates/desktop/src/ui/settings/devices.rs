//! Devices in Settings (Phase 3 plan T5.5; `devices/devices-section.tsx`, `pair-dialog.tsx`, `relay-row.tsx`,
//! `use-devices.ts`, `pairing.ts`; row 15): the phones and other PCs paired with this one, Pair a device, Rename and
//! Revoke, and — in this PC's own window — the relay and the firewall note. The list is read when the section opens and
//! again on `devices.changed`. A pairing code exists only in its dialog: closing the dialog by any route cancels it.

use std::sync::Arc;

use futures::StreamExt as _;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Task, Window, canvas, div, point, px, size, svg,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::InputState;
use protocol::model::SettingsPatch;
use protocol::relay::DEFAULT_RELAY_URL;
use protocol::remote::{
    Device, DevicesRename, PairingStart, RelayState, RelayStatus, RelayUrl, RemoteName, RemoteStatus,
};
use protocol::wire::{Empty, Id, method};
use qrcodegen::{QrCode, QrCodeEcc};

use super::diagnostics::value;
use crate::guard::{guarded, guarded_async};
use crate::model::format::{stamp, time_ago};
use crate::model::labels::error_text;
use crate::model::state::Connection;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{TextField, label, problem};
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

const NOTE: &str = "Every paired device has full control of Tjommie, including pairing and revoking devices. Phones and \
                    other PCs can connect over Wi-Fi or, through the relay, from anywhere. This PC has to be awake. This \
                    PC's own window always works.";

/// "9:41" while there is time, "Expired" once there is none (`countdown`). The code works once and for ten minutes.
pub fn countdown(expires: &str, now_ms: i64) -> (String, bool) {
    let end = expires
        .parse::<jiff::Timestamp>()
        .map_or(0, |at| at.as_millisecond());
    let left = (end - now_ms).max(0);
    if left == 0 {
        return ("Expired".to_owned(), true);
    }
    let seconds = (left + 999) / 1000;
    (format!("{}:{:02}", seconds / 60, seconds % 60), false)
}

/// How the relay is doing, as the PC last said (`RelayRow`): its state, and whether push works through it once it is
/// connected.
pub fn relay_line(relay: &RelayStatus) -> String {
    let state = match relay.state {
        RelayState::Online => "Connected",
        RelayState::Connecting => "Connecting",
        RelayState::Error => "Can't reach the relay",
        RelayState::Off => "Off",
    };
    match (relay.state, relay.push) {
        (RelayState::Online, true) => format!("{state} · Push: available"),
        (RelayState::Online, false) => format!("{state} · Push: not set up on this relay"),
        _ => state.to_owned(),
    }
}

/// What a typed relay address saves as (`saveRelayUrl`'s check): none to turn the relay off, or an https address.
pub fn relay_address(typed: &str) -> Result<Option<RelayUrl>, &'static str> {
    let trimmed = typed.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match RelayUrl::new(trimmed) {
        Ok(url) if url.as_str().starts_with("https://") => Ok(Some(url)),
        _ => Err("Use an https:// address."),
    }
}

fn devices_event(event: &protocol::Event) -> bool {
    matches!(event, protocol::Event::DevicesChanged(_))
}

/// The paired devices and what the host is doing (`useDevices`), read when the section opens and again whenever the
/// host says its devices changed.
struct Devices {
    list: Option<Vec<Device>>,
    status: Option<RemoteStatus>,
    error: Option<SharedString>,
    read_for: Option<(bool, u64)>,
    tick: u64,
    _events: Task<()>,
}

impl Devices {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut feed = store::events(cx, devices_event);
        let events = cx.spawn(async move |this, cx| {
            guarded_async("follow the devices' changes", async move {
                while feed.arrivals.next().await.is_some() {
                    while let Ok(()) = feed.arrivals.try_recv() {}
                    let reloaded = this.update(cx, |devices, cx| {
                        devices.tick += 1;
                        cx.notify();
                    });
                    if reloaded.is_err() {
                        break;
                    }
                }
            })
            .await;
        });
        Devices {
            list: None,
            status: None,
            error: None,
            read_for: None,
            tick: 0,
            _events: events,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.tick += 1;
        cx.notify();
    }
}

/// Reads the list and the status when what they were read for changed.
fn read(devices: &Entity<Devices>, cx: &mut App) {
    let ready = store::state(cx).connection == Connection::Ready;
    let wanted = (ready, devices.read(cx).tick);
    if !ready || devices.read(cx).read_for == Some(wanted) {
        return;
    }
    devices.update(cx, |devices, _| devices.read_for = Some(wanted));
    let (list, status) = (
        store::request::<method::DevicesList>(cx, Empty {}),
        store::request::<method::RemoteStatus>(cx, Empty {}),
    );
    let devices = devices.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("read the paired devices", async move {
            let read = match (list.await, status.await) {
                (Ok(list), Ok(status)) => Ok((list, status)),
                (Err(error), _) | (_, Err(error)) => Err(error_text(&error)),
            };
            let _ = cx.update(|cx| {
                devices.update(cx, |devices, cx| {
                    match read {
                        Ok((list, status)) => {
                            devices.list = Some(list);
                            devices.status = Some(status);
                            devices.error = None;
                        }
                        Err(error) => devices.error = Some(error.into()),
                    }
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

/// A device's line: when it was paired, and whether it is on the line or when it was last seen.
fn device_line(device: &Device, connected: bool, now: &jiff::Zoned) -> String {
    let seen = if connected {
        "Connected now".to_owned()
    } else {
        match &device.last_seen_at {
            Some(seen) => format!("Last seen {}", time_ago(seen, now)),
            None => "Not seen yet".to_owned(),
        }
    };
    format!("Paired {} · {seen}", stamp(&device.paired_at, now))
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let devices = window.use_keyed_state("settings.devices.state", cx, |_, cx| Devices::new(cx));
    read(&devices, cx);
    let own_window = store::state(cx).host.is_none();
    let saved_relay = store::state(cx)
        .settings
        .as_ref()
        .and_then(|settings| settings.relay_url.as_ref().map(|url| url.as_str().to_owned()));
    let (list, status, error) = {
        let devices = devices.read(cx);
        (devices.list.clone(), devices.status.clone(), devices.error.clone())
    };
    let relay_status = status.as_ref().map(|status| status.relay.clone());
    let relay = own_window.then(|| relay_row(saved_relay, relay_status, window, cx));
    let now = jiff::Zoned::now();
    let theme = theme(cx);
    let colors = palette(cx);
    let active: Vec<Device> = list
        .iter()
        .flatten()
        .filter(|device| device.revoked_at.is_none())
        .cloned()
        .collect();
    let rows: Vec<AnyElement> = active
        .iter()
        .map(|device| {
            let connected = status
                .as_ref()
                .is_some_and(|status| status.connected_device_ids.contains(&device.id));
            let id = &device.id;
            let (renaming, revoking) = ((device.clone(), devices.clone()), (device.clone(), devices.clone()));
            div()
                .semantic(format!("settings.devices.{id}"), Role::ListItem)
                .aria_label(device.name.as_str().to_owned())
                .flex()
                .flex_wrap()
                .items_center()
                .gap(r(theme.space.s12))
                .p(r(theme.space.s16))
                .rounded(r(theme.radius.field))
                .bg(hsla(colors.surface.raised))
                .child(
                    svg()
                        .path("icons/i/175/smartphone.svg")
                        .size(r(20.))
                        .flex_none()
                        .text_color(hsla(colors.icon.base)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .type_style(&theme.typography.desktop.ui_strong)
                                .text_color(hsla(colors.text.primary))
                                .child(device.name.as_str().to_owned()),
                        )
                        .child(value(
                            format!("settings.devices.{id}.line"),
                            device_line(device, connected, &now),
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap(r(theme.space.s8))
                        .child(
                            Pill::new(format!("settings.devices.{id}.rename"), "Rename")
                                .small()
                                .on_press(move |window, cx| {
                                    open_rename(renaming.0.clone(), renaming.1.clone(), window, cx)
                                }),
                        )
                        .child(
                            Pill::new(format!("settings.devices.{id}.revoke"), "Revoke")
                                .small()
                                .variant(PillVariant::Danger)
                                .on_press(move |window, cx| {
                                    open_revoke(revoking.0.clone(), revoking.1.clone(), window, cx)
                                }),
                        ),
                )
                .into_any_element()
        })
        .collect();
    let body: AnyElement = match &list {
        None if error.is_none() => value("settings.devices.loading", "Loading devices", cx),
        None => div().into_any_element(),
        Some(_) if active.is_empty() => value("settings.devices.none", "No devices paired yet.", cx),
        Some(_) => div()
            .semantic("settings.devices.list", Role::List)
            .aria_label("Paired devices")
            .flex()
            .flex_col()
            .gap(r(theme.space.s12))
            .children(rows)
            .into_any_element(),
    };
    super::frame("devices", "Devices", NOTE, cx)
        .child(
            div().mt(r(theme.space.s16)).child(
                Pill::new("settings.devices.pair", "Pair a device")
                    .variant(PillVariant::Primary)
                    .small()
                    .on_press(open_pair),
            ),
        )
        .children(status.as_ref().filter(|status| !status.listening).map(|_| {
            div().mt(r(theme.space.s12)).child(value(
                "settings.devices.not_listening",
                "This PC isn't accepting devices right now.",
                cx,
            ))
        }))
        .children(error.map(|error| problem("settings.devices.problem", error, cx)))
        .child(div().mt(r(theme.space.s24)).child(body))
        .children(relay.map(|relay| {
            div()
                .mt(r(theme.space.s24))
                .flex()
                .flex_col()
                .gap(r(theme.space.s12))
                .child(relay)
                .child(value(
                    "settings.devices.firewall",
                    "On the same Wi-Fi, Windows Firewall asks once. If you say no, the relay still works.",
                    cx,
                ))
        }))
        .into_any_element()
}

// The relay (`RelayRow`).

struct Relay {
    draft: Entity<InputState>,
    busy: bool,
    problem: Option<SharedString>,
}

fn save_relay(relay: &Entity<Relay>, typed: String, cx: &mut App) {
    let url = match relay_address(&typed) {
        Ok(url) => url,
        Err(said) => {
            relay.update(cx, |relay, cx| {
                relay.problem = Some(said.into());
                cx.notify();
            });
            return;
        }
    };
    relay.update(cx, |relay, cx| {
        relay.busy = true;
        relay.problem = None;
        cx.notify();
    });
    let call = store::request::<method::SettingsSet>(
        cx,
        SettingsPatch {
            chain_limit: None,
            relay_url: Some(url),
        },
    );
    let relay = relay.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("save the relay's address", async move {
            let saved = call.await;
            let _ = cx.update(|cx| {
                relay.update(cx, |relay, cx| {
                    relay.busy = false;
                    relay.problem = saved.err().map(|error| error_text(&error).into());
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

fn relay_row(saved: Option<String>, state: Option<RelayStatus>, window: &mut Window, cx: &mut App) -> AnyElement {
    let start = saved.clone().unwrap_or_default();
    let relay = window.use_keyed_state(
        SharedString::from(format!("settings.devices.relay.{start}")),
        cx,
        move |window, cx| Relay {
            draft: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("https://")
                    .default_value(start.clone())
            }),
            busy: false,
            problem: None,
        },
    );
    let (draft, busy, failed) = {
        let relay = relay.read(cx);
        (relay.draft.clone(), relay.busy, relay.problem.clone())
    };
    let typed = draft.read(cx).value().to_string();
    let changed = typed.trim() != saved.as_deref().unwrap_or("");
    let theme = theme(cx);
    let save = {
        let (relay, draft) = (relay.clone(), draft.clone());
        Pill::new("settings.devices.relay.save", "Save")
            .variant(PillVariant::Primary)
            .small()
            .disabled(busy || !changed)
            .on_press(move |_, cx| {
                let typed = draft.read(cx).value().to_string();
                save_relay(&relay, typed, cx)
            })
    };
    let public = DEFAULT_RELAY_URL
        .filter(|public| saved.as_deref() != Some(*public))
        .map(|public| {
            let relay = relay.clone();
            Pill::new("settings.devices.relay.public", "Use the public relay")
                .small()
                .disabled(busy)
                .on_press(move |_, cx| save_relay(&relay, public.to_owned(), cx))
        });
    let line = state.map(|relay| value("settings.devices.relay.state", relay_line(&relay), cx));
    super::card(cx)
        .py(r(theme.space.s16))
        .gap(r(theme.space.s12))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(r(theme.space.s12))
                .child(
                    div()
                        .type_style(&theme.typography.desktop.ui_strong)
                        .text_color(hsla(palette(cx).text.primary))
                        .child("Relay"),
                )
                .children(line),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(label("Address", cx))
                .child(
                    div()
                        .flex()
                        .gap(r(theme.space.s8))
                        .child(div().flex_1().min_w_0().child(TextField::new(
                            "settings.devices.relay.address",
                            "Address",
                            &draft,
                        )))
                        .child(save),
                ),
        )
        .children(failed.map(|said| problem("settings.devices.relay.problem", said, cx)))
        .children(public)
        .into_any_element()
}

// The dialogs.

fn closer(form: gpui::AnyWindowHandle, cx: &mut App) {
    let _ = form.update(cx, |_, window, cx| window.close_dialog(cx));
}

/// Rename device (`RenameBody`): starts with the name; Save is off until it changes.
struct Rename {
    device: Device,
    name: Entity<InputState>,
    busy: bool,
    problem: Option<SharedString>,
}

fn open_rename(device: Device, devices: Entity<Devices>, window: &mut Window, cx: &mut App) {
    let start = device.name.as_str().to_owned();
    let name = cx.new(|cx| InputState::new(window, cx).default_value(start));
    let form = cx.new(|_| Rename {
        device,
        name: name.clone(),
        busy: false,
        problem: None,
    });
    let handle = window.window_handle();
    open_dialog(window, cx, move |dialog, window, cx| {
        let state = form.read(cx);
        let typed = state.name.read(cx).value().trim().to_owned();
        let changed = !typed.is_empty() && typed != state.device.name.as_str();
        let ready = changed && !state.busy;
        let failed = state.problem.clone();
        let theme = theme(cx);
        let save = {
            let (form, devices) = (form.clone(), devices.clone());
            Pill::new("device_form.save", "Save")
                .variant(PillVariant::Primary)
                .disabled(!ready)
                .on_press(move |_, cx| rename(&form, &devices, handle, cx))
        };
        dialog
            .w(window.rem_size() * 20.0)
            .title(title("Rename device", cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .mt(r(theme.space.s16))
                    .flex()
                    .flex_col()
                    .child(label("Name", cx))
                    .child(TextField::new("device_form.name", "Name", &state.name))
                    .children(failed.map(|said| problem("device_form.problem", said, cx)))
                    .child(
                        div()
                            .mt(r(theme.space.s20))
                            .flex()
                            .justify_end()
                            .gap(r(theme.space.s8))
                            .child(Pill::new("device_form.cancel", "Cancel").on_press(|window, cx| window.close_dialog(cx)))
                            .child(save),
                    ),
            )
    });
    window.defer(cx, move |window, cx| {
        guarded("focus the device's name", || name.update(cx, |name, cx| name.focus(window, cx)));
    });
}

fn rename(form: &Entity<Rename>, devices: &Entity<Devices>, window: gpui::AnyWindowHandle, cx: &mut App) {
    let (id, typed) = {
        let state = form.read(cx);
        (state.device.id.clone(), state.name.read(cx).value().trim().to_owned())
    };
    let name = match RemoteName::new(&typed) {
        Ok(name) => name,
        Err(invalid) => {
            form.update(cx, |form, cx| {
                form.problem = Some(invalid.0.into());
                cx.notify();
            });
            return;
        }
    };
    form.update(cx, |form, cx| {
        form.busy = true;
        form.problem = None;
        cx.notify();
    });
    let call = store::request::<method::DevicesRename>(cx, DevicesRename { id, name });
    let (form, devices) = (form.downgrade(), devices.downgrade());
    cx.spawn(async move |cx| {
        guarded_async("rename a device", async move {
            let renamed = call.await;
            cx.update(|cx| match renamed {
                Ok(_) => {
                    let _ = devices.update(cx, |devices, cx| devices.reload(cx));
                    closer(window, cx);
                }
                Err(error) => {
                    let _ = form.update(cx, |form, cx| {
                        form.busy = false;
                        form.problem = Some(error_text(&error).into());
                        cx.notify();
                    });
                }
            });
        })
        .await;
    })
    .detach();
}

/// Revoke (`RevokeDevice`): asks, and says why a refusal happened without closing.
struct Revoke {
    busy: bool,
    problem: Option<SharedString>,
}

fn open_revoke(device: Device, devices: Entity<Devices>, window: &mut Window, cx: &mut App) {
    let form = cx.new(|_| Revoke {
        busy: false,
        problem: None,
    });
    let handle = window.window_handle();
    let name = device.name.as_str().to_owned();
    open_dialog(window, cx, move |dialog, window, cx| {
        let state = form.read(cx);
        let (busy, failed) = (state.busy, state.problem.clone());
        let theme = theme(cx);
        let revoke = {
            let (form, devices, id) = (form.clone(), devices.clone(), device.id.clone());
            Pill::new("dialog.confirm", if busy { "Revoking…" } else { "Revoke" })
                .variant(PillVariant::Danger)
                .disabled(busy)
                .on_press(move |_, cx| revoke(&form, &devices, id.clone(), handle, cx))
        };
        dialog
            .w(window.rem_size() * 20.0)
            .title(title(format!("Revoke {name}?"), cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s16))
                    .child(value(
                        "settings.devices.revoke_note",
                        "It is cut off at once and can't connect again unless you pair it again. Your other devices \
                         aren't affected.",
                        cx,
                    ))
                    .children(failed.map(|said| problem("settings.devices.revoke_problem", said, cx)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(r(theme.space.s8))
                            .child(Pill::new("dialog.cancel", "Cancel").on_press(|window, cx| window.close_dialog(cx)))
                            .child(revoke),
                    ),
            )
    });
}

fn revoke(form: &Entity<Revoke>, devices: &Entity<Devices>, id: String, window: gpui::AnyWindowHandle, cx: &mut App) {
    form.update(cx, |form, cx| {
        form.busy = true;
        form.problem = None;
        cx.notify();
    });
    let call = store::request::<method::DevicesRevoke>(cx, Id { id });
    let (form, devices) = (form.downgrade(), devices.downgrade());
    cx.spawn(async move |cx| {
        guarded_async("revoke a device", async move {
            let revoked = call.await;
            cx.update(|cx| match revoked {
                Ok(_) => {
                    let _ = devices.update(cx, |devices, cx| devices.reload(cx));
                    closer(window, cx);
                }
                Err(error) => {
                    let _ = form.update(cx, |form, cx| {
                        form.busy = false;
                        form.problem = Some(error_text(&error).into());
                        cx.notify();
                    });
                }
            });
        })
        .await;
    })
    .detach();
}

/// Pair a device (`PairBody`): nothing is made until Show pairing code; the code lives only in this dialog.
struct Pairing {
    made: Option<PairingStart>,
    busy: bool,
    problem: Option<SharedString>,
    copied: bool,
    /// False once the dialog is gone: a code that arrives after that is cancelled at once, never shown.
    open: bool,
    _ticks: Option<Task<()>>,
}

/// Sends `pairing.cancel` for `id` and doesn't wait for the answer, as the TypeScript's `.catch(() => {})`.
fn cancel_code(id: String, cx: &mut App) {
    let call = store::request::<method::PairingCancel>(cx, Id { id });
    cx.spawn(async move |_| {
        guarded_async("cancel a pairing code", async move {
            let _ = call.await;
        })
        .await;
    })
    .detach();
}

/// Cancels the code, if one was made, and forgets it.
fn cancel_pairing(pairing: &Entity<Pairing>, cx: &mut App) {
    let made = pairing.update(cx, |pairing, _| {
        pairing.open = false;
        pairing.made.take()
    });
    if let Some(made) = made {
        cancel_code(made.id, cx);
    }
}

fn show_code(pairing: &Entity<Pairing>, cx: &mut App) {
    pairing.update(cx, |pairing, cx| {
        pairing.busy = true;
        pairing.problem = None;
        pairing.copied = false;
        cx.notify();
    });
    let call = store::request::<method::PairingCreate>(cx, Empty {});
    let weak = pairing.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("make a pairing code", async move {
            let made = call.await;
            cx.update(|cx| {
                let open = weak.upgrade().is_some_and(|pairing| pairing.read(cx).open);
                match made {
                    // The dialog went while the code was on its way: it is cancelled, never shown.
                    Ok(made) if !open => cancel_code(made.id, cx),
                    Ok(made) => {
                        let _ = weak.update(cx, |pairing, cx| {
                            pairing.made = Some(made);
                            pairing.busy = false;
                            // The countdown, each second.
                            pairing._ticks = Some(cx.spawn(async move |this, cx| {
                                guarded_async("count the pairing code down", async move {
                                    loop {
                                        cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                                            break;
                                        }
                                    }
                                })
                                .await;
                            }));
                            cx.notify();
                        });
                    }
                    Err(error) => {
                        let _ = weak.update(cx, |pairing, cx| {
                            pairing.busy = false;
                            pairing.problem = Some(error_text(&error).into());
                            cx.notify();
                        });
                    }
                }
            });
        })
        .await;
    })
    .detach();
}

/// The QR code, black on white whatever the theme: a code must be to scan (the one place raw colours are right, as the
/// TypeScript says).
fn qr(uri: &str) -> AnyElement {
    let Ok(code) = QrCode::encode_text(uri, QrCodeEcc::Medium) else {
        return div().into_any_element();
    };
    let code = Arc::new(code);
    div()
        .semantic("settings.devices.pair.qr", Role::Image)
        .aria_label("Pairing QR code")
        .mx_auto()
        .p(px(12.))
        .rounded(px(12.))
        .bg(gpui::white())
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let modules = code.size();
                    let module = bounds.size.width / modules.max(1) as f32;
                    for y in 0..modules {
                        for x in 0..modules {
                            if code.get_module(x, y) {
                                let at = point(bounds.origin.x + module * x as f32, bounds.origin.y + module * y as f32);
                                window.paint_quad(gpui::fill(Bounds::new(at, size(module, module)), gpui::black()));
                            }
                        }
                    }
                },
            )
            .size(px(192.)),
        )
        .into_any_element()
}

fn open_pair(window: &mut Window, cx: &mut App) {
    let pairing = cx.new(|_| Pairing {
        made: None,
        busy: false,
        problem: None,
        copied: false,
        open: true,
        _ticks: None,
    });
    let closing = pairing.clone();
    open_dialog(window, cx, move |dialog, window, cx| {
        let state = pairing.read(cx);
        let now = jiff::Timestamp::now().as_millisecond();
        let left = state.made.as_ref().map(|made| countdown(&made.expires, now));
        let theme = theme(cx);
        let colors = palette(cx);
        let showing = state.made.as_ref().filter(|_| left.as_ref().is_some_and(|(_, expired)| !expired));
        let shown: AnyElement = match showing {
            Some(made) => {
                let copy = {
                    let (pairing, code) = (pairing.clone(), made.code.clone());
                    Pill::new("settings.devices.pair.copy", if state.copied { "Copied" } else { "Copy code" })
                        .small()
                        .icon("copy")
                        .on_press(move |_, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                            pairing.update(cx, |pairing, cx| {
                                pairing.copied = true;
                                cx.notify();
                            });
                        })
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s12))
                    .child(qr(&made.uri))
                    .child(
                        div().flex().justify_center().child(value(
                            "settings.devices.pair.left",
                            format!("Expires in {}", left.as_ref().map_or("", |(text, _)| text.as_str())),
                            cx,
                        )),
                    )
                    .child(value("settings.devices.pair.or", "Or paste this code", cx))
                    .child(
                        div()
                            .semantic("settings.devices.pair.code", Role::Code)
                            .aria_label(made.code.clone())
                            .id("settings.devices.pair.code.scroll")
                            .max_h(r(96.))
                            .overflow_y_scroll()
                            .p(r(theme.space.s8))
                            .rounded(r(theme.radius.field))
                            .border_1()
                            .border_color(hsla(colors.line.field))
                            .type_style(&theme.typography.desktop.meta)
                            .font_family(crate::assets::MONO_FAMILY)
                            .text_color(hsla(colors.text.primary))
                            .child(made.code.clone()),
                    )
                    .child(div().flex().justify_end().child(copy))
                    .into_any_element()
            }
            None => {
                let expired = left.as_ref().is_some_and(|(_, expired)| *expired);
                let busy = state.busy;
                let make = {
                    let pill = Pill::new(
                        "settings.devices.pair.show",
                        if expired { "Make a new code" } else { "Show pairing code" },
                    )
                    .variant(PillVariant::Primary)
                    .small()
                    .disabled(busy);
                    let pairing = pairing.clone();
                    pill.on_press(move |_, cx| show_code(&pairing, cx))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s12))
                    .children(expired.then(|| {
                        value("settings.devices.pair.expired", "That code expired. Make a new one.", cx)
                    }))
                    .child(div().flex().justify_end().child(make))
                    .into_any_element()
            }
        };
        let close = {
            let pairing = pairing.clone();
            Pill::new(
                "settings.devices.pair.close",
                if state.made.is_some() { "Cancel pairing" } else { "Close" },
            )
            .on_press(move |window, cx| {
                cancel_pairing(&pairing, cx);
                window.close_dialog(cx);
            })
        };
        let failed = state.problem.clone();
        let closing = closing.clone();
        // pair-dialog.tsx's 420 px, as rems.
        dialog
            .w(window.rem_size() * 26.25)
            .title(title("Pair a device", cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            // Closing it by any route cancels the code.
            .on_close(move |_, _, cx| {
                guarded("cancel the pairing code", || cancel_pairing(&closing, cx));
            })
            .child(
                div()
                    .mt(r(theme.space.s12))
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s16))
                    .child(value(
                        "settings.devices.pair.how",
                        "Open Tjommie on the phone or other PC, and scan this code or paste it there. It works once \
                         and lasts ten minutes. Pairing works on the same Wi-Fi as this PC, or over mobile data when \
                         the relay is on.",
                        cx,
                    ))
                    .child(shown)
                    .children(failed.map(|said| problem("settings.devices.pair.problem", said, cx)))
                    .child(div().flex().justify_end().child(close)),
            )
    });
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use protocol::remote::{Device, PairingStart, RelayState, RelayStatus, RemoteStatus};
    use protocol::wire::method;
    use serde_json::{Value, json};

    use super::{countdown, relay_address, relay_line};
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, click, draw, node, state, text};

    fn device(id: &str, name: &str, seen: Option<&str>) -> Device {
        serde_json::from_value(json!({
            "id": id, "name": name, "publicKey": "A".repeat(43), "pairedAt": "2026-11-01T09:00:00.000Z",
            "lastSeenAt": seen, "revokedAt": null
        }))
        .unwrap()
    }

    fn status(connected: &[&str], listening: bool) -> RemoteStatus {
        serde_json::from_value(json!({
            "hostId": "host-1", "hostName": "Workshop PC", "listening": listening, "port": 47201,
            "connectedDeviceIds": connected, "relay": { "url": null, "state": "off", "push": false }
        }))
        .unwrap()
    }

    fn rpc(devices: Vec<Device>, remote: RemoteStatus) -> FakeRpc {
        FakeRpc::new()
            .on::<method::DevicesList>(move |_| Ok(devices.clone()))
            .on::<method::RemoteStatus>(move |_| Ok(remote.clone()))
            .on::<method::PairingCreate>(|_| {
                Ok(PairingStart {
                    id: "pair-1".into(),
                    code: "SAMPLE-PAIRING-CODE".into(),
                    uri: "tjommie://pair?code=SAMPLE".into(),
                    expires: "2099-01-01T00:00:00.000Z".into(),
                })
            })
            .on::<method::PairingCancel>(|_| Ok(protocol::wire::Empty {}))
            .on::<method::DevicesRename>(|params| {
                let mut renamed = serde_json::from_value::<Value>(params).unwrap();
                renamed["publicKey"] = json!("A".repeat(43));
                renamed["pairedAt"] = json!("2026-11-01T09:00:00.000Z");
                renamed["lastSeenAt"] = Value::Null;
                renamed["revokedAt"] = Value::Null;
                Ok(serde_json::from_value(renamed).unwrap())
            })
    }

    fn calls(calls: &[(&'static str, Value)], method: &str) -> Vec<Value> {
        calls.iter().filter(|(name, _)| *name == method).map(|(_, params)| params.clone()).collect()
    }

    /// pairing.test.ts: "counts down in minutes and seconds, and says Expired at zero".
    #[test]
    fn counts_down_in_minutes_and_seconds_and_says_expired_at_zero() {
        let expires = "2026-11-02T10:10:00.000Z";
        let at = |text: &str| text.parse::<jiff::Timestamp>().unwrap().as_millisecond();
        assert_eq!(countdown(expires, at("2026-11-02T10:00:00.000Z")), ("10:00".to_owned(), false));
        assert_eq!(countdown(expires, at("2026-11-02T10:08:19.200Z")), ("1:41".to_owned(), false));
        assert_eq!(countdown(expires, at("2026-11-02T10:10:00.000Z")), ("Expired".to_owned(), true));
    }

    /// relay-row.test.tsx: "says how the relay is doing", "says whether push works through it once it's connected",
    /// "refuses anything but an https address", "turns the relay off when the address is cleared".
    #[test]
    fn the_relay_line_and_the_address_check() {
        let relay = |state, push| RelayStatus { url: None, state, push };
        assert_eq!(relay_line(&relay(RelayState::Online, true)), "Connected · Push: available");
        assert_eq!(relay_line(&relay(RelayState::Online, false)), "Connected · Push: not set up on this relay");
        assert_eq!(relay_line(&relay(RelayState::Connecting, true)), "Connecting");
        assert_eq!(relay_line(&relay(RelayState::Error, false)), "Can't reach the relay");
        assert_eq!(relay_line(&relay(RelayState::Off, false)), "Off");
        assert_eq!(relay_address("  ").unwrap(), None);
        assert_eq!(relay_address(" https://relay.example.com ").unwrap().unwrap().as_str(), "https://relay.example.com");
        assert!(relay_address("http://relay.example.com").is_err());
        assert!(relay_address("not a url").is_err());
    }

    /// devices.test.tsx: "says what pairing means, honestly", "has the Relay row and keeps the Windows Firewall note",
    /// "has Pair a device as an explicit button, and makes no code on its own".
    #[gpui::test]
    fn says_what_pairing_means_and_makes_no_code_on_its_own(cx: &mut TestAppContext) {
        let (_, calls_made) = app(cx, Route::Settings(Section::Devices), state(vec![], vec![], vec![]), rpc(vec![], status(&[], true)));
        assert!(node("settings.devices.pair").is_some());
        assert!(text("settings.devices.firewall").contains("Windows Firewall asks once"));
        assert!(node("settings.devices.relay.address").is_some());
        assert_eq!(text("settings.devices.none"), "No devices paired yet.");
        assert!(calls(&calls_made.calls(), "pairing.create").is_empty());
    }

    /// devices.test.tsx: "shows the name, when it was paired and last seen, and Rename and Revoke by name", and "says
    /// Connected now for a device on the line, and Not seen yet for one that never connected".
    #[gpui::test]
    fn shows_each_device_with_when_it_was_seen(cx: &mut TestAppContext) {
        let devices = vec![device("d1", "Sample Phone", None), device("d2", "Spare Laptop", Some("2026-11-01T10:00:00.000Z"))];
        let (_, _) = app(cx, Route::Settings(Section::Devices), state(vec![], vec![], vec![]), rpc(devices, status(&["d1"], false)));
        assert!(node("settings.devices.d1.rename").is_some() && node("settings.devices.d2.revoke").is_some());
        assert!(text("settings.devices.d1.line").ends_with("Connected now"));
        assert!(text("settings.devices.d2.line").contains("Last seen"));
        assert_eq!(text("settings.devices.d1"), "Sample Phone");
        assert_eq!(text("settings.devices.not_listening"), "This PC isn't accepting devices right now.");
    }

    /// devices.test.tsx: "makes nothing until Show pairing code is pressed: no QR, no code"; closing cancels the code.
    #[gpui::test]
    fn makes_a_code_only_on_show_and_cancels_it_on_close(cx: &mut TestAppContext) {
        let (cx, calls_made) = app(cx, Route::Settings(Section::Devices), state(vec![], vec![], vec![]), rpc(vec![], status(&[], true)));
        click(cx, "settings.devices.pair");
        assert!(node("settings.devices.pair.qr").is_none());
        assert!(node("settings.devices.pair.code").is_none());
        assert!(calls(&calls_made.calls(), "pairing.create").is_empty());
        click(cx, "settings.devices.pair.show");
        draw(cx);
        assert!(node("settings.devices.pair.qr").is_some());
        assert_eq!(text("settings.devices.pair.code"), "SAMPLE-PAIRING-CODE");
        assert!(text("settings.devices.pair.left").starts_with("Expires in"));
        click(cx, "settings.devices.pair.close");
        assert_eq!(calls(&calls_made.calls(), "pairing.cancel"), [json!({ "id": "pair-1" })]);
    }

    /// devices.test.tsx: "starts with the device's name and keeps Save off until it changes".
    #[gpui::test]
    fn rename_starts_with_the_name_and_saves_a_change(cx: &mut TestAppContext) {
        let (cx, calls_made) = app(cx, Route::Settings(Section::Devices), state(vec![], vec![], vec![]), rpc(vec![device("d1", "Sample Phone", None)], status(&[], true)));
        click(cx, "settings.devices.d1.rename");
        assert_eq!(text("dialog.title"), "Rename device");
        click(cx, "device_form.save");
        assert!(calls(&calls_made.calls(), "devices.rename").is_empty(), "off until it changes");
        click(cx, "device_form.name");
        cx.simulate_input(" 2");
        draw(cx);
        click(cx, "device_form.save");
        draw(cx);
        assert_eq!(calls(&calls_made.calls(), "devices.rename"), [json!({ "id": "d1", "name": "Sample Phone 2" })]);
        assert!(node("device_form.save").is_none(), "saved: the dialog closed");
    }

    /// relay-row.test.tsx: "keeps Save off until the address is changed", "saves a typed https address, trimmed", and
    /// "saves the public relay's address with Use the public relay".
    #[gpui::test]
    fn the_relay_saves_a_changed_https_address(cx: &mut TestAppContext) {
        let rpc = rpc(vec![], status(&[], true)).on::<method::SettingsSet>(|_| Ok(serde_json::from_value(json!({ "chainLimit": 5, "relayUrl": null })).unwrap()));
        let (cx, calls_made) = app(cx, Route::Settings(Section::Devices), state(vec![], vec![], vec![]), rpc);
        click(cx, "settings.devices.relay.save");
        assert!(calls(&calls_made.calls(), "settings.set").is_empty(), "off until changed");
        click(cx, "settings.devices.relay.address");
        cx.simulate_input("https://relay.example.com ");
        draw(cx);
        click(cx, "settings.devices.relay.save");
        draw(cx);
        assert_eq!(calls(&calls_made.calls(), "settings.set"), [json!({ "relayUrl": "https://relay.example.com" })]);
        if node("settings.devices.relay.public").is_some() {
            click(cx, "settings.devices.relay.public");
            draw(cx);
            assert_eq!(calls(&calls_made.calls(), "settings.set").len(), 2);
        }
    }

    /// The list reads again when the host says its devices changed.
    #[gpui::test]
    fn reads_again_when_the_devices_change(cx: &mut TestAppContext) {
        let (cx, calls_made) = app(cx, Route::Settings(Section::Devices), state(vec![], vec![], vec![]), rpc(vec![], status(&[], true)));
        assert_eq!(calls(&calls_made.calls(), "devices.list").len(), 1);
        cx.update(|_, cx| crate::ui::store::fake_event(cx, &protocol::Event::DevicesChanged(protocol::wire::Empty {})));
        draw(cx);
        draw(cx);
        assert_eq!(calls(&calls_made.calls(), "devices.list").len(), 2);
    }
}
