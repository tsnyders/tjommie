//! The keymap (Phase 3 plan T2.4; gate condition 2): the one list of the app's key bindings, bound at start. The
//! app-wide ones (`app_wide()`) work wherever the focus is in the app's window. A context binding works inside an
//! element with that `key_context` (`COMPOSER`, `MENTIONS`, `APPROVAL`, `BUTTON`). Any binding added later goes in
//! `bindings()`.
//!
//! Escape and the overlays, for the Browser UI lane's T3.7 (M2 review F4): GPUI doesn't rank a deeper context's binding
//! above a context-free one, so a context-free Escape bound after gpui-kit's would take the key from an open sheet or
//! dialog, and gpui-kit's sheet passes Escape on (`cx.propagate()`) after closing. So `Dismiss` is bound only
//! `OUTSIDE_OVERLAYS`: while a dialog, a sheet or an open menu has the focus, Escape closes that overlay and nothing
//! else. T3.7 handles `Dismiss` (closing the browser panel) wherever it likes, the shell's root or `App::on_action`;
//! it needs no overlay checks of its own. `keymap::tests::escape_in_an_overlay_closes_the_overlay_alone` holds this.
//!
//! The shortcut forwarding source, for the Browser lane's T3.5 (gate condition 2). While a bot's page in WebView2 has
//! the keyboard, its keys go to the page, except the app-wide chords with Ctrl or Alt, and Escape:
//! - T3.5's `AcceleratorKeyPressed` handler turns the event into a `gpui::Keystroke`: the virtual key as GPUI names
//!   it on the current keyboard layout ("b", "=", "escape"), and the modifiers from the event.
//! - When `is_forwarded(&keystroke)` is true, it marks the event handled and dispatches the keystroke to the window
//!   (`Window::dispatch_keystroke`), so the keymap resolves it as if the app had the keyboard. Every other key goes
//!   to the page.
//! - `page_forwarded()` lists the set. Both come from `app_wide()`, so an app-wide chord can't be left out.
//! - Tab and Shift+Tab are not shortcuts: WebView2's `MoveFocusRequested` hands the focus back (T3.5).

use gpui::{KeyBinding, Keystroke, actions};

actions!(
    desktop,
    [
        /// One zoom step in (T0.5).
        ZoomIn,
        /// One zoom step out.
        ZoomOut,
        /// Back to 100%.
        ZoomReset,
        /// Opens or closes the browser panel (T3.7).
        TogglePanel,
        /// Closes the browser panel when no overlay is open; an overlay takes Escape first (T3.7).
        Dismiss,
        /// Approves in the focused approval card (T4.2).
        Approve,
        /// Sends the composer's message (T4.3).
        Send,
        /// Starts a new line in the composer (T4.3).
        NewLine,
        /// The mention list's row above (T4.3, T5.2).
        MentionUp,
        /// The row below.
        MentionDown,
        /// Picks the highlighted row.
        MentionPick,
        /// Closes the list, keeping what was typed.
        MentionClose,
        /// Presses the focused button (Enter or Space): a binding, so the button's context beats a dialog's own
        /// Enter (review F1).
        Press,
    ]
);

/// A composer's text field: Enter sends, Shift+Enter starts a line.
pub const COMPOSER: &str = "Composer";
/// The room composer's mention list while it is open (`room-composer.tsx:119-135`).
pub const MENTIONS: &str = "Mentions";
/// A focused approval card.
pub const APPROVAL: &str = "Approval";
/// A focused button: a pill, an icon button, a toggle, a sidebar row (`ui::primitives::button::pressable`).
pub const BUTTON: &str = "Button";
/// Not inside an open overlay: gpui-kit's dialog, sheet and menu (popover content) contexts.
pub const OUTSIDE_OVERLAYS: &str = "!Sheet && !Dialog && !PopoverContent";

/// Every binding. App-wide first.
pub fn bindings() -> Vec<KeyBinding> {
    let mut bindings = app_wide();
    bindings.extend([
        KeyBinding::new("ctrl-enter", Approve, Some(APPROVAL)),
        KeyBinding::new("enter", Send, Some(COMPOSER)),
        KeyBinding::new("shift-enter", NewLine, Some(COMPOSER)),
        KeyBinding::new("up", MentionUp, Some(MENTIONS)),
        KeyBinding::new("down", MentionDown, Some(MENTIONS)),
        KeyBinding::new("tab", MentionPick, Some(MENTIONS)),
        KeyBinding::new("enter", MentionPick, Some(MENTIONS)),
        KeyBinding::new("escape", MentionClose, Some(MENTIONS)),
        KeyBinding::new("enter", Press, Some(BUTTON)),
        KeyBinding::new("space", Press, Some(BUTTON)),
    ]);
    bindings
}

/// The bindings that work wherever the focus is in the app's window (`Dismiss` stands aside inside an open overlay).
pub fn app_wide() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ZoomReset, None),
        KeyBinding::new("ctrl-b", TogglePanel, None),
        KeyBinding::new("escape", Dismiss, Some(OUTSIDE_OVERLAYS)),
    ]
}

/// The app-wide bindings the page gives back: those with Ctrl or Alt, and Escape.
fn forwarded() -> impl Iterator<Item = KeyBinding> {
    app_wide().into_iter().filter(|binding| {
        binding.keystrokes().iter().all(|keystroke| {
            let keystroke = keystroke.inner();
            keystroke.modifiers.control || keystroke.modifiers.alt || keystroke.key == "escape"
        })
    })
}

