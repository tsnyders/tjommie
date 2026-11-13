mod m4_runner_support;
use m4_runner_support::*;

use std::sync::Arc;

use daemon::bots::{StoredBot, delete_bot};
use daemon::queue::{WorkItem, WorkSource, WorkStatus};
use daemon::rooms::{create_room, delete_room};
use daemon::team::{HostAccess, get_exchange, with_host};
use protocol::ErrorCode;
use protocol::model::{
    Exchange, ExchangeHandoffData, ExchangeStatus, Message, MessageKind, RoomInput, TEAM_ROOM_ID,
};
use serde_json::json;

/// The TypeScript `beforeEach`: Alice may message Bob, and Bob may message Carol.
fn three_bots(h: &Harness) -> (StoredBot, StoredBot, StoredBot) {
    let (alice, bob, carol) = (h.bot("Alice"), h.bot("Bob"), h.bot("Carol"));
    let alice = h.set_can_message(&alice.id, &[bob.id.as_str()]);
    let bob = h.set_can_message(&bob.id, &[carol.id.as_str()]);
    (alice, bob, carol)
}

/// `runner.submit`: the person's message to a bot.
fn submit(h: &Harness, bot: &StoredBot, text: &str) {
    h.runner.submit(&bot.id, text, &[]).unwrap();
}

/// `team.postToRoom`, run on the runner's host.
fn post_to_room(h: &Harness, room_id: &str, text: &str) -> Message {
    with_host(&h.runner, |host| {
        h.m4.team.post_to_room(host, room_id, text)
    })
    .unwrap()
}

/// `ask(from, to, message)`: the `message_bot` tool, called as `from`. The tool wakes the runner for the request it
/// queued, so the newest work row is what TypeScript's `delivered.at(-1)` gave.
async fn ask(h: &Harness, from: &StoredBot, to: &StoredBot, message: &str) -> WorkItem {
    let access: Arc<dyn HostAccess> = Arc::new(h.runner.clone());
    let tools = h.m4.team.tools(access);
    let tool = tools
        .iter()
        .find(|tool| tool.name == "message_bot")
        .expect("message_bot is one of the team's tools");
    tool.call(&from.id, &json!({ "bot": to.name, "message": message }))
        .await
        .expect("message_bot sends the request");
    h.latest_work()
}

fn exchange(h: &Harness, id: &str) -> Exchange {
    get_exchange(&h.conn(), id).unwrap()
}

fn exchange_id(item: &WorkItem) -> &str {
    item.exchange_id
        .as_deref()
        .expect("the item belongs to an exchange")
}

/// The fields a TypeScript `toMatchObject` names on a work item. A `None` field is not checked.
#[derive(Default)]
struct Shape<'a> {
    bot: Option<&'a str>,
    source: Option<WorkSource>,
    conversation: Option<&'a str>,
    exchange: Option<&'a str>,
    chain: Option<&'a str>,
}

