//! General in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx (GeneralSection)`). The desktop branch: the
//! window closes to the tray, and the start-at-sign-in and updater controls (`DesktopApp`) aren't reachable from the UI
//! yet, so they say they're unavailable, as the TypeScript does for a desktop without them.

use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, Role, StatefulInteractiveElement as _,
    Window, div,
};

use crate::semantic::SemanticExt as _;
use crate::ui::primitives::settings_row::SettingsRow;

pub(super) fn section(_window: &mut Window, cx: &mut App) -> AnyElement {
    super::frame(
        "general",
        "General",
        "These app controls belong to the device you are using, even when you connect to another PC.",
        cx,
    )
    .child(
        super::card(cx)
            .child(
                SettingsRow::new("Closing the desktop window").description(
                    "Tjommie keeps running in this device's tray. Choose Quit from the tray to stop it.",
                ),
            )
            .child(
                div()
                    .semantic("settings.general.updates", Role::Group)
                    .aria_label("Unavailable in this app version.")
                    .child(
                        SettingsRow::new("Startup and update controls")
                            .description("Unavailable in this app version.")
                            .ruled(),
                    ),
            )
            .child(
                SettingsRow::new("Connected host settings")
                    .description(
                        "Agents, Accounts, Usage and Devices refer to the selected host PC. Switching PCs changes those sections.",
                    )
                    .ruled(),
            ),
    )
    .into_any_element()
}
