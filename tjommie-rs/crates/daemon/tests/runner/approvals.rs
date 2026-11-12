//! "Runner approvals", "Runner linked messages" and "Runner.start".

use super::*;
use daemon::approvals::{NewApproval, create_approval, resolve_approval};
use daemon::bots::set_limited_until;
use daemon::chat::{MessagePatch, NewMessage, add_message, update_message};
use daemon::queue::{enqueue, set_status};
use daemon::runner::{TurnOutcome, TurnResult};
use protocol::model::{ApprovalDecision, ApprovalKind, ApprovalStatus, MessageKind, MessageStatus};
use serde_json::Map;

fn card_status(message: &Message) -> Option<Value> {
    message
        .data
        .as_ref()
        .and_then(|data| data.get("status").cloned())
}

fn object(value: Value) -> Map<String, Value> {
    let Value::Object(map) = value else {
        panic!("not an object")
    };
    map
}

#[tokio::test]
async fn finishes_and_continues_queued_work_when_an_approval_card_is_malformed() {
    for data in [
        None,
        Some(object(json!({ "corrupt": "missing approval fields" }))),
    ] {
        let h = harness();
        let bot = h.bot(json!({}));
        let first = h.runner.submit(&bot.id, "First", &[]).unwrap();
        wait_for(|| h.sent(0) > 0).await;
        let session = h.session(0);
        session.emit(approval("corrupted-card", "Write file", None));
        settle().await;
        let added = h.added_approvals().remove(0);
        update_message(
            &h.db(),
            &added.message_id,
            &MessagePatch {
                data: Some(data.clone()),
                ..MessagePatch::default()
            },
        )
        .unwrap();
        let next = h
            .runner
            .deliver(work_in(&bot.id, WorkSource::Room, "Next turn", "team"))
            .unwrap();
        session.emit(turn_done(true, None));
        wait_for(|| session.sent().len() == 2).await;
        assert_eq!(h.item_for_message(&first.id).status, WorkStatus::Done);
        assert_eq!(
            h.runner.current_work(&bot.id).map(|item| item.id),
            Some(next.id)
        );
        let mut expected = data.clone().unwrap_or_default();
        expected.insert("status".into(), json!("denied"));
        assert_eq!(h.message(&added.message_id).data, Some(expected));
        assert!(
            h.events()
                .iter()
                .any(|event| matches!(event, Event::ApprovalResolved(approval)
            if approval.id == added.id && approval.status == ApprovalStatus::Denied))
        );
        session.emit(turn_done(true, None));
        settle().await;
        assert_eq!(h.state(&bot.id), BotState::Idle);
    }
}

#[tokio::test]
async fn finishes_a_turn_when_one_approval_was_already_answered_and_expires_the_remaining_cards() {
    let ended: Recorded<TurnResult> = Arc::default();
    let record = ended.clone();
    let hooks = RunnerHooks {
        turn_ended: Some(Arc::new(move |_core, _item, result: &TurnResult| {
            record.lock().unwrap().push(result.clone());
            Ok(())
        })),
        ..RunnerHooks::default()
    };
    let h = harness_with(Setup {
        hooks,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "two approvals"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    for id in ["first", "second"] {
        h.session(0).emit(approval(id, id, None));
    }
    settle().await;
    let cards = h.added_approvals();
    resolve_approval(&h.db(), &cards[0].id, ApprovalDecision::Once).unwrap();
    h.session(0).emit(turn_done(true, None));
    settle().await;
    assert_eq!(h.status(&item.id), WorkStatus::Done);
    assert_eq!(h.state(&bot.id), BotState::Idle);
    assert_eq!(
        recorded(&ended)
            .iter()
            .map(|result| result.outcome)
            .collect::<Vec<_>>(),
        [TurnOutcome::Done]
    );
    assert_eq!(
        h.strings("SELECT status FROM approvals ORDER BY rowid"),
        ["allowed", "denied"]
    );
}

#[tokio::test]
async fn updates_the_card_and_broadcasts_the_resolved_approval_when_answered() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "write", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(approval("provider-a1", "Write hello.txt", Some("Write")));
    settle().await;
    let added = h.added_approvals().remove(0);
    assert_eq!(
        card_status(&h.message(&added.message_id)),
        Some(json!("pending"))
    );

    let resolved = h
        .runner
        .answer_approval(&added.id, ApprovalDecision::Always)
        .unwrap();
    assert_eq!(resolved.status, ApprovalStatus::Allowed);
    assert_eq!(
        session.answers(),
        [("provider-a1".to_owned(), ApprovalAnswer::Always)]
    );
    assert_eq!(
        card_status(&h.message(&added.message_id)),
        Some(json!("allowed"))
    );
    let events = h.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::MessageUpdated(message)
        if message.id == added.message_id && card_status(message) == Some(json!("allowed"))))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ApprovalResolved(approval)
        if approval.id == added.id && approval.status == ApprovalStatus::Allowed))
    );
}

