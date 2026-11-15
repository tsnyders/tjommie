//! Notifications in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx (NotificationsSection)`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame("notifications", "Notifications", "Notification choices belong to this device. Approval requests remain available inside Tjommie.", cx).into_any_element()
}
