//! The chat's view tests (T4.1): the component tests' behaviours, over a fake store, read back through the semantic
//! tree. Sample data is invented.

use gpui::{
    Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase, point, px,
};
use protocol::model::{BotState, Message, MessageKind, MessageStatus};
use protocol::wire::{MessagePage, method};
use serde_json::{Value, json};

use crate::model::state::{AppState, Conversation, ConversationStatus};
use crate::model::testing::FakeRpc;
use crate::ui::shell::route::{self, Route};
use crate::ui::store;
use crate::ui::testing::{app, bot, click, draw, node, state};

fn message(id: &str, author: &str, body: &str, minute: usize) -> Message {
    Message {
        id: id.into(),
        conversation_id: "b1".into(),
        author: author.into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status: MessageStatus::Done,
        created_at: format!(
            "2026-11-15T{:02}:{:02}:00.000Z",
            8 + minute / 60,
            minute % 60
        ),
    }
}

fn with(kind: MessageKind, data: Value, mut message: Message) -> Message {
    message.kind = kind;
    message.data = data.as_object().cloned();
    message
}

/// A turn of `count` messages, yours and Forge's in turn.
fn turns(count: usize) -> Vec<Message> {
    (0..count)
        .map(|n| {
            let author = if n % 2 == 0 { "you" } else { "b1" };
            message(&format!("m{n}"), author, &format!("Message {n}"), n)
        })
        .collect()
}

/// Forge's chat, loaded with `messages`.
fn chat(messages: Vec<Message>, has_more: bool) -> AppState {
    let mut state = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    state.conversations.insert(
        "b1".into(),
        Conversation {
            status: ConversationStatus::Ready,
            messages,
            has_more,
            pending: Vec::new(),
        },
    );
    state
}

fn calls_to(rpc: &FakeRpc, name: &str) -> Vec<Value> {
    rpc.calls()
        .into_iter()
        .filter(|(method, _)| *method == name)
        .map(|(_, params)| params)
        .collect()
}

#[gpui::test]
fn opens_on_the_newest_page_and_loads_it_once(cx: &mut TestAppContext) {
    let page = turns(3);
    let rpc = FakeRpc::new().on::<method::ChatHistory>(move |_| {
        Ok(MessagePage {
            messages: page.clone(),
            has_more: false,
        })
    });
    let ready = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    let (cx, calls) = app(cx, Route::Chat("b1".into()), ready, rpc);
    draw(cx);
    draw(cx);
    assert!(node("bubble.m0").is_some(), "your first message shows");
    assert!(node("prose.m1").is_some(), "Forge's reply shows");
    let history = calls_to(&calls, "chat.history");
    assert_eq!(history.len(), 1, "one load: {history:?}");
    assert_eq!(history[0]["botId"], "b1");
    assert!(history[0].get("before").is_none(), "the newest page");
}

#[gpui::test]
fn a_long_thread_sits_at_the_bottom_and_draws_only_what_shows(cx: &mut TestAppContext) {
    let (cx, _) = app(
        cx,
        Route::Chat("b1".into()),
        chat(turns(300), false),
        FakeRpc::new(),
    );
    draw(cx);
    assert!(node("bubble.m298").is_some(), "the newest of yours shows");
    assert!(node("prose.m299").is_some(), "and the reply under it");
    assert!(node("bubble.m0").is_none(), "the oldest isn't drawn");
    let screen = cx.debug_bounds("screen.chat").unwrap();
    let last = cx.debug_bounds("prose.m299").unwrap();
    assert!(
        screen.bottom() - last.bottom() < px(48.),
        "bottom-anchored: {last:?} in {screen:?}"
    );
}

