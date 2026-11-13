//! "Runner turn settlement" and "Runner idle shutdown".

use super::*;
use daemon::chat::{NewMessage, add_message};
use daemon::queue::{enqueue, set_status};
use protocol::model::{ErrorData, MessageKind, MessageStatus};

fn error_data(message: &Message) -> Option<ErrorData> {
    match message_data(message) {
        Ok(MessageData::Error(data)) => data,
        other => panic!("not an error row: {other:?}"),
    }
}

fn first_error(h: &Harness, bot_id: &str) -> Option<Message> {
    h.history(bot_id)
        .into_iter()
        .find(|item| item.kind == MessageKind::Error)
}

fn retry_of(message_id: &str) -> Option<ErrorData> {
    Some(ErrorData {
        retry_message_id: message_id.into(),
        retry_bot_id: None,
    })
}

fn add(
    h: &Harness,
    conversation_id: &str,
    author: &str,
    body: &str,
    status: MessageStatus,
) -> Message {
    let new = NewMessage {
        conversation_id: conversation_id.into(),
        author: author.into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status,
    };
    add_message(&h.db(), &new).unwrap()
}

#[tokio::test]
async fn queues_chat_until_stopped_provider_termination() {
    for completed in [false, true] {
        let h = harness();
        let bot = h.bot(json!({}));
        let first = h.runner.submit(&bot.id, "timer", &[]).unwrap();
        wait_for(|| h.sent(0) > 0).await;
        let (stopping, stop) = gate();
        h.session(0).set(|state| state.interrupt = stopping);
        let stopped = h.runner.stop(&bot.id);
        let followup = h.runner.submit(&bot.id, "next", &[]).unwrap();
        settle().await;
        assert_eq!(h.session(0).steered(), Vec::<String>::new());
        assert_eq!(h.item_for_message(&followup.id).status, WorkStatus::Queued);
        h.session(0).emit(if completed {
            exit_completed(true)
        } else {
            exit(None)
        });
        settle().await;
        stop.open(Ok(()));
        stopped.await.unwrap();
        wait_for(|| h.sent(1) > 0).await;
        let expected = if completed {
            WorkStatus::Done
        } else {
            WorkStatus::Interrupted
        };
        assert_eq!(
            h.item_for_message(&first.id).status,
            expected,
            "completed: {completed}"
        );
        assert_eq!(h.item_for_message(&followup.id).status, WorkStatus::Running);
        assert!(h.session(1).sent().last().unwrap().contains("next"));
    }
}

#[tokio::test]
async fn settles_a_stop_mid_tool_before_broadcasting_the_final_activity_row() {
    let h = harness();
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "read", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(tool("read", "Read", "Read notes.md"));
    settle().await;
    let activity = h.activity_for(&bot.id);
    let before = h.event_count();
    h.runner.stop(&bot.id).await.unwrap();
    h.session(0).emit(turn_done(false, None));
    settle().await;
    assert_eq!(
        h.item_for_message(&message.id).status,
        WorkStatus::Interrupted
    );
    h.expect_activity_settlement(&activity.id, &[ActivityStatus::Failed], before);
}

#[tokio::test]
async fn settles_a_tool_left_open_when_the_cli_exits_mid_turn() {
    let h = harness();
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "read", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(tool("read", "Read", "Read notes.md"));
    settle().await;
    let activity = h.activity_for(&bot.id);
    let before = h.event_count();
    h.session(0).emit(exit(Some("The CLI crashed.")));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
    h.expect_activity_settlement(&activity.id, &[ActivityStatus::Failed], before);
}

#[tokio::test]
async fn settles_only_unfinished_steps_when_a_turn_fails() {
    let h = harness();
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "read and write", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(tool("read", "Read", "Read notes.md"));
    h.session(0).emit(tool_done("read", true));
    h.session(0).emit(tool("write", "Write", "Write notes.md"));
    settle().await;
    let activity = h.activity_for(&bot.id);
    let before = h.event_count();
    h.session(0).emit(turn_done(false, Some("Write failed.")));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
    h.expect_activity_settlement(
        &activity.id,
        &[ActivityStatus::Done, ActivityStatus::Failed],
        before,
    );
}