#[tokio::test]
async fn updates_and_broadcasts_every_approval_card_expired_at_startup() {
    let h = harness_with(Setup {
        signed_in: false,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let card = add_message(
        &h.db(),
        &NewMessage {
            conversation_id: bot.id.clone(),
            author: bot.id.clone(),
            kind: MessageKind::Approval,
            body: "Write hello.txt".into(),
            data: Some(object(json!({ "approvalId": "a-restart", "tool": "Write", "detail": "{}", "rule": "Write", "status": "pending" }))),
            status: MessageStatus::Done,
        },
    )
    .unwrap();
    create_approval(
        &h.db(),
        &NewApproval {
            id: "a-restart".into(),
            bot_id: bot.id.clone(),
            kind: ApprovalKind::Tool,
            tool: "Write".into(),
            summary: "Write hello.txt".into(),
            detail: "{}".into(),
            rule: Some("Write".into()),
            payload: None,
            message_id: card.id.clone(),
        },
    )
    .unwrap();

    h.runner.start().unwrap();

    assert_eq!(card_status(&h.message(&card.id)), Some(json!("denied")));
    let events = h.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::MessageUpdated(message)
        if message.id == card.id && card_status(message) == Some(json!("denied"))))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ApprovalResolved(approval)
        if approval.id == "a-restart" && approval.status == ApprovalStatus::Denied))
    );
}

#[tokio::test]
async fn keeps_the_linked_room_or_routine_message_done_through_start_limit_requeue_and_recovery() {
    for source in [WorkSource::Room, WorkSource::Routine] {
        let h = harness();
        let bot = h.bot(json!({}));
        let room = source == WorkSource::Room;
        let conversation_id = if room {
            "team".to_owned()
        } else {
            bot.id.clone()
        };
        let linked = add_message(
            &h.db(),
            &NewMessage {
                conversation_id: conversation_id.clone(),
                author: if room { "you" } else { "system" }.into(),
                kind: if room { MessageKind::Text } else { MessageKind::Routine },
                body: "Linked message".into(),
                data: (!room).then(|| object(json!({ "routineId": "routine", "runId": "run", "status": "queued", "note": null }))),
                status: MessageStatus::Done,
            },
        )
        .unwrap();
        let item = h
            .runner
            .deliver(NewWork {
                message_id: Some(linked.id.clone()),
                ..work_in(&bot.id, source, "Run", &conversation_id)
            })
            .unwrap();
        wait_for(|| h.sent(0) > 0).await;
        assert_eq!(h.message(&linked.id).status, MessageStatus::Done);
        let session = h.session(0);
        session.emit(limit_reached(Some(FAR_RESET)));
        session.emit(turn_done(false, Some("Limited")));
        settle().await;
        assert_eq!(h.status(&item.id), WorkStatus::Queued);
        assert_eq!(h.message(&linked.id).status, MessageStatus::Done);
        set_limited_until(&h.db(), &bot.id, None).unwrap();
        h.runner.pump(&bot.id);
        wait_for(|| session.sent().len() == 2).await;
        drop(h.runner.shutdown());
        let restarted = Runner::new(h.deps(Arc::new(|_, _| Err("no sessions".to_owned()))));
        restarted.recover().unwrap();
        assert_eq!(h.status(&item.id), WorkStatus::Interrupted);
        assert_eq!(h.message(&linked.id), linked);
        assert!(!h.events().iter().any(
            |event| matches!(event, Event::MessageUpdated(message) if message.id == linked.id)
        ));
    }
}

#[tokio::test]
async fn keeps_a_bot_replys_linked_message_done_when_steering() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "Start", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let linked = add_message(
        &h.db(),
        &NewMessage {
            conversation_id: bot.id.clone(),
            author: bot.id.clone(),
            kind: MessageKind::Text,
            body: "Reply".into(),
            data: None,
            status: MessageStatus::Done,
        },
    )
    .unwrap();
    h.runner
        .deliver(NewWork {
            message_id: Some(linked.id.clone()),
            ..work(&bot.id, WorkSource::BotReply, "Reply")
        })
        .unwrap();
    wait_for(|| !h.session(0).steered().is_empty()).await;
    assert_eq!(h.message(&linked.id), linked);
}

