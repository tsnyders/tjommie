//! The screens' form controls (forms-settings.html; `controls.tsx`'s FIELD, `Segmented` and radio groups): a text field
//! and a text area over gpui-kit's input, a radio group (a segmented control, the colour and shape swatches), an
//! underlined text action, a problem line, and arrow keys for a list of tab stops. Every control is reachable from the
//! keyboard and shows the Aegis focus ring (T5.6).
//!
//! A radio group is one tab stop, as Base UI's is: Tab reaches the chosen option, the arrow keys move the choice (and
//! the focus with it), and a click picks one. A list (`arrows`) keeps each item a tab stop and lets the arrow keys
//! step through them too, staying inside the list.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, FocusHandle, Focusable as _, InteractiveElement, IntoElement,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Toggled, Window, div, px, svg,
};
use gpui_kit::component::input::Input;
use gpui_kit::component::input::{InputState, TextareaState};

use crate::guard::guarded;
use crate::keymap::{self, Next, Prev};
use crate::semantic::{SemanticDisabled as _, SemanticExt as _, SemanticFocus as _};
use crate::ui::primitives::button::pressable;
use crate::ui::primitives::focus_handle;
use crate::ui::style::{
    TypeExt as _, focus_ring, focus_visible, hsla, palette, r, ring_or_reveal, theme,
};

type OnChange<V> = Rc<dyn Fn(V, &mut Window, &mut App)>;

/// A field's label (`.field-label`).
pub fn label(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .mb(r(theme.space.s6))
        .type_style(&theme.typography.desktop.ui_strong)
        .text_color(hsla(palette(cx).text.primary))
        .child(text.into())
        .into_any_element()
}

/// Why a field or an action failed (`.err`, `role="alert"`): an alert, so Windows reads it out.
pub fn problem(id: impl Into<SharedString>, text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let text = text.into();
    div()
        .semantic(id, Role::Alert)
        .aria_label(text.clone())
        .flex()
        .items_center()
        .gap(r(theme.space.s6))
        .mt(r(theme.space.s6))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(colors.text.primary))
        .child(
            svg()
                .path("icons/i/175/circle-alert.svg")
                .size(r(15.))
                .flex_none()
                .text_color(hsla(colors.text.primary)),
        )
        .child(text)
        .into_any_element()
}

/// A one-line text field (`.textfield`): 40 px, the field edge, white; `bad` gives it the 2 px ink edge of a field
/// with a problem. The text lives in `state`, which the screen owns.
#[derive(IntoElement)]
pub struct TextField {
    id: SharedString,
    label: SharedString,
    state: Entity<InputState>,
    bad: bool,
}

impl TextField {
    /// `label` is its accessible name.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        state: &Entity<InputState>,
    ) -> Self {
        TextField {
            id: id.into(),
            label: label.into(),
            state: state.clone(),
            bad: false,
        }
    }

    pub fn bad(mut self, bad: bool) -> Self {
        self.bad = bad;
        self
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = self.state.focus_handle(cx);
        field_frame(
            self.id,
            self.label.clone(),
            &focus,
            self.bad,
            40.,
            window,
            cx,
        )
        .child(
            Input::new(&self.state)
                .appearance(false)
                .aria_label(self.label),
        )
    }
}

/// A text area (`.textfield`, several lines) for what an agent or a routine should do.
#[derive(IntoElement)]
pub struct TextArea {
    id: SharedString,
    label: SharedString,
    state: Entity<TextareaState>,
}

impl TextArea {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        state: &Entity<TextareaState>,
    ) -> Self {
        TextArea {
            id: id.into(),
            label: label.into(),
            state: state.clone(),
        }
    }
}

impl RenderOnce for TextArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = self.state.focus_handle(cx);
        field_frame(self.id, self.label.clone(), &focus, false, 72., window, cx)
            .py(r(theme(cx).space.s10))
            .child(
                gpui_kit::component::input::Textarea::new(&self.state)
                    .appearance(false)
                    .aria_label(self.label),
            )
    }
}