#[gpui::test]
fn your_own_message_always_comes_into_view(cx: &mut TestAppContext) {
    let (cx, _) = app(
        cx,
        Route::Chat("b1".into()),
        chat(turns(300), false),
        FakeRpc::new(),
    );
    draw(cx);
    let thread = cx.debug_bounds("screen.chat").unwrap().center();
    cx.simulate_event(ScrollEvent {
        position: thread,
        delta: ScrollDelta::Pixels(point(px(0.), px(6000.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    draw(cx);
    assert!(
        node("prose.m299").is_none(),
        "scrolled up, away from the newest"
    );
    cx.update(|_, cx| {
        store::update(cx, |state| {
            let messages = &mut state.conversations.get_mut("b1").unwrap().messages;
            messages.push(message("m300", "b1", "A reply you scrolled away from", 300));
            true
        });
    });
    draw(cx);
    assert!(
        node("prose.m300").is_none(),
        "an agent's message doesn't pull you down"
    );
    cx.update(|_, cx| {
        store::update(cx, |state| {
            let messages = &mut state.conversations.get_mut("b1").unwrap().messages;
            messages.push(message("m301", "you", "Also run clippy.", 301));
            true
        });
    });
    draw(cx);
    draw(cx);
    assert!(
        node("bubble.m301").is_some(),
        "your own message brings the thread to the bottom"
    );
}

#[gpui::test]
fn the_older_page_loads_when_the_top_is_in_reach(cx: &mut TestAppContext) {
    let older = vec![message("m-1", "b1", "An older reply", 0)];
    let rpc = FakeRpc::new().on::<method::ChatHistory>(move |_| {
        Ok(MessagePage {
            messages: older.clone(),
            has_more: false,
        })
    });
    let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(4), true), rpc);
    draw(cx);
    draw(cx);
    draw(cx);
    let history = calls_to(&calls, "chat.history");
    assert_eq!(history.len(), 1, "one older page: {history:?}");
    assert_eq!(
        history[0]["before"], "m0",
        "the page before the oldest message"
    );
    assert!(node("prose.m-1").is_some(), "the older reply shows above");
}

#[gpui::test]
fn an_empty_chat_says_the_agent_is_ready(cx: &mut TestAppContext) {
    let _ = app(
        cx,
        Route::Chat("b1".into()),
        chat(Vec::new(), false),
        FakeRpc::new(),
    );
    assert_eq!(node("chat.empty").unwrap().text, "Forge is ready");
}

#[gpui::test]
fn a_chat_that_didnt_load_offers_retry(cx: &mut TestAppContext) {
    let ready = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    let (cx, calls) = app(cx, Route::Chat("b1".into()), ready, FakeRpc::new());
    draw(cx);
    assert_eq!(
        node("chat.failed.text").unwrap().text,
        "Couldn't load this chat."
    );
    click(cx, "chat.failed.retry");
    draw(cx);
    assert_eq!(calls_to(&calls, "chat.history").len(), 2, "loaded again");
}

#[gpui::test]
fn a_deleted_agent_has_no_chat(cx: &mut TestAppContext) {
    let ready = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    let (cx, _) = app(cx, Route::Chat("gone".into()), ready, FakeRpc::new());
    draw(cx);
    assert_eq!(cx.update(|_, cx| route::current(cx)), Route::Agents);
}

#[gpui::test]
fn a_reply_says_thinking_until_its_first_word_and_a_blank_one_shows_nothing(
    cx: &mut TestAppContext,
) {
    let mut typing = message("m1", "b1", "", 1);
    typing.status = MessageStatus::Streaming;
    let mut blank = message("m2", "b1", "\n\n", 2);
    blank.status = MessageStatus::Done;
    let (cx, _) = app(
        cx,
        Route::Chat("b1".into()),
        chat(vec![message("m0", "you", "Hi", 0), typing, blank], false),
        FakeRpc::new(),
    );
    assert!(
        node("thinking.m1").is_some(),
        "a reply with no words yet says Thinking"
    );
    assert!(
        node("prose.m2").is_none() && node("thinking.m2").is_none(),
        "a blank reply shows nothing"
    );
    cx.update(|_, cx| {
        store::update(cx, |state| {
            let messages = &mut state.conversations.get_mut("b1").unwrap().messages;
            messages[1].body = "**Done.**".into();
            true
        });
    });
    draw(cx);
    assert!(
        node("prose.m1").is_some() && node("thinking.m1").is_none(),
        "its words replace Thinking"
    );
}

#[gpui::test]
fn an_error_with_retry_runs_the_work_again(cx: &mut TestAppContext) {
    let error = with(
        MessageKind::Error,
        json!({ "retryMessageId": "m0" }),
        message("m1", "system", "Codex stopped unexpectedly.", 1),
    );
    let rpc = FakeRpc::new().on::<method::ChatRetry>(|_| Ok(message("m0", "you", "Hi", 0)));
    let (cx, calls) = app(
        cx,
        Route::Chat("b1".into()),
        chat(vec![message("m0", "you", "Hi", 0), error], false),
        rpc,
    );
    click(cx, "error.m1.retry");
    draw(cx);
    let retries = calls_to(&calls, "chat.retry");
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0]["messageId"], "m0");
}

#[gpui::test]
fn a_room_answer_opens_its_room(cx: &mut TestAppContext) {
    let answer = with(
        MessageKind::Handoff,
        json!({ "roomMessageId": "p1", "roomId": "r 1" }),
        message("m1", "b1", "Answered in Growth", 1),
    );
    let (cx, _) = app(
        cx,
        Route::Chat("b1".into()),
        chat(vec![answer], false),
        FakeRpc::new(),
    );
    assert_eq!(node("handoff.m1").unwrap().text, "Answered in Growth");
    click(cx, "handoff.m1");
    assert_eq!(
        cx.update(|_, cx| route::current(cx)),
        Route::Room("r 1".into())
    );
}

#[gpui::test]
fn pause_asks_the_daemon(cx: &mut TestAppContext) {
    let paused = bot("b1", "Forge", BotState::Paused);
    let rpc = FakeRpc::new().on::<method::BotsPause>(move |_| Ok(paused.clone()));
    let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), rpc);
    assert_eq!(node("chat.b1.name").unwrap().text, "Forge");
    click(cx, "chat.b1.pause");
    draw(cx);
    let pauses = calls_to(&calls, "bots.pause");
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0], json!({ "id": "b1", "paused": true }));
}
