//! The Routines screen and the routine editor (T5.4): the rows, Recent runs, Save, Delete and Add routine, through the
//! store's fake. Sample data is invented.

use gpui::TestAppContext;
use gpui_kit::component::WindowExt as _;
use protocol::model::{BotState, Routine, RoutineRun, RoutineRunStatus, TimeZone};
use protocol::wire::method;

use crate::model::testing::FakeRpc;
use crate::ui::shell::route::Route;
use crate::ui::testing::{
    app, bot, click, dialog_fits, dialog_fits_with_problem, draw, ids, node, state, text,
};

fn routine(id: &str, bot_id: &str, name: &str, enabled: bool) -> Routine {
    Routine {
        id: id.into(),
        bot_id: bot_id.into(),
        name: name.into(),
        prompt: "Check the inbox and sum it up.".into(),
        days: vec![1, 2, 3, 4, 5],
        times: vec!["08:30".into()],
        time_zone: TimeZone::from_stored("UTC"),
        time_limit_minutes: None,
        enabled,
        next_run_at: None,
        created_at: "2026-11-01T09:00:00.000Z".into(),
    }
}

fn run(id: &str, routine_id: &str) -> RoutineRun {
    RoutineRun {
        id: id.into(),
        bot_id: "b1".into(),
        routine_id: routine_id.into(),
        due_at: "2026-11-14T08:30:00.000Z".into(),
        started_at: Some("2026-11-14T08:30:01.000Z".into()),
        ended_at: Some("2026-11-14T08:31:30.000Z".into()),
        status: RoutineRunStatus::Done,
        note: None,
    }
}

fn two_agents() -> crate::model::state::AppState {
    state(
        vec![
            bot("b1", "Forge", BotState::Idle),
            bot("b2", "Scout", BotState::Idle),
        ],
        vec![],
        vec![
            routine("r1", "b1", "Morning check", true),
            routine("r2", "b2", "Weekly report", false),
        ],
    )
}

#[gpui::test]
fn each_agent_shows_its_routines_and_when_they_run(cx: &mut TestAppContext) {
    let (_, _) = app(cx, Route::Routines, two_agents(), FakeRpc::new());
    assert_eq!(text("screen.routines"), "Routines");
    assert_eq!(text("routines.summary"), "2 routines");
    assert!(node("routines.b1.group").is_some());
    let morning = text("routines.b1.r1");
    assert!(morning.starts_with("Morning check, "), "{morning}");
    // A paused routine says so instead of when it runs.
    assert_eq!(text("routines.b2.r2"), "Weekly report, Paused");
    assert!(node("routines.b2.add").is_some(), "Add routine");
}

#[gpui::test]
fn a_routine_opens_in_the_editor_with_its_recent_runs(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new().on::<method::RoutinesRuns>(|_| Ok(vec![run("u1", "r1")]));
    let (cx, calls) = app(cx, Route::Routines, two_agents(), rpc);
    click(cx, "routines.b1.r1");
    draw(cx);
    assert_eq!(text("dialog.title"), "Edit routine");
    assert_eq!(text("routine.name"), "Name");
    assert!(node("routine.run.u1").is_some(), "its run");
    let asked: Vec<_> = calls
        .calls()
        .into_iter()
        .filter(|(name, _)| *name == "routines.runs")
        .collect();
    assert_eq!(asked[0].1["routineId"], "r1");
    // Monday to Friday are on; Sunday isn't.
    assert!(ids("routine.day.").len() == 7);
}

#[gpui::test]
fn saving_an_edit_sends_the_whole_draft_and_closes(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::RoutinesRuns>(|_| Ok(vec![]))
        .on::<method::RoutinesUpdate>(|_| Ok(routine("r1", "b1", "Morning check", false)));
    let (cx, calls) = app(cx, Route::Routines, two_agents(), rpc);
    click(cx, "routines.b1.r1");
    draw(cx);
    assert_eq!(text("routine.runs.none"), "No runs yet.");
    click(cx, "routine.on");
    click(cx, "routine.save");
    draw(cx);
    let updated: Vec<_> = calls
        .calls()
        .into_iter()
        .filter(|(name, _)| *name == "routines.update")
        .collect();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].1["id"], "r1");
    assert_eq!(updated[0].1["patch"]["enabled"], false);
    assert_eq!(updated[0].1["patch"]["name"], "Morning check");
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "closed"
    );
}

#[gpui::test]
fn a_save_that_fails_keeps_the_editor_and_says_why(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::RoutinesRuns>(|_| Ok(vec![]))
        .on::<method::RoutinesUpdate>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "That time is taken.".into(),
            })
        });
    let (cx, _) = app(cx, Route::Routines, two_agents(), rpc);
    click(cx, "routines.b1.r1");
    click(cx, "routine.save");
    draw(cx);
    assert!(
        cx.update(|window, cx| window.has_active_dialog(cx)),
        "still open"
    );
    assert!(text("routine.problem").contains("That time is taken."));
}

#[gpui::test]
fn a_new_routine_says_what_it_is_missing_before_asking_the_daemon(cx: &mut TestAppContext) {
    let (cx, calls) = app(cx, Route::Routines, two_agents(), FakeRpc::new());
    click(cx, "routines.b2.add");
    draw(cx);
    assert_eq!(text("dialog.title"), "New routine");
    click(cx, "routine.save");
    draw(cx);
    assert!(node("routine.problem").is_some(), "what's missing");
    assert!(
        calls
            .calls()
            .iter()
            .all(|(name, _)| *name != "routines.create")
    );
}