fn field_frame(
    id: SharedString,
    label: SharedString,
    focus: &FocusHandle,
    bad: bool,
    min_height: f32,
    window: &Window,
    cx: &App,
) -> crate::semantic::Semantic<gpui::Stateful<gpui::Div>> {
    let theme = theme(cx);
    let colors = palette(cx);
    let ring = focus_visible(focus, window);
    div()
        .semantic(id, Role::TextInput)
        .aria_label(label)
        .semantic_focus(focus)
        .relative()
        .flex()
        .items_center()
        .min_h(r(min_height))
        .px(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .border(px(if bad { 2. } else { 1. }))
        .border_color(hsla(if bad {
            colors.text.primary
        } else {
            colors.line.field
        }))
        .bg(hsla(colors.surface.raised))
        .type_style(&theme.typography.desktop.ui)
        .text_color(hsla(colors.text.primary))
        .when(ring, |frame| {
            frame.child(focus_ring(theme.radius.field, cx))
        })
}

/// How a radio group draws: a segmented control (`.seg`), or round swatches and shape tiles beside each other.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Segmented,
    Swatches,
}

type Draw = Box<dyn Fn(bool, &App) -> AnyElement>;

struct Choice<V> {
    value: V,
    key: SharedString,
    label: SharedString,
    draw: Option<Draw>,
}

/// One choice of several (a radio group): the segmented control, or the colour and shape swatches.
#[derive(IntoElement)]
pub struct RadioGroup<V: Copy + PartialEq + 'static> {
    id: SharedString,
    label: SharedString,
    value: V,
    choices: Vec<Choice<V>>,
    look: Look,
    on_change: Option<OnChange<V>>,
}

impl<V: Copy + PartialEq + 'static> RadioGroup<V> {
    /// `label` is the group's accessible name; `value` is the choice now.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, value: V) -> Self {
        RadioGroup {
            id: id.into(),
            label: label.into(),
            value,
            choices: Vec::new(),
            look: Look::Segmented,
            on_change: None,
        }
    }

    pub fn look(mut self, look: Look) -> Self {
        self.look = look;
        self
    }

    /// A choice with its text: `key` names it in its id (`<group>.<key>`), `label` is what it says.
    pub fn choice(
        mut self,
        value: V,
        key: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> Self {
        self.choices.push(Choice {
            value,
            key: key.into(),
            label: label.into(),
            draw: None,
        });
        self
    }

    /// A choice that draws itself (a swatch, a shape), told whether it is chosen.
    pub fn drawn(
        mut self,
        value: V,
        key: impl Into<SharedString>,
        label: impl Into<SharedString>,
        draw: impl Fn(bool, &App) -> AnyElement + 'static,
    ) -> Self {
        self.choices.push(Choice {
            value,
            key: key.into(),
            label: label.into(),
            draw: Some(Box::new(draw)),
        });
        self
    }

    /// Runs when a choice is picked (a click, Enter, Space or an arrow key), inside the group's guarded handlers.
    pub fn on_choose(mut self, on_change: impl Fn(V, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(on_change));
        self
    }
}

impl<V: Copy + PartialEq + 'static> RenderOnce for RadioGroup<V> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ids: Vec<SharedString> = self
            .choices
            .iter()
            .map(|choice| format!("{}.{}", self.id, choice.key).into())
            .collect();
        let handles: Vec<FocusHandle> = ids.iter().map(|id| focus_handle(id, window, cx)).collect();
        let theme = theme(cx);
        let colors = palette(cx);
        let values: Vec<V> = self.choices.iter().map(|choice| choice.value).collect();
        let chosen = values.iter().position(|value| *value == self.value);
        let segmented = self.look == Look::Segmented;
        let mut group = div()
            .semantic(self.id.clone(), Role::RadioGroup)
            .aria_label(self.label)
            .flex()
            .items_center();
        group = if segmented {
            group
                .p(px(3.))
                .gap(px(2.))
                .rounded_full()
                .bg(hsla(colors.surface.sunk))
        } else {
            group.gap(r(theme.space.s10)).flex_wrap()
        };
        let moves = |by: isize| {
            let handles = handles.clone();
            let values = values.clone();
            let on_change = self.on_change.clone();
            move |window: &mut Window, cx: &mut App| {
                let Some(at) = handles.iter().position(|handle| handle.is_focused(window)) else {
                    return;
                };
                let count = handles.len() as isize;
                let to = (at as isize + by).rem_euclid(count) as usize;
                if let (Some(handle), Some(value)) = (handles.get(to), values.get(to)) {
                    handle.focus(window, cx);
                    if let Some(on_change) = &on_change {
                        on_change(*value, window, cx);
                    }
                }
            }
        };
        let (back, on) = (moves(-1), moves(1));
        let group = group
            .key_context(keymap::LIST)
            .on_action(move |_: &Prev, window, cx| {
                guarded("move back in a radio group", || back(window, cx));
            })
            .on_action(move |_: &Next, window, cx| {
                guarded("move on in a radio group", || on(window, cx));
            });
        let options = self.choices.into_iter().enumerate().map(|(at, choice)| {
            let on = Some(at) == chosen;
            let id = ids.get(at).cloned().unwrap_or_default();
            // Roving: only the chosen option is a tab stop (the first when none is).
            let stop = on || (chosen.is_none() && at == 0);
            let handle = handles
                .get(at)
                .cloned()
                .unwrap_or_else(|| cx.focus_handle())
                .tab_stop(stop);
            let ring = focus_visible(&handle, window);
            let option = div()
                .semantic(id, Role::RadioButton)
                .aria_label(choice.label.clone())
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .track_focus(&handle)
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .map(|option| match &choice.draw {
                    Some(draw) => option.rounded_full().child(draw(on, cx)),
                    None => option
                        .h(px(30.))
                        .px(r(theme.space.s14))
                        .rounded_full()
                        .type_style(&theme.typography.desktop.button)
                        .when(on, |option| {
                            option
                                .bg(hsla(colors.surface.raised))
                                .shadow(crate::ui::style::shadows(&theme.shadow.light.card))
                        })
                        .text_color(hsla(if on {
                            colors.text.primary
                        } else {
                            colors.text.secondary
                        }))
                        .when(!on, |option| {
                            option.hover(|style| style.bg(hsla(colors.state.hover)))
                        })
                        .child(choice.label.clone()),
                })
                .when(ring, |option| {
                    option.child(focus_ring(theme.radius.pill, cx))
                });
            let value = choice.value;
            let on_change = self.on_change.clone();
            let press: Option<crate::ui::primitives::button::Handler> =
                on_change.map(|on_change| {
                    Rc::new(move |window: &mut Window, cx: &mut App| on_change(value, window, cx))
                        as crate::ui::primitives::button::Handler
                });
            pressable(option, press).into_any_element()
        });
        group.children(options)
    }
}

