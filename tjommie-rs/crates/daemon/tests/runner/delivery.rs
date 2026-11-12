//! "Runner destinations and delivery" and "Runner steering boundaries".

use super::*;
use daemon::chat::{NewMessage, add_message};
use daemon::queue::{enqueue, new_chain, set_status};
use protocol::model::{MessageKind, MessageStatus};
use protocol::wire::Notify;

fn add_exchange(h: &Harness, id: &str) {
    h.db()
        .execute(
            "INSERT INTO conversations (id, kind) VALUES (?1, 'exchange')",
            [id],
        )
        .unwrap();
}

fn you(h: &Harness, conversation_id: &str, body: &str, status: MessageStatus) -> Message {
    let new = NewMessage {
        conversation_id: conversation_id.into(),
        author: "you".into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status,
    };
    add_message(&h.db(), &new).unwrap()
}

#[tokio::test]
async fn starts_prequeued_work_only_when_delivered_without_duplicating_it_and_uses_its_persisted_destination_and_hooks()
 {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let other = h.bot(json!({}));
    let chain = new_chain(&h.db()).unwrap();
    let request = NewWork {
        chain_id: Some(chain),
        exchange_id: Some("exchange-1".into()),
        ..work_in(&bot.id, WorkSource::BotRequest, "Persisted request", "team")
    };
    let queued = enqueue(&h.db(), &request).unwrap();
    settle().await;
    assert_eq!(h.session_count(), 0);
    assert_eq!(lifecycle.started(), Vec::<String>::new());
    assert_eq!(h.status(&queued.id), WorkStatus::Queued);

    let delivered = h
        .runner
        .deliver(WorkItem {
            bot_id: other.id.clone(),
            text: "Stale request".into(),
            ..queued.clone()
        })
        .unwrap();
    assert_eq!(delivered, queued);
    assert_eq!(
        h.runner.current_work(&bot.id).map(|item| item.id),
        Some(queued.id.clone())
    );
    assert_eq!(h.runner.current_work(&other.id), None);
    wait_for(|| h.sent(0) > 0).await;
    h.runner.deliver(queued.clone()).unwrap();
    assert_eq!(h.count("SELECT COUNT(*) FROM work_items"), 1);
    assert_eq!(h.session(0).sent(), ["Persisted request"]);
    assert_eq!(lifecycle.started(), std::slice::from_ref(&queued.id));
    h.session(0).emit(text("The answer"));
    h.session(0).emit(ProviderEvent::TextEnd);
    h.session(0).emit(turn_done(true, None));
    settle().await;
    let ended = lifecycle.ended();
    assert_eq!(ended.len(), 1);
    let (item, result) = &ended[0];
    assert_eq!(
        (
            item.id.as_str(),
            item.conversation_id.as_str(),
            item.exchange_id.as_deref()
        ),
        (queued.id.as_str(), "team", Some("exchange-1"))
    );
    assert_eq!(
        (result.outcome, result.text.as_deref()),
        (TurnOutcome::Done, Some("The answer"))
    );
    assert_eq!(h.status(&queued.id), WorkStatus::Done);
    assert_eq!(h.count("SELECT COUNT(*) FROM work_items"), 1);
}

