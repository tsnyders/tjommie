//! Files in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx (FilesSection)`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame("files", "Files", "Previews and saves use this device. Files are read from the selected host through your existing connection.", cx).into_any_element()
}
