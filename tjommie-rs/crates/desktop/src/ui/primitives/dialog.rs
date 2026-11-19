//! The Aegis dialog (overlays.html `.dialog`) on gpui-kit's dialog layer: raised, card corners, the glass shadow, a
//! light top edge and 24 px of padding, over the scrim. It raises the browser shield until it closes, however it
//! closes. It has no entrance, so nothing animates out and Escape closes it at once (keyboard actions never animate);
//! the motion layer (T7.1) adds the Aegis entrance.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ClickEvent, Div, InteractiveElement as _, IntoElement, ParentElement, Pixels,
    Role, ScrollHandle, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, div,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dialog::{Dialog, DialogEntrance};

use super::button::{Pill, PillVariant};
use super::scroll::follow_focus;
use super::shield::BrowserShield;
use crate::semantic::{Semantic, SemanticExt as _};
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// How many dialogs this app has opened (`open_dialog` counts them). A form reads it when Save is pressed, while its
/// dialog is the one in front; a save that answers late closes the dialog only if none has opened since (M5 review
/// Minor 4; the account dialogs' epoch, for every form).
#[derive(Default)]
struct Opened(u64);

impl gpui::Global for Opened {}

/// The number of the dialog opened last.
pub fn opened(cx: &App) -> u64 {
    cx.try_global::<Opened>().map_or(0, |opened| opened.0)
}

/// A dialog as its builder fills it (`open_dialog`): gpui-kit's dialog, with its body kept apart. gpui-kit scrolls a
/// body taller than the window, but in a scroll area of its own that Tab can't move; this body is one of ours
/// (`scroll::follow_focus`), so Tab brings each of its controls into view on a small window (M5 re-review). The title
/// stays above it.
pub struct Surface {
    dialog: Dialog,
    body: Vec<AnyElement>,
}

impl Surface {
    pub fn w(mut self, width: impl Into<Pixels>) -> Self {
        self.dialog = self.dialog.w(width);
        self
    }

    pub fn title(mut self, title: impl IntoElement) -> Self {
        self.dialog = self.dialog.title(title);
        self
    }

    pub fn on_ok(
        mut self,
        on_ok: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        // unguarded: forwards the builder's own handler, which the rule checks where the builder writes it
        self.dialog = self.dialog.on_ok(on_ok);
        self
    }

    pub fn on_close(
        mut self,
        on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        // unguarded: forwards the builder's own handler, which the rule checks where the builder writes it
        self.dialog = self.dialog.on_close(on_close);
        self
    }

    pub fn on_cancel(
        mut self,
        on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        // unguarded: forwards the builder's own handler, which the rule checks where the builder writes it
        self.dialog = self.dialog.on_cancel(on_cancel);
        self
    }

