//! The Aegis menu (overlays.html `.menu`): a raised card of rows beside its trigger, on gpui-kit's popover. It raises
//! the browser shield while open; Escape closes it at once. Selecting a row runs its handler (guarded here) and closes
//! the menu. Arrow-key movement between rows comes with full keyboard navigation (T5.6).

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, AnyElement, App, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Toggled, Window, div, px, svg,
};
use gpui_kit::base::{Popover, PopoverState};

use super::shield::{BrowserShield, ShieldGuard};
use crate::guard::guarded;
use crate::semantic::SemanticExt as _;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

type Trigger = Box<dyn FnOnce(bool, &Window, &App) -> AnyElement>;
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

enum Entry {
    Item(MenuItem),
    Rule,
    Note(SharedString),
}

pub struct MenuItem {
    id: SharedString,
    label: SharedString,
    icon: Option<&'static str>,
    end: Option<SharedString>,
    detail: Option<SharedString>,
    selected: bool,
    checked: Option<bool>,
    on_select: Handler,
}

impl MenuItem {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on_select: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        MenuItem {
            id: id.into(),
            label: label.into(),
            icon: None,
            end: None,
            detail: None,
            selected: false,
            checked: None,
            on_select: Rc::new(on_select),
        }
    }

    /// A Lucide icon by name, drawn at 17 px.
    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
        self
    }

    /// Quiet text at the row's end, such as a shortcut or a count.
    pub fn end(mut self, text: impl Into<SharedString>) -> Self {
        self.end = Some(text.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// A second, quieter line under the label, as the host list's rows have (navigation-host-switcher.html
    /// `.host-row`): the row grows to 52 px and its icon to 18 px.
    pub fn detail(mut self, text: impl Into<SharedString>) -> Self {
        self.detail = Some(text.into());
        self
    }

    /// One choice of a set (a radio item): the chosen one is selected and ends with a check.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
}

#[derive(IntoElement)]
pub struct Menu {
    id: ElementId,
    trigger: Trigger,
    entries: Vec<Entry>,
    anchor: Anchor,
    offset: Pixels,
}

impl Menu {
    /// `trigger` draws the control that opens the menu; it is told whether the menu is open.
    pub fn new(
        id: impl Into<ElementId>,
        trigger: impl FnOnce(bool, &Window, &App) -> AnyElement + 'static,
    ) -> Self {
        Menu {
            id: id.into(),
            trigger: Box::new(trigger),
            entries: Vec::new(),
            anchor: Anchor::TopLeft,
            offset: px(0.),
        }
    }

    /// Which corner of the menu meets the trigger: the top left (below it) by default, `BottomLeft` to open above.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// The gap between the trigger and the menu.
    pub fn offset(mut self, offset: Pixels) -> Self {
        self.offset = offset;
        self
    }

    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(Entry::Item(item));
        self
    }

    pub fn rule(mut self) -> Self {
        self.entries.push(Entry::Rule);
        self
    }

    pub fn note(mut self, text: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Note(text.into()));
        self
    }
}

impl RenderOnce for Menu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // The guard outlives this render: it lives in the element's state while the menu is open.
        let shield = window.use_keyed_state(self.id.clone(), cx, |_, _| None::<ShieldGuard>);
        let entries = self.entries;
        Popover::new(self.id)
            .anchor(self.anchor)
            .offset(self.offset)
            .trigger_with(self.trigger)
            .on_open_change(move |open, _, cx| {
                guarded("open or close a menu", || {
                    let open = *open;
                    shield.update(cx, |held, cx| {
                        *held = open.then(|| BrowserShield::raise(cx))
                    });
                });
            })
            .content(move |_, _, cx| {
                let popover = cx.entity();
                let theme = theme(cx);
                let colors = palette(cx);
                div()
                    .min_w(r(240.))
                    .p(r(theme.space.s6))
                    .flex()
                    .flex_col()
                    .bg(hsla(colors.surface.raised))
                    .rounded(r(theme.radius.md))
                    .border_t_1()
                    .border_color(hsla(colors.overlay.highlight))
                    .shadow(shadows(&theme.shadow.light.raised))
                    .children(entries.into_iter().map(|entry| {
                        match entry {
                            Entry::Item(item) => row(item, popover.clone(), cx),
                            Entry::Rule => div()
                                .mx(r(theme.space.s4))
                                .my(r(theme.space.s6))
                                .h(px(1.))
                                .bg(hsla(colors.line.base))
                                .into_any_element(),
                            Entry::Note(text) => div()
                                .px(r(theme.space.s10))
                                .pt(r(theme.space.s6))
                                .pb(r(theme.space.s8))
                                .type_style(&theme.typography.desktop.small)
                                .text_color(hsla(colors.text.tertiary))
                                .child(text)
                                .into_any_element(),
                        }
                    }))
            })
    }
}

