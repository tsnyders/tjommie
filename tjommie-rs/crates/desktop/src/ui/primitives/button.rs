//! Buttons (overlays.html `.btn` and `.icon-btn`; `pill.tsx`, `icon-button.tsx`). Pressing compresses the fill to the
//! press-fill size and leaves the label where it is. Enter and Space press a focused button. The press handler is
//! guarded here, so callers pass a plain closure.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Div, Entity, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    RenderOnce, Rgba, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Transformation, Window, div, relative, svg,
};

use super::focus_handle;
use crate::guard::guarded;
use crate::keymap::{self, Press};
use crate::semantic::{Semantic, SemanticDisabled as _, SemanticExt as _};
use crate::ui::style::{TypeExt as _, hsla, palette, r, ring_or_reveal, shadows, theme};

/// A press or select handler; the primitives run it inside the guard.
pub type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

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
    name: Option<SharedString>,
    icon: Option<&'static str>,
    variant: PillVariant,
    small: bool,
    tight: bool,
    disabled: bool,
    expanded: Option<bool>,
    on_press: Option<Handler>,
}

impl Pill {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Pill {
            id: id.into(),
            label: label.into(),
            name: None,
            icon: None,
            variant: PillVariant::Grey,
            small: false,
            tight: false,
            disabled: false,
            expanded: None,
            on_press: None,
        }
    }

    /// A menu's trigger: whether its menu is open, as `aria-expanded` said (M4 review C-N3).
    pub fn expanded(mut self, open: bool) -> Self {
        self.expanded = Some(open);
        self
    }

    /// Unavailable now (M5 review M2): dimmed, not pressable and not a Tab stop, and said so (", unavailable" in its
    /// name until gpui's accessibility tree has a disabled state). A focused one keeps the focus, so Tab goes on from
    /// where it was.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn variant(mut self, variant: PillVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Its accessible name, when that says more than the label (M5 review Minor 25): a card's Rename reads as
    /// "Rename Sample Phone".
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
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
        let focus = focus_handle(&self.id, window, cx).tab_stop(!self.disabled);
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
        let hover = if self.disabled { fill } else { hover };
        let named = self.name.clone().unwrap_or_else(|| self.label.clone());
        let name: SharedString = if self.disabled {
            format!("{named}, unavailable").into()
        } else {
            named
        };
        let (scale, down) = press_motion(&self.id, window, cx);
        let theme = crate::ui::style::theme(cx);
        let pill = div()
            .semantic(self.id.clone(), Role::Button)
            .aria_label(name)
            .when_some(self.expanded, |pill, open| pill.aria_expanded(open))
            .semantic_disabled(self.disabled)
            .track_focus(&focus)
            .h(r(height))
            .px(r(pad))
            .gap(r(theme.space.s8))
            .type_style(&theme.typography.desktop.button)
            .text_color(hsla(text))
            .child(
                press_fill(self.id.clone(), fill, hover, scale)
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
            .children(ring_or_reveal(&focus, theme.radius.pill, window, cx));
        if self.disabled {
            return base(pill, self.id).opacity(0.38).cursor_default();
        }
        pressable(press_listeners(base(pill, self.id), down), self.on_press)
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
    disabled: bool,
    busy: bool,
    expanded: Option<bool>,
    on_press: Option<Handler>,
    icon_scale: f32,
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
            disabled: false,
            busy: false,
            expanded: None,
            on_press: None,
            icon_scale: 1.,
        }
    }

    /// The icon scaled about its centre: an icon button may scale whole, as the send button's pop does (motion.html
    /// "Press, hover and lift"); GPUI scales the icon, the fill keeps its size.
    pub fn icon_scale(mut self, scale: f32) -> Self {
        self.icon_scale = scale;
        self
    }

    pub fn variant(mut self, variant: IconVariant) -> Self {
        self.variant = variant;
        self
    }

    /// A menu's trigger: whether its menu is open, as `aria-expanded` said (M4 review C-N3).
    pub fn expanded(mut self, open: bool) -> Self {
        self.expanded = Some(open);
        self
    }

    /// At work (a refresh, a check): the icon is the turning loader (it stands still under reduced motion), the name
    /// says so, and a press does nothing. It keeps the focus, so the key press that started the work keeps its place.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Unavailable now, as [`Pill::disabled`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_press(mut self, on_press: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx).tab_stop(!self.disabled);
        let icon_size = theme(cx).size.icon;
        let spinner = self
            .busy
            .then(|| crate::ui::shell::connection::spinner(icon_size, window, cx));
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
        let hover = if self.disabled || self.busy {
            fill
        } else {
            hover
        };
        let name: SharedString = if self.disabled {
            format!("{}, unavailable", self.label).into()
        } else if self.busy {
            format!("{}, busy", self.label).into()
        } else {
            self.label
        };
        let glyph = spinner.unwrap_or_else(|| {
            svg()
                .path(format!("icons/i/175/{}.svg", self.icon))
                .size(r(icon_size))
                .text_color(hsla(icon))
                .with_transformation(Transformation::scale(gpui::size(
                    self.icon_scale,
                    self.icon_scale,
                )))
                .into_any_element()
        });
        let (scale, down) = press_motion(&self.id, window, cx);
        let theme = crate::ui::style::theme(cx);
        let button = div()
            .semantic(self.id.clone(), Role::Button)
            .aria_label(name)
            .when_some(self.expanded, |button, open| button.aria_expanded(open))
            .semantic_disabled(self.disabled)
            .track_focus(&focus)
            .size(r(size))
            .flex_none()
            .child(press_fill(self.id.clone(), fill, hover, scale))
            .child(glyph)
            .children(ring_or_reveal(&focus, theme.radius.pill, window, cx));
        if self.disabled {
            return base(button, self.id).opacity(0.38).cursor_default();
        }
        if self.busy {
            return base(button, self.id).cursor_default();
        }
        pressable(press_listeners(base(button, self.id), down), self.on_press)
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

