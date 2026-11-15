//! The thread's quiet lines (`notices.tsx`): time labels, the daemon's notices ("Stopped.", a time or usage limit), an
//! error with Retry when the daemon can run that work again, and the label above a routine's run.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use protocol::model::{ErrorData, RoutineLabelData, RoutineRunStatus};
use protocol::wire::{ChatRetry, method};

use crate::guard::guarded_async;
use crate::model::labels::error_text;
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::Pill;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// A time label between groups of messages.
pub fn stamp(label: &SharedString, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .w_full()
        .py(r(theme.space.s8))
        .flex()
        .justify_center()
        .type_style(&theme.typography.desktop.meta)
        .text_color(hsla(palette(cx).text.tertiary))
        .child(label.clone())
        .into_any_element()
}

/// The icon of one of the daemon's notices.
fn notice_icon(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    if lower.contains("time limit") {
        Some("clock")
    } else if lower.contains("usage limit") {
        Some("hourglass")
    } else if lower.starts_with("stopped") {
        Some("square")
    } else {
        None
    }
}

/// One of the daemon's notices, centred in the thread.
pub fn notice(id: &str, text: &str, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(format!("notice.{id}"), Role::Status)
        .aria_label(SharedString::from(text.to_owned()))
        .w_full()
        .py(r(theme.space.s4))
        .flex()
        .items_center()
        .justify_center()
        .gap(r(theme.space.s8))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(colors.text.secondary))
        .when_some(notice_icon(text), |row, icon| {
            row.child(
                svg()
                    .path(format!("icons/i/175/{icon}.svg"))
                    .size(r(14.))
                    .flex_none()
                    .text_color(hsla(colors.icon.base)),
            )
        })
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

/// An error's Retry: whether it is running, and what went wrong with it.
#[derive(Default)]
struct Retry {
    busy: bool,
    problem: Option<SharedString>,
}

/// An error, with Retry when the daemon can run that work again (`chat.retry`).
pub fn error(
    id: &str,
    text: &str,
    retry: Option<ErrorData>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let state: Entity<Retry> = window.use_keyed_state(
        SharedString::from(format!("error.{id}.retry")),
        cx,
        |_, _| Retry::default(),
    );
    let (busy, problem) = {
        let retry = state.read(cx);
        (retry.busy, retry.problem.clone())
    };
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(format!("error.{id}"), Role::Group)
        .aria_label(SharedString::from(text.to_owned()))
        .w_full()
        .max_w(r(600.))
        .flex()
        .items_center()
        .gap(r(theme.space.s12))
        .px(r(theme.space.s16))
        .py(r(theme.space.s10))
        .bg(hsla(colors.surface.raised))
        .rounded(r(theme.radius.slot))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .shadow(shadows(&theme.shadow.light.card))
        .child(
            svg()
                .path("icons/i/175/circle-alert.svg")
                .size(r(16.))
                .flex_none()
                .text_color(hsla(colors.icon.base)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.primary))
                .child(SharedString::from(text.to_owned()))
                .when_some(problem, |column, problem| {
                    column.child(
                        div()
                            .semantic(format!("error.{id}.problem"), Role::Alert)
                            .text_color(hsla(theme.color.bot.red))
                            .child(problem),
                    )
                }),
        )
        .when_some(retry.filter(|_| !busy), |row, retry| {
            row.child(
                Pill::new(format!("error.{id}.retry"), "Retry")
                    .icon("rotate-cw")
                    .small()
                    .on_press(move |_, cx| run_retry(&state, &retry, cx)),
            )
        })
        .into_any_element()
}

fn run_retry(state: &Entity<Retry>, target: &ErrorData, cx: &mut App) {
    state.update(cx, |retry, cx| {
        retry.busy = true;
        retry.problem = None;
        cx.notify();
    });
    let call = store::request::<method::ChatRetry>(
        cx,
        ChatRetry {
            message_id: target.retry_message_id.clone(),
            bot_id: target.retry_bot_id.clone(),
        },
    );
    let state = state.clone();
    cx.spawn(async move |cx| {
        guarded_async("retry an error", async move {
            let outcome = call.await;
            state.update(cx, |retry, cx| {
                retry.busy = false;
                retry.problem = outcome.err().map(|error| error_text(&error).into());
                cx.notify();
            })
        })
        .await;
    })
    .detach();
}

/// "Routine: <name>" above a run, saying why a run was missed or skipped.
pub fn routine_label(text: &str, data: &RoutineLabelData) -> String {
    let note = data.note.as_deref().unwrap_or("").trim();
    let note = note.strip_suffix('.').unwrap_or(note);
    let why = match data.status {
        RoutineRunStatus::Missed => Some("Missed, the PC was off".to_owned()),
        RoutineRunStatus::Skipped if note.is_empty() => Some("Skipped".to_owned()),
        RoutineRunStatus::Skipped => Some(format!("Skipped: {note}")),
        _ => None,
    };
    match why {
        Some(why) => format!("{text} · {why}"),
        None => text.to_owned(),
    }
}

/// The routine label, with a clock: grey when the run was missed or skipped.
pub fn routine(id: &str, text: &str, data: &RoutineLabelData, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let quiet = matches!(
        data.status,
        RoutineRunStatus::Missed | RoutineRunStatus::Skipped
    );
    let label = SharedString::from(routine_label(text, data));
    div()
        .semantic(format!("routine.{id}"), Role::Label)
        .aria_label(label.clone())
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .py(r(theme.space.s4))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(colors.text.tertiary))
        .child(
            svg()
                .path("icons/i/175/clock.svg")
                .size(r(14.))
                .flex_none()
                .text_color(hsla(if quiet {
                    colors.icon.muted
                } else {
                    colors.text.primary
                })),
        )
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(status: RoutineRunStatus, note: Option<&str>) -> String {
        routine_label(
            "Routine: Pnet apply 9am",
            &RoutineLabelData {
                routine_id: "r1".into(),
                run_id: "u1".into(),
                status,
                note: note.map(str::to_owned),
            },
        )
    }

    #[test]
    fn says_why_a_run_was_missed_or_skipped() {
        assert_eq!(
            label(RoutineRunStatus::Done, None),
            "Routine: Pnet apply 9am"
        );
        assert_eq!(
            label(RoutineRunStatus::Missed, None),
            "Routine: Pnet apply 9am · Missed, the PC was off"
        );
        assert_eq!(
            label(RoutineRunStatus::Skipped, Some("Pnet apply was paused.")),
            "Routine: Pnet apply 9am · Skipped: Pnet apply was paused"
        );
        assert_eq!(
            label(RoutineRunStatus::Skipped, None),
            "Routine: Pnet apply 9am · Skipped"
        );
        assert_eq!(
            label(RoutineRunStatus::Skipped, Some("")),
            "Routine: Pnet apply 9am · Skipped"
        );
    }

    #[test]
    fn notices_pick_their_icon() {
        assert_eq!(
            notice_icon("Stopped at the time limit (3 hours)."),
            Some("clock")
        );
        assert_eq!(
            notice_icon("Paused: the usage limit was reached."),
            Some("hourglass")
        );
        assert_eq!(notice_icon("Stopped."), Some("square"));
        assert_eq!(notice_icon("Forge joined."), None);
    }
}