fn row(item: MenuItem, popover: Entity<PopoverState>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let on_select = item.on_select;
    let on = item.selected || item.checked == Some(true);
    let two_lines = item.detail.is_some();
    let role = if item.checked.is_some() {
        Role::MenuItemRadio
    } else {
        Role::MenuItem
    };
    let label: SharedString = match &item.detail {
        Some(detail) => format!("{}, {detail}", item.label).into(),
        None => item.label.clone(),
    };
    div()
        .semantic(item.id, role)
        .aria_label(label)
        .when_some(item.checked, |row, checked| {
            row.aria_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            })
        })
        .flex()
        .items_center()
        .gap(r(if two_lines {
            theme.space.s12
        } else {
            theme.space.s10
        }))
        .min_h(r(if two_lines { 52. } else { 36. }))
        .px(r(theme.space.s10))
        .rounded(r(theme.radius.sm))
        .type_style(&theme.typography.desktop.ui)
        .text_color(hsla(colors.text.primary))
        .when(on, |row| row.bg(hsla(colors.state.hover)))
        .hover(|style| style.bg(hsla(colors.state.hover)))
        .on_click(move |_, window, cx| {
            guarded("choose a menu row", || {
                on_select(window, cx);
                popover.update(cx, |state, cx| state.dismiss(window, cx));
            });
        })
        .when_some(item.icon, |row, icon| {
            row.child(
                svg()
                    .path(format!("icons/i/175/{icon}.svg"))
                    .size(r(if two_lines { 18. } else { 17. }))
                    .flex_none()
                    .text_color(hsla(colors.icon.base)),
            )
        })
        .map(|row| match item.detail {
            Some(detail) => row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .type_style(&theme.typography.desktop.ui_strong)
                            .child(item.label),
                    )
                    .child(
                        div()
                            .type_style(&theme.typography.desktop.meta)
                            .text_color(hsla(colors.text.tertiary))
                            .child(detail),
                    ),
            ),
            None => row.child(item.label),
        })
        .when_some(item.end, |row, end| {
            row.child(
                div()
                    .ml_auto()
                    .type_style(&theme.typography.desktop.meta)
                    .text_color(hsla(colors.text.tertiary))
                    .child(end),
            )
        })
        .when(item.checked == Some(true), |row| {
            row.child(
                svg()
                    .path("icons/i/175/check.svg")
                    .size(r(18.))
                    .flex_none()
                    .ml_auto()
                    .text_color(hsla(colors.text.primary)),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::primitives::test_window;
    use gpui::{Modifiers, point};

    #[gpui::test]
    fn raises_shield(cx: &mut gpui::TestAppContext) {
        let cx = test_window(cx, |_, _| {
            Menu::new("test.menu", |_, _, _| {
                div().id("test.trigger").size(px(40.)).into_any_element()
            })
            .item(MenuItem::new("test.one", "One", |_, _| {}))
            .into_any_element()
        });
        assert!(!cx.update(|_, cx| BrowserShield::is_raised(cx)));

        cx.simulate_click(point(px(20.), px(20.)), Modifiers::none());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.update(|_, cx| BrowserShield::is_raised(cx)),
            "open: the shield is up"
        );

        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            !cx.update(|_, cx| BrowserShield::is_raised(cx)),
            "closed: the shield is down"
        );
    }
}
