//! "Runner bot deletion".

use super::*;
use daemon::queue::enqueue;
use protocol::model::{ApprovalDecision, MessageKind, MessageStatus};

fn delete_bot_row(h: &Harness, bot_id: &str) {
    h.db()
        .execute("DELETE FROM bots WHERE id = ?1", [bot_id])
        .unwrap();
}

fn work_rows(h: &Harness, bot_id: &str) -> Vec<String> {
    let db = h.db();
    let mut statement = db
        .prepare("SELECT id, status, started_at, ended_at, text FROM work_items WHERE bot_id = ?1 ORDER BY rowid")
        .unwrap();
    statement
        .query_map([bot_id], |row| {
            let started: Option<String> = row.get(2)?;
            let ended: Option<String> = row.get(3)?;
            Ok(format!(
                "{}|{}|{started:?}|{ended:?}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(4)?
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[tokio::test]
async fn interrupts_a_streaming_team_room_reply_exactly_once_when_its_bot_is_deleted() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner
        .deliver(work_in(&bot.id, WorkSource::Room, "reply", "team"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(text("partial reply"));
    settle().await;
    let reply = h
        .history("team")
        .into_iter()
        .find(|message| message.author == bot.id && message.kind == MessageKind::Text)
        .unwrap();
    let before = h.event_count();
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    delete_bot_row(&h, &bot.id);
    h.runner.forget(&bot.id);
    let saved = h.message(&reply.id);
    assert_eq!(
        (saved.body.as_str(), saved.status),
        ("partial reply", MessageStatus::Interrupted)
    );
    let updates: Vec<Message> = h.events()[before..]
        .iter()
        .filter_map(|event| match event {
            Event::MessageUpdated(message) if message.id == reply.id => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(updates, [saved]);
}

#[tokio::test]
async fn settles_an_activity_row_in_a_shared_conversation_when_its_bot_is_deleted() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner
        .deliver(work_in(&bot.id, WorkSource::Room, "read", "team"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(tool("read", "Read", "Read notes.md"));
    settle().await;
    let activity = h.activity_for("team");
    let before = h.event_count();
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    delete_bot_row(&h, &bot.id);
    h.runner.forget(&bot.id);
    h.expect_activity_settlement(&activity.id, &[ActivityStatus::Failed], before);
}

#[tokio::test]
async fn clears_text_activity_and_the_last_answer_when_a_failed_deletion_restores_unstopped_work() {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "complete this"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let old = h.session(0);
    old.emit(text("old answer"));
    old.emit(ProviderEvent::TextEnd);
    old.emit(tool("old", "Read", "Read old.txt"));
    old.emit(text("old partial"));
    settle().await;
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    h.runner.cancel_deletion(&bot.id);
    wait_for(|| h.sent(1) > 0).await;
    let replacement = h.session(1);
    replacement.emit(tool("new", "Read", "Read new.txt"));
    replacement.emit(turn_done(false, Some("new failure")));
    settle().await;
    assert_eq!(h.status(&item.id), WorkStatus::Failed);
    let failed = TurnResult {
        outcome: TurnOutcome::Failed,
        text: None,
        text_message_id: None,
        error: Some("new failure".into()),
    };
    assert_eq!(lifecycle.results(), [failed]);
    let messages = h.history(&bot.id);
    assert_eq!(
        messages
            .iter()
            .find(|message| message.body == "old partial")
            .unwrap()
            .status,
        MessageStatus::Interrupted
    );
    let activity: Vec<&str> = messages
        .iter()
        .filter(|message| message.kind == MessageKind::Activity)
        .map(|message| message.body.as_str())
        .collect();
    assert_eq!(activity, ["1 step: Read old.txt", "1 step: Read new.txt"]);
}

#[tokio::test]
async fn keeps_a_requested_stop_when_deletion_fails_instead_of_relaunching_the_stopped_item() {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "stop this"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.runner
        .stop_item(&item.id, "Routine time limit reached.")
        .await
        .unwrap();
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    h.runner.cancel_deletion(&bot.id);
    settle().await;
    assert_eq!(h.session_count(), 1);
    assert_eq!(h.status(&item.id), WorkStatus::Interrupted);
    let outcomes: Vec<(String, TurnOutcome)> = lifecycle
        .ended()
        .into_iter()
        .map(|(work, result)| (work.id, result.outcome))
        .collect();
    assert_eq!(outcomes, [(item.id.clone(), TurnOutcome::Interrupted)]);
    assert_eq!(h.state(&bot.id), BotState::Idle);
    assert_eq!(h.last(&bot.id).body, "Routine time limit reached.");
}

#[tokio::test]
async fn restores_a_failed_deletion_only_after_its_delayed_process_exit_settles() {
    let h = harness();
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "continue later"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let (exit, exited) = gate();
    h.session(0).set(|state| state.exit = exit);
    let closing = h.runner.close_for_deletion(&bot.id).unwrap();
    h.runner.cancel_deletion(&bot.id);
    h.runner.pump(&bot.id);
    assert_eq!(h.session_count(), 1);
    let error = h.runner.close_for_deletion(&bot.id).err().unwrap();
    assert!(error.message.contains("still closing"), "{}", error.message);
    exited.open(());
    closing.await;
    wait_for(|| h.sent(1) > 0).await;
    assert_eq!(
        h.runner.current_work(&bot.id).map(|work| work.id),
        Some(item.id.clone())
    );
    assert_eq!(h.status(&item.id), WorkStatus::Running);
}

#[tokio::test]
async fn restores_deletion_after_an_approval_was_answered_during_the_close_wait() {
    let h = harness();
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "needs approval"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0)
        .emit(approval("old-request", "Write file", None));
    settle().await;
    let added = h.added_approvals().remove(0);
    let (exit, exited) = gate();
    h.session(0).set(|state| state.exit = exit);
    let closing = h.runner.close_for_deletion(&bot.id).unwrap();
    h.runner
        .answer_approval(&added.id, ApprovalDecision::Once)
        .unwrap();
    exited.open(());
    closing.await;
    h.runner.cancel_deletion(&bot.id);
    wait_for(|| h.sent(1) > 0).await;
    assert_eq!(
        h.runner.current_work(&bot.id).map(|work| work.id),
        Some(item.id.clone())
    );
    assert_eq!(h.state(&bot.id), BotState::Working);
    assert_eq!(
        h.message(&added.message_id).data.unwrap()["status"],
        json!("allowed")
    );
    let resolved = h
        .events()
        .iter()
        .filter(
            |event| matches!(event, Event::ApprovalResolved(approval) if approval.id == added.id),
        )
        .count();
    assert_eq!(resolved, 1);
}

#[tokio::test]
async fn kills_a_busy_detached_session_using_the_pid_captured_before_close_cleared_it() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "running", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let (exit, exited) = gate();
    h.session(0).set(|state| {
        state.exit = exit;
        state.hide_pid_when_closed = true;
    });
    let closing = h.runner.close_for_deletion(&bot.id).unwrap();
    assert_eq!(h.session(0).pid(), None);
    drop(h.runner.shutdown());
    assert_eq!(*recorded(&h.kills), [Some(4321)]);
    exited.open(());
    closing.await;
}

#[tokio::test]
async fn does_not_kill_a_detached_pid_after_its_process_has_already_exited() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "running", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    drop(h.runner.shutdown());
    assert_eq!(*recorded(&h.kills), []);
}

#[tokio::test]
async fn waits_for_process_exit_and_blocks_every_pump_until_deletion_is_cancelled() {
    let session = FakeSession::new();
    let (exit_gate, exit) = gate();
    session.set(|state| state.exit = exit_gate);
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "first", &[]).unwrap();
    wait_for(|| !session.sent().is_empty()).await;
    let settled = Arc::new(AtomicBool::new(false));
    let closing = {
        let settled = settled.clone();
        let closed = h.runner.close_for_deletion(&bot.id).unwrap();
        tokio::spawn(async move {
            closed.await;
            settled.store(true, Ordering::SeqCst);
        })
    };
    h.runner.pump(&bot.id);
    h.runner.pump_all();
    h.runner
        .deliver(work(&bot.id, WorkSource::Routine, "wait"))
        .unwrap();
    settle().await;
    assert!(!settled.load(Ordering::SeqCst));
    assert_eq!(h.session_count(), 1);
    exit.open(());
    closing.await.unwrap();
    h.runner.pump(&bot.id);
    settle().await;
    assert_eq!(h.session_count(), 1);
}

#[tokio::test]
async fn closes_a_session_again_when_startup_succeeds_after_it_was_detached() {
    let session = FakeSession::new();
    let (start, started) = gate();
    session.set(|state| state.start = start);
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "first", &[]).unwrap();
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    assert_eq!(session.close_calls(), 1);
    started.open(Ok(()));
    settle().await;
    assert_eq!(session.close_calls(), 2);
    assert_eq!(session.sent(), Vec::<String>::new());
}

#[tokio::test]
async fn starts_a_tree_kill_synchronously_for_a_busy_session_on_shutdown() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "running", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    drop(h.runner.shutdown());
    assert_eq!(*recorded(&h.kills), [Some(4321)]);
    assert!(h.session(0).closed());
}

#[tokio::test]
async fn closes_an_idle_session_without_a_tree_kill_on_shutdown() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "finished", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(turn_done(true, None));
    settle().await;
    drop(h.runner.shutdown());
    assert_eq!(*recorded(&h.kills), []);
    assert!(h.session(0).closed());
}

#[tokio::test]
async fn keeps_queued_work_unchanged_while_closing_for_deletion_and_pumps_it_after_failure() {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let item = enqueue(
        &h.db(),
        &work(&bot.id, WorkSource::Routine, "Queued routine"),
    )
    .unwrap();
    let before = work_rows(&h, &bot.id);
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    assert_eq!(work_rows(&h, &bot.id), before);
    assert_eq!(lifecycle.ended_ids(), Vec::<String>::new());
    h.runner.cancel_deletion(&bot.id);
    wait_for(|| h.sent(0) > 0).await;
    assert_eq!(
        h.runner.current_work(&bot.id).map(|work| work.id),
        Some(item.id)
    );
}

#[tokio::test]
async fn restarts_active_work_after_failed_deletion_and_ignores_a_late_start_or_send_rejection() {
    for boundary in ["start", "send"] {
        let (pending, reject) = gate();
        let old = FakeSession::new();
        old.set(|state| {
            if boundary == "start" {
                state.start = pending
            } else {
                state.send = pending
            }
        });
        let replacement = FakeSession::new();
        let lifecycle = Lifecycle::default();
        let made = AtomicU32::new(0);
        let (first, next) = (old.clone(), replacement.clone());
        let factory: Factory = Arc::new(move |_| {
            Ok(if made.fetch_add(1, Ordering::SeqCst) == 0 {
                first.clone()
            } else {
                next.clone()
            })
        });
        let h = harness_with(Setup {
            factory,
            hooks: lifecycle.hooks(),
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let item = h
            .runner
            .deliver(work(&bot.id, WorkSource::Routine, "Active routine"))
            .unwrap();
        if boundary == "send" {
            wait_for(|| !old.sent().is_empty()).await;
        }
        let before = work_rows(&h, &bot.id);
        h.runner.close_for_deletion(&bot.id).unwrap().await;
        assert!(old.closed());
        assert_eq!(work_rows(&h, &bot.id), before);
        assert_eq!(lifecycle.ended_ids(), Vec::<String>::new());
        h.runner.cancel_deletion(&bot.id);
        wait_for(|| !replacement.sent().is_empty()).await;
        reject.open(Err("Old operation rejected after replacement".into()));
        old.emit(exit(Some("Old exit")));
        old.emit(text("Old text"));
        settle().await;
        assert_eq!(
            h.runner.current_work(&bot.id).map(|work| work.id),
            Some(item.id.clone()),
            "{boundary}"
        );
        assert_eq!(h.status(&item.id), WorkStatus::Running);
        assert_eq!(lifecycle.started(), std::slice::from_ref(&item.id));
        assert_eq!(lifecycle.requeued(), Vec::<String>::new());
        assert_eq!(lifecycle.ended_ids(), Vec::<String>::new());
        replacement.emit(turn_done(true, None));
        settle().await;
        assert_eq!(lifecycle.ended_ids(), std::slice::from_ref(&item.id));
    }
}

#[tokio::test]
async fn ends_captured_active_and_queued_routine_work_after_successful_deletion() {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let active = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "Active routine"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let queued = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "Queued routine"))
        .unwrap();
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    delete_bot_row(&h, &bot.id);
    h.runner.forget(&bot.id);
    h.runner.forget(&bot.id);
    let outcomes: Vec<(String, TurnOutcome)> = lifecycle
        .ended()
        .into_iter()
        .map(|(work, result)| (work.id, result.outcome))
        .collect();
    assert_eq!(
        outcomes,
        [
            (active.id, TurnOutcome::Interrupted),
            (queued.id, TurnOutcome::Interrupted)
        ]
    );
    assert_eq!(h.runner.current_work(&bot.id), None);
    assert_eq!(h.runner.process_id(&bot.id), None);
}

#[tokio::test]
async fn ignores_pending_steering_after_deletion() {
    for deleted in [false, true] {
        let (steer_pending, resolve_steer) = gate();
        let old = FakeSession::new();
        old.set(|state| state.steer_pending = Some(steer_pending));
        let replacement = FakeSession::new();
        let h = harness_with(Setup {
            factory: sequence(vec![old.clone(), replacement.clone()]),
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        h.runner.submit(&bot.id, "First", &[]).unwrap();
        wait_for(|| !old.sent().is_empty()).await;
        let steered = h.runner.submit(&bot.id, "Still needed", &[]).unwrap();
        wait_for(|| !old.steered().is_empty()).await;
        h.runner.close_for_deletion(&bot.id).unwrap().await;
        if deleted {
            delete_bot_row(&h, &bot.id);
            h.runner.forget(&bot.id);
        } else {
            h.runner.cancel_deletion(&bot.id);
            wait_for(|| !replacement.steered().is_empty()).await;
            assert_eq!(replacement.steered(), ["Still needed"]);
            assert_eq!(h.item_for_message(&steered.id).status, WorkStatus::Done);
        }
        resolve_steer.open(false);
        settle().await;
        assert!(h.no_logs(), "{:?}", recorded(&h.logs));
        if !deleted {
            assert_eq!(h.item_for_message(&steered.id).status, WorkStatus::Done);
        }
    }
}

#[tokio::test]
async fn leaves_interrupted_by_shutdown_work_running_for_startup_recovery_without_turn_end_hooks() {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "Recover on restart"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    drop(h.runner.shutdown());
    assert!(h.session(0).closed());
    assert_eq!(h.status(&item.id), WorkStatus::Running);
    assert_eq!(lifecycle.ended_ids(), Vec::<String>::new());
}

#[tokio::test]
async fn expires_obsolete_approval_cards_only_when_failed_deletion_resumes_the_bot() {
    let h = harness();
    let bot = h.bot(json!({}));
    let job = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "Needs approval"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0)
        .emit(approval("old-provider-request", "Write notes", None));
    settle().await;
    let added = h.added_approvals().remove(0);
    h.runner.close_for_deletion(&bot.id).unwrap().await;
    assert_eq!(
        h.message(&added.message_id).data.unwrap()["status"],
        json!("pending")
    );
    h.runner.cancel_deletion(&bot.id);
    wait_for(|| h.sent(1) > 0).await;
    assert_eq!(
        h.message(&added.message_id).data.unwrap()["status"],
        json!("denied")
    );
    assert_eq!(h.state(&bot.id), BotState::Working);
    assert_eq!(h.status(&job.id), WorkStatus::Running);
    assert_eq!(h.session(1).answers(), []);
}

/// TypeScript also makes the first `close()` throw. `ProviderSession::close` can't fail in Rust, so this keeps the rest:
/// shutdown closes every session and reports every one closed, in order.
#[tokio::test]
async fn closes_every_session_and_revokes_every_token_on_shutdown() {
    let closed: Recorded<String> = Arc::default();
    let record = closed.clone();
    let hooks = RunnerHooks {
        session_closed: Some(Arc::new(move |_core, bot_id: &str| {
            record.lock().unwrap().push(bot_id.to_owned());
            Ok(())
        })),
        ..RunnerHooks::default()
    };
    let h = harness_with(Setup {
        hooks,
        ..Setup::default()
    });
    let first = h.bot(json!({}));
    let second = h.bot(json!({}));
    h.runner.submit(&first.id, "First", &[]).unwrap();
    h.runner.submit(&second.id, "Second", &[]).unwrap();
    wait_for(|| h.sent(0) > 0 && h.sent(1) > 0).await;
    drop(h.runner.shutdown());
    assert!(recorded(&h.sessions).iter().all(|session| session.closed()));
    assert_eq!(*recorded(&closed), [first.id, second.id]);
}
