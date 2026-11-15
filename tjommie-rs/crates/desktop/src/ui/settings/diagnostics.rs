//! Diagnostics in Settings (Phase 3 plan T5.5; `settings/diagnostics-section.tsx (DiagnosticsSection)`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame(
        "diagnostics",
        "Diagnostics",
        "Read-only checks. No model turn, upload or chat attachment is created.",
        cx,
    )
    .into_any_element()
}
