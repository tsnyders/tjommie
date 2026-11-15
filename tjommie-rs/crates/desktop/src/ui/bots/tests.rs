//! New agent and Agent settings (T5.1): `bot-form/identity.test.tsx` one for one, and the pages' flows, through the
//! store's fake. Sample data is invented.

use gpui::TestAppContext;
use gpui_kit::component::WindowExt as _;
use protocol::model::{Bot, BotState, Routine, TimeZone};
use protocol::wire::{Empty, Notes, method};

use crate::model::testing::FakeRpc;
use crate::ui::shell::route::{self, Route};
use crate::ui::testing::{app, bot, click, draw, ids, node, state, text};

fn made(id: &str, name: &str) -> Bot {
    bot(id, name, BotState::Idle)
}

fn routine(id: &str, bot_id: &str, name: &str) -> Routine {
    Routine {
        id: id.into(),
        bot_id: bot_id.into(),
        name: name.into(),
        prompt: "Sum up the inbox.".into(),
        days: vec![1, 2, 3, 4, 5],
        times: vec!["08:30".into()],
        time_zone: TimeZone::from_stored("UTC"),
        time_limit_minutes: None,
        enabled: true,
        next_run_at: None,
        created_at: "2026-11-01T09:00:00.000Z".into(),
    }
}

fn calls_of(calls: &[(&'static str, serde_json::Value)], method: &str) -> Vec<serde_json::Value> {
    calls
        .iter()
        .filter(|(name, _)| *name == method)
        .map(|(_, params)| params.clone())
        .collect()
}

/// `identity.test.tsx`: "makes the color and shape pickers two radio groups, with the chosen swatches checked".
#[gpui::test]
fn the_colour_and_shape_pickers_are_two_radio_groups_with_the_choices_checked(
    cx: &mut TestAppContext,
) {
    let (cx, _) = app(
        cx,
        Route::NewBot,
        state(vec![], vec![], vec![]),
        FakeRpc::new(),
    );
    assert_eq!(node("bot_form.color").unwrap().role, "RadioGroup");
    assert_eq!(text("bot_form.color"), "Color");
    assert_eq!(node("bot_form.shape").unwrap().role, "RadioGroup");
    assert_eq!(text("bot_form.shape"), "Shape");
    let radios = ids("bot_form.color.").len() + ids("bot_form.shape.").len();
    assert_eq!(radios, 18);
    click(cx, "bot_form.color.teal");
    click(cx, "bot_form.shape.cloud");
    let checked: Vec<String> = crate::test_driver::nodes()
        .into_iter()
        .filter(|node| {
            (node.id.starts_with("bot_form.color.") || node.id.starts_with("bot_form.shape."))
                && node.toggled
        })
        .map(|node| node.text)
        .collect();
    assert_eq!(checked, ["teal", "cloud"]);
    assert_eq!(text("bot_form.color.teal"), "teal");
    assert_eq!(text("bot_form.shape.rounded-square"), "rounded square");
}

#[gpui::test]
fn create_agent_makes_the_agent_and_opens_its_chat(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new().on::<method::BotsCreate>(|params| {
        let mut bot = made("b9", params["name"].as_str().unwrap_or_default());
        bot.instructions = params["instructions"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        Ok(bot)
    });
    let (cx, calls) = app(cx, Route::NewBot, state(vec![], vec![], vec![]), rpc);
    assert_eq!(text("screen.new"), "New agent");
    click(cx, "bot_form.name");
    cx.simulate_input("Harbour");
    click(cx, "bot_form.instructions");
    cx.simulate_input("Watch the shipping schedule.");
    click(cx, "new_bot.create");
    draw(cx);
    let made = calls_of(&calls.calls(), "bots.create");
    assert_eq!(made.len(), 1);
    assert_eq!(made[0]["name"], "Harbour");
    assert_eq!(made[0]["instructions"], "Watch the shipping schedule.");
    assert_eq!(
        cx.update(|_, cx| route::current(cx)),
        Route::Chat("b9".into())
    );
}

#[gpui::test]
fn a_name_another_agent_has_is_refused_before_asking_the_daemon(cx: &mut TestAppContext) {
    let (cx, calls) = app(
        cx,
        Route::NewBot,
        state(vec![made("b1", "Forge")], vec![], vec![]),
        FakeRpc::new(),
    );
    click(cx, "bot_form.name");
    cx.simulate_input("Forge");
    click(cx, "new_bot.create");
    draw(cx);
    assert!(node("bot_form.name.problem").is_some(), "says why");
    assert!(calls_of(&calls.calls(), "bots.create").is_empty());
}

#[gpui::test]
fn a_suggestion_fills_the_form(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new().on::<method::BotsCreate>(|params| {
        Ok(made("b9", params["name"].as_str().unwrap_or_default()))
    });
    let (cx, calls) = app(cx, Route::NewBot, state(vec![], vec![], vec![]), rpc);
    let first = crate::model::suggestions::suggestions()
        .into_iter()
        .next()
        .expect("a suggestion");
    click(cx, "new_bot.suggestion.0");
    click(cx, "new_bot.create");
    draw(cx);
    let made = calls_of(&calls.calls(), "bots.create");
    assert_eq!(made[0]["name"], first.form.name);
    // Its routines are created with it.
    assert_eq!(
        calls_of(&calls.calls(), "routines.create").len(),
        first.routines.len()
    );
}

#[gpui::test]
fn a_routine_that_fails_leaves_the_agent_made_and_names_it(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::BotsCreate>(|_| Ok(made("b9", "Harbour")))
        .on::<method::RoutinesCreate>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "No time left today.".into(),
            })
        });
    let (cx, _) = app(cx, Route::NewBot, state(vec![], vec![], vec![]), rpc);
    click(cx, "bot_form.name");
    cx.simulate_input("Harbour");
    click(cx, "bot_form.routines");
    click(cx, "new_bot.routines.add");
    cx.simulate_input("Morning check");
    click(cx, "routine.prompt");
    cx.simulate_input("Check the schedule.");
    click(cx, "routine.save");
    draw(cx);
    click(cx, "new_bot.create");
    draw(cx);
    let said = text("new_bot.problem");
    assert!(
        said.starts_with("Harbour was created, but not these routines: Morning check ("),
        "{said}"
    );
    assert_eq!(text("new_bot.open"), "Open Harbour");
    assert!(node("new_bot.create").is_none(), "it isn't offered again");
}

