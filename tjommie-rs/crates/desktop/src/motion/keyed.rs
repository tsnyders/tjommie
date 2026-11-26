//! A spring kept per element (by its id) across frames, for the motions of elements that keep no state of their own: a
//! toggle's knob, the send button's pop. It rests at the first value it is given, so an element appears in place; after
//! that each new target springs from wherever the value is, keeping its velocity, or lands at once with `snap` (a key's
//! change, `Trigger::snaps`, or reduced motion for a change that isn't a command, such as typing).

use gpui::{App, SharedString, Window};

use super::clock;
use super::spring::Spring;
use crate::theme;

/// This frame's value of the spring under `key`, heading for `target` on `tokens` (at once with `snap`); `scale` is the
/// pixels one unit spans (`Spring::scaled`). Asks for the next frame while it moves.
pub fn spring(
    key: &str,
    target: f32,
    tokens: &theme::Spring,
    scale: f32,
    snap: bool,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let state = window.use_keyed_state(SharedString::from(format!("{key}.spring")), cx, |_, _| {
        Spring::scaled(target, tokens, scale)
    });
    let dt = clock::dt(cx);
    let (value, moving) = state.update(cx, |spring, _| {
        if spring.target != target {
            if snap {
                spring.snap(target);
            } else {
                spring.set_target(target);
            }
        }
        let moving = spring.step(dt);
        (spring.value, moving)
    });
    if moving {
        clock::keep_moving(window, cx);
    }
    value
}
