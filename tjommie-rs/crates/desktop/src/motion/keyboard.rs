//! Keyboard actions never animate (Phase 3 plan T2.3; T2.2's Escape rule): a `Trigger` travels with every state
//! change, and a keyboard change completes at once, as it does under reduced motion.

use gpui::{App, Window};

use super::reduced;
use super::spring::Spring;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Pointer,
    Keyboard,
}

impl Trigger {
    /// What caused the change being handled: a key press, or the pointer.
    pub fn of(window: &Window) -> Self {
        if window.last_input_was_keyboard() {
            Trigger::Keyboard
        } else {
            Trigger::Pointer
        }
    }

    /// Whether the change completes at once: after a key, or under reduced motion.
    pub fn snaps(self, cx: &App) -> bool {
        self == Trigger::Keyboard || reduced::is_on(cx)
    }
}

/// Sends `spring` to `target`: on the spring after the pointer, at once after a key or under reduced motion.
pub fn animate(spring: &mut Spring, target: f32, trigger: Trigger, cx: &App) {
    if trigger.snaps(cx) {
        spring.snap(target);
    } else {
        spring.set_target(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[gpui::test]
    fn snaps(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let calm = Theme::aegis().unwrap().motion.spring.calm;
            let mut by_key = Spring::new(0., &calm);
            animate(&mut by_key, 100., Trigger::Keyboard, cx);
            assert!(
                by_key.at_rest() && by_key.value == 100.,
                "a key: done at once"
            );
            let mut by_pointer = Spring::new(0., &calm);
            animate(&mut by_pointer, 100., Trigger::Pointer, cx);
            assert!(!by_pointer.at_rest(), "the pointer: on the spring");
        });
    }
}
