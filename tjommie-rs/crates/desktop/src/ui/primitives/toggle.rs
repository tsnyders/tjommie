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
        // The knob: 14 px at (4, 3.5) when off, 16 px at (19, 2.5) when on.
        let (knob, left, top, color) = if on {
            (16., 19., 2.5, colors.on.primary)
        } else {
            (14., 4., 3.5, colors.text.secondary)
        };
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
                    .absolute()
                    .left(r(left - 1.5))
                    .top(r(top - 1.5))
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
