//! Logins in Settings (Phase 3 plan T5.5; `settings/logins-section.tsx`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame("logins", "Logins", "Saved login metadata belongs to the selected host. Enter usernames and passwords only in its dedicated native Logins window; this screen never receives them.", cx).into_any_element()
}
