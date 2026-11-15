//! Usage in Settings (Phase 3 plan T5.5; `usage/usage-section.tsx`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame("usage", "Usage", "", cx).into_any_element()
}
