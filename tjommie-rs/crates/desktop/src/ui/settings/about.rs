//! About in Settings (Phase 3 plan T5.5; `settings/diagnostics-section.tsx (AboutSection)`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame(
        "about",
        "About",
        "Tjommie connects this device to agents running on the selected host PC.",
        cx,
    )
    .into_any_element()
}
