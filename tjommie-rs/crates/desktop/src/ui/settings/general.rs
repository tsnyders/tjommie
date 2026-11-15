//! General in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx (GeneralSection)`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame("general", "General", "These app controls belong to the device you are using, even when you connect to another PC.", cx).into_any_element()
}