#[gpui::test]
fn agent_settings_saves_only_what_changed_and_goes_to_the_chat(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::BotsNotes>(|_| {
            Ok(Notes {
                text: String::new(),
            })
        })
        .on::<method::BotsUpdate>(|_| Ok(made("b1", "Forge Two")));
    let (cx, calls) = app(
        cx,
        Route::BotSettings("b1".into()),
        state(vec![made("b1", "Forge")], vec![], vec![]),
        rpc,
    );
    assert_eq!(text("screen.bot_settings"), "Forge");
    assert_eq!(text("bot_settings.notes"), "No notes yet.");
    click(cx, "bot_form.name");
    cx.simulate_input(" Two");
    click(cx, "bot_settings.save");
    draw(cx);
    let updated = calls_of(&calls.calls(), "bots.update");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0]["id"], "b1");
    assert_eq!(updated[0]["patch"]["name"], "Forge Two");
    assert!(
        updated[0]["patch"].get("instructions").is_none(),
        "only the name"
    );
    assert_eq!(
        cx.update(|_, cx| route::current(cx)),
        Route::Chat("b1".into())
    );
}

#[gpui::test]
fn delete_agent_asks_first_and_then_goes_home(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::BotsNotes>(|_| {
            Ok(Notes {
                text: "Ship log kept.".into(),
            })
        })
        .on::<method::BotsDelete>(|_| Ok(Empty::default()));
    let (cx, calls) = app(
        cx,
        Route::BotSettings("b1".into()),
        state(vec![made("b1", "Forge")], vec![], vec![]),
        rpc,
    );
    assert_eq!(text("bot_settings.notes"), "Ship log kept.");
    click(cx, "bot_settings.delete");
    draw(cx);
    assert_eq!(text("dialog.title"), "Delete Forge?");
    assert!(
        calls_of(&calls.calls(), "bots.delete").is_empty(),
        "asks first"
    );
    click(cx, "dialog.confirm");
    draw(cx);
    assert_eq!(calls_of(&calls.calls(), "bots.delete")[0]["id"], "b1");
    assert_eq!(cx.update(|_, cx| route::current(cx)), Route::Agents);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui::test]
fn an_agent_s_settings_list_its_routines(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new().on::<method::BotsNotes>(|_| {
        Ok(Notes {
            text: String::new(),
        })
    });
    let (cx, _) = app(
        cx,
        Route::BotSettings("b1".into()),
        state(
            vec![made("b1", "Forge")],
            vec![],
            vec![routine("r1", "b1", "Morning check")],
        ),
        rpc,
    );
    click(cx, "bot_form.routines");
    assert!(text("bot_settings.routines.r1").starts_with("Morning check, "));
}

#[gpui::test]
fn an_unknown_agent_has_no_settings_and_goes_home(cx: &mut TestAppContext) {
    let (cx, _) = app(
        cx,
        Route::BotSettings("gone".into()),
        state(vec![], vec![], vec![]),
        FakeRpc::new(),
    );
    draw(cx);
    assert_eq!(cx.update(|_, cx| route::current(cx)), Route::Agents);
}

/// T5.6: Tab leaves a multi-line field (gpui-kit's would indent, trapping the keyboard), and Shift+Tab comes back.
#[gpui::test]
fn tab_leaves_the_instructions_and_shift_tab_comes_back(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::NewBot, state(vec![], vec![], vec![]), FakeRpc::new());
    click(cx, "bot_form.instructions");
    assert!(node("bot_form.instructions").unwrap().focused);
    cx.simulate_keystrokes("tab");
    draw(cx);
    assert!(!node("bot_form.instructions").unwrap().focused, "Tab moved on");
    cx.simulate_keystrokes("shift-tab");
    draw(cx);
    assert!(node("bot_form.instructions").unwrap().focused, "Shift+Tab came back");
}