#[tokio::test]
async fn finishes_a_step_left_open_by_an_otherwise_successful_turn() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "read", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(tool("read", "Read", "Read notes.md"));
    settle().await;
    let activity = h.activity_for(&bot.id);
    let before = h.event_count();
    h.session(0).emit(turn_done(true, None));
    settle().await;
    h.expect_activity_settlement(&activity.id, &[ActivityStatus::Done], before);
}

#[tokio::test]
async fn settles_an_abandoned_step_before_retrying_a_session_that_could_not_resume() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.insert_session(&bot.id, "saved-session");
    let message = h.runner.submit(&bot.id, "read", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(tool("old-read", "Read", "Read old.txt"));
    settle().await;
    let activity = h.activity_for(&bot.id);
    let before = h.event_count();
    h.session(0).emit(exit(None));
    wait_for(|| h.sent(1) > 0).await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Running);
    h.expect_activity_settlement(&activity.id, &[ActivityStatus::Failed], before);
    h.session(1).emit(tool("new-read", "Read", "Read new.txt"));
    settle().await;
    assert_ne!(h.activity_for(&bot.id).id, activity.id);
}

#[tokio::test]
async fn keeps_a_turn_running_when_its_pending_send_stop_request_fails() {
    let session = FakeSession::new();
    let (sending, send) = gate();
    let (stopping, stop) = gate();
    session.set(|state| {
        state.send = sending;
        state.interrupt = stopping;
    });
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    let stop_failed = tokio::spawn(h.runner.stop(&bot.id));
    send.open(Ok(()));
    settle().await;
    stop.open(Err("interrupt failed".into()));
    assert_eq!(
        stop_failed.await.unwrap().unwrap_err().message(),
        "interrupt failed"
    );
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Running);
    assert_eq!(h.state(&bot.id), BotState::Working);
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Done);
}

#[tokio::test]
async fn ignores_a_late_pending_send_stop_failure_after_the_next_turn_was_stopped() {
    let session = FakeSession::new();
    let (sending, send) = gate();
    let (stopping, stop) = gate();
    session.set(|state| {
        state.send = sending;
        state.interrupt = stopping;
    });
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let first = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "first"))
        .unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    let stop_failed = tokio::spawn(h.runner.stop(&bot.id));
    send.open(Ok(()));
    settle().await;
    let next = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "next"))
        .unwrap();
    session.emit(turn_done(true, None));
    wait_for(|| session.sent().len() == 2).await;
    session.set(|state| state.interrupt = ready(Ok(())));
    h.runner.stop(&bot.id).await.unwrap();
    stop.open(Err("old interrupt failed".into()));
    assert_eq!(
        stop_failed.await.unwrap().unwrap_err().message(),
        "old interrupt failed"
    );
    settle().await;
    assert_eq!(h.status(&first.id), WorkStatus::Done);
    assert_eq!(h.status(&next.id), WorkStatus::Running);
    session.emit(turn_done(false, None));
    settle().await;
    assert_eq!(h.status(&next.id), WorkStatus::Interrupted);
}

#[tokio::test]
async fn a_stray_text_end_or_tool_done_after_a_finished_turn_leaves_the_bot_idle() {
    for stray in [ProviderEvent::TextEnd, tool_done("old-tool", true)] {
        let h = harness();
        let bot = h.bot(json!({}));
        h.runner.submit(&bot.id, "finish", &[]).unwrap();
        wait_for(|| h.sent(0) > 0).await;
        h.session(0).emit(turn_done(true, None));
        h.session(0).emit(stray);
        settle().await;
        assert_eq!(h.state(&bot.id), BotState::Idle);
    }
}

#[tokio::test]
async fn repeats_a_stop_once_the_pending_send_has_reached_the_provider() {
    let session = FakeSession::new();
    let (sending, sent) = gate();
    session.set(|state| state.send = sending);
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    h.runner.stop(&bot.id).await.unwrap();
    assert_eq!(session.interrupts(), 1);
    sent.open(Ok(()));
    settle().await;
    assert_eq!(session.interrupts(), 2);
    session.emit(turn_done(false, None));
    settle().await;
    assert_eq!(
        h.item_for_message(&message.id).status,
        WorkStatus::Interrupted
    );
}

