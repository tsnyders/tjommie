//! The Aegis switch (forms-settings `.toggle`; the `Switch` in `controls.tsx`): a 40 × 24 track whose knob grows and
//! moves right when on. Click, Enter or Space flip it; `on_toggle` gets the new value and is guarded here.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Toggled, Window, div, px,
};

use super::button::{Handler, pressable};

type ToggleHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
use super::focus_handle;
use crate::semantic::SemanticExt as _;
use crate::ui::style::{focus_ring, focus_visible, hsla, palette, r, theme};

#[derive(IntoElement)]
pub struct Toggle {
    id: SharedString,
    label: SharedString,
    on: bool,
    on_toggle: Option<ToggleHandler>,
}

impl Toggle {
    /// `label` is its accessible name.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, on: bool) -> Self {
        Toggle {
            id: id.into(),
            label: label.into(),
            on,
            on_toggle: None,
        }
    }

    pub fn on_toggle(mut self, on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx);
        let ring = focus_visible(&focus, window);
        let theme = theme(cx);
        let colors = palette(cx);
        let on = self.on;
        let (track, edge) = if on {
            (colors.action.primary, colors.action.primary)
        } else {
            (colors.surface.raised, colors.line.field)
        };
        // The knob: 14 px at (4, 3.5) when off, 16 px at (19, 2.5) when on, from inside the 1.5 px border, where both
        // CSS and Taffy measure an absolute child's insets (M2 review F9). It travels between them on the snappy spring
        // (motion.html "Press, hover and lift": toggles land with the snappy settle); a key or reduced motion lands it at once.
        let snappy = theme.motion.spring.snappy.clone();
        let snap = crate::motion::keyboard::Trigger::of(window).snaps(cx);
        let travel = crate::motion::keyed::spring(
            &format!("{}.knob", self.id),
            if on { 1. } else { 0. },
            &snappy,
            15.,
            snap,
            window,
            cx,
        );
        let theme = crate::ui::style::theme(cx);
        let colors = palette(cx);
        let color = if on {
            colors.on.primary
        } else {
            colors.text.secondary
        };
        let (knob, left, top) = (14. + 2. * travel, 4. + 15. * travel, 3.5 - travel);
        let knob_id = self.id.clone();
        let toggle = div()
            .semantic(self.id, Role::Switch)
            .aria_label(self.label)
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .track_focus(&focus)
            .relative()
            .flex_none()
            .w(r(40.))
            .h(r(24.))
            .rounded_full()
            .border(px(1.5))
            .border_color(hsla(edge))
            .bg(hsla(track))
            .cursor_pointer()
            .child(
                div()
                    .debug_selector(|| format!("{knob_id}.knob"))
                    .absolute()
                    .left(r(left))
                    .top(r(top))
                    .size(r(knob))
                    .rounded_full()
                    .bg(hsla(color)),
            )
            .when(ring, |toggle| {
                toggle.child(focus_ring(theme.radius.pill, cx))
            });
        let flip = self
            .on_toggle
            .map(|on_toggle| -> Handler { Rc::new(move |window, cx| on_toggle(!on, window, cx)) });
        pressable(toggle, flip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::primitives::test_window;
    use gpui::point;

    /// The knob against overlays.html's `.toggle::after`: its insets are from the padding box, inside the 1.5 px
    /// border, so from the track's outer edge it sits at (5.5, 5) off and (20.5, 4) on, centred (M2 review F9).
    #[gpui::test]
    fn the_knob_springs_across_after_a_click_and_lands_at_once_after_a_key(cx: &mut gpui::TestAppContext) {
        use crate::ui::testing::{click, draw, next_frame};
        use std::cell::Cell;
        let on = Rc::new(Cell::new(false));
        let shown = on.clone();
        let cx = crate::ui::primitives::keyboard_window(cx, move |_, _| {
            let flip = shown.clone();
            Toggle::new("test.toggle", "Done", shown.get())
                .on_toggle(move |value, _, _| flip.set(value))
                .into_any_element()
        });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        let track = cx.debug_bounds("test.toggle").expect("the track");
        click(cx, "test.toggle");
        assert!(on.get());
        let first = cx.debug_bounds("test.toggle.knob").expect("the knob").origin.x - track.origin.x;
        assert!(first < px(20.5), "it starts from where it was: {first:?}");
        let mut frames = 0;
        while next_frame(cx) {
            frames += 1;
            assert!(frames < 60, "it lands");
        }
        assert!(frames > 3, "over several frames: {frames}");
        let landed = cx.debug_bounds("test.toggle.knob").expect("the knob").origin.x - track.origin.x;
        assert_eq!(landed, px(20.5), "on: where the design puts it");
        // By key: Tab to it, Space flips it, and the knob is there at once.
        cx.simulate_keystrokes("tab");
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(!on.get());
        assert!(!next_frame(cx), "no frames in between after a key");
        let off = cx.debug_bounds("test.toggle.knob").expect("the knob").origin.x - track.origin.x;
        assert_eq!(off, px(5.5));
    }

    #[gpui::test]
    fn the_knob_sits_where_the_design_puts_it(cx: &mut gpui::TestAppContext) {
        let cx = test_window(cx, |_, _| {
            div()
                .flex()
                .gap(px(8.))
                .child(Toggle::new("test.off", "Off", false))
                .child(Toggle::new("test.on", "On", true))
                .into_any_element()
        });
        for (id, knob, (x, y), side) in [
            ("test.off", "test.off.knob", (5.5, 5.), 14.),
            ("test.on", "test.on.knob", (20.5, 4.), 16.),
        ] {
            let track = cx.debug_bounds(id).expect("the track");
            let knob = cx.debug_bounds(knob).expect("the knob");
            assert_eq!(knob.origin - track.origin, point(px(x), px(y)), "{id}");
            assert_eq!(knob.size.width, px(side), "{id}");
        }
    }
}