#[tokio::test]
async fn writes_text_deltas_activity_errors_and_stop_to_the_destination() {
    for destination in ["chat", "team", "exchange"] {
        let h = harness();
        let bot = h.bot(json!({}));
        let conversation_id = if destination == "chat" {
            bot.id.clone()
        } else {
            destination.to_owned()
        };
        if destination == "exchange" {
            add_exchange(&h, &conversation_id);
        }
        let request = NewWork {
            chain_id: Some("chain-a".into()),
            exchange_id: Some("request-a".into()),
            ..work_in(
                &bot.id,
                WorkSource::BotRequest,
                "answer here",
                &conversation_id,
            )
        };
        let first = h.runner.deliver(request).unwrap();
        assert_eq!(
            (
                first.bot_id.as_str(),
                first.conversation_id.as_str(),
                first.chain_id.as_deref(),
                first.exchange_id.as_deref()
            ),
            (
                bot.id.as_str(),
                conversation_id.as_str(),
                Some("chain-a"),
                Some("request-a")
            )
        );
        assert_eq!(
            h.runner.current_work(&bot.id).map(|item| item.id),
            Some(first.id.clone())
        );
        wait_for(|| h.sent(0) > 0).await;
        let session = h.session(0);
        session.emit(text("A thought"));
        session.emit(ProviderEvent::TextEnd);
        session.emit(tool("read", "Read", "Read notes.md"));
        session.emit(tool_done("read", true));
        session.emit(text("unfinished answer"));
        session.emit(turn_done(false, Some("answer failed")));
        settle().await;

        assert_eq!(h.runner.current_work(&bot.id), None);
        assert_eq!(h.status(&first.id), WorkStatus::Failed);
        let rows: Vec<(String, MessageKind, String, MessageStatus)> = h
            .history(&conversation_id)
            .into_iter()
            .map(|message| (message.author, message.kind, message.body, message.status))
            .collect();
        for expected in [
            (
                bot.id.clone(),
                MessageKind::Text,
                "A thought".to_owned(),
                MessageStatus::Done,
            ),
            (
                bot.id.clone(),
                MessageKind::Activity,
                "1 step: Read notes.md".to_owned(),
                MessageStatus::Done,
            ),
            (
                bot.id.clone(),
                MessageKind::Text,
                "unfinished answer".to_owned(),
                MessageStatus::Failed,
            ),
            (
                "system".to_owned(),
                MessageKind::Error,
                "answer failed".to_owned(),
                MessageStatus::Done,
            ),
        ] {
            assert!(
                rows.contains(&expected),
                "{destination}: {expected:?} in {rows:?}"
            );
        }
        let deltas: Vec<String> = h
            .events()
            .into_iter()
            .filter_map(|event| match event {
                Event::MessageDelta(delta) => Some(delta.conversation_id),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, [conversation_id.clone(), conversation_id.clone()]);

        let second = h
            .runner
            .deliver(work_in(
                &bot.id,
                WorkSource::Routine,
                "stop here",
                &conversation_id,
            ))
            .unwrap();
        wait_for(|| session.sent().len() == 2).await;
        h.runner.stop(&bot.id).await.unwrap();
        session.emit(turn_done(false, None));
        settle().await;
        assert_eq!(h.status(&second.id), WorkStatus::Interrupted);
        let last = h.history(&conversation_id).pop().unwrap();
        assert_eq!(
            (last.kind, last.body.as_str()),
            (MessageKind::Notice, "Stopped.")
        );
        if destination != "chat" {
            assert_eq!(h.history(&bot.id), Vec::<Message>::new());
        }
    }
}

#[tokio::test]
async fn keeps_approvals_and_usage_limit_notices_in_the_bots_chat_while_writing_elsewhere() {
    for conversation_id in ["team", "exchange"] {
        let h = harness();
        let bot = h.bot(json!({}));
        if conversation_id == "exchange" {
            add_exchange(&h, conversation_id);
        }
        h.runner
            .deliver(work_in(
                &bot.id,
                WorkSource::BotRequest,
                "write",
                conversation_id,
            ))
            .unwrap();
        wait_for(|| h.sent(0) > 0).await;
        let session = h.session(0);
        session.emit(text("Working here"));
        session.emit(approval("approval-1", "Write a file", None));
        session.emit(limit_reached(Some(FAR_RESET)));
        settle().await;
        let chat = h.history(&bot.id);
        assert!(
            chat.iter()
                .any(|message| message.kind == MessageKind::Approval
                    && message.body == "Write a file")
        );
        assert!(
            chat.iter()
                .any(|message| message.kind == MessageKind::Notice
                    && message.body.contains("hit its usage limit"))
        );
        let elsewhere: Vec<(MessageKind, String)> = h
            .history(conversation_id)
            .into_iter()
            .map(|message| (message.kind, message.body))
            .collect();
        assert_eq!(elsewhere, [(MessageKind::Text, "Working here".to_owned())]);
        let notices: Vec<Notify> = h
            .events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Notify(notify) => Some(notify),
                _ => None,
            })
            .collect();
        let card = chat
            .iter()
            .find(|message| message.kind == MessageKind::Approval)
            .unwrap();
        let approval_id = card.data.as_ref().unwrap()["approvalId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            matches!(&notices[..], [
            Notify::Approval { bot_id: first, approval_id: Some(id), .. },
            Notify::Limit { bot_id: second, .. },
        ] if *first == bot.id && *second == bot.id && *id == approval_id),
            "{notices:?}"
        );
        session.emit(turn_done(true, None));
        settle().await;
    }
}

