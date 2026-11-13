//! "Runner queueing" and "Runner session costs".

use super::*;
use daemon::chat::{NewMessage, add_message};
use daemon::queue::{enqueue, set_status};
use protocol::model::{MessageKind, MessageStatus};

fn chat_message(h: &Harness, bot_id: &str, body: &str, status: MessageStatus) -> Message {
    let new = NewMessage {
        conversation_id: bot_id.into(),
        author: "you".into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status,
    };
    add_message(&h.db(), &new).unwrap()
}

#[tokio::test]
async fn starts_one_session_and_steers_a_second_immediate_message_in_order() {
    let session = FakeSession::new();
    let (start, release_start) = gate();
    session.set(|state| state.start = start);
    let h = harness_with(Setup {
        factory: single(&session),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let first = h.runner.submit(&bot.id, "first", &[]).unwrap();
    let second = h.runner.submit(&bot.id, "second", &[]).unwrap();
    assert_eq!(h.session_count(), 1);
    release_start.open(Ok(()));
    wait_for(|| session.sent().len() == 1 && session.steered().len() == 1).await;
    assert_eq!(session.sent(), ["first"]);
    assert_eq!(session.steered(), ["second"]);
    assert_eq!(h.item_for_message(&second.id).status, WorkStatus::Done);
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(h.item_for_message(&first.id).status, WorkStatus::Done);
}

#[tokio::test]
async fn rejects_retry_when_the_latest_item_is_queued_running_or_done() {
    for status in [WorkStatus::Queued, WorkStatus::Running, WorkStatus::Done] {
        let h = harness_with(Setup {
            signed_in: false,
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let message = chat_message(&h, &bot.id, "status", MessageStatus::Done);
        let item = enqueue(
            &h.db(),
            &NewWork {
                message_id: Some(message.id.clone()),
                ..work(&bot.id, WorkSource::You, "status")
            },
        )
        .unwrap();
        if status != WorkStatus::Queued {
            set_status(&h.db(), &item.id, status).unwrap();
        }
        let count = format!(
            "SELECT count(*) FROM work_items WHERE message_id = '{}'",
            message.id
        );
        let before = h.count(&count);
        let error = h.runner.retry(&message.id, None).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadState, "{status:?}");
        assert_eq!(h.count(&count), before);
    }
}

#[tokio::test]
async fn allows_retry_when_the_latest_item_is_failed_or_interrupted() {
    for (status, message_status) in [
        (WorkStatus::Failed, MessageStatus::Failed),
        (WorkStatus::Interrupted, MessageStatus::Interrupted),
    ] {
        let h = harness_with(Setup {
            signed_in: false,
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let message = chat_message(&h, &bot.id, "status", message_status);
        let item = enqueue(
            &h.db(),
            &NewWork {
                message_id: Some(message.id.clone()),
                ..work(&bot.id, WorkSource::You, "status")
            },
        )
        .unwrap();
        set_status(&h.db(), &item.id, status).unwrap();
        assert_eq!(
            h.runner.retry(&message.id, None).unwrap().status,
            MessageStatus::Queued
        );
        assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Queued);
    }
}

#[tokio::test]
async fn preserves_the_cost_baseline_across_a_daemon_restart_and_records_only_the_resumed_delta() {
    let h = harness();
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "first", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(session_id("saved"));
    h.session(0).emit(usage(0.0169623, 0.0169623));
    h.session(0).emit(turn_done(true, None));
    settle().await;
    drop(h.runner.shutdown());

    let resumed = FakeSession::new();
    let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let captured = options.clone();
    let reuse = resumed.clone();
    let create_session: SessionFactory = Arc::new(move |_bot, start: StartOptions| {
        captured.lock().unwrap().push(start);
        Ok(reuse.clone() as Arc<dyn ProviderSession>)
    });
    let restarted = Runner::new(h.deps(create_session));
    restarted.submit(&bot.id, "continue", &[]).unwrap();
    wait_for(|| !resumed.sent().is_empty()).await;
    let first = recorded(&options)[0].clone();
    assert_eq!(
        (first.resume_id.as_deref(), first.resume_cost_usd),
        (Some("saved"), Some(0.0169623))
    );
    resumed.emit(session_id("saved"));
    settle().await;
    let total = |h: &Harness| -> Option<f64> {
        h.db()
            .query_row(
                "SELECT total_cost_usd FROM sessions WHERE bot_id = ?1",
                [&bot.id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(total(&h), Some(0.0169623));
    resumed.emit(usage(0.0029083, 0.0198706));
    resumed.emit(turn_done(true, None));
    settle().await;
    assert_eq!(total(&h), Some(0.0198706));
    let cost: f64 = h
        .db()
        .query_row(
            "SELECT sum(cost_usd) FROM usage WHERE bot_id = ?1",
            [&bot.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!((cost - 0.0198706).abs() < 1e-10, "{cost}");
}

#[tokio::test]
async fn clears_the_saved_cost_when_the_provider_account_or_cli_session_changes() {
    let cases = [
        ("codex", "claude-main", "saved", None, None),
        ("claude", "old-account", "saved", None, None),
        (
            "claude",
            "claude-main",
            "replacement",
            Some("saved"),
            Some(0.0169623),
        ),
    ];
    for (provider, account_id, emitted, resume_id, resume_cost) in cases {
        let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
        let h = harness_with(Setup {
            factory: capturing(&options),
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        h.db()
            .execute(
                "INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at, total_cost_usd) VALUES (?1, ?2, ?3, 'saved', 'then', 0.0169623)",
                [&bot.id, provider, account_id],
            )
            .unwrap();
        h.runner.submit(&bot.id, "continue", &[]).unwrap();
        wait_for(|| h.sent(0) > 0).await;
        let first = recorded(&options)[0].clone();
        assert_eq!(
            (first.resume_id.as_deref(), first.resume_cost_usd),
            (resume_id, resume_cost),
            "{provider} {account_id}"
        );
        h.session(0).emit(session_id(emitted));
        settle().await;
        let row: (String, String, String, Option<f64>) = h
            .db()
            .query_row("SELECT provider, account_id, cli_session_id, total_cost_usd FROM sessions WHERE bot_id = ?1", [&bot.id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap();
        assert_eq!(
            row,
            ("claude".into(), "claude-main".into(), emitted.into(), None),
            "{provider} {account_id}"
        );
    }
}