#[gpui::test]
fn add_routine_creates_it_for_that_agent(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::RoutinesCreate>(|_| Ok(routine("r9", "b2", "Evening sum-up", true)));
    let (cx, calls) = app(cx, Route::Routines, two_agents(), rpc);
    click(cx, "routines.b2.add");
    cx.simulate_input("Evening sum-up");
    click(cx, "routine.prompt");
    cx.simulate_input("Sum up the day.");
    click(cx, "routine.day.3");
    click(cx, "routine.add_time");
    cx.simulate_input("18:00");
    cx.simulate_keystrokes("enter");
    click(cx, "routine.save");
    draw(cx);
    let made: Vec<_> = calls
        .calls()
        .into_iter()
        .filter(|(name, _)| *name == "routines.create")
        .collect();
    assert_eq!(made.len(), 1, "{:?}", calls.calls());
    assert_eq!(made[0].1["botId"], "b2");
    assert_eq!(made[0].1["name"], "Evening sum-up");
    // The new routine starts at 09:00 (`EMPTY_ROUTINE`); 18:00 joins it.
    assert_eq!(made[0].1["times"], serde_json::json!(["09:00", "18:00"]));
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "closed"
    );
}

#[gpui::test]
fn delete_removes_the_routine(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::RoutinesRuns>(|_| Ok(vec![]))
        .on::<method::RoutinesDelete>(|_| Ok(protocol::wire::Empty::default()));
    let (cx, calls) = app(cx, Route::Routines, two_agents(), rpc);
    click(cx, "routines.b1.r1");
    click(cx, "routine.delete");
    draw(cx);
    assert!(
        calls
            .calls()
            .iter()
            .any(|(name, params)| *name == "routines.delete" && params["id"] == "r1")
    );
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "closed"
    );
}

/// The README pictures (a release blocker): at 1000 x 600 the routine editor is taller than the window. Its footer (Delete,
/// Cancel, Save) stays inside the window and unclipped, the body scrolling between the title and the footer, and Save
/// works where it is.
#[gpui::test]
fn at_1000_by_600_the_routine_editors_footer_stays_in_the_window(cx: &mut TestAppContext) {
    use gpui::{px, size};
    let rpc = FakeRpc::new()
        .on::<method::RoutinesRuns>(|_| Ok(vec![]))
        .on::<method::RoutinesUpdate>(|_| Ok(routine("r1", "b1", "Morning check", true)));
    let (cx, calls) = app(cx, Route::Routines, two_agents(), rpc);
    cx.simulate_resize(size(px(1000.), px(600.)));
    draw(cx);
    click(cx, "routines.b1.r1");
    draw(cx);
    for id in ["routine.delete", "routine.cancel", "routine.save"] {
        let control = node(id).unwrap_or_else(|| panic!("{id} is drawn"));
        let bounds = control.bounds;
        assert!(
            bounds.y >= 0. && bounds.y + bounds.height <= 600.,
            "{id} is inside the window: {bounds:?}"
        );
        assert_eq!(
            control.visible,
            Some(bounds),
            "{id} is whole, not clipped by the scrolling body"
        );
    }
    click(cx, "routine.save");
    draw(cx);
    assert!(
        calls
            .calls()
            .iter()
            .any(|(name, _)| *name == "routines.update"),
        "Save works where it is"
    );
}

/// The dialog fit (every dialog's check): the routine editor, editing (Delete, Cancel and Save) and new (Cancel and
/// Save), and after a failed save of each, why above the actions (release review m2 and m4: at 360 wide it was squeezed
/// between Delete and Cancel).
#[gpui::test]
fn dialog_fits_the_routine_editor(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new().on::<method::RoutinesRuns>(|_| Ok(vec![]));
    let (cx, _) = app(cx, Route::Routines, two_agents(), rpc);
    click(cx, "routines.b1.r1");
    let editing = ["routine.delete", "routine.cancel", "routine.save"];
    dialog_fits(cx, &editing);
    // The fake has no routines.update: the save fails.
    click(cx, "routine.save");
    dialog_fits_with_problem(cx, "routine.problem", &editing);
    click(cx, "routine.cancel");
    click(cx, "routines.b2.add");
    assert_eq!(text("dialog.title"), "New routine");
    let new = ["routine.cancel", "routine.save"];
    dialog_fits(cx, &new);
    // Empty, it says what it is missing.
    click(cx, "routine.save");
    dialog_fits_with_problem(cx, "routine.problem", &new);
}

/// At 1000 x 600 the routine editor's body scrolls between its title and its footer: Tab brings each control it
/// reaches into view, whole, down through the body (Stop after, at its foot) and on to the footer's Save.
#[gpui::test]
fn tab_brings_each_routine_editor_control_into_view(cx: &mut TestAppContext) {
    use gpui::{px, size};
    let rpc = FakeRpc::new().on::<method::RoutinesRuns>(|_| Ok(vec![]));
    let (cx, _) = app(cx, Route::Routines, two_agents(), rpc);
    cx.simulate_resize(size(px(1000.), px(600.)));
    draw(cx);
    click(cx, "routines.b1.r1");
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..30 {
        cx.simulate_keystrokes("tab");
        draw(cx);
        // The scroll that brings the control into view shows on the frame after the focus moves.
        draw(cx);
        let Some(focused) = crate::test_driver::nodes()
            .into_iter()
            .find(|node| node.focused)
        else {
            continue;
        };
        assert_eq!(
            focused.visible,
            Some(focused.bounds),
            "{} is in view, whole",
            focused.id
        );
        if !seen.contains(&focused.id) {
            seen.push(focused.id);
        }
    }
    for id in ["routine.name", "routine.prompt", "routine.save"] {
        assert!(
            seen.iter().any(|seen| seen == id),
            "Tab reached {id}: {seen:?}"
        );
    }
    assert!(
        seen.iter().any(|seen| seen.starts_with("routine.stop.")),
        "Tab reached Stop after, at the body's foot: {seen:?}"
    );
}
