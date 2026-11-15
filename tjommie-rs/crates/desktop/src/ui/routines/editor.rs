//! The routine editor (`routines/routine-editor.tsx`; WF7), in a 560 px dialog: on or off, the name, what to do, the
//! days and times, the zone they're read in, Stop after, a saved routine's Recent runs, and Delete, Cancel and Save.
//! It stays open until Save or Delete works, and says why when they don't. A Save or Delete that ends after the editor
//! was closed closes nothing.
//!
//! One difference from the TypeScript's time field: Enter adds the time and keeps the field open, empty, for the next
//! one (the TypeScript handed the focus back to Add time); leaving the field adds what it holds and closes it.

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Toggled, Window, div, svg,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use protocol::model::RoutineRun;
use protocol::wire::{RoutinesRuns, method};

use crate::guard::{guarded, guarded_async};
use crate::model::bot_form::RoutineDraft;
use crate::model::format::times_label;
use crate::model::labels::error_text;
use crate::model::routine_form::{
    RunIcon, WEEK, add_time, routine_problem, run_line, stop_after_options, toggle_day, zone_line,
};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Look, RadioGroup, TextArea, TextField, label, problem};
use crate::ui::primitives::button::{IconButton, Pill, PillVariant, pressable};
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::toggle::Toggle;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

/// What Save does with the draft: create or update it on the daemon, or keep it in a form's list.
pub type Save = Rc<dyn Fn(RoutineDraft, &mut App) -> LocalBoxFuture<'static, Result<(), String>>>;
/// What Delete does.
pub type Delete = Rc<dyn Fn(&mut App) -> LocalBoxFuture<'static, Result<(), String>>>;

/// How the editor opens.
pub struct RoutineEditor {
    pub title: &'static str,
    pub initial: RoutineDraft,
    /// The zone the times are read in.
    pub time_zone: String,
    /// A saved routine's id: shows its Recent runs.
    pub runs_for: Option<String>,
    pub on_save: Save,
    pub on_delete: Option<Delete>,
}

/// A saved routine's Recent runs: loading, failed, or the list.
enum Runs {
    Loading,
    Failed,
    Loaded(Vec<RoutineRun>),
}

struct Editor {
    title: &'static str,
    days: Vec<u8>,
    times: Vec<String>,
    limit: Option<u32>,
    enabled: bool,
    name: Entity<InputState>,
    prompt: Entity<TextareaState>,
    time: Entity<InputState>,
    adding: bool,
    zone: String,
    runs_for: Option<String>,
    runs: Runs,
    /// The routine's newest run when its runs were last read; a new one reads them again.
    read_at: Option<Option<RoutineRun>>,
    busy: bool,
    problem: Option<SharedString>,
    window: AnyWindowHandle,
    on_save: Save,
    on_delete: Option<Delete>,
    _times: Option<Subscription>,
}

/// Opens the routine editor.
pub fn open_routine_editor(spec: RoutineEditor, window: &mut Window, cx: &mut App) {
    let draft = spec.initial;
    let name = cx.new(|cx| InputState::new(window, cx).default_value(draft.name.clone()));
    let prompt = cx.new(|cx| TextareaState::new(window, cx).default_value(draft.prompt.clone()));
    let time = cx.new(|cx| InputState::new(window, cx).placeholder("08:30"));
    let editor = cx.new(|_| Editor {
        title: spec.title,
        days: draft.days,
        times: draft.times,
        limit: draft.time_limit_minutes,
        enabled: draft.enabled,
        name: name.clone(),
        prompt,
        time: time.clone(),
        adding: false,
        zone: spec.time_zone,
        runs_for: spec.runs_for,
        runs: Runs::Loading,
        read_at: None,
        busy: false,
        problem: None,
        window: window.window_handle(),
        on_save: spec.on_save,
        on_delete: spec.on_delete,
        _times: None,
    });
    // Enter adds the time and keeps the field for the next; leaving it adds what it holds.
    let adding = editor.clone();
    let subscription = cx.subscribe(&time, move |field, event: &InputEvent, cx| {
        guarded("add a routine time", || match event {
            InputEvent::PressEnter { .. } => add(&adding, &field, true, cx),
            InputEvent::Blur => add(&adding, &field, false, cx),
            _ => {}
        });
    });
    editor.update(cx, |editor, _| editor._times = Some(subscription));
    open_dialog(window, cx, move |dialog, window, cx| {
        let body = body(&editor, window, cx);
        let heading = editor.read(cx).title;
        // routine-editor.tsx's 560 px, as rems.
        dialog
            .w(window.rem_size() * 35.0)
            .title(title(heading, cx))
            // Enter in a field is the field's (a time, Save): it never closes the editor through the dialog's Confirm.
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(body)
    });
    // The name has the focus (`initialFocus`), after the dialog takes it.
    window.defer(cx, move |window, cx| {
        guarded("focus the routine's name", || {
            name.update(cx, |name, cx| name.focus(window, cx))
        });
    });
}

