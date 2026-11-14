//! A settings row (forms-settings `.setting`): the label and an optional description on the left, the control on the
//! right. Rows after the first carry a rule above them (`.setting + .setting`).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div, px,
};

use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

#[derive(IntoElement)]
pub struct SettingsRow {
    label: SharedString,
    description: Option<SharedString>,
    control: Option<AnyElement>,
    ruled: bool,
}

impl SettingsRow {
    pub fn new(label: impl Into<SharedString>) -> Self {
        SettingsRow {
            label: label.into(),
            description: None,
            control: None,
            ruled: false,
        }
    }

    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }

    /// A rule above: every row but a list's first.
    pub fn ruled(mut self) -> Self {
        self.ruled = true;
        self
    }
}

impl RenderOnce for SettingsRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = palette(cx);
        div()
            .flex()
            .items_center()
            .gap(r(theme.space.s16))
            .py(r(theme.space.s14))
            .when(self.ruled, |row| {
                row.border_t(px(1.)).border_color(hsla(colors.line.base))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(r(2.))
                    .child(
                        div()
                            .type_style(&theme.typography.desktop.ui_strong)
                            .text_color(hsla(colors.text.primary))
                            .child(self.label),
                    )
                    .when_some(self.description, |text, description| {
                        text.child(
                            div()
                                .type_style(&theme.typography.desktop.small)
                                .text_color(hsla(colors.text.secondary))
                                .child(description),
                        )
                    }),
            )
            .when_some(self.control, |row, control| row.child(control))
    }
}