#[tokio::test]
async fn starts_a_separate_chain_for_every_submitted_message_and_every_retry() {
    let h = harness_with(Setup {
        signed_in: false,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let first = h.runner.submit(&bot.id, "first", &[]).unwrap();
    let second = h.runner.submit(&bot.id, "second", &[]).unwrap();
    let first_item = h.item_for_message(&first.id);
    let second_item = h.item_for_message(&second.id);
    set_status(&h.db(), &first_item.id, WorkStatus::Failed).unwrap();
    h.runner.retry(&first.id, None).unwrap();
    let retried = h.item_for_message(&first.id);
    let chains = [
        first_item.chain_id.clone(),
        second_item.chain_id.clone(),
        retried.chain_id.clone(),
    ];
    assert!(
        chains
            .iter()
            .all(|id| id.as_deref().is_some_and(|id| !id.is_empty()))
    );
    assert_eq!(
        chains
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    assert_eq!(
        h.count("SELECT count(*) FROM chains WHERE requests_made = 0"),
        3
    );
    assert_eq!(
        (
            retried.conversation_id.as_str(),
            retried.source,
            retried.text.as_str(),
            retried.message_id.as_deref()
        ),
        (
            bot.id.as_str(),
            WorkSource::You,
            "first",
            Some(first.id.as_str())
        )
    );
}

#[tokio::test]
async fn retry_preserves_the_prior_destination_and_routes_through_delivery() {
    let h = harness();
    let bot = h.bot(json!({}));
    let message = you(&h, "team", "room retry", MessageStatus::Done);
    let failed = enqueue(
        &h.db(),
        &NewWork {
            message_id: Some(message.id.clone()),
            ..work_in(&bot.id, WorkSource::Room, "room retry", "team")
        },
    )
    .unwrap();
    set_status(&h.db(), &failed.id, WorkStatus::Failed).unwrap();
    h.runner.retry(&message.id, None).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    assert_eq!(h.message(&message.id), message);
    let current = h.runner.current_work(&bot.id).unwrap();
    assert_eq!(
        (
            current.conversation_id.as_str(),
            current.source,
            current.message_id.as_deref()
        ),
        ("team", WorkSource::Room, Some(message.id.as_str()))
    );
    h.session(0).emit(text("ROOM"));
    h.session(0).emit(turn_done(true, None));
    settle().await;
    let last = h.history("team").pop().unwrap();
    assert_eq!(
        (last.body.as_str(), last.status),
        ("ROOM", MessageStatus::Done)
    );
}

#[tokio::test]
async fn retry_preserves_a_failed_bot_requests_exchange_link_for_the_completion_hook_but_starts_a_new_chain_without_the_old_routine_run()
 {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let exchange_id = "exchange-to-retry";
    add_exchange(&h, exchange_id);
    let message = add_message(
        &h.db(),
        &NewMessage {
            conversation_id: exchange_id.into(),
            author: bot.id.clone(),
            kind: MessageKind::Text,
            body: "Please help".into(),
            data: None,
            status: MessageStatus::Failed,
        },
    )
    .unwrap();
    let original_chain = new_chain(&h.db()).unwrap();
    let request = NewWork {
        message_id: Some(message.id.clone()),
        exchange_id: Some(exchange_id.into()),
        chain_id: Some(original_chain.clone()),
        routine_run_id: Some("original-run".into()),
        ..work_in(&bot.id, WorkSource::BotRequest, "Please help", exchange_id)
    };
    let failed = enqueue(&h.db(), &request).unwrap();
    set_status(&h.db(), &failed.id, WorkStatus::Failed).unwrap();

    h.runner.retry(&message.id, None).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let retried = h.item_for_message(&message.id);
    assert_eq!(
        (
            retried.bot_id.as_str(),
            retried.source,
            retried.text.as_str(),
            retried.message_id.as_deref()
        ),
        (
            bot.id.as_str(),
            WorkSource::BotRequest,
            "Please help",
            Some(message.id.as_str())
        )
    );
    assert_eq!(
        (
            retried.conversation_id.as_str(),
            retried.exchange_id.as_deref(),
            retried.routine_run_id.as_deref(),
            retried.status
        ),
        (exchange_id, Some(exchange_id), None, WorkStatus::Running)
    );
    assert_ne!(retried.id, failed.id);
    assert!(retried.chain_id.is_some());
    assert_ne!(retried.chain_id.as_deref(), Some(original_chain.as_str()));
    assert_eq!(h.count("SELECT count(*) FROM chains"), 2);
    assert_eq!(h.status(&failed.id), WorkStatus::Failed);
    h.session(0).emit(text("Here is the help"));
    h.session(0).emit(turn_done(true, None));
    settle().await;
    let ended = lifecycle.ended();
    assert_eq!(ended.len(), 1);
    let (item, result) = &ended[0];
    assert_eq!(
        (
            item.id.as_str(),
            item.conversation_id.as_str(),
            item.exchange_id.as_deref(),
            item.chain_id.clone(),
            item.routine_run_id.as_deref()
        ),
        (
            retried.id.as_str(),
            exchange_id,
            Some(exchange_id),
            retried.chain_id.clone(),
            None
        )
    );
    assert_eq!(
        (
            result.outcome,
            result.text.as_deref(),
            result.error.as_deref()
        ),
        (TurnOutcome::Done, Some("Here is the help"), None)
    );
    let last = h.last(exchange_id);
    assert_eq!(
        (last.body.as_str(), last.status),
        ("Here is the help", MessageStatus::Done)
    );
}

#[tokio::test]
async fn stop_item_only_interrupts_its_current_item_and_resets_its_custom_notice_before_later_work()
{
    let h = harness();
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(NewWork {
            routine_run_id: Some("run-1".into()),
            ..work_in(&bot.id, WorkSource::Routine, "first", "team")
        })
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let session = h.session(0);
    drop(h.runner.stop_item("another-item", "Wrong notice"));
    assert_eq!(session.interrupts(), 0);
    let current = h.runner.current_work(&bot.id).unwrap();
    assert_eq!(
        (current.id.as_str(), current.routine_run_id.as_deref()),
        (item.id.as_str(), Some("run-1"))
    );
    drop(h.runner.stop_item(&item.id, "Routine time limit reached."));
    assert_eq!(session.interrupts(), 1);
    session.emit(turn_done(false, None));
    settle().await;
    assert_eq!(h.runner.current_work(&bot.id), None);
    assert_eq!(h.last("team").body, "Routine time limit reached.");

    let later = h
        .runner
        .deliver(work_in(&bot.id, WorkSource::Routine, "second", "team"))
        .unwrap();
    wait_for(|| session.sent().len() == 2).await;
    drop(h.runner.stop_item(&item.id, "Stale timer"));
    assert_eq!(session.interrupts(), 1);
    assert_eq!(
        h.runner.current_work(&bot.id).map(|work| work.id),
        Some(later.id)
    );
    h.runner.stop(&bot.id).await.unwrap();
    session.emit(exit(None));
    settle().await;
    assert_eq!(h.last("team").body, "Stopped.");
}

#[tokio::test]
async fn steers_only_same_conversation_messages_and_replies_past_older_non_steerable_queue_entries()
{
    let session = FakeSession::new();
    let (sending, release_send) = gate();
    session.set(|state| state.send = sending);
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let running = h
        .runner
        .deliver(work_in(&bot.id, WorkSource::Room, "running", "team"))
        .unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    let room = h
        .runner
        .deliver(work_in(&bot.id, WorkSource::Room, "room waits", "team"))
        .unwrap();
    let request = h
        .runner
        .deliver(work_in(
            &bot.id,
            WorkSource::BotRequest,
            "request waits",
            "team",
        ))
        .unwrap();
    let routine = h
        .runner
        .deliver(work_in(
            &bot.id,
            WorkSource::Routine,
            "routine waits",
            "team",
        ))
        .unwrap();
    let chat = h.runner.submit(&bot.id, "chat waits", &[]).unwrap();
    let reply_elsewhere = h
        .runner
        .deliver(work_in(
            &bot.id,
            WorkSource::BotReply,
            "other reply waits",
            &bot.id,
        ))
        .unwrap();
    let reply = h
        .runner
        .deliver(work_in(&bot.id, WorkSource::BotReply, "reply now", "team"))
        .unwrap();
    let message = you(&h, "team", "steer now", MessageStatus::Done);
    let yours = h
        .runner
        .deliver(NewWork {
            message_id: Some(message.id.clone()),
            ..work_in(&bot.id, WorkSource::You, "steer now", "team")
        })
        .unwrap();
    assert_eq!(session.steered(), Vec::<String>::new());
    release_send.open(Ok(()));
    wait_for(|| session.steered().len() == 2).await;
    assert_eq!(session.steered(), ["reply now", "steer now"]);
    assert_eq!(h.status(&reply.id), WorkStatus::Done);
    assert_eq!(h.status(&yours.id), WorkStatus::Done);
    assert_eq!(h.message(&message.id).status, MessageStatus::Done);
    for item in [room, request, routine, reply_elsewhere] {
        assert_eq!(h.status(&item.id), WorkStatus::Queued, "{}", item.text);
    }
    assert_eq!(h.message(&chat.id).status, MessageStatus::Queued);
    assert_eq!(
        h.runner.current_work(&bot.id).map(|item| item.id),
        Some(running.id)
    );
}

#[tokio::test]
async fn queues_a_declined_steer_and_logs_only_an_unexpected_rejection() {
    for unexpected in [false, true] {
        let session = FakeSession::new();
        session.set(|state| {
            state.steer_result = false;
            if unexpected {
                state.steer_error = Some("steer transport failed".into());
            }
        });
        let h = harness_with(Setup {
            factory: single(&session),
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        h.runner.submit(&bot.id, "first", &[]).unwrap();
        wait_for(|| !session.sent().is_empty()).await;
        settle().await;
        let second = h.runner.submit(&bot.id, "second", &[]).unwrap();
        wait_for(|| !session.steered().is_empty()).await;
        settle().await;
        assert_eq!(h.item_for_message(&second.id).status, WorkStatus::Queued);
        assert_eq!(h.message(&second.id).status, MessageStatus::Queued);
        if unexpected {
            assert!(h.logged("steer transport failed"));
        } else {
            assert!(h.no_logs(), "{:?}", recorded(&h.logs));
        }
        session.emit(turn_done(true, None));
        wait_for(|| session.sent().len() == 2).await;
        assert_eq!(session.sent()[1], "second");
        assert_eq!(h.item_for_message(&second.id).status, WorkStatus::Running);
    }
}
