//! The Routines screen and the routine editor (T5.4): the rows, Recent runs, Save, Delete and Add routine, through the
//! store's fake. Sample data is invented.

use gpui::TestAppContext;
use gpui_kit::component::WindowExt as _;
use protocol::model::{BotState, Routine, RoutineRun, RoutineRunStatus, TimeZone};
use protocol::wire::method;

use crate::model::testing::FakeRpc;
use crate::ui::shell::route::Route;
use crate::ui::testing::{app, bot, click, draw, ids, node, state, text};

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