#[tokio::test]
async fn restarts_a_failed_resumed_session_fresh_with_a_recap() {
    let resumed = FakeSession::new();
    let fresh = FakeSession::new();
    let (sending, reject_resumed_send) = gate();
    resumed.set(|state| state.send = sending);
    let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let captured = options.clone();
    let sessions = sequence(vec![resumed.clone(), fresh.clone()]);
    let factory: Factory = Arc::new(move |start| {
        captured.lock().unwrap().push(start.clone());
        sessions(start)
    });
    let h = harness_with(Setup {
        factory,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    add(&h, &bot.id, "you", "Remember ZEBRA", MessageStatus::Done);
    add(&h, &bot.id, &bot.id, "Noted ZEBRA", MessageStatus::Done);
    h.insert_session(&bot.id, "missing-session");

    let message = h.runner.submit(&bot.id, "continue", &[]).unwrap();
    wait_for(|| !resumed.sent().is_empty()).await;
    assert_eq!(
        recorded(&options)[0].resume_id.as_deref(),
        Some("missing-session")
    );
    assert_eq!(resumed.sent(), ["continue"]);

    resumed.emit(exit(None));
    wait_for(|| !fresh.sent().is_empty()).await;
    let resume_ids: Vec<Option<String>> = recorded(&options)
        .iter()
        .map(|item| item.resume_id.clone())
        .collect();
    assert_eq!(resume_ids, [Some("missing-session".to_owned()), None]);
    let opening = fresh.sent()[0].clone();
    assert!(
        opening.contains("[Tjommie: this is a new session of an ongoing chat."),
        "{opening}"
    );
    assert!(opening.contains("Person: Remember ZEBRA"));
    assert!(opening.contains(&format!("{}: Noted ZEBRA", bot.name)));
    assert!(opening.contains("continue"));
    assert_eq!(h.saved_session(&bot.id), None);
    assert_eq!(
        h.count(&format!(
            "SELECT count(*) FROM work_items WHERE message_id = '{}'",
            message.id
        )),
        1
    );
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Running);
    assert_eq!(first_error(&h, &bot.id), None);

    reject_resumed_send.open(Err("old session stopped".into()));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Running);
    assert_eq!(first_error(&h, &bot.id), None);

    fresh.emit(exit(None));
    settle().await;
    assert_eq!(h.session_count(), 2);
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
    assert_eq!(
        error_data(&first_error(&h, &bot.id).unwrap()),
        retry_of(&message.id)
    );
}

