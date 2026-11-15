//! The Agents overview's view tests (T5.1): `agents/cards.test.tsx` one for one, and the page's behaviours.

use gpui::TestAppContext;
use protocol::model::BotState;
use protocol::wire::method;

use crate::model::overview::overview_summary;
use crate::model::state::{Conversation, ConversationStatus};
use crate::model::testing::FakeRpc;
use crate::ui::shell::route::{self, Route};
use crate::ui::testing::{app, bot, click, draw, ids, node, room, state, text};

fn conversation(status: ConversationStatus) -> Conversation {
    Conversation {
        status,
        messages: Vec::new(),
        has_more: false,
        pending: Vec::new(),
    }
}

/// `cards.test.tsx`: "says nothing of the room's posts while they're being read, and that it has none once they are".
#[gpui::test]
fn a_room_card_says_nothing_while_its_posts_are_read_and_that_it_has_none_once_they_are(
    cx: &mut TestAppContext,
) {
    let mut reading = state(vec![], vec![room("r1", "Growth", &["b1"])], vec![]);
    reading
        .conversations
        .insert("r1".into(), conversation(ConversationStatus::Loading));
    let (cx, _) = app(cx, Route::Agents, reading, FakeRpc::new());
    assert!(node("agents.room.r1").is_some(), "the room's card");
    assert!(
        node("agents.room.r1.none").is_none(),
        "nothing while reading"
    );
    // A blank line that keeps its height: no "No posts yet" while reading.
    assert!(text("agents.room.r1.line").trim().is_empty());

    cx.update(|_, cx| {
        crate::ui::store::update(cx, |state| {
            state
                .conversations
                .insert("r1".into(), conversation(ConversationStatus::Ready));
            true
        })
    });
    draw(cx);
    assert_eq!(text("agents.room.r1.none"), "No agent chats yet");
    assert_eq!(text("agents.room.r1.line"), "No posts yet");
}

#[gpui::test]
fn every_agent_has_a_card_under_the_summary(cx: &mut TestAppContext) {
    let bots = vec![
        bot("b1", "Forge", BotState::Waiting),
        bot("b2", "Scout", BotState::Working),
        bot("b3", "Ledger", BotState::Idle),
    ];
    let summary = overview_summary(&bots);
    let (_, _) = app(
        cx,
        Route::Agents,
        state(bots, vec![], vec![]),
        FakeRpc::new(),
    );
    assert_eq!(text("screen.agents"), "Agents");
    assert_eq!(text("agents.summary"), summary);
    for id in ["agents.card.b1", "agents.card.b2", "agents.card.b3"] {
        assert!(node(id).is_some(), "{id}");
    }
    assert_eq!(text("agents.card.b2.name"), "Scout");
    // The New agent tile, with everything shown.
    assert!(node("agents.new_bot").is_some());
}

#[gpui::test]
fn the_filter_and_search_narrow_the_cards(cx: &mut TestAppContext) {
    let bots = vec![
        bot("b1", "Forge", BotState::Waiting),
        bot("b2", "Scout", BotState::Working),
        bot("b3", "Ledger", BotState::Idle),
    ];
    let (cx, _) = app(
        cx,
        Route::Agents,
        state(bots, vec![], vec![]),
        FakeRpc::new(),
    );
    click(cx, "agents.filter.working");
    assert_eq!(cards(), ["agents.card.b2"]);
    assert!(
        node("agents.new_bot").is_none(),
        "the tiles only show with everything"
    );

    click(cx, "agents.filter.all");
    click(cx, "agents.search.open");
    cx.simulate_input("led");
    draw(cx);
    assert_eq!(cards(), ["agents.card.b3"]);

    cx.simulate_input("zzz");
    draw(cx);
    assert!(cards().is_empty());
    assert_eq!(text("agents.empty"), "No agents match.");
}

/// The agents' cards on screen, in order.
fn cards() -> Vec<String> {
    ids("agents.card.")
        .into_iter()
        .filter(|id| id.matches('.').count() == 2)
        .collect()
}

#[gpui::test]
fn a_waiting_agent_needs_you_and_review_opens_its_chat(cx: &mut TestAppContext) {
    let bots = vec![
        bot("b1", "Forge", BotState::Waiting),
        bot("b2", "Scout", BotState::Working),
    ];
    let (cx, _) = app(
        cx,
        Route::Agents,
        state(bots, vec![], vec![]),
        FakeRpc::new(),
    );
    assert!(node("agents.need.b1").is_some(), "Forge needs you");
    assert!(node("agents.need.b2").is_none());
    assert_eq!(text("agents.needs.count"), "1 need you");
    click(cx, "agents.need.b1.review");
    assert_eq!(
        cx.update(|_, cx| route::current(cx)),
        Route::Chat("b1".into())
    );
}

#[gpui::test]
fn pause_on_a_card_asks_the_daemon_and_a_failure_says_why(cx: &mut TestAppContext) {
    let bots = vec![bot("b2", "Scout", BotState::Working)];
    let rpc = FakeRpc::new().on::<method::BotsPause>(|_| {
        Err(client::RpcError::Daemon {
            code: protocol::ErrorCode::Internal,
            message: "The agent is busy.".into(),
        })
    });
    let (cx, calls) = app(cx, Route::Agents, state(bots, vec![], vec![]), rpc);
    click(cx, "agents.card.b2.pause");
    let paused: Vec<_> = calls
        .calls()
        .into_iter()
        .filter(|(name, _)| *name == "bots.pause")
        .collect();
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].1["id"], "b2");
    assert_eq!(paused[0].1["paused"], true);
    assert!(
        text("agents.card.b2.problem").contains("The agent is busy."),
        "{}",
        text("agents.card.b2.problem")
    );
}

#[gpui::test]
fn new_agent_opens_the_new_agent_screen(cx: &mut TestAppContext) {
    let (cx, _) = app(
        cx,
        Route::Agents,
        state(vec![], vec![], vec![]),
        FakeRpc::new(),
    );
    click(cx, "agents.new");
    assert_eq!(cx.update(|_, cx| route::current(cx)), Route::NewBot);
}