/// The chords a bot's page gives back to the app (gate condition 2).
pub fn page_forwarded() -> Vec<Keystroke> {
    forwarded()
        .flat_map(|binding| {
            binding
                .keystrokes()
                .iter()
                .map(|keystroke| keystroke.inner().clone())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether the page gives `typed` back to the app, matched as the keymap matches it.
pub fn is_forwarded(typed: &Keystroke) -> bool {
    forwarded().any(|binding| binding.match_keystrokes(std::slice::from_ref(typed)) == Some(false))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _, div};

    use super::*;
    use crate::guard::guarded;
    use crate::ui::primitives::test_window;

    fn typed(chord: &'static str) -> Keystroke {
        Keystroke::parse(chord).unwrap()
    }

    #[test]
    fn forwarded_set_equals_app_chords() {
        let app_chords = ["ctrl-=", "ctrl-+", "ctrl--", "ctrl-0", "ctrl-b", "escape"];
        for chord in app_chords {
            assert!(
                is_forwarded(&typed(chord)),
                "{chord} reaches the app from the page"
            );
        }
        assert_eq!(
            page_forwarded().len(),
            app_chords.len(),
            "a new app-wide chord with Ctrl or Alt is forwarded too: add it here"
        );
        for binding in app_wide() {
            for keystroke in binding.keystrokes() {
                let modifiers = &keystroke.inner().modifiers;
                if modifiers.control || modifiers.alt {
                    assert!(is_forwarded(keystroke.inner()), "{:?}", keystroke.inner());
                }
            }
        }
        // The page keeps its own keys and the context ones: typing, copying, a composer's Enter, the mention list.
        for chord in [
            "a",
            "ctrl-c",
            "enter",
            "shift-enter",
            "up",
            "down",
            "tab",
            "ctrl-enter",
        ] {
            assert!(!is_forwarded(&typed(chord)), "{chord} stays with the page");
        }
    }

    #[gpui::test]
    fn a_context_binding_wins_inside_its_element(cx: &mut gpui::TestAppContext) {
        let (closed, dismissed) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
        let (on_close, on_dismiss) = (closed.clone(), dismissed.clone());
        // The test window binds the app's keymap.
        let focus = cx.update(|cx| cx.focus_handle());
        let list = focus.clone();
        let cx = test_window(cx, move |_, _| {
            let (on_close, on_dismiss) = (on_close.clone(), on_dismiss.clone());
            div()
                .size_full()
                .on_action(move |_: &Dismiss, _, _| {
                    guarded("test dismiss", || on_dismiss.set(true));
                })
                .child(
                    div()
                        .key_context(MENTIONS)
                        .track_focus(&list)
                        .size_full()
                        .on_action(move |_: &MentionClose, _, _| {
                            guarded("test close the list", || on_close.set(true));
                        }),
                )
                .into_any_element()
        });
        cx.update(|window, cx| focus.focus(window, cx));
        cx.simulate_keystrokes("escape");
        assert!(closed.get(), "the mention list closed");
        assert!(!dismissed.get(), "and the panel stayed");
    }

    #[gpui::test]
    fn escape_in_an_overlay_closes_the_overlay_alone(cx: &mut gpui::TestAppContext) {
        use crate::ui::primitives::sheet::open_sheet;
        use gpui_kit::component::WindowExt as _;

        let dismissed = Rc::new(Cell::new(false));
        let on_dismiss = dismissed.clone();
        // The shell's root holds the focus when nothing else has it (GPUI limit 17); so does this one.
        let root = cx.update(|cx| cx.focus_handle());
        let held = root.clone();
        let cx = test_window(cx, move |_, _| {
            div().track_focus(&held).size_full().into_any_element()
        });
        // Wherever T3.7 handles Dismiss, an app-level handler hears it from any focus: the sheet sits in gpui-kit's
        // root layer, beside the shell, not inside it.
        cx.update(|_, cx| {
            cx.on_action(move |_: &Dismiss, _: &mut gpui::App| {
                guarded("test dismiss", || on_dismiss.set(true));
            });
        });
        cx.update(|window, cx| open_sheet(window, cx, |sheet, _, _| sheet.child("Logins")));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.update(|window, cx| window.has_active_sheet(cx)));

        cx.simulate_keystrokes("escape");
        assert!(
            !cx.update(|window, cx| window.has_active_sheet(cx)),
            "the sheet closed"
        );
        assert!(!dismissed.get(), "and the browser panel stays (review F4)");

        // A dialog the same way: its own Escape closes it, and only it.
        cx.update(|window, cx| {
            crate::ui::primitives::dialog::open_dialog(window, cx, |dialog, _, _| {
                dialog.title("Remove Forge?")
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("escape");
        assert!(
            !cx.update(|window, cx| window.has_active_dialog(cx)),
            "the dialog closed"
        );
        assert!(!dismissed.get(), "and the browser panel stays");

        // With no overlay open, Escape is Dismiss's.
        cx.update(|window, cx| {
            root.focus(window, cx);
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("escape");
        assert!(dismissed.get(), "Escape closes the browser panel");
    }
}
