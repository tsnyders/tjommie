//! An agent's routines as rows (`routines/routine-rows.tsx`): each a button that opens the routine editor, with its
//! title and when it runs, or Paused.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};

use crate::model::format::{routine_title, schedule};
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::pressable;
use crate::ui::primitives::focus_handle;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

/// What a row shows (`RoutineRow`): `key` names it in its id and in `on_open`.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutineRow {
    pub key: String,
    pub name: String,
    pub time_limit_minutes: Option<u32>,
    pub days: Vec<u8>,
    pub times: Vec<String>,
    pub enabled: bool,
}

pub type OnOpen = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// The rows under `prefix` (`<prefix>.<key>`), or "No routines yet."
pub fn routine_rows(
    prefix: &str,
    rows: &[RoutineRow],
    on_open: OnOpen,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if rows.is_empty() {
        let theme = theme(cx);
        return div()
            .semantic(format!("{prefix}.none"), Role::Paragraph)
            .aria_label("No routines yet.")
            .type_style(&theme.typography.desktop.ui)
            .text_color(hsla(palette(cx).text.secondary))
            .child("No routines yet.")
            .into_any_element();
    }
    let rows: Vec<AnyElement> = rows
        .iter()
        .map(|row| one(prefix, row, on_open.clone(), window, cx))
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(r(theme(cx).space.s4))
        .children(rows)
        .into_any_element()
}

fn one(
    prefix: &str,
    row: &RoutineRow,
    on_open: OnOpen,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id: SharedString = format!("{prefix}.{}", row.key).into();
    let focus = focus_handle(&id, window, cx);
    let ring = focus_visible(&focus, window);
    let theme = theme(cx);
    let colors = palette(cx);
    let title = routine_title(&row.name, row.time_limit_minutes);
    let times: Vec<&str> = row.times.iter().map(String::as_str).collect();
    let when = if row.enabled {
        schedule(&row.days, &times)
    } else {
        "Paused".to_owned()
    };
    let key = row.key.clone();
    let button = div()
        .semantic(id, Role::Button)
        .aria_label(format!("{title}, {when}"))
        .track_focus(&focus)
        .relative()
        .flex()
        .items_start()
        .gap(r(theme.space.s10))
        .px(r(theme.space.s8))
        .py(r(theme.space.s4))
        .rounded(r(theme.radius.field))
        .cursor_pointer()
        .hover(|style| style.bg(hsla(colors.state.hover)))
        .child(
            svg()
                .path(if row.enabled {
                    "icons/i/175/clock.svg"
                } else {
                    "icons/i/175/pause.svg"
                })
                .mt(r(2.))
                .size(r(16.))
                .flex_none()
                .text_color(hsla(if row.enabled {
                    colors.text.primary
                } else {
                    colors.text.secondary
                })),
        )
        .child(
            div()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .type_style(&theme.typography.desktop.ui)
                        .text_color(hsla(colors.text.primary))
                        .child(title),
                )
                .child(
                    div()
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.secondary))
                        .child(when),
                ),
        )
        .when(ring, |button| {
            button.child(focus_ring(theme.radius.field, cx))
        });
    pressable(
        button,
        Some(Rc::new(move |window: &mut Window, cx: &mut App| {
            on_open(&key, window, cx)
        })),
    )
    .into_any_element()
}
