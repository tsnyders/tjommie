//! The first case of `database-upgrade.test.ts`: a version 1 database upgrades, then the runner recovers it and
//! resumes its queued work. The rest of that file is in `tests/db_fixture.rs`.

use super::*;
use daemon::approvals::get_approval;
use daemon::db::{self, MIGRATIONS};
use protocol::model::{ApprovalKind, ApprovalStatus, MessageKind, MessageStatus};

const SEED: &str = "
    INSERT INTO conversations (id, kind, bot_id) VALUES ('legacy-bot', 'bot', 'legacy-bot');
    INSERT INTO messages (id, conversation_id, author, kind, body, data, status, created_at, seq) VALUES
      ('running-message', 'legacy-bot', 'you', 'text', 'First request', NULL, 'sent', '2026-09-25T10:01:00.000Z', 1),
      ('approval-card', 'legacy-bot', 'legacy-bot', 'approval', 'Run a command', '{\"approvalId\":\"legacy-approval\",\"tool\":\"Bash\",\"detail\":\"echo hello\",\"rule\":null,\"status\":\"pending\"}', 'done', '2026-09-25T10:02:00.000Z', 2),
      ('partial-reply', 'legacy-bot', 'legacy-bot', 'text', 'Unfinished answer', NULL, 'streaming', '2026-09-25T10:03:00.000Z', 3),
      ('queued-message', 'legacy-bot', 'you', 'text', 'Next request', NULL, 'queued', '2026-09-25T10:04:00.000Z', 4);
    INSERT INTO approvals (id, bot_id, tool, summary, detail, rule, status, message_id, created_at)
      VALUES ('legacy-approval', 'legacy-bot', 'Bash', 'Run a command', 'echo hello', NULL, 'pending', 'approval-card', '2026-09-25T10:02:00.000Z');
    INSERT INTO work_items (id, bot_id, source, text, message_id, status, created_at, started_at) VALUES
      ('running-work', 'legacy-bot', 'you', 'First request', 'running-message', 'running', '2026-09-25T10:01:00.000Z', '2026-09-25T10:01:01.000Z'),
      ('queued-work', 'legacy-bot', 'you', 'Next request', 'queued-message', 'queued', '2026-09-25T10:04:00.000Z', NULL);
    INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at)
      VALUES ('legacy-bot', 'claude', 'claude-main', 'saved-session', '2026-09-25T10:01:01.000Z');
";

fn card() -> Value {
    json!({ "approvalId": "legacy-approval", "tool": "Bash", "detail": "echo hello", "rule": null, "status": "pending" })
}

