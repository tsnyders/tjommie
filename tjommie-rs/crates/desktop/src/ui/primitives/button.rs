//! Buttons (overlays.html `.btn` and `.icon-btn`; `pill.tsx`, `icon-button.tsx`). Pressing compresses the fill to the
//! press-fill size and leaves the label where it is. Enter and Space press a focused button. The press handler is
//! guarded here, so callers pass a plain closure.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Div, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Rgba, Role,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, relative,
    svg,
};

use super::focus_handle;
use crate::guard::guarded;
use crate::keymap::{self, Press};
use crate::semantic::{Semantic, SemanticExt as _};
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, shadows, theme};

pub(crate) type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PillVariant {
    /// The near-black action.
    Primary,
    /// Stone: the quiet choice.
    Grey,
    /// White with the card shadow.
    Raised,
    /// A destructive confirm, quiet but in the danger colour (`danger.text`).
    Danger,
}

/// A text button: 40 px, or 32 px small.
#[derive(IntoElement)]
pub struct Pill {
    id: SharedString,
    label: SharedString,
    icon: Option<&'static str>,
    variant: PillVariant,
    small: bool,
    tight: bool,
    on_press: Option<Handler>,
}

impl Pill {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Pill {
            id: id.into(),
            label: label.into(),
            icon: None,
            variant: PillVariant::Grey,
            small: false,
            tight: false,
            on_press: None,
        }
    }

    pub fn variant(mut self, variant: PillVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// The full height with 16 px sides, as the sidebar's New agent pill has.
    pub fn tight(mut self) -> Self {
        self.tight = true;
        self
    }

    /// A Lucide icon before the label.
    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
        self
    }

    pub fn on_press(mut self, on_press: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }
}

impl RenderOnce for Pill {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx);
        let ring = focus_visible(&focus, window);
        let theme = theme(cx);
        let colors = palette(cx);
        let (fill, hover, text): (Rgba, Rgba, Rgba) = match self.variant {
            PillVariant::Primary => (colors.action.primary, colors.ink.hover, colors.on.primary),
            PillVariant::Grey => (
                colors.surface.sunk,
                colors.state.sunk_hover,
                colors.text.primary,
            ),
            PillVariant::Raised => (
                colors.surface.raised,
                colors.surface.raised,
                colors.text.primary,
            ),
            PillVariant::Danger => (
                colors.surface.sunk,
                colors.state.sunk_hover,
                colors.danger.text,
            ),
        };
        let (height, pad) = if self.small {
            (32.0, theme.space.s16)
        } else if self.tight {
            (theme.size.control_desktop, theme.space.s16)
        } else {
            (theme.size.control_desktop, theme.space.s20)
        };
        let pill = div()
            .semantic(self.id.clone(), Role::Button)
            .aria_label(self.label.clone())
            .track_focus(&focus)
            .h(r(height))
            .px(r(pad))
            .gap(r(theme.space.s8))
            .type_style(&theme.typography.desktop.button)
            .text_color(hsla(text))
            .child(
                press_fill(self.id.clone(), fill, hover)
                    .when(self.variant == PillVariant::Raised, |fill| {
                        fill.shadow(shadows(&theme.shadow.light.card))
                    }),
            )
            .when_some(self.icon, |pill, icon| {
                pill.child(
                    svg()
                        .path(format!("icons/i/175/{icon}.svg"))
                        .size(r(18.))
                        .flex_none()
                        .text_color(hsla(text)),
                )
            })
            .child(self.label)
            .when(ring, |pill| pill.child(focus_ring(theme.radius.pill, cx)));
        pressable(base(pill, self.id), self.on_press)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconVariant {
    /// No fill until hover.
    Plain,
    /// On stone.
    Grey,
    /// Near-black.
    Dark,
    /// On a dark surface, such as a toast.
    OnDark,
}

/// A round 36 px icon button. `label` is its accessible name.
#[derive(IntoElement)]
pub struct IconButton {
    id: SharedString,
    icon: &'static str,
    label: SharedString,
    variant: IconVariant,
    on_press: Option<Handler>,
}

impl IconButton {
    pub fn new(
        id: impl Into<SharedString>,
        icon: &'static str,
        label: impl Into<SharedString>,
    ) -> Self {
        IconButton {
            id: id.into(),
            icon,
            label: label.into(),
            variant: IconVariant::Plain,
            on_press: None,
        }
    }

    pub fn variant(mut self, variant: IconVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn on_press(mut self, on_press: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx);
        let ring = focus_visible(&focus, window);
        let theme = theme(cx);
        let colors = palette(cx);
        let clear = colors.surface.raised_clear;
        let (fill, hover, icon): (Rgba, Rgba, Rgba) = match self.variant {
            IconVariant::Plain => (clear, colors.state.hover, colors.icon.base),
            IconVariant::Grey => (
                colors.surface.sunk,
                colors.state.sunk_hover,
                colors.text.primary,
            ),
            IconVariant::Dark => (colors.action.primary, colors.ink.hover, colors.on.primary),
            IconVariant::OnDark => (clear, colors.ink.hover, colors.on.primary),
        };
        let size = theme.size.icon_button;
        let button = div()
            .semantic(self.id.clone(), Role::Button)
            .aria_label(self.label)
            .track_focus(&focus)
            .size(r(size))
            .flex_none()
            .child(press_fill(self.id.clone(), fill, hover))
            .child(
                svg()
                    .path(format!("icons/i/175/{}.svg", self.icon))
                    .size(r(theme.size.icon))
                    .text_color(hsla(icon)),
            )
            .when(ring, |button| {
                button.child(focus_ring(theme.radius.pill, cx))
            });
        pressable(base(button, self.id), self.on_press)
    }
}

/// The round, centred frame both buttons share, its press group named after the button.
fn base(element: Semantic<Stateful<Div>>, id: SharedString) -> Semantic<Stateful<Div>> {
    element
        .group(id)
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
}

/// The fill behind the label: it takes the hover colour, and on press it shrinks to the press-fill size (0.94).
fn press_fill(group: SharedString, fill: Rgba, hover: Rgba) -> Stateful<Div> {
    let inset = relative(0.03);
    div()
        .id("press-fill")
        .absolute()
        .inset_0()
        .rounded_full()
        .bg(hsla(fill))
        .group_hover(group.clone(), |style| style.bg(hsla(hover)))
        .group_active(group, |style| {
            style.top(inset).bottom(inset).left(inset).right(inset)
        })
}

/// A click presses the button, and so do Enter and Space while it has the focus: they are the keymap's `Press`
/// binding in the `Button` context, which beats an enclosing dialog's own Enter (review F1) and ignores Shift+Enter
/// and the like (F5). Each runs through the guard.
pub(crate) fn pressable(
    element: Semantic<Stateful<Div>>,
    on_press: Option<Handler>,
) -> Semantic<Stateful<Div>> {
    let Some(on_press) = on_press else {
        return element;
    };
    let on_key = on_press.clone();
    element
        .key_context(keymap::BUTTON)
        .on_click(move |_, window, cx| {
            guarded("press a button", || on_press(window, cx));
        })
        .on_action(move |_: &Press, window, cx| {
            guarded("press a button by key", || on_key(window, cx));
        })
}
