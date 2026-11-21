//! A settings row (forms-settings `.setting`): the label and an optional description on the left, the control on the
//! right. Rows after the first carry a rule above them (`.setting + .setting`). In compact spacing (Appearance) a row is
//! at least 44 px tall with 8 px above and below (`[data-slot="settings-row"]` in `globals.css`).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div, px,
};

use crate::ui::style::{TypeExt as _, appearance, hsla, palette, r, theme};

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
        let compact = appearance(cx).compact();
        let theme = theme(cx);
        let colors = palette(cx);
        div()
            .flex()
            .items_center()
            .gap(r(theme.space.s16))
            .map(|row| {
                if compact {
                    row.min_h(r(44.)).py(r(8.))
                } else {
                    row.py(r(theme.space.s14))
                }
            })
            .when(self.ruled, |row| {
                row.border_t(px(1.)).border_color(hsla(colors.line.base))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s2))
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

#[cfg(test)]
mod tests {
    use desktop_host::device_settings::Density;
    use gpui::{IntoElement as _, ParentElement as _, Role, div};

    use super::SettingsRow;
    use crate::semantic::SemanticExt as _;
    use crate::ui::style::Appearance;
    use crate::ui::testing::{draw, node};

    /// Screens' request: compact spacing brings a row down to 44 px, with 8 px above and below.
    #[gpui::test]
    fn a_compact_row_is_44_px(cx: &mut gpui::TestAppContext) {
        let cx =
            crate::ui::primitives::test_window(cx, |_, _| {
                div()
                    .child(div().semantic("test.row", Role::Group).child(
                        SettingsRow::new("Start at login").description("Opens with Windows."),
                    ))
                    .into_any_element()
            });
        draw(cx);
        let comfortable = node("test.row").unwrap().bounds.height;
        cx.update(|_, cx| {
            cx.set_global(Appearance {
                density: Density::Compact,
                ..Appearance::default()
            })
        });
        cx.update(|window, _| window.refresh());
        draw(cx);
        let compact = node("test.row").unwrap().bounds.height;
        assert!(compact < comfortable, "{compact} < {comfortable}");
        assert!(compact >= 44., "at least 44 px: {compact}");
    }
}
