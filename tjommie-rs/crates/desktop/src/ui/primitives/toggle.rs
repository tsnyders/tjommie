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
        // CSS and Taffy measure an absolute child's insets (M2 review F9).
        let (knob, left, top, color) = if on {
            (16., 19., 2.5, colors.on.primary)
        } else {
            (14., 4., 3.5, colors.text.secondary)
        };
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