#[tokio::test]
async fn refuses_a_routine_retry_without_a_scheduler_hook_before_changing_anything() {
    let h = harness_with(Setup {
        signed_in: false,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let label = add_message(
        &h.db(),
        &NewMessage {
            conversation_id: bot.id.clone(),
            author: "system".into(),
            kind: MessageKind::Routine,
            body: "Routine: Check".into(),
            data: Some(object(json!({ "routineId": "routine", "runId": "run", "status": "failed", "note": "Failed" }))),
            status: MessageStatus::Done,
        },
    )
    .unwrap();
    let item = enqueue(
        &h.db(),
        &NewWork {
            message_id: Some(label.id.clone()),
            routine_run_id: Some("run".into()),
            ..work(&bot.id, WorkSource::Routine, "Check")
        },
    )
    .unwrap();
    set_status(&h.db(), &item.id, WorkStatus::Failed).unwrap();
    let error = h.runner.retry(&label.id, None).unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (ErrorCode::BadState, "Routine retry is unavailable")
    );
    assert_eq!(h.message(&label.id), label);
    assert_eq!(h.count("SELECT COUNT(*) FROM work_items"), 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM chains"), 0);
}

#[tokio::test]
async fn settles_a_crashed_daemons_running_activity_during_restart_recovery_once() {
    let h = harness();
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "read", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(tool("read", "Read", "Read notes.md"));
    settle().await;
    let activity = h.activity_for(&bot.id);
    drop(h.runner.shutdown());
    let before = h.event_count();
    let restarted = Runner::new(h.deps(Arc::new(|_, _| Err("no sessions".to_owned()))));
    restarted.recover().unwrap();
    restarted.recover().unwrap();
    assert_eq!(
        h.item_for_message(&message.id).status,
        WorkStatus::Interrupted
    );
    assert_eq!(h.message(&activity.id).status, MessageStatus::Interrupted);
    h.expect_activity_settlement(&activity.id, &[ActivityStatus::Failed], before);
}

#[tokio::test]
async fn recovers_before_listening_and_holds_work_delivered_by_recovery_hooks_until_start() {
    let hooks = RunnerHooks {
        turn_ended: Some(Arc::new(|core, item: &WorkItem, _result: &TurnResult| {
            core.deliver(work(&item.bot_id, WorkSource::BotReply, "Recovery reply"))
                .map(drop)
                .map_err(|error| error.message)
        })),
        ..RunnerHooks::default()
    };
    let h = harness_with(Setup {
        hooks,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let interrupted = enqueue(
        &h.db(),
        &work(&bot.id, WorkSource::Routine, "Interrupted routine"),
    )
    .unwrap();
    set_status(&h.db(), &interrupted.id, WorkStatus::Running).unwrap();
    h.runner.recover().unwrap();
    h.runner.pump_all();
    assert_eq!(h.status(&interrupted.id), WorkStatus::Interrupted);
    assert_eq!(h.session_count(), 0);
    h.runner.start().unwrap();
    wait_for(|| h.sent(0) > 0).await;
    assert_eq!(h.session_count(), 1);
    assert!(h.session(0).sent()[0].contains("Recovery reply"));
    assert_eq!(h.status(&interrupted.id), WorkStatus::Interrupted);
}

#[tokio::test]
async fn reports_a_turn_a_restart_cut_off_to_the_hooks_and_says_so_where_the_turn_was_writing() {
    let ended: Recorded<(String, TurnResult)> = Arc::default();
    let record = ended.clone();
    let hooks = RunnerHooks {
        turn_ended: Some(Arc::new(
            move |_core, item: &WorkItem, result: &TurnResult| {
                record
                    .lock()
                    .unwrap()
                    .push((item.id.clone(), result.clone()));
                Ok(())
            },
        )),
        ..RunnerHooks::default()
    };
    let h = harness_with(Setup {
        hooks,
        ..Setup::default()
    });
    let bot = h.bot(json!({ "name": "Writer", "instructions": "", "model": "sonnet" }));
    let item = enqueue(
        &h.db(),
        &work_in(&bot.id, WorkSource::Room, "@Writer hi", "team"),
    )
    .unwrap();
    set_status(&h.db(), &item.id, WorkStatus::Running).unwrap();
    h.runner.start().unwrap();
    let restarted = "Tjommie restarted while this was running.";
    let expected = TurnResult {
        outcome: TurnOutcome::Failed,
        text: None,
        text_message_id: None,
        error: Some(restarted.into()),
    };
    assert_eq!(*recorded(&ended), [(item.id.clone(), expected)]);
    let notice = h.last("team");
    assert_eq!(
        (notice.author.as_str(), notice.kind, notice.body.as_str()),
        ("system", MessageKind::Error, restarted)
    );
}