fn add(editor: &Entity<Editor>, field: &Entity<InputState>, again: bool, cx: &mut App) {
    let typed = field.read(cx).value().to_string();
    editor.update(cx, |editor, cx| {
        if !editor.adding {
            return;
        }
        editor.times = add_time(&editor.times, &typed);
        editor.adding = again;
        cx.notify();
    });
    if again {
        let window = editor.read(cx).window;
        let field = field.clone();
        let _ = window.update(cx, |_, window, cx| {
            field.update(cx, |field, cx| field.set_value("", window, cx))
        });
    }
}

fn draft(editor: &Editor, cx: &App) -> RoutineDraft {
    RoutineDraft {
        name: editor.name.read(cx).value().to_string(),
        prompt: editor.prompt.read(cx).value().to_string(),
        days: editor.days.clone(),
        times: editor.times.clone(),
        time_limit_minutes: editor.limit,
        enabled: editor.enabled,
    }
}

/// Reads the routine's runs when the editor opens, and again when the daemon reports a run of it.
fn read_runs(editor: &Entity<Editor>, cx: &mut App) {
    let Some(id) = editor.read(cx).runs_for.clone() else {
        return;
    };
    let newest = store::state(cx).latest_runs.get(&id).cloned();
    if editor.read(cx).read_at.as_ref() == Some(&newest) {
        return;
    }
    editor.update(cx, |editor, _| editor.read_at = Some(newest));
    let call = store::request::<method::RoutinesRuns>(cx, RoutinesRuns { routine_id: id });
    let editor = editor.clone();
    cx.spawn(async move |cx| {
        guarded_async("read a routine's runs", async move {
            let runs = call.await;
            cx.update(|cx| {
                editor.update(cx, |editor, cx| {
                    editor.runs = match runs {
                        Ok(runs) => Runs::Loaded(runs),
                        Err(_) => Runs::Failed,
                    };
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

/// Save (`save`), or Delete: says what's missing, or runs it and closes the editor when it works.
fn act(editor: &Entity<Editor>, delete: bool, cx: &mut App) {
    let state = editor.read(cx);
    if state.busy {
        return;
    }
    let work = if delete {
        match state.on_delete.clone() {
            Some(on_delete) => on_delete(cx),
            None => return,
        }
    } else {
        let draft = draft(editor.read(cx), cx);
        if let Some(missing) = routine_problem(&draft) {
            editor.update(cx, |editor, cx| {
                editor.problem = Some(missing.into());
                cx.notify();
            });
            return;
        }
        let on_save = editor.read(cx).on_save.clone();
        on_save(draft, cx)
    };
    editor.update(cx, |editor, cx| {
        editor.busy = true;
        editor.problem = None;
        cx.notify();
    });
    let editor = editor.clone();
    cx.spawn(async move |cx| {
        guarded_async("save or delete a routine", async move {
            let done = work.await;
            cx.update(|cx| {
                let window = editor.read(cx).window;
                let _ = window.update(cx, |_, window, cx| {
                    // Closed meanwhile: nothing is this editor's to close.
                    if !window.has_active_dialog(cx) {
                        return;
                    }
                    match done {
                        Ok(()) => window.close_dialog(cx),
                        Err(why) => editor.update(cx, |editor, cx| {
                            editor.problem = Some(why.into());
                            editor.busy = false;
                            cx.notify();
                        }),
                    }
                });
            });
        })
        .await;
    })
    .detach();
}

fn body(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> AnyElement {
    read_runs(editor, cx);
    let days = days(editor, window, cx);
    let times = times(editor, window, cx);
    let state = editor.read(cx);
    let theme = theme(cx);
    let colors = palette(cx);
    let on = state.enabled;
    let switch = {
        let editor = editor.clone();
        Toggle::new("routine.on", "Routine on", on).on_toggle(move |enabled, _, cx| {
            editor.update(cx, |editor, cx| {
                editor.enabled = enabled;
                cx.notify();
            })
        })
    };
    let stop = {
        let editor = editor.clone();
        // Each choice's value is its limit: "none" is no limit, else the minutes.
        stop_after_options(state.limit)
            .into_iter()
            .fold(
                RadioGroup::new("routine.stop", "Stop after", state.limit),
                |group, option| group.choice(option.value.parse().ok(), option.value, option.label),
            )
            .look(Look::Segmented)
            .on_choose(move |limit: Option<u32>, _, cx| {
                editor.update(cx, |editor, cx| {
                    editor.limit = limit;
                    cx.notify();
                })
            })
    };
    let runs = state.runs_for.as_ref().map(|_| runs(&state.runs, cx));
    let failed = state.problem.clone();
    let save = {
        let editor = editor.clone();
        Pill::new("routine.save", "Save")
            .variant(PillVariant::Primary)
            .on_press(move |_, cx| act(&editor, false, cx))
    };
    let delete = state.on_delete.is_some().then(|| {
        let editor = editor.clone();
        IconButton::new("routine.delete", "trash-2", "Delete routine")
            .on_press(move |_, cx| act(&editor, true, cx))
    });
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .justify_end()
                .items_center()
                .gap(r(theme.space.s8))
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.secondary))
                .child("On")
                .child(switch),
        )
        .child(
            div()
                .mt(r(theme.space.s8))
                .flex()
                .flex_col()
                .child(label("Name", cx))
                .child(TextField::new("routine.name", "Name", &state.name)),
        )
        .child(
            div()
                .mt(r(theme.space.s16))
                .flex()
                .flex_col()
                .child(label("What to do", cx))
                .child(TextArea::new("routine.prompt", "What to do", &state.prompt)),
        )
        .child(small("Days", cx))
        .child(days)
        .child(small("Times", cx))
        .child(times)
        .child(
            div()
                .mt(r(theme.space.s12))
                .flex()
                .items_center()
                .gap(r(theme.space.s6))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.secondary))
                .child(
                    svg()
                        .path("icons/i/175/globe.svg")
                        .size(r(14.))
                        .flex_none()
                        .text_color(hsla(colors.text.secondary)),
                )
                .child(zone_line(&state.zone, None)),
        )
        .child(small("Stop after", cx))
        .child(div().mt(r(theme.space.s4)).child(stop))
        .children(runs)
        .child(
            div()
                .mt(r(theme.space.s20))
                .pt(r(theme.space.s12))
                .border_t_1()
                .border_color(hsla(colors.line.base))
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .children(delete)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .children(failed.map(|text| problem("routine.problem", text, cx))),
                )
                .child(
                    Pill::new("routine.cancel", "Cancel")
                        .on_press(|window, cx| window.close_dialog(cx)),
                )
                .child(save),
        )
        .into_any_element()
}

fn small(text: &'static str, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .mt(r(theme.space.s16))
        .type_style(&theme.typography.desktop.ui)
        .text_color(hsla(palette(cx).text.secondary))
        .child(text)
        .into_any_element()
}

/// Days (`role="group"`): a toggle button each, Monday first.
fn days(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> AnyElement {
    let chosen = editor.read(cx).days.clone();
    let buttons: Vec<AnyElement> = WEEK
        .iter()
        .map(|week_day| {
            let on = chosen.contains(&week_day.day);
            let id: SharedString = format!("routine.day.{}", week_day.day).into();
            let focus = focus_handle(&id, window, cx);
            let ring = focus_visible(&focus, window);
            let theme = theme(cx);
            let colors = palette(cx);
            let button = div()
                .semantic(id, Role::Button)
                .aria_label(week_day.name)
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .track_focus(&focus)
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .h(r(32.))
                .w(r(40.))
                .rounded_full()
                .cursor_pointer()
                .type_style(&theme.typography.desktop.ui)
                .map(|button| {
                    if on {
                        button
                            .bg(hsla(colors.action.primary))
                            .text_color(hsla(colors.on.primary))
                    } else {
                        button
                            .bg(hsla(colors.surface.sunk))
                            .text_color(hsla(colors.text.secondary))
                    }
                })
                .child(week_day.letter.to_string())
                .when(ring, |button| {
                    button.child(focus_ring(theme.radius.pill, cx))
                });
            let (editor, day) = (editor.clone(), week_day.day);
            pressable(
                button,
                Some(Rc::new(move |_, cx: &mut App| {
                    editor.update(cx, |editor, cx| {
                        editor.days = toggle_day(&editor.days, day);
                        cx.notify();
                    })
                })),
            )
            .into_any_element()
        })
        .collect();
    div()
        .semantic("routine.days", Role::Group)
        .aria_label("Days")
        .mt(r(theme(cx).space.s4))
        .flex()
        .gap(r(theme(cx).space.s8))
        .children(buttons)
        .into_any_element()
}

/// Times: a chip each with its Remove, then Add time, or the field a new one is typed in.
fn times(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> AnyElement {
    let (times, adding, field) = {
        let editor = editor.read(cx);
        (editor.times.clone(), editor.adding, editor.time.clone())
    };
    let theme = theme(cx);
    let colors = palette(cx);
    let chips: Vec<AnyElement> = times
        .iter()
        .enumerate()
        .map(|(at, time)| {
            let shown = times_label(&[time.as_str()]);
            let (editor, time) = (editor.clone(), time.clone());
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s4))
                .h(r(32.))
                .pl(r(theme.space.s12))
                .pr(r(theme.space.s2))
                .rounded_full()
                .bg(hsla(colors.surface.sunk))
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.primary))
                .child(shown.clone())
                .child(
                    IconButton::new(
                        format!("routine.time.{at}.remove"),
                        "x",
                        format!("Remove {shown}"),
                    )
                    .on_press(move |_, cx| {
                        editor.update(cx, |editor, cx| {
                            editor.times.retain(|each| *each != time);
                            cx.notify();
                        })
                    }),
                )
                .into_any_element()
        })
        .collect();
    let add: AnyElement = if adding {
        div()
            .w(r(120.))
            .child(TextField::new("routine.new_time", "New time", &field))
            .into_any_element()
    } else {
        let (editor, field) = (editor.clone(), field.clone());
        Pill::new("routine.add_time", "Add time")
            .icon("plus")
            .small()
            .on_press(move |window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.adding = true;
                    cx.notify();
                });
                field.update(cx, |field, cx| field.focus(window, cx));
            })
            .into_any_element()
    };
    let _ = window;
    div()
        .mt(r(theme.space.s4))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(r(theme.space.s8))
        .children(chips)
        .child(add)
        .into_any_element()
}