fn assert_work(item: &WorkItem, shape: Shape<'_>) {
    if let Some(bot) = shape.bot {
        assert_eq!(item.bot_id, bot);
    }
    if let Some(source) = shape.source {
        assert_eq!(item.source, source);
    }
    if let Some(conversation) = shape.conversation {
        assert_eq!(item.conversation_id, conversation);
    }
    if let Some(exchange) = shape.exchange {
        assert_eq!(item.exchange_id.as_deref(), Some(exchange));
    }
    if let Some(chain) = shape.chain {
        assert_eq!(item.chain_id.as_deref(), Some(chain));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_queue_a_retry_for_a_deleted_custom_room() {
    let h = harness(true);
    let alice = h.bot("Alice");
    let room = create_room(
        &h.conn(),
        &RoomInput {
            name: "Planning".into(),
            bot_ids: vec![alice.id.clone()],
        },
    )
    .unwrap();
    let post = post_to_room(&h, &room.id, "@Alice prepare a report");
    let turn = h.first_work_for(&post.id);
    let session = h.sent(&alice.id, 1).await;
    session.emit(turn_done(false, Some("Temporary failure")));
    h.await_status(&turn.id, WorkStatus::Failed).await;
    delete_room(&h.conn(), &room.id).unwrap();
    let before = h.count_work_for(&post.id);
    let error = h
        .runner
        .retry(&post.id, Some(alice.id.as_str()))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(h.count_work_for(&post.id), before);
    assert!(h.runner.current_work(&alice.id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_a_custom_room_reply_there_and_links_to_it_from_the_bot_chat() {
    let h = harness(true);
    let alice = h.bot("Alice");
    let room = create_room(
        &h.conn(),
        &RoomInput {
            name: "Planning".into(),
            bot_ids: vec![alice.id.clone()],
        },
    )
    .unwrap();
    let post = post_to_room(&h, &room.id, "@Alice prepare a report");
    let turn = h.first_work_for(&post.id);
    let session = h.sent(&alice.id, 1).await;
    session.finish("Report ready");
    h.await_status(&turn.id, WorkStatus::Done).await;
    let answer = h.chat(&room.id, 10).pop().unwrap();
    assert_eq!(answer.author, alice.id);
    assert_eq!(answer.body, "Report ready");
    let linked = h.chat(&alice.id, 10).iter().any(|message| {
        message.kind == MessageKind::Handoff
            && message.body == "Answered in Planning"
            && data_of(message) == json!({ "roomId": room.id, "roomMessageId": answer.id })
    });
    assert!(linked, "Alice's chat links to the room answer");
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_nested_replies_into_the_asking_exchange_and_sends_its_later_answer_as_a_follow_up()
{
    let h = harness(true);
    let (alice, bob, carol) = three_bots(&h);
    submit(&h, &alice, "Prepare a report");
    let alice_session = h.sent(&alice.id, 1).await;
    let first = ask(&h, &alice, &bob, "Write the report").await;
    let bob_session = h.sent(&bob.id, 1).await;
    let nested = ask(&h, &bob, &carol, "Find a source").await;
    let carol_session = h.sent(&carol.id, 1).await;
    assert_eq!(nested.chain_id, first.chain_id);

    bob_session.finish("First draft");
    h.await_status(&first.id, WorkStatus::Done).await;
    let initial_reply = h.latest_work();
    assert_work(
        &initial_reply,
        Shape {
            bot: Some(alice.id.as_str()),
            source: Some(WorkSource::BotReply),
            conversation: Some(alice.id.as_str()),
            exchange: Some(exchange_id(&first)),
            ..Shape::default()
        },
    );
    wait_until("Alice's session to steer the first reply in", || {
        alice_session.steered().len() == 1
    })
    .await;

    carol_session.finish("Verified source");
    h.await_status(&nested.id, WorkStatus::Done).await;
    let nested_reply = h.latest_work();
    assert_work(
        &nested_reply,
        Shape {
            bot: Some(bob.id.as_str()),
            source: Some(WorkSource::BotReply),
            conversation: Some(first.conversation_id.as_str()),
            exchange: Some(exchange_id(&nested)),
            chain: first.chain_id.as_deref(),
        },
    );
    h.sent(&bob.id, 2).await;
    let bob_messages = bob_session.sent();
    assert!(bob_messages[1].contains("Carol replied"));
    assert!(bob_messages[1].contains("Verified source"));
    bob_session.finish("Report with the verified source");
    h.await_status(&nested_reply.id, WorkStatus::Done).await;
    let follow_up = h.latest_work();
    assert_work(
        &follow_up,
        Shape {
            bot: Some(alice.id.as_str()),
            source: Some(WorkSource::BotReply),
            conversation: Some(alice.id.as_str()),
            exchange: Some(exchange_id(&first)),
            chain: first.chain_id.as_deref(),
        },
    );
    assert!(follow_up.text.contains("Bob followed up"));
    wait_until("Alice's session to steer the follow-up in", || {
        alice_session.steered().len() == 2
    })
    .await;
    assert!(alice_session.steered()[1].contains("Report with the verified source"));
    let first_row = exchange(&h, exchange_id(&first));
    assert_eq!(first_row.status, ExchangeStatus::Replied);
    assert_eq!(first_row.reply.as_deref(), Some("First draft"));
    let nested_row = exchange(&h, exchange_id(&nested));
    assert_eq!(nested_row.status, ExchangeStatus::Replied);
    assert_eq!(nested_row.reply.as_deref(), Some("Verified source"));
    let handoff = h
        .chat(&alice.id, 20)
        .into_iter()
        .find(|message| message.kind == MessageKind::Handoff)
        .expect("Alice's chat has the exchange's handoff row");
    let handoff: ExchangeHandoffData = serde_json::from_value(data_of(&handoff)).unwrap();
    assert_eq!(handoff.messages, 3);
    assert_eq!(handoff.status, ExchangeStatus::Replied);
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_the_original_requester_when_failed_exchange_work_is_retried_on_a_new_chain() {
    let h = harness(true);
    let (alice, bob, _carol) = three_bots(&h);
    submit(&h, &alice, "Prepare a report");
    let alice_session = h.sent(&alice.id, 1).await;
    let request = ask(&h, &alice, &bob, "Write the report").await;
    let bob_session = h.sent(&bob.id, 1).await;
    bob_session.emit(turn_done(false, Some("Temporary failure")));
    h.await_status(&request.id, WorkStatus::Failed).await;
    assert_eq!(
        exchange(&h, exchange_id(&request)).status,
        ExchangeStatus::Failed
    );
    wait_until("Alice's session to steer the failure in", || {
        alice_session.steered().len() == 1
    })
    .await;

    let message_id = request
        .message_id
        .clone()
        .expect("the request has a message");
    h.runner.retry(&message_id, None).unwrap();
    let retry = h
        .runner
        .current_work(&bob.id)
        .expect("the retry is running");
    assert_work(
        &retry,
        Shape {
            conversation: Some(request.conversation_id.as_str()),
            exchange: Some(exchange_id(&request)),
            ..Shape::default()
        },
    );
    assert_ne!(retry.chain_id, request.chain_id);
    h.sent(&bob.id, 2).await;
    bob_session.finish("Recovered report");
    h.await_status(&retry.id, WorkStatus::Done).await;
    let reply = h.latest_work();
    assert_work(
        &reply,
        Shape {
            bot: Some(alice.id.as_str()),
            source: Some(WorkSource::BotReply),
            conversation: Some(alice.id.as_str()),
            exchange: Some(exchange_id(&request)),
            ..Shape::default()
        },
    );
    assert!(reply.text.contains("Bob replied"));
    assert!(reply.text.contains("Recovered report"));
    wait_until("Alice's session to steer the recovered reply in", || {
        alice_session.steered().len() == 2
    })
    .await;
    let row = exchange(&h, exchange_id(&request));
    assert_eq!(row.status, ExchangeStatus::Replied);
    assert_eq!(row.reply.as_deref(), Some("Recovered report"));
    assert_eq!(h.status(&retry.id), WorkStatus::Done);
}

#[tokio::test(flavor = "multi_thread")]
async fn fails_a_deleted_requesters_queued_and_running_exchanges_without_reviving_them_on_late_output()
 {
    let h = harness(true);
    let (alice, bob, _carol) = three_bots(&h);
    submit(&h, &alice, "Prepare reports");
    h.sent(&alice.id, 1).await;
    let running = ask(&h, &alice, &bob, "First report").await;
    let bob_session = h.sent(&bob.id, 1).await;
    let queued = ask(&h, &alice, &bob, "Second report").await;
    assert_eq!(h.status(&running.id), WorkStatus::Running);
    assert_eq!(h.status(&queued.id), WorkStatus::Queued);

    let deleting = with_host(&h.runner, |host| h.m4.team.bot_deleting(host, &alice.id)).unwrap();
    delete_bot(&h.conn(), &alice.id).unwrap();
    with_host(&h.runner, |host| h.m4.team.bot_deleted(host, &deleting)).unwrap();
    h.runner.forget(&alice.id);
    assert_eq!(h.status(&queued.id), WorkStatus::Failed);
    assert_eq!(h.status(&running.id), WorkStatus::Running);
    assert_eq!(bob_session.interrupts(), 1);
    assert_eq!(
        exchange(&h, exchange_id(&queued)).status,
        ExchangeStatus::Failed
    );
    assert_eq!(
        exchange(&h, exchange_id(&running)).status,
        ExchangeStatus::Failed
    );
    // Late output after the requester is gone: the turn finishes, but nothing is revived or posted to the room.
    bob_session.finish("Finished for the room");
    h.await_status(&running.id, WorkStatus::Done).await;
    let settled = exchange(&h, exchange_id(&running));
    assert_eq!(settled.status, ExchangeStatus::Failed);
    assert_eq!(settled.reply.as_deref(), Some("Alice was deleted"));
    // TypeScript's `delivered` held every delivery the team made, replies included, so a revived reply would show here.
    // The person's own submit (`You`) is not a team delivery.
    let delivered = h
        .all_work()
        .iter()
        .filter(|item| item.source != WorkSource::You)
        .count();
    assert_eq!(delivered, 2);
    assert_eq!(bob_session.sent().len(), 1);
    assert!(
        !h.chat(TEAM_ROOM_ID, 20)
            .iter()
            .any(|message| message.body == "Finished for the room")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_help_requested_from_the_team_room_back_there_and_records_the_later_room_answer_in_the_bots_chat()
 {
    let h = harness(true);
    let (alice, bob, _carol) = three_bots(&h);
    let room_post = post_to_room(&h, TEAM_ROOM_ID, "@Alice prepare a report");
    let room_turn = h.first_work_for(&room_post.id);
    let alice_session = h.sent(&alice.id, 1).await;
    let request = ask(&h, &alice, &bob, "Write the report").await;
    let bob_session = h.sent(&bob.id, 1).await;
    alice_session.finish("I have asked Bob for a report");
    h.await_status(&room_turn.id, WorkStatus::Done).await;
    bob_session.finish("The finished report");
    h.await_status(&request.id, WorkStatus::Done).await;
    let reply = h.latest_work();
    assert_work(
        &reply,
        Shape {
            bot: Some(alice.id.as_str()),
            source: Some(WorkSource::BotReply),
            conversation: Some(TEAM_ROOM_ID),
            exchange: Some(exchange_id(&request)),
            chain: room_turn.chain_id.as_deref(),
        },
    );
    h.sent(&alice.id, 2).await;
    alice_session.finish("Here is Bob's report");
    h.await_status(&reply.id, WorkStatus::Done).await;
    let answer = h.chat(TEAM_ROOM_ID, 20).pop().unwrap();
    assert_eq!(answer.author, alice.id);
    assert_eq!(answer.body, "Here is Bob's report");
    let handoffs: Vec<Message> = h
        .chat(&alice.id, 20)
        .into_iter()
        .filter(|message| message.body == "Answered in Everyone")
        .collect();
    assert_eq!(handoffs.len(), 2);
    assert_eq!(
        data_of(handoffs.last().unwrap())["roomMessageId"],
        json!(answer.id)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn steers_a_nested_reply_without_a_separate_turn_hook_or_duplicate_upstream_answer() {
    let h = harness(true);
    let (alice, bob, carol) = three_bots(&h);
    submit(&h, &alice, "Prepare a report");
    h.sent(&alice.id, 1).await;
    let first = ask(&h, &alice, &bob, "Write the report").await;
    let bob_session = h.sent(&bob.id, 1).await;
    let nested = ask(&h, &bob, &carol, "Find a source").await;
    let carol_session = h.sent(&carol.id, 1).await;
    carol_session.finish("The source");
    h.await_status(&nested.id, WorkStatus::Done).await;
    let steered = h.latest_work();
    assert_work(
        &steered,
        Shape {
            bot: Some(bob.id.as_str()),
            conversation: Some(first.conversation_id.as_str()),
            exchange: Some(exchange_id(&nested)),
            ..Shape::default()
        },
    );
    h.await_status(&steered.id, WorkStatus::Done).await;
    assert_eq!(bob_session.steered().len(), 1);
    assert!(
        !recorded(&h.started)
            .iter()
            .any(|item| item.id == steered.id)
    );
    assert!(!recorded(&h.ended).iter().any(|item| item.id == steered.id));
    let alice_replies = |h: &Harness| -> Vec<WorkItem> {
        h.all_work()
            .into_iter()
            .filter(|item| item.bot_id == alice.id && item.source == WorkSource::BotReply)
            .collect()
    };
    assert!(alice_replies(&h).is_empty());

    bob_session.finish("Report using the source");
    h.await_status(&first.id, WorkStatus::Done).await;
    let upstream = alice_replies(&h);
    assert_eq!(upstream.len(), 1);
    assert_work(
        &upstream[0],
        Shape {
            exchange: Some(exchange_id(&first)),
            conversation: Some(alice.id.as_str()),
            ..Shape::default()
        },
    );
    let ended_by_bob: Vec<String> = recorded(&h.ended)
        .iter()
        .filter(|item| item.bot_id == bob.id)
        .map(|item| item.id.clone())
        .collect();
    assert_eq!(ended_by_bob, vec![first.id.clone()]);
    let posted = h
        .chat(TEAM_ROOM_ID, 20)
        .iter()
        .filter(|message| message.author == bob.id && message.body == "Report using the source")
        .count();
    assert_eq!(posted, 1);
}
