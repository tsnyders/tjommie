//! The browser shield. The browser panel's page is a native child window that GPUI can't draw over, so every overlay
//! raises the shield while it is open and the panel shows a snapshot of the page instead (T3.5 and T3.7). This is
//! the port of Electron's `dialog:scrim` (`main.ts:582-588`). Raises nest: the shield is up while any guard lives.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, Global};

#[derive(Default)]
pub struct BrowserShield(Rc<Cell<usize>>);

impl Global for BrowserShield {}

/// Holds the shield up until dropped, however the overlay that holds it closes.
pub struct ShieldGuard(Rc<Cell<usize>>);

impl Drop for ShieldGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

impl BrowserShield {
    pub fn raise(cx: &mut App) -> ShieldGuard {
        let raised = cx.default_global::<BrowserShield>().0.clone();
        raised.set(raised.get() + 1);
        ShieldGuard(raised)
    }

    /// Whether any overlay is open over the page. The panel reads it as it renders.
    pub fn is_raised(cx: &App) -> bool {
        Self::raised(cx) > 0
    }

    /// How many overlays are open. The media player, itself inside a dialog, compares it with the count it opened under.
    pub fn raised(cx: &App) -> usize {
        cx.try_global::<BrowserShield>()
            .map_or(0, |shield| shield.0.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn raises_nest_until_every_guard_drops(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(!BrowserShield::is_raised(cx));
            let first = BrowserShield::raise(cx);
            let second = BrowserShield::raise(cx);
            drop(first);
            assert!(BrowserShield::is_raised(cx));
            drop(second);
            assert!(!BrowserShield::is_raised(cx));
        });
    }
}
