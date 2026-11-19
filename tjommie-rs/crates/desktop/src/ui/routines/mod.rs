//! Routines (Phase 3 plan T5.4; `routines/*`): every agent's routines in one place, under the sidebar's Routines. The
//! TypeScript showed them only on each agent's form and its chat panel, so this screen is those rows gathered: each
//! agent, its routines (each opens the routine editor, with its Recent runs) and Add routine.

pub mod editor;
pub mod rows;

use std::rc::Rc;

use futures::FutureExt as _;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use protocol::model::{Bot, Routine};

use self::editor::{RoutineEditor, local_zone, open_routine_editor, said};
use self::rows::{RoutineRow, routine_rows};
use crate::model::routine_form::{draft_from, empty_routine};
use crate::model::routines::{remove_routine, save_routine};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::Link;
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::scroll::{follow_focus, scroll_handle};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// A routine as a row (`routines.map((each) => ({ ...each, key: each.id }))`).
pub fn row_of(routine: &Routine) -> RoutineRow {
    RoutineRow {
        key: routine.id.clone(),
        name: routine.name.clone(),
        time_limit_minutes: routine.time_limit_minutes,
        days: routine.days.clone(),
        times: routine.times.clone(),
        enabled: routine.enabled,
    }
}

/// Opens a saved routine in the editor: Save updates it and Delete removes it, on the daemon.
pub fn edit(routine: Routine, window: &mut Window, cx: &mut App) {
    let (saved, removed) = (routine.clone(), routine.clone());
    open_routine_editor(
        RoutineEditor {
            title: "Edit routine",
            initial: draft_from(&routine),
            time_zone: routine.time_zone.as_str().to_owned(),
            runs_for: Some(routine.id.clone()),
            on_save: Rc::new(move |draft, cx| {
                let (rpc, routine) = (store::rpc(cx), saved.clone());
                async move {
                    save_routine(&rpc, &routine.bot_id, Some(&routine), &draft)
                        .await
                        .map_err(said)
                }
                .boxed_local()
            }),
            on_delete: Some(Rc::new(move |cx| {
                let (rpc, routine) = (store::rpc(cx), removed.clone());
                async move { remove_routine(&rpc, &routine).await.map_err(said) }.boxed_local()
            })),
        },
        window,
        cx,
    );
}

/// Opens a new routine for `bot_id` in the editor: Save creates it on the daemon.
pub fn add(bot_id: String, window: &mut Window, cx: &mut App) {
    open_routine_editor(
        RoutineEditor {
            title: "New routine",
            initial: empty_routine(),
            time_zone: local_zone(),
            runs_for: None,
            on_save: Rc::new(move |draft, cx| {
                let (rpc, bot_id) = (store::rpc(cx), bot_id.clone());
                async move {
                    save_routine(&rpc, &bot_id, None, &draft)
                        .await
                        .map_err(said)
                }
                .boxed_local()
            }),
            on_delete: None,
        },
        window,
        cx,
    );
}

/// The Routines screen.
pub fn screen(window: &mut Window, cx: &mut App) -> AnyElement {
    let state = store::state(cx).clone();
    let groups: Vec<AnyElement> = state
        .bots
        .iter()
        .map(|bot| group(bot, &state.routines, window, cx))
        .collect();
    let count = state.routines.len();
    let scroll = scroll_handle("routines.scroll", window, cx);
    let theme = theme(cx);
    let colors = palette(cx);
    let summary = match count {
        1 => "1 routine".to_owned(),
        _ => format!("{count} routines"),
    };
    let empty = state.bots.is_empty().then(|| {
        div()
            .semantic("routines.empty", Role::Paragraph)
            .aria_label("No agents yet.")
            .type_style(&theme.typography.desktop.ui)
            .text_color(hsla(colors.text.secondary))
            .child("No agents yet.")
    });
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(r(theme.space.s24))
        .pt(r(theme.space.s32))
        .px(r(theme.space.s32))
        .child(
            div()
                .child(
                    div()
                        .semantic("screen.routines", Role::Heading)
                        .aria_label("Routines")
                        .type_style(&theme.typography.desktop.display_l)
                        .text_color(hsla(colors.text.primary))
                        .child("Routines"),
                )
                .child(
                    div()
                        .semantic("routines.summary", Role::Paragraph)
                        .aria_label(summary.clone())
                        .mt(r(theme.space.s10))
                        .type_style(&theme.typography.desktop.ui)
                        .text_color(hsla(colors.text.secondary))
                        .child(summary),
                ),
        )
        .child(follow_focus(
            &scroll,
            div()
                .id("routines.scroll")
                .track_scroll(&scroll)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .pb(r(theme.space.s28))
                .flex()
                .flex_col()
                .gap(r(theme.space.s16))
                .children(groups)
                .children(empty),
        ))
        .into_any_element()
}

/// An agent's routines: its name, its rows and Add routine.
fn group(bot: &Bot, routines: &[Routine], window: &mut Window, cx: &mut App) -> AnyElement {
    let mine: Vec<Routine> = routines
        .iter()
        .filter(|routine| routine.bot_id == bot.id)
        .cloned()
        .collect();
    let rows: Vec<RoutineRow> = mine.iter().map(row_of).collect();
    let open = Rc::new(move |key: &str, window: &mut Window, cx: &mut App| {
        if let Some(routine) = mine.iter().find(|routine| routine.id == key) {
            edit(routine.clone(), window, cx);
        }
    });
    let list = routine_rows(&format!("routines.{}", bot.id), &rows, open, window, cx);
    let bot_id = bot.id.clone();
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(format!("routines.{}.group", bot.id), Role::Group)
        .aria_label(bot.name.clone())
        .p(r(theme.space.s16))
        .rounded(r(theme.radius.slot))
        .bg(hsla(colors.surface.raised))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .shadow(shadows(&theme.shadow.light.card))
        .flex()
        .flex_col()
        .gap(r(theme.space.s12))
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s10))
                .child(BotAvatar::new(bot.color, bot.shape, theme.size.avatar28))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .type_style(&theme.typography.desktop.ui_strong)
                        .text_color(hsla(colors.text.primary))
                        .child(bot.name.clone()),
                )
                .child(
                    Link::new(format!("routines.{}.add", bot.id), "Add routine")
                        .on_press(move |window, cx| add(bot_id.clone(), window, cx)),
                ),
        )
        .child(list)
        .into_any_element()
}

#[cfg(test)]
mod tests;