/// An underlined text action (`.link`), quiet or not: a button to Windows.
#[derive(IntoElement)]
pub struct Link {
    id: SharedString,
    label: SharedString,
    quiet: bool,
    icon: Option<&'static str>,
    disabled: bool,
    on_press: Option<crate::ui::primitives::button::Handler>,
}

impl Link {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Link {
            id: id.into(),
            label: label.into(),
            quiet: false,
            icon: None,
            disabled: false,
            on_press: None,
        }
    }

    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    /// Unavailable for now, and says so: dimmed and pressing does nothing, but it keeps the keyboard if it has it (M4
    /// re-review R9).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn on_press(mut self, on_press: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }
}

impl RenderOnce for Link {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx);
        let theme = theme(cx);
        let colors = palette(cx);
        let (text, edge) = if self.quiet {
            (colors.text.secondary, colors.line.soft)
        } else {
            (colors.text.primary, colors.text.primary)
        };
        let link = div()
            .semantic(self.id, Role::Button)
            .aria_label(self.label.clone())
            .semantic_disabled(self.disabled)
            .when(self.disabled, |link| link.opacity(0.5))
            .track_focus(&focus)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(r(theme.space.s6))
            .pb(px(1.))
            .border_b(px(1.))
            .border_color(hsla(edge))
            .cursor_pointer()
            .type_style(&theme.typography.desktop.button)
            .line_height(r(18.))
            .text_color(hsla(text))
            .hover(|style| style.border_color(hsla(colors.accent.marker)))
            .when_some(self.icon, |link, icon| {
                link.child(
                    svg()
                        .path(format!("icons/i/175/{icon}.svg"))
                        .size(r(16.))
                        .flex_none()
                        .text_color(hsla(text)),
                )
            })
            .child(self.label)
            .children(ring_or_reveal(&focus, theme.radius.xs, window, cx));
        pressable(link, self.on_press.filter(|_| !self.disabled))
    }
}

/// A number with a less and a more button (`controls.tsx`'s `Stepper`), from `min` to `max`. The value is read out
/// with the label; each button is a tab stop, off at its end of the range.
#[derive(IntoElement)]
pub struct Stepper {
    id: SharedString,
    label: SharedString,
    value: u32,
    min: u32,
    max: u32,
    on_change: Option<OnChange<u32>>,
}