#[tokio::test]
async fn fails_with_retry_when_a_fresh_session_exits_before_reporting_its_session() {
    let session = FakeSession::new();
    let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let captured = options.clone();
    let reuse = session.clone();
    let factory: Factory = Arc::new(move |start| {
        captured.lock().unwrap().push(start.clone());
        Ok(reuse.clone())
    });
    let h = harness_with(Setup {
        factory,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    assert_eq!(recorded(&options)[0].resume_id, None);
    session.emit(exit(None));
    settle().await;
    assert_eq!(h.session_count(), 1);
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
    assert_eq!(
        error_data(&first_error(&h, &bot.id).unwrap()),
        retry_of(&message.id)
    );
    assert_eq!(h.state(&bot.id), BotState::Idle);
}

#[tokio::test]
async fn interrupts_a_stopped_resumed_session_without_retrying_it_fresh() {
    let resumed = FakeSession::new();
    let fresh = FakeSession::new();
    let h = harness_with(Setup {
        factory: sequence(vec![resumed.clone(), fresh]),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.insert_session(&bot.id, "saved-session");
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !resumed.sent().is_empty()).await;
    h.runner.stop(&bot.id).await.unwrap();
    resumed.emit(exit(None));
    settle().await;
    assert_eq!(resumed.interrupts(), 1);
    assert_eq!(h.session_count(), 1);
    assert_eq!(
        h.item_for_message(&message.id).status,
        WorkStatus::Interrupted
    );
    assert_eq!(h.saved_session(&bot.id).as_deref(), Some("saved-session"));
    assert!(
        h.history(&bot.id)
            .iter()
            .any(|item| item.kind == MessageKind::Notice && item.body == "Stopped.")
    );
}

#[tokio::test]
async fn settles_a_successful_turn_as_done_even_after_a_reached_limit_event() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "finish this once", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(limit_reached(Some(FAR_RESET)));
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Done);
    assert_eq!(h.message(&message.id).status, MessageStatus::Sent);
}

#[tokio::test]
async fn settles_an_early_stop_after_startup_without_sending_and_clears_it_before_later_work() {
    let session = FakeSession::new();
    let (start, release_start) = gate();
    session.set(|state| state.start = start);
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let stopped = h.runner.submit(&bot.id, "do not send", &[]).unwrap();
    h.runner.stop(&bot.id).await.unwrap();
    release_start.open(Ok(()));
    wait_for(|| h.item_for_message(&stopped.id).status == WorkStatus::Interrupted).await;
    assert_eq!(session.interrupts(), 1);
    assert_eq!(session.sent(), Vec::<String>::new());
    assert_eq!(h.state(&bot.id), BotState::Idle);
    assert_eq!(h.runner.process_id(&bot.id), Some(4321));

    let later = h.runner.submit(&bot.id, "later work", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(exit(Some("later failure")));
    settle().await;
    assert_eq!(h.item_for_message(&later.id).status, WorkStatus::Failed);
    let failure = h
        .history(&bot.id)
        .into_iter()
        .find(|item| item.kind == MessageKind::Error && error_data(item) == retry_of(&later.id));
    assert_eq!(failure.unwrap().body, "later failure");
}

#[tokio::test]
async fn fails_partial_assistant_text_and_offers_retry_when_the_cli_exits_mid_turn_without_an_error()
 {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(text("partial reply"));
    session.emit(exit(None));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
    let reply = h
        .history(&bot.id)
        .into_iter()
        .find(|item| item.author == bot.id && item.kind == MessageKind::Text)
        .unwrap();
    assert_eq!(
        (reply.body.as_str(), reply.status),
        ("partial reply", MessageStatus::Failed)
    );
    assert_eq!(
        error_data(&first_error(&h, &bot.id).unwrap()),
        retry_of(&message.id)
    );
    assert_eq!(h.state(&bot.id), BotState::Idle);
}

#[tokio::test]
async fn interrupts_an_open_turn_when_a_requested_stop_is_followed_only_by_exit() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(text("partial reply"));
    settle().await;
    h.runner.stop(&bot.id).await.unwrap();
    session.emit(exit(None));
    settle().await;
    assert_eq!(session.interrupts(), 1);
    assert_eq!(
        h.item_for_message(&message.id).status,
        WorkStatus::Interrupted
    );
    let reply = h
        .history(&bot.id)
        .into_iter()
        .find(|item| item.author == bot.id && item.kind == MessageKind::Text)
        .unwrap();
    assert_eq!(reply.status, MessageStatus::Interrupted);
    assert_eq!(first_error(&h, &bot.id), None);
    assert_eq!(h.state(&bot.id), BotState::Idle);
}

#[tokio::test]
async fn treats_exit_as_terminal_when_a_limit_event_arrived_first() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(limit_reached(Some(FAR_RESET)));
    session.emit(exit(None));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
    assert_eq!(
        error_data(&first_error(&h, &bot.id).unwrap()),
        retry_of(&message.id)
    );
    assert_eq!(h.state(&bot.id), BotState::Limited);
}

/// The lost-process contract from T3.3: a session that loses its CLI ends with an exit carrying an error, and the
/// runner fails the turn like a crash, with the provider's message and Retry.
#[tokio::test]
async fn fails_a_turn_like_a_crash_when_the_provider_reports_its_process_lost() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(exit(Some("The Codex app-server stopped unexpectedly.")));
    settle().await;
    assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
    let failure = first_error(&h, &bot.id).unwrap();
    assert_eq!(
        (failure.body.as_str(), error_data(&failure)),
        (
            "The Codex app-server stopped unexpectedly.",
            retry_of(&message.id)
        )
    );
    assert_eq!(h.state(&bot.id), BotState::Idle);
}