    /// gpui-kit's dialog, its body in one scroll area that follows the focus. The body goes in gpui-kit's content slot,
    /// which sits in the dialog's height-capped column (not in its own scroll area), so a body taller than the window
    /// is bounded here and scrolls here.
    fn into_dialog(self, scroll: &ScrollHandle) -> Dialog {
        if self.body.is_empty() {
            return self.dialog;
        }
        // gpui-kit may call the content builder again; each frame builds a new dialog, so the body is taken once.
        let body = std::cell::RefCell::new(Some(self.body));
        let scroll = scroll.clone();
        self.dialog.content(move |content, _, _| {
            let children = body.borrow_mut().take().unwrap_or_default();
            content.min_h_0().child(follow_focus(
                &scroll,
                div()
                    .id("dialog.body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .flex()
                    .flex_col()
                    .children(children),
            ))
        })
    }
}

impl ParentElement for Surface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

impl FluentBuilder for Surface {}

/// Opens a dialog. `build` fills the styled surface; gpui-kit may run it again while the dialog is open, so it creates
/// no entities.
pub fn open_dialog(
    window: &mut Window,
    cx: &mut App,
    build: impl Fn(Surface, &mut Window, &mut App) -> Surface + 'static,
) {
    cx.default_global::<Opened>().0 += 1;
    let shield = Rc::new(BrowserShield::raise(cx));
    // The body's scroll position while the dialog is open: gpui-kit runs the builder again on each frame.
    let scroll = ScrollHandle::new();
    window.open_dialog(cx, move |dialog, window, cx| {
        let _held_while_open = &shield;
        let theme = theme(cx);
        let colors = palette(cx);
        let surface = dialog
            .entrance(DialogEntrance::None)
            .keyboard(true)
            .bg(hsla(colors.surface.raised))
            .rounded(r(theme.radius.card))
            .border_t_1()
            .border_color(hsla(colors.overlay.highlight))
            .shadow(shadows(&theme.shadow.light.glass))
            .p(r(theme.space.s24));
        build(
            Surface {
                dialog: surface,
                body: Vec::new(),
            },
            window,
            cx,
        )
        .into_dialog(&scroll)
    });
}

/// A dialog's title in the display face: its heading (M2 review F8).
pub fn title(text: impl Into<SharedString>, cx: &App) -> Semantic<Stateful<Div>> {
    let text = text.into();
    div()
        .semantic("dialog.title", Role::Heading)
        .aria_label(text.clone())
        .type_style(&theme(cx).typography.desktop.display_s)
        .text_color(hsla(palette(cx).text.primary))
        .child(text)
}

/// A yes-or-no question (the Full auto warning, Remove agent), as `ConfirmDialog` in `dialog.tsx`: the body reads
/// as the description, and Cancel and the confirm button sit right-aligned on one line.
pub fn open_confirm(
    window: &mut Window,
    cx: &mut App,
    heading: impl Into<SharedString>,
    body: impl Into<SharedString>,
    confirm: impl Into<SharedString>,
    danger: bool,
    on_confirm: impl Fn(&mut Window, &mut App) + 'static,
) {
    let (heading, body, confirm) = (heading.into(), body.into(), confirm.into());
    let on_confirm = Rc::new(on_confirm);
    open_dialog(window, cx, move |dialog, window, cx| {
        let on_confirm = on_confirm.clone();
        let colors = palette(cx);
        let variant = if danger {
            PillVariant::Danger
        } else {
            PillVariant::Primary
        };
        // dialog.tsx's default width, 320 px, as rems so it follows the zoom.
        dialog
            .w(window.rem_size() * 20.0)
            .title(title(heading.clone(), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(r(theme(cx).space.s20))
                    .child(
                        div()
                            .type_style(&theme(cx).typography.desktop.body)
                            .text_color(hsla(colors.text.secondary))
                            .child(body.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(r(theme(cx).space.s8))
                            .child(
                                Pill::new("dialog.cancel", "Cancel")
                                    .variant(PillVariant::Grey)
                                    .on_press(|window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Pill::new("dialog.confirm", confirm.clone())
                                    .variant(variant)
                                    .on_press(move |window, cx| {
                                        on_confirm(window, cx);
                                        window.close_dialog(cx);
                                    }),
                            ),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::primitives::test_window;
    use std::cell::Cell;

    /// Opens the Remove agent confirm; the count is how many times it removed.
    fn remove_confirm(cx: &mut gpui::VisualTestContext) -> Rc<Cell<u32>> {
        let removed = Rc::new(Cell::new(0));
        let on_remove = removed.clone();
        cx.update(|window, cx| {
            open_confirm(
                window,
                cx,
                "Remove Forge?",
                "Its chats stay on this PC.",
                "Remove",
                true,
                move |_, _| on_remove.set(on_remove.get() + 1),
            )
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        removed
    }

    #[gpui::test]
    fn enter_presses_the_focused_pill(cx: &mut gpui::TestAppContext) {
        let cx = test_window(cx, |_, _| div().into_any_element());
        let removed = remove_confirm(cx);
        assert!(
            cx.debug_bounds("dialog.title").is_some(),
            "the title is the dialog's heading"
        );
        // Tab to Cancel, then to Remove (the dialog keeps the focus). Shift+Enter is not a press (review F5); Enter is.
        cx.simulate_keystrokes("tab tab shift-enter");
        assert_eq!(removed.get(), 0, "Shift+Enter doesn't press");
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("enter");
        assert_eq!(removed.get(), 1, "Enter on Remove removes");
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn enter_on_cancel_removes_nothing(cx: &mut gpui::TestAppContext) {
        let cx = test_window(cx, |_, _| div().into_any_element());
        let removed = remove_confirm(cx);
        cx.simulate_keystrokes("tab enter");
        assert_eq!(removed.get(), 0, "Enter on Cancel cancels");
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn escape_closes_without_frames(cx: &mut gpui::TestAppContext) {
        let cx = test_window(cx, |_, _| div().into_any_element());
        // With motion on, a dialog that animates out would keep drawing (and holding the shield) after Escape.
        cx.update(|_, cx| cx.set_reduce_motion(false));
        cx.update(|window, cx| {
            open_dialog(window, cx, |dialog, _, _| dialog.title("Remove Forge?"))
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(cx.update(|_, cx| BrowserShield::is_raised(cx)));

        let before = cx.update(|_, cx| crate::motion::clock::frames(cx).len());
        cx.simulate_keystrokes("escape");
        // Gone in the same update, with no exit transition left to draw, and the shield is down with it.
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(!cx.update(|_, cx| BrowserShield::is_raised(cx)));
        // And the motion clock drew no moving frame after Escape (T2.3): nothing animated out.
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let frames = cx.update(|_, cx| crate::motion::clock::frames(cx));
        assert!(frames.len() > before, "the close was drawn");
        assert!(
            frames[before..].iter().all(|frame| !frame.moving),
            "{frames:?}"
        );
    }
}
