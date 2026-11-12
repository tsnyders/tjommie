//! `queue.test.ts`

mod common;

use common::{TempRoot, new_bot, test_db};
use daemon::queue::{
    NewWork, WorkSource, WorkStatus, enqueue, get_work_item, last_item_for_message, new_chain,
    next_queued, next_steerable, recover_after_restart, set_status, use_chain_request,
};
use rusqlite::Connection;

const TIED_CREATED_AT: &str = "2026-09-26T10:00:00.000Z";

struct Fixture {
    db: Connection,
    bot_id: String,
    _root: TempRoot,
}

fn fixture() -> Fixture {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    let bot_id = new_bot(&db, &root.0.join("bots"), "Writer", "codex").id;
    Fixture {
        db,
        bot_id,
        _root: root,
    }
}

impl Fixture {
    fn add(&self, text: &str, source: WorkSource, extra: NewWork) -> daemon::queue::WorkItem {
        enqueue(
            &self.db,
            &NewWork {
                bot_id: self.bot_id.clone(),
                source,
                text: text.into(),
                ..extra
            },
        )
        .unwrap()
    }

    fn work(&self) -> NewWork {
        NewWork::new(&self.bot_id, WorkSource::You, "")
    }

    fn tie(&self) {
        self.db
            .execute("UPDATE work_items SET created_at = ?1", [TIED_CREATED_AT])
            .unwrap();
    }

    fn column(&self, sql: &str, id: &str) -> Option<String> {
        self.db.query_row(sql, [id], |row| row.get(0)).unwrap()
    }
}

fn task_context(extra: NewWork) -> NewWork {
    NewWork {
        task_attempt_id: Some("attempt-a".into()),
        task_revision: Some(1),
        execution_context_key: Some("context-a".into()),
        ..extra
    }
}

#[test]
fn persists_complete_task_context_and_never_steers_work_across_contexts() {
    let f = fixture();
    let task = f.add("task", WorkSource::Task, task_context(f.work()));
    let stored = get_work_item(&f.db, &task.id).unwrap();
    assert_eq!(
        (
            stored.task_attempt_id.as_deref(),
            stored.task_revision,
            stored.execution_context_key.as_deref()
        ),
        (Some("attempt-a"), Some(1), Some("context-a"))
    );
    let ordinary = f.add("standalone reply", WorkSource::BotReply, f.work());
    let reply = f.add("task reply", WorkSource::BotReply, task_context(f.work()));
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, &f.bot_id, None)
            .unwrap()
            .unwrap()
            .id,
        ordinary.id
    );
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, &f.bot_id, Some("context-a"))
            .unwrap()
            .unwrap()
            .id,
        reply.id
    );
    let partial = NewWork {
        task_attempt_id: Some("attempt-a".into()),
        ..f.work()
    };
    let error = enqueue(
        &f.db,
        &NewWork {
            source: WorkSource::Task,
            text: "bad".into(),
            ..partial
        },
    )
    .unwrap_err();
    assert!(
        error.message.contains("complete immutable"),
        "{}",
        error.message
    );
    let error = enqueue(
        &f.db,
        &NewWork {
            source: WorkSource::Task,
            text: "bad".into(),
            ..f.work()
        },
    )
    .unwrap_err();
    assert!(
        error.message.contains("needs an attempt"),
        "{}",
        error.message
    );
}

#[test]
fn hands_out_queued_work_oldest_first_within_a_priority_even_within_one_millisecond() {
    let f = fixture();
    let [a, b, c] = ["a", "b", "c"].map(|text| f.add(text, WorkSource::You, f.work()));
    f.tie();
    assert_eq!(next_queued(&f.db, &f.bot_id).unwrap().unwrap().id, a.id);
    set_status(&f.db, &a.id, WorkStatus::Running).unwrap();
    assert_eq!(next_queued(&f.db, &f.bot_id).unwrap().unwrap().id, b.id);
    set_status(&f.db, &b.id, WorkStatus::Done).unwrap();
    assert_eq!(next_queued(&f.db, &f.bot_id).unwrap().unwrap().id, c.id);
    set_status(&f.db, &c.id, WorkStatus::Done).unwrap();
    assert_eq!(next_queued(&f.db, &f.bot_id).unwrap(), None);
}

#[test]
fn puts_your_messages_and_replies_ahead_of_bot_requests_and_routine_runs() {
    let f = fixture();
    let routine = f.add("routine", WorkSource::Routine, f.work());
    let request = f.add("request", WorkSource::BotRequest, f.work());
    let reply = f.add("reply", WorkSource::BotReply, f.work());
    let room = f.add(
        "room",
        WorkSource::Room,
        NewWork {
            conversation_id: Some("team".into()),
            ..f.work()
        },
    );
    let mine = f.add("mine", WorkSource::You, f.work());
    f.tie();
    let mut statement =
        f.db.prepare("SELECT source, priority FROM work_items ORDER BY rowid")
            .unwrap();
    let rows: Vec<(String, i64)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let expected = [
        ("routine", 1),
        ("bot_request", 1),
        ("bot_reply", 0),
        ("room", 0),
        ("you", 0),
    ]
    .map(|(source, priority)| (source.to_owned(), priority));
    assert_eq!(rows, expected);
    for item in [reply, room, mine, routine, request] {
        assert_eq!(next_queued(&f.db, &f.bot_id).unwrap().unwrap().id, item.id);
        set_status(&f.db, &item.id, WorkStatus::Done).unwrap();
    }
}