/// The spawn-failure contract from T3.3: Rust reports a CLI that can't start as a `start()` error, where TypeScript saw
/// an exit after a successful start. The user sees the same failed turn, message and idle bot.
#[tokio::test]
async fn fails_a_fresh_turn_with_the_providers_message_when_its_cli_cannot_start() {
    let session = FakeSession::new();
    session.set(|state| state.start = ready(Err("spawn claude ENOENT".into())));
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "start", &[]).unwrap();
    wait_for(|| h.item_for_message(&message.id).status == WorkStatus::Failed).await;
    let failure = first_error(&h, &bot.id).unwrap();
    assert_eq!(
        (failure.body.as_str(), error_data(&failure)),
        ("spawn claude ENOENT", retry_of(&message.id))
    );
    assert_eq!(session.sent(), Vec::<String>::new());
    assert!(session.closed());
    assert_eq!(h.state(&bot.id), BotState::Idle);
}

#[tokio::test]
async fn recovers_interrupted_work_and_continues_queued_work_on_start() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let interrupted_message = add(&h, &bot.id, "you", "old", MessageStatus::Sent);
    let interrupted = enqueue(
        &h.db(),
        &NewWork {
            message_id: Some(interrupted_message.id.clone()),
            ..work(&bot.id, WorkSource::You, "old")
        },
    )
    .unwrap();
    set_status(&h.db(), &interrupted.id, WorkStatus::Running).unwrap();
    let queued_message = add(&h, &bot.id, "you", "next", MessageStatus::Queued);
    let queued = enqueue(
        &h.db(),
        &NewWork {
            message_id: Some(queued_message.id.clone()),
            ..work(&bot.id, WorkSource::You, "next")
        },
    )
    .unwrap();
    let streaming = add(
        &h,
        &bot.id,
        &bot.id,
        "partial reply",
        MessageStatus::Streaming,
    );

    h.runner.start().unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    assert_eq!(h.status(&interrupted.id), WorkStatus::Interrupted);
    assert_eq!(
        h.message(&interrupted_message.id).status,
        MessageStatus::Interrupted
    );
    assert_eq!(h.message(&streaming.id).status, MessageStatus::Interrupted);
    assert_eq!(h.status(&queued.id), WorkStatus::Running);
    assert!(session.sent()[0].contains("next"));
    let events = h.events();
    assert!(events.iter().any(|event| matches!(event, Event::MessageAdded(message)
        if message.kind == MessageKind::Error && error_data(message) == retry_of(&interrupted_message.id))));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::MessageUpdated(message)
        if message.id == streaming.id && message.status == MessageStatus::Interrupted))
    );
}

#[tokio::test]
async fn closes_an_idle_cli_after_ten_minutes_while_the_bot_is_limited() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "hit the limit", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    tokio::time::pause();
    let resets_at = jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_hours(2))
        .unwrap();
    session.emit(limit_reached(Some(
        &resets_at.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
    )));
    session.emit(turn_done(false, Some("Usage limit reached")));
    settle().await;
    assert_eq!(h.state(&bot.id), BotState::Limited);
    assert!(!session.closed());
    // Tokio rounds a timer's deadline up to the next millisecond, so ten minutes on the dot can fall just short.
    tokio::time::advance(Duration::from_secs(10 * 60) + Duration::from_millis(1)).await;
    settle().await;
    assert!(session.closed());
    assert_eq!(h.runner.process_id(&bot.id), None);
}

/// TypeScript writes `toLocaleTimeString` and `toLocaleDateString` in the PC's locale; the Rust daemon writes the
/// en-US form in the local zone.
#[tokio::test]
async fn says_when_a_limited_bot_picks_up_again_as_a_time_and_weekday() {
    let session = FakeSession::new();
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "hit the limit", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    session.emit(limit_reached(Some("2099-09-25T10:00:00.000Z")));
    settle().await;
    let notice = h
        .history(&bot.id)
        .into_iter()
        .find(|item| item.kind == MessageKind::Notice)
        .unwrap();
    let prefix = format!(
        "{} hit its usage limit. It will pick up again at ",
        bot.name
    );
    let at = notice
        .body
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix('.'))
        .unwrap_or_else(|| panic!("{}", notice.body));
    let format = regex::Regex::new(r"^\d{1,2}:\d{2} (AM|PM) on [A-Z][a-z]+day$").unwrap();
    assert!(format.is_match(at), "{at}");
}
