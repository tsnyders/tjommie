//! Appearance in Settings (Phase 3 plan T5.5; `settings/appearance-section.tsx`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame(
        "appearance",
        "Appearance",
        "Saved on this device. Switching PCs keeps your appearance choices.",
        cx,
    )
    .into_any_element()
}
