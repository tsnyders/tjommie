//! Dictation's link to the speech runtime (T4.3 over desktop-host's T6.5). A composer begins a session and finishes or
//! cancels it here; the runtime's events (`SpeechEvent`) come back through the window's `Voice`, which the dictating
//! composers subscribe to. `speech::start` installs desktop-host's `speech::capture::Dictation` as the backend, on the
//! live runtime; without it (no speech folder this run, or a view test that installs none) dictation says it isn't
//! available.

use std::rc::Rc;

use gpui::{App, AppContext as _, Entity, EventEmitter, Global};
use protocol::speech::SpeechEvent;

/// What dictation asks of the speech runtime.
pub trait VoiceBackend {
    fn begin(&self, session_id: &str, cx: &mut App);
    fn finish(&self, session_id: &str, cx: &mut App);
    fn cancel(&self, session_id: &str, cx: &mut App);
    /// The window lost focus: a recording that has heard something is thrown away, as 0.5.x did (`main.ts:640`).
    fn blur(&self, _: &mut App) {}
}

/// The speech runtime as the window sees it: the backend, and the events it reports.
#[derive(Default)]
pub struct Voice {
    backend: Option<Rc<dyn VoiceBackend>>,
}

impl EventEmitter<SpeechEvent> for Voice {}

struct VoiceGlobal(Entity<Voice>);

impl Global for VoiceGlobal {}

/// The window's `Voice`, made on first use.
pub fn voice(cx: &mut App) -> Entity<Voice> {
    if let Some(voice) = cx.try_global::<VoiceGlobal>() {
        return voice.0.clone();
    }
    let voice = cx.new(|_| Voice::default());
    cx.set_global(VoiceGlobal(voice.clone()));
    voice
}

/// Installs the speech runtime (the app's wiring) or a test's fake.
pub fn install(backend: Rc<dyn VoiceBackend>, cx: &mut App) {
    voice(cx).update(cx, |voice, _| voice.backend = Some(backend));
}

/// The installed backend, if any.
pub fn backend(cx: &mut App) -> Option<Rc<dyn VoiceBackend>> {
    voice(cx).read(cx).backend.clone()
}

/// The window lost focus (the shell's activation observer): the backend decides what that ends (M4 re-review R2).
pub fn blur(cx: &mut App) {
    if let Some(backend) = backend(cx) {
        backend.blur(cx);
    }
}

/// Tells the dictating composers what the runtime did (the backend's `emit`).
pub fn emit(event: SpeechEvent, cx: &mut App) {
    voice(cx).update(cx, |_, cx| cx.emit(event));
}