impl Stepper {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        value: u32,
        min: u32,
        max: u32,
    ) -> Self {
        Stepper {
            id: id.into(),
            label: label.into(),
            value,
            min,
            max,
            on_change: None,
        }
    }

    /// Runs with the next value when a button is pressed, inside the button's guarded handler.
    pub fn on_choose(mut self, on_change: impl Fn(u32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(on_change));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = palette(cx);
        let button = |key: &str, icon: &'static str, label: &str, next: Option<u32>| {
            let mut button = crate::ui::primitives::button::IconButton::new(
                format!("{}.{key}", self.id),
                icon,
                format!("{label}: {}", self.label),
            )
            .variant(crate::ui::primitives::button::IconVariant::Grey)
            .disabled(next.is_none());
            if let (Some(next), Some(on_change)) = (next, self.on_change.clone()) {
                button = button.on_press(move |window, cx| on_change(next, window, cx));
            }
            button
        };
        let less = (self.value > self.min).then(|| self.value.saturating_sub(1));
        let more = (self.value < self.max).then(|| self.value.saturating_add(1));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(r(theme.space.s8))
            .child(button("less", "minus", "Fewer", less))
            .child(
                div()
                    .semantic(format!("{}.value", self.id), Role::Status)
                    .aria_label(self.value.to_string())
                    .min_w(r(28.))
                    .text_center()
                    .font_family(crate::assets::MONO_FAMILY)
                    .type_style(&theme.typography.desktop.ui_strong)
                    .text_color(hsla(colors.text.primary))
                    .child(self.value.to_string()),
            )
            .child(button("more", "plus", "More", more))
    }
}

/// The arrow keys step between a list's tab stops (Recents, a card grid, a menu's rows), staying inside it: at either
/// end they stop there. `focus` is the list's own handle, kept by the caller.
pub fn arrows<E: InteractiveElement>(element: E, focus: &FocusHandle) -> E {
    let (back, on) = (focus.clone(), focus.clone());
    element
        .track_focus(focus)
        .key_context(keymap::LIST)
        .on_action(move |_: &Prev, window, cx| {
            guarded("move back in a list", || step(&back, false, window, cx));
        })
        .on_action(move |_: &Next, window, cx| {
            guarded("move on in a list", || step(&on, true, window, cx));
        })
}

fn step(list: &FocusHandle, forward: bool, window: &mut Window, cx: &mut App) {
    let before = window.focused(cx);
    if forward {
        window.focus_next(cx);
    } else {
        window.focus_prev(cx);
    }
    // Out of the list: back to where it was.
    if !list.contains_focused(window, cx)
        && let Some(before) = before
    {
        before.focus(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::primitives::keyboard_window;
    use crate::ui::testing::{draw, node};
    use std::cell::Cell;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Size {
        Small,
        Large,
        Huge,
    }

    #[gpui::test]
    fn a_radio_group_is_one_tab_stop_whose_arrows_move_the_choice(cx: &mut gpui::TestAppContext) {
        let picked = Rc::new(Cell::new(Size::Large));
        let shown = picked.clone();
        let cx = keyboard_window(cx, move |_, _| {
            let set = shown.clone();
            RadioGroup::new("test.size", "Size", shown.get())
                .choice(Size::Small, "small", "Small")
                .choice(Size::Large, "large", "Large")
                .choice(Size::Huge, "huge", "Huge")
                .on_choose(move |value, _, _| set.set(value))
                .into_any_element()
        });
        draw(cx);
        let group = node("test.size").expect("the group");
        assert_eq!(
            (group.role.as_str(), group.text.as_str()),
            ("RadioGroup", "Size")
        );
        // Tab reaches the chosen option.
        cx.simulate_keystrokes("tab");
        draw(cx);
        assert!(node("test.size.large").unwrap().focused);
        // The arrows move the choice and the focus, wrapping at the ends.
        cx.simulate_keystrokes("right");
        draw(cx);
        assert_eq!(picked.get(), Size::Huge);
        assert!(node("test.size.huge").unwrap().focused);
        cx.simulate_keystrokes("right");
        draw(cx);
        assert_eq!(picked.get(), Size::Small);
        cx.simulate_keystrokes("up");
        draw(cx);
        assert_eq!(picked.get(), Size::Huge);
    }

    #[gpui::test]
    fn a_link_is_a_button_that_enter_presses(cx: &mut gpui::TestAppContext) {
        let pressed = Rc::new(Cell::new(0));
        let count = pressed.clone();
        let cx = keyboard_window(cx, move |_, _| {
            let count = count.clone();
            Link::new("test.link", "Open chat")
                .on_press(move |_, _| count.set(count.get() + 1))
                .into_any_element()
        });
        draw(cx);
        assert_eq!(node("test.link").unwrap().role, "Button");
        cx.simulate_keystrokes("tab enter");
        assert_eq!(pressed.get(), 1);
    }
}