/// The fill behind the label: it takes the hover colour, and is drawn at `scale` of the button (the press).
fn press_fill(group: SharedString, fill: Rgba, hover: Rgba, scale: f32) -> Stateful<Div> {
    let inset = relative(((1. - scale) / 2.).max(0.));
    div()
        .id("press-fill")
        .debug_selector(|| format!("{group}.fill"))
        .absolute()
        .top(inset)
        .bottom(inset)
        .left(inset)
        .right(inset)
        .rounded_full()
        .bg(hsla(fill))
        .group_hover(group, |style| style.bg(hsla(hover)))
}

/// The press (motion.html "Press, hover and lift"): the pointer pressing springs the fill in to the press-fill size,
/// letting go springs it back with the snappy settle, and the words never move. A key's press (Enter, Space) shows none;
/// reduced motion lands it at once. The fill's scale now, and the pressed state the button's listeners set.
fn press_motion(id: &SharedString, window: &mut Window, cx: &mut App) -> (f32, Entity<bool>) {
    let down = window.use_keyed_state(SharedString::from(format!("{id}.down")), cx, |_, _| false);
    let motion = &theme(cx).motion;
    let (snappy, pressed) = (motion.spring.snappy.clone(), motion.distance.press_fill);
    let target = if *down.read(cx) { pressed } else { 1. };
    let scale = crate::motion::keyed::spring(
        &format!("{id}.press"),
        target,
        &snappy,
        // A 40 px button: a unit of scale is 40 px of fill.
        40.,
        crate::motion::reduced::is_on(cx),
        window,
        cx,
    );
    (scale, down)
}

