//! Notifications in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx (NotificationsSection)`). The desktop
//! branch: Windows controls notification permission and sound; Tjommie's own window shows the notices.

use gpui::{AnyElement, App, IntoElement, ParentElement as _, Window};

use crate::ui::primitives::settings_row::SettingsRow;

pub(super) fn section(_window: &mut Window, cx: &mut App) -> AnyElement {
    super::frame(
        "notifications",
        "Notifications",
        "Notification choices belong to this device. Approval requests remain available inside Tjommie.",
        cx,
    )
    .child(super::card(cx).child(SettingsRow::new("Desktop notifications").description(
        "Windows controls desktop notification permission and sound. Tjommie shows notifications when its window is not focused.",
    )))
    .into_any_element()
}
