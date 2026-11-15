//! Voice in Settings (Phase 3 plan T5.5; `settings/voice-section.tsx`).

use gpui::{AnyElement, App, IntoElement, Window};

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    super::frame("voice", "Voice", "Dictation runs locally on the device capturing your voice, even when the host PC is offline. Transcripts stay editable until you choose Send.", cx).into_any_element()
}