#[test]
fn defaults_the_conversation_to_the_bots_own_chat() {
    let f = fixture();
    let item = enqueue(&f.db, &NewWork::new(&f.bot_id, WorkSource::You, "x")).unwrap();
    assert_eq!(item.conversation_id, f.bot_id);
    let stored = get_work_item(&f.db, &item.id).unwrap();
    assert_eq!(stored.conversation_id, f.bot_id);
    assert_eq!(
        (
            stored.message_id,
            stored.chain_id,
            stored.exchange_id,
            stored.routine_run_id
        ),
        (None, None, None, None)
    );
    assert_eq!(
        f.column(
            "SELECT conversation_id FROM work_items WHERE id = ?1",
            &item.id
        )
        .as_deref(),
        Some(f.bot_id.as_str())
    );
    let room = f.add(
        "y",
        WorkSource::Room,
        NewWork {
            conversation_id: Some("team".into()),
            ..f.work()
        },
    );
    assert_eq!(room.conversation_id, "team");
}

#[test]
fn round_trips_the_destination_and_chain_exchange_and_routine_metadata() {
    let f = fixture();
    let item = f.add(
        "reply text",
        WorkSource::BotReply,
        NewWork {
            message_id: Some("m1".into()),
            conversation_id: Some("exchange-1".into()),
            chain_id: Some("chain-1".into()),
            exchange_id: Some("exchange-1".into()),
            routine_run_id: Some("run-1".into()),
            ..f.work()
        },
    );
    let stored = get_work_item(&f.db, &item.id).unwrap();
    assert_eq!(stored, item);
    assert_eq!(
        (
            stored.source,
            stored.text.as_str(),
            stored.message_id.as_deref(),
            stored.conversation_id.as_str()
        ),
        (WorkSource::BotReply, "reply text", Some("m1"), "exchange-1")
    );
    assert_eq!(
        (
            stored.chain_id.as_deref(),
            stored.exchange_id.as_deref(),
            stored.routine_run_id.as_deref(),
            stored.status
        ),
        (
            Some("chain-1"),
            Some("exchange-1"),
            Some("run-1"),
            WorkStatus::Queued
        )
    );
    assert_eq!(
        (
            stored.task_attempt_id,
            stored.task_revision,
            stored.execution_context_key
        ),
        (None, None, None)
    );
    assert_eq!(next_queued(&f.db, &f.bot_id).unwrap(), Some(item));
}

#[test]
fn rejects_stored_work_with_a_null_conversation_id() {
    let f = fixture();
    f.db.execute(
        "INSERT INTO work_items (id, bot_id, source, text, status, created_at, conversation_id) VALUES ('null-conversation', ?1, 'you', 'broken', 'queued', ?2, NULL)",
        [&f.bot_id, TIED_CREATED_AT],
    )
    .unwrap();
    assert_eq!(
        get_work_item(&f.db, "null-conversation")
            .unwrap_err()
            .message,
        "Work item null-conversation has a null conversation_id"
    );
}

#[test]
fn only_steers_your_messages_and_replies_and_only_into_the_same_conversation() {
    let f = fixture();
    let team = || NewWork {
        conversation_id: Some("team".into()),
        ..f.work()
    };
    f.add("room post", WorkSource::Room, team());
    f.add("request", WorkSource::BotRequest, team());
    f.add("routine", WorkSource::Routine, team());
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, &f.bot_id, None).unwrap(),
        None
    );
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, "team", None).unwrap(),
        None
    );
    let other_root = TempRoot::new();
    let other = new_bot(&f.db, &other_root.0.join("bots"), "Other", "codex");
    let other_work = enqueue(
        &f.db,
        &NewWork {
            conversation_id: Some(f.bot_id.clone()),
            ..NewWork::new(&other.id, WorkSource::You, "other bot")
        },
    )
    .unwrap();
    f.db.execute(
        "UPDATE work_items SET created_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
        [&other_work.id],
    )
    .unwrap();
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, &f.bot_id, None).unwrap(),
        None
    );
    assert_eq!(
        next_queued(&f.db, &f.bot_id).unwrap().unwrap().bot_id,
        f.bot_id
    );
    let mine = f.add("mine", WorkSource::You, f.work());
    let reply = f.add("reply", WorkSource::BotReply, f.work());
    f.tie();
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, &f.bot_id, None)
            .unwrap()
            .unwrap()
            .id,
        mine.id
    );
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, "team", None).unwrap(),
        None
    );
    set_status(&f.db, &mine.id, WorkStatus::Running).unwrap();
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, &f.bot_id, None)
            .unwrap()
            .unwrap()
            .id,
        reply.id
    );
    set_status(&f.db, &reply.id, WorkStatus::Done).unwrap();
    assert_eq!(
        next_steerable(&f.db, &f.bot_id, &f.bot_id, None).unwrap(),
        None
    );
    assert_eq!(
        next_steerable(&f.db, &other.id, &f.bot_id, None)
            .unwrap()
            .unwrap()
            .id,
        other_work.id
    );
}