#[tokio::test]
async fn upgrades_and_recovers_a_version_1_chat_before_resuming_its_queued_work() {
    let root = TempRoot::new();
    let file = root.0.join("legacy.db");
    let home = root.0.join("bots").join("legacy-bot").display().to_string();
    let account_folder = root
        .0
        .join("accounts")
        .join("claude-main")
        .display()
        .to_string();
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&account_folder).unwrap();
    // The original schema and only 1A columns, so this exercises the real upgrade path.
    {
        let legacy = Connection::open(&file).unwrap();
        legacy.execute_batch(MIGRATIONS[0]).unwrap();
        legacy
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA user_version = 1;")
            .unwrap();
        legacy
            .execute(
                "INSERT INTO accounts (id, provider, label, kind, folder, created_at)
                 VALUES ('claude-main', 'claude', 'Claude', 'subscription', ?1, '2026-09-25T10:00:00.000Z')",
                [&account_folder],
            )
            .unwrap();
        legacy
            .execute(
                "INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
                 VALUES ('legacy-bot', 'Writer', 'blue', 'circle', 'Be terse.', 'claude', 'haiku', 'folder', ?1, '2026-09-25T10:00:00.000Z')",
                [&home],
            )
            .unwrap();
        legacy.execute_batch(SEED).unwrap();
        let version: i64 = legacy
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    let db: daemon::context::Db = Arc::new(Mutex::new(db::open(&file).unwrap()));
    let session = FakeSession::new();
    let started: Recorded<StartOptions> = Arc::default();
    let (record, reuse) = (started.clone(), session.clone());
    let create_session: SessionFactory = Arc::new(move |_bot, options| {
        record.lock().unwrap().push(options);
        Ok(reuse.clone() as Arc<dyn ProviderSession>)
    });
    let runner = Runner::new(RunnerDeps::new(
        db.clone(),
        Arc::new(|_| Ok(())),
        create_session,
        common::no_cli_setup(),
    ));
    let lock = || db.lock().unwrap();
    let strings = |sql: &str| -> Vec<String> {
        let db = lock();
        let mut statement = db.prepare(sql).unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };

    let version: i64 = lock()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, MIGRATIONS.len() as i64);
    assert_eq!(
        strings(
            "SELECT id || '|' || kind || '|' || COALESCE(bot_id, 'null') FROM conversations ORDER BY id"
        ),
        ["legacy-bot|bot|legacy-bot", "team|room|null"]
    );
    assert_eq!(
        strings(
            "SELECT id || '|' || conversation_id || '|' || priority || '|' || COALESCE(chain_id, 'null') || '|' || COALESCE(exchange_id, 'null')
             || '|' || COALESCE(routine_run_id, 'null') FROM work_items ORDER BY created_at"
        ),
        ["running-work|legacy-bot|0|null|null|null", "queued-work|legacy-bot|0|null|null|null"]
    );
    let bot = get_stored_bot(&lock(), "legacy-bot").unwrap();
    assert_eq!(
        (bot.name.as_str(), bot.home_folder.as_str()),
        ("Writer", home.as_str())
    );
    let approval = get_approval(&lock(), "legacy-approval").unwrap();
    assert_eq!(
        (approval.kind, approval.status, approval.payload),
        (ApprovalKind::Tool, ApprovalStatus::Pending, None)
    );
    assert_eq!(
        get_message(&lock(), "approval-card")
            .unwrap()
            .data
            .map(Value::Object),
        Some(card())
    );
    let saved: (String, Option<f64>) = lock()
        .query_row(
            "SELECT cli_session_id, total_cost_usd FROM sessions WHERE bot_id = 'legacy-bot'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved, ("saved-session".to_owned(), None));

    runner.recover().unwrap();
    assert_eq!(recorded(&started).len(), 0);
    assert_eq!(
        get_work_item(&lock(), "running-work").unwrap().status,
        WorkStatus::Interrupted
    );
    assert_eq!(
        get_work_item(&lock(), "queued-work").unwrap().status,
        WorkStatus::Queued
    );
    assert_eq!(
        get_message(&lock(), "running-message").unwrap().status,
        MessageStatus::Interrupted
    );
    let partial = get_message(&lock(), "partial-reply").unwrap();
    assert_eq!(
        (partial.body.as_str(), partial.status),
        ("Unfinished answer", MessageStatus::Interrupted)
    );
    let approval = get_approval(&lock(), "legacy-approval").unwrap();
    assert_eq!(
        (approval.kind, approval.status, approval.payload),
        (ApprovalKind::Tool, ApprovalStatus::Denied, None)
    );
    let mut denied = card();
    denied["status"] = json!("denied");
    assert_eq!(
        get_message(&lock(), "approval-card")
            .unwrap()
            .data
            .map(Value::Object),
        Some(denied)
    );
    let notices =
        strings("SELECT id FROM messages WHERE conversation_id = 'legacy-bot' AND kind = 'error'");
    assert_eq!(notices.len(), 1);
    let notice = get_message(&lock(), &notices[0]).unwrap();
    assert_eq!(notice.body, "Tjommie restarted while this was running.");
    assert_eq!(
        notice.data.map(Value::Object),
        Some(json!({ "retryMessageId": "running-message" }))
    );
    assert_eq!(notice.kind, MessageKind::Error);

    runner.start().unwrap();
    wait_for(|| session.sent().len() == 1).await;
    assert_eq!(session.sent(), ["Next request"]);
    let options = recorded(&started).clone();
    assert_eq!(options.len(), 1);
    assert_eq!(
        (
            options[0].cwd.as_str(),
            options[0].resume_id.as_deref(),
            options[0].resume_cost_usd
        ),
        (home.as_str(), Some("saved-session"), None)
    );
    assert_eq!(
        runner.current_work("legacy-bot").map(|item| item.id),
        Some("queued-work".to_owned())
    );
    let queued = get_work_item(&lock(), "queued-work").unwrap();
    assert_eq!(
        (queued.status, queued.conversation_id.as_str()),
        (WorkStatus::Running, "legacy-bot")
    );
    assert_eq!(
        get_work_item(&lock(), "running-work").unwrap().status,
        WorkStatus::Interrupted
    );
    assert_eq!(
        get_message(&lock(), "queued-message").unwrap().status,
        MessageStatus::Sent
    );
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(
        get_work_item(&lock(), "queued-work").unwrap().status,
        WorkStatus::Done
    );
    drop(runner.shutdown());
}