/// The pointer down on the button presses its fill; up, here or anywhere, lets go.
fn press_listeners(
    element: Semantic<Stateful<Div>>,
    down: Entity<bool>,
) -> Semantic<Stateful<Div>> {
    let set = |down: &Entity<bool>, pressed: bool, window: &mut Window, cx: &mut App| {
        down.update(cx, |down, _| *down = pressed);
        window.refresh();
    };
    let (press, release, outside) = (down.clone(), down.clone(), down);
    element
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            guarded("press a button's fill", || set(&press, true, window, cx));
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            guarded("let go of a button", || set(&release, false, window, cx));
        })
        .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
            guarded("let go of a button outside it", || {
                set(&outside, false, window, cx)
            });
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{IntoElement as _, ParentElement as _, div};

    use super::Pill;
    use crate::ui::testing::{click, draw, node};

    /// motion.html "Press, hover and lift": the pointer pressing springs the fill in to 94%, letting go springs it back;
    /// the words stay put. Enter presses with no motion.
    #[gpui::test]
    fn a_press_springs_the_fill_in_and_back(cx: &mut gpui::TestAppContext) {
        use crate::ui::testing::next_frame;
        use gpui::{Modifiers, MouseButton};
        let cx = crate::ui::primitives::keyboard_window(cx, |_, _| {
            div()
                .child(Pill::new("test.press", "Approve").on_press(|_, _| {}))
                .into_any_element()
        });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        let button = cx.debug_bounds("test.press").expect("the pill");
        let full = cx
            .debug_bounds("test.press.fill")
            .expect("its fill")
            .size
            .width;
        cx.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::none());
        draw(cx);
        let mut frames = 0;
        while next_frame(cx) {
            frames += 1;
            assert!(frames < 60, "it lands");
        }
        assert!(frames > 2, "pressed over several frames: {frames}");
        let pressed = cx
            .debug_bounds("test.press.fill")
            .expect("its fill")
            .size
            .width;
        assert!(
            (pressed / full - 0.94).abs() < 0.01,
            "pressed to 94%: {pressed:?} of {full:?}"
        );
        cx.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::none());
        draw(cx);
        assert!(next_frame(cx), "letting go springs it back");
        while next_frame(cx) {}
        let back = cx
            .debug_bounds("test.press.fill")
            .expect("its fill")
            .size
            .width;
        assert_eq!(back, full);
        cx.simulate_keystrokes("tab");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(!next_frame(cx), "a key's press shows no motion");
    }

    /// M5 review M2: an unavailable pill is no Tab stop, says so, and does nothing when pressed.
    #[gpui::test]
    fn an_unavailable_pill_is_skipped_by_tab_and_says_so(cx: &mut gpui::TestAppContext) {
        let pressed = Rc::new(Cell::new(0));
        let counted = pressed.clone();
        let cx = crate::ui::primitives::keyboard_window(cx, move |_, _| {
            let (off, on) = (counted.clone(), counted.clone());
            div()
                .child(
                    Pill::new("test.off", "Save")
                        .disabled(true)
                        .on_press(move |_, _| off.set(off.get() + 1)),
                )
                .child(Pill::new("test.on", "Cancel").on_press(move |_, _| on.set(on.get() + 10)))
                .into_any_element()
        });
        draw(cx);
        let off = node("test.off").unwrap();
        assert!(!off.enabled, "the snapshot reads it unavailable");
        assert_eq!(off.text, "Save, unavailable");
        assert!(node("test.on").unwrap().enabled);
        for _ in 0..2 {
            cx.simulate_keystrokes("tab");
            draw(cx);
            assert!(
                node("test.on").unwrap().focused,
                "Tab goes to the available pill only"
            );
            assert!(!node("test.off").unwrap().focused);
        }
        click(cx, "test.off");
        assert_eq!(pressed.get(), 0, "pressing it does nothing");
    }

    /// Screens' request: a busy icon button says so, keeps its Tab stop and ignores presses.
    #[gpui::test]
    fn a_busy_icon_button_says_so_and_ignores_presses(cx: &mut gpui::TestAppContext) {
        let pressed = Rc::new(Cell::new(0));
        let counted = pressed.clone();
        let cx = crate::ui::primitives::keyboard_window(cx, move |_, _| {
            let counted = counted.clone();
            div()
                .child(
                    super::IconButton::new("test.refresh", "rotate-cw", "Refresh")
                        .busy(true)
                        .on_press(move |_, _| counted.set(counted.get() + 1)),
                )
                .into_any_element()
        });
        draw(cx);
        assert_eq!(node("test.refresh").unwrap().text, "Refresh, busy");
        cx.simulate_keystrokes("tab");
        draw(cx);
        assert!(node("test.refresh").unwrap().focused, "still a Tab stop");
        cx.simulate_keystrokes("enter");
        click(cx, "test.refresh");
        assert_eq!(pressed.get(), 0, "neither Enter nor a click presses it");
    }
}