#[test]
fn puts_running_work_back_in_the_queue_with_its_start_time_cleared() {
    let f = fixture();
    let item = f.add("x", WorkSource::You, f.work());
    let times = |id: &str| -> (Option<String>, Option<String>) {
        f.db.query_row(
            "SELECT started_at, ended_at FROM work_items WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    };
    let iso = |value: &Option<String>| {
        value
            .as_ref()
            .is_some_and(|time| time.len() == 24 && &time[10..11] == "T")
    };
    set_status(&f.db, &item.id, WorkStatus::Running).unwrap();
    set_status(&f.db, &item.id, WorkStatus::Queued).unwrap();
    assert_eq!(
        get_work_item(&f.db, &item.id).unwrap().status,
        WorkStatus::Queued
    );
    assert_eq!(times(&item.id).0, None);
    for status in [
        WorkStatus::Done,
        WorkStatus::Failed,
        WorkStatus::Interrupted,
    ] {
        set_status(&f.db, &item.id, WorkStatus::Running).unwrap();
        assert_eq!(
            get_work_item(&f.db, &item.id).unwrap().status,
            WorkStatus::Running
        );
        let (started, ended) = times(&item.id);
        assert!(iso(&started) && ended.is_none());
        set_status(&f.db, &item.id, status).unwrap();
        assert_eq!(get_work_item(&f.db, &item.id).unwrap().status, status);
        assert!(iso(&times(&item.id).1));
        set_status(&f.db, &item.id, WorkStatus::Queued).unwrap();
        assert_eq!(times(&item.id), (None, None));
    }
}

#[test]
fn finds_the_latest_attempt_for_a_message() {
    let f = fixture();
    let with_message = || NewWork {
        message_id: Some("m1".into()),
        ..f.work()
    };
    f.add("first try", WorkSource::You, with_message());
    let retry = f.add("first try", WorkSource::You, with_message());
    f.tie();
    assert_eq!(
        last_item_for_message(&f.db, "m1", None)
            .unwrap()
            .unwrap()
            .id,
        retry.id
    );
    assert_eq!(last_item_for_message(&f.db, "nope", None).unwrap(), None);
}

#[test]
fn marks_running_work_interrupted_after_a_restart_and_leaves_queued_work_alone() {
    let f = fixture();
    let running = f.add("running", WorkSource::You, f.work());
    let queued = f.add("queued", WorkSource::You, f.work());
    set_status(&f.db, &running.id, WorkStatus::Running).unwrap();
    let recovered: Vec<(String, WorkStatus)> = recover_after_restart(&f.db)
        .unwrap()
        .into_iter()
        .map(|item| (item.id, item.status))
        .collect();
    assert_eq!(recovered, [(running.id.clone(), WorkStatus::Interrupted)]);
    assert_eq!(
        get_work_item(&f.db, &running.id).unwrap().status,
        WorkStatus::Interrupted
    );
    assert!(
        f.column("SELECT ended_at FROM work_items WHERE id = ?1", &running.id)
            .is_some()
    );
    assert_eq!(
        get_work_item(&f.db, &queued.id).unwrap().status,
        WorkStatus::Queued
    );
    assert_eq!(
        next_queued(&f.db, &f.bot_id).unwrap().unwrap().id,
        queued.id
    );
    assert_eq!(recover_after_restart(&f.db).unwrap(), []);
}

#[test]
fn counts_bot_to_bot_requests_per_chain_up_to_the_limit() {
    let f = fixture();
    let made = |chain: &str| -> i64 {
        f.db.query_row(
            "SELECT requests_made FROM chains WHERE id = ?1",
            [chain],
            |row| row.get(0),
        )
        .unwrap()
    };
    let chain = new_chain(&f.db).unwrap();
    assert_eq!(made(&chain), 0);
    assert_eq!(
        [1, 2, 3].map(|_| use_chain_request(&f.db, &chain, 2).unwrap()),
        [true, true, false]
    );
    assert_eq!(made(&chain), 2);
    assert!(!use_chain_request(&f.db, &chain, 2).unwrap());
    assert_eq!(made(&chain), 2);
    let independent = new_chain(&f.db).unwrap();
    assert_ne!(independent, chain);
    assert!(use_chain_request(&f.db, &independent, 2).unwrap());
}