/// Recent runs (WF7): each run's icon, when it was due, what happened and how long it ran.
fn runs(runs: &Runs, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let now = jiff::Zoned::now();
    let note = |id: &'static str, text: &'static str| {
        div()
            .semantic(id, Role::Paragraph)
            .aria_label(text)
            .mt(r(theme.space.s8))
            .type_style(&theme.typography.desktop.ui)
            .text_color(hsla(colors.text.secondary))
            .child(text)
            .into_any_element()
    };
    let list = match runs {
        Runs::Loading => div().into_any_element(),
        Runs::Failed => note("routine.runs.failed", "Couldn't load the runs."),
        Runs::Loaded(runs) if runs.is_empty() => note("routine.runs.none", "No runs yet."),
        Runs::Loaded(runs) => div()
            .mt(r(theme.space.s8))
            .flex()
            .flex_col()
            .children(runs.iter().map(|run| {
                let line = run_line(run, &now);
                let (icon, bright) = match line.icon {
                    RunIcon::Done => ("check", true),
                    RunIcon::Running => ("loader-circle", true),
                    RunIcon::Stopped => ("timer-off", false),
                    RunIcon::Failed => ("circle-alert", false),
                    RunIcon::Missed => ("circle-dashed", false),
                    RunIcon::Skipped => ("pause", false),
                };
                let tone = if bright {
                    colors.text.primary
                } else {
                    colors.text.secondary
                };
                div()
                    .semantic(format!("routine.run.{}", run.id), Role::ListItem)
                    .aria_label(format!("{}, {}", line.when, line.text))
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s10))
                    .h(r(26.))
                    .type_style(&theme.typography.desktop.ui)
                    .child(
                        svg()
                            .path(format!("icons/i/175/{icon}.svg"))
                            .size(r(14.))
                            .flex_none()
                            .text_color(hsla(tone)),
                    )
                    .child(
                        div()
                            .w(r(142.))
                            .flex_none()
                            .text_color(hsla(colors.text.primary))
                            .child(line.when),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(hsla(tone))
                            .child(line.text),
                    )
                    .children(line.length.map(|length| {
                        div()
                            .flex_none()
                            .type_style(&theme.typography.desktop.meta)
                            .text_color(hsla(colors.text.tertiary))
                            .child(length)
                    }))
                    .into_any_element()
            }))
            .into_any_element(),
    };
    div()
        .mt(r(theme.space.s20))
        .pt(r(theme.space.s16))
        .border_t_1()
        .border_color(hsla(colors.line.base))
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .child(
                    div()
                        .type_style(&theme.typography.desktop.label)
                        .text_color(hsla(colors.text.tertiary))
                        .child("Recent runs"),
                )
                .child(
                    div()
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.tertiary))
                        .child("Last 20 runs are kept"),
                ),
        )
        .child(list)
        .into_any_element()
}

/// The zone this PC reads times in (`localZone`).
pub fn local_zone() -> String {
    jiff::tz::TimeZone::system()
        .iana_name()
        .unwrap_or("UTC")
        .to_owned()
}

/// A failed call's sentence, for an editor's `Save` or `Delete`.
pub fn said(error: impl std::fmt::Display) -> String {
    error_text(&error)
}
