//! Devices in Settings (Phase 3 plan T5.5; `devices/devices-section.tsx`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame("devices", "Devices", "", cx).into_any_element()
}
