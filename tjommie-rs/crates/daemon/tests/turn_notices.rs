//! `turn-notices.test.ts`

mod common;

use common::{TempRoot, new_bot, test_db};
use daemon::queue::{NewWork, WorkItem, WorkSource, WorkStatus, enqueue, new_chain, set_status};
use daemon::turn_notices::{TurnOutcome, TurnResult, turn_notice};
use protocol::wire::Notify;
use rusqlite::Connection;

struct Fixture {
    db: Connection,
    bot_id: String,
    root: TempRoot,
}

fn setup() -> Fixture {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    let bot_id = new_bot(&db, &root.0.join("bots"), "Writer", "codex").id;
    Fixture { db, bot_id, root }
}

fn result(outcome: TurnOutcome, text: Option<&str>, error: Option<&str>) -> TurnResult {
    TurnResult {
        outcome,
        text: text.map(Into::into),
        error: error.map(Into::into),
        text_message_id: None,
    }
}

fn done(text: Option<&str>) -> TurnResult {
    result(TurnOutcome::Done, text, None)
}

fn failed(error: Option<&str>) -> TurnResult {
    result(TurnOutcome::Failed, None, error)
}

fn done_notice(bot_id: &str, room_id: Option<&str>, title: &str, body: &str) -> Option<Notify> {
    Some(Notify::Done {
        bot_id: bot_id.into(),
        title: title.into(),
        body: body.into(),
        room_id: room_id.map(Into::into),
    })
}

fn failed_notice(bot_id: &str, room_id: Option<&str>, title: &str, body: &str) -> Option<Notify> {
    Some(Notify::Failed {
        bot_id: bot_id.into(),
        title: title.into(),
        body: body.into(),
        room_id: room_id.map(Into::into),
    })
}

impl Fixture {
    fn enqueue(&self, source: WorkSource, text: &str, extra: NewWork) -> WorkItem {
        enqueue(
            &self.db,
            &NewWork {
                source,
                text: text.into(),
                ..extra
            },
        )
        .unwrap()
    }

    fn work(&self, chain_id: Option<&str>) -> NewWork {
        NewWork {
            chain_id: chain_id.map(Into::into),
            ..NewWork::new(&self.bot_id, WorkSource::You, "")
        }
    }

    fn notice(&self, item: &WorkItem, result: &TurnResult) -> Option<Notify> {
        turn_notice(&self.db, item, result).unwrap()
    }
}

#[test]
fn announces_a_finished_message_once_the_chain_has_no_other_work() {
    let f = setup();
    let chain = new_chain(&f.db).unwrap();
    let item = f.enqueue(WorkSource::You, "Write", f.work(Some(&chain)));
    set_status(&f.db, &item.id, WorkStatus::Running).unwrap();
    assert!(matches!(
        f.notice(&item, &done(None)),
        Some(Notify::Done { .. })
    ));
    set_status(&f.db, &item.id, WorkStatus::Done).unwrap();
    assert_eq!(
        f.notice(&item, &done(Some("A finished reply"))),
        done_notice(&f.bot_id, None, "Writer is done", "A finished reply")
    );
    let body = |text: &str| match f.notice(&item, &done(Some(text))) {
        Some(Notify::Done { body, .. }) => body,
        other => panic!("{other:?}"),
    };
    assert_eq!(body(&"x".repeat(141)).chars().count(), 140);
    assert_eq!(
        body(&format!("{}\u{1f600}y", "x".repeat(139))),
        format!("{}\u{1f600}", "x".repeat(139))
    );
    let detached = WorkItem {
        chain_id: None,
        status: WorkStatus::Running,
        ..item
    };
    assert_eq!(
        f.notice(&detached, &done(None)),
        done_notice(&f.bot_id, None, "Writer is done", "")
    );
}

#[test]
fn re_reads_a_chain_assigned_during_a_turn_before_announcing_completion() {
    let f = setup();
    let item = f.enqueue(WorkSource::You, "Delegate", f.work(None));
    let chain = new_chain(&f.db).unwrap();
    f.db.execute(
        "UPDATE work_items SET chain_id = ?1 WHERE id = ?2",
        [&chain, &item.id],
    )
    .unwrap();
    f.enqueue(WorkSource::BotRequest, "Research", f.work(Some(&chain)));
    assert_eq!(f.notice(&item, &done(None)), None);
}

#[test]
fn waits_through_a_bot_request_and_the_open_exchange_then_announces_the_askers_reply() {
    let f = setup();
    let chain = new_chain(&f.db).unwrap();
    let first = f.enqueue(WorkSource::You, "Delegate", f.work(Some(&chain)));
    let asked = f.enqueue(WorkSource::BotRequest, "Research", f.work(Some(&chain)));
    f.db.execute(
        "INSERT INTO exchanges(id,from_bot_id,to_bot_id,request,status,chain_id,reply_to,created_at) VALUES('exchange',?1,?1,'Research','waiting',?2,?1,'now')",
        [&f.bot_id, &chain],
    )
    .unwrap();
    set_status(&f.db, &first.id, WorkStatus::Done).unwrap();
    assert_eq!(f.notice(&first, &done(Some("First turn"))), None);
    set_status(&f.db, &asked.id, WorkStatus::Done).unwrap();
    assert_eq!(f.notice(&asked, &done(Some("Research"))), None);
    assert_eq!(f.notice(&first, &done(Some("First turn"))), None);
    let reply = f.enqueue(WorkSource::BotReply, "Result", f.work(Some(&chain)));
    f.db.execute(
        "UPDATE exchanges SET status = 'replied' WHERE id = 'exchange'",
        [],
    )
    .unwrap();
    assert_eq!(
        f.notice(&reply, &done(Some("Final answer"))),
        done_notice(&f.bot_id, None, "Writer is done", "Final answer")
    );
}

#[test]
fn waits_for_the_last_of_three_room_bots_and_routes_the_notice_to_the_room() {
    let f = setup();
    let chain = new_chain(&f.db).unwrap();
    let others: Vec<String> = ["Researcher", "Analyst"]
        .map(|name| new_bot(&f.db, &f.root.0.join("bots"), name, "codex").id)
        .into();
    let members = [f.bot_id.clone(), others[0].clone(), others[1].clone()];
    let items: Vec<WorkItem> = members
        .iter()
        .enumerate()
        .map(|(index, member)| {
            let work = NewWork {
                conversation_id: Some("team".into()),
                chain_id: Some(chain.clone()),
                ..NewWork::new(member, WorkSource::Room, &format!("Part {index}"))
            };
            enqueue(&f.db, &work).unwrap()
        })
        .collect();
    set_status(&f.db, &items[0].id, WorkStatus::Done).unwrap();
    assert_eq!(f.notice(&items[0], &done(None)), None);
    set_status(&f.db, &items[1].id, WorkStatus::Done).unwrap();
    assert_eq!(f.notice(&items[1], &done(None)), None);
    set_status(&f.db, &items[2].id, WorkStatus::Done).unwrap();
    assert_eq!(
        f.notice(&items[2], &done(Some("Last reply"))),
        done_notice(&others[1], Some("team"), "Done in Everyone", "Last reply")
    );
}

#[test]
fn announces_direct_failures_with_a_fallback_body_and_skips_delegated_failures() {
    let f = setup();
    let direct = f.enqueue(WorkSource::You, "Write", f.work(None));
    let delegated = f.enqueue(WorkSource::BotRequest, "Research", f.work(None));
    assert_eq!(
        f.notice(&direct, &failed(Some("CLI exited"))),
        failed_notice(&f.bot_id, None, "Writer ran into a problem", "CLI exited")
    );
    assert_eq!(
        f.notice(&direct, &failed(None)),
        failed_notice(
            &f.bot_id,
            None,
            "Writer ran into a problem",
            "Something went wrong."
        )
    );
    let room = f.enqueue(
        WorkSource::Room,
        "Report",
        NewWork {
            conversation_id: Some("team".into()),
            ..f.work(None)
        },
    );
    assert_eq!(
        f.notice(&room, &failed(Some("Room failed"))),
        failed_notice(
            &f.bot_id,
            Some("team"),
            "Writer ran into a problem",
            "Room failed"
        )
    );
    assert_eq!(f.notice(&delegated, &failed(Some("CLI exited"))), None);
}

#[test]
fn skips_interruptions_and_routine_completion_including_routine_chains() {
    let f = setup();
    let chain = new_chain(&f.db).unwrap();
    let routine = f.enqueue(
        WorkSource::Routine,
        "Digest",
        NewWork {
            routine_run_id: Some("run".into()),
            ..f.work(Some(&chain))
        },
    );
    let reply = f.enqueue(WorkSource::BotReply, "Answer", f.work(Some(&chain)));
    set_status(&f.db, &routine.id, WorkStatus::Done).unwrap();
    assert_eq!(f.notice(&routine, &done(None)), None);
    assert_eq!(f.notice(&reply, &done(None)), None);
    assert_eq!(
        f.notice(&reply, &failed(Some("Reply failed"))),
        failed_notice(&f.bot_id, None, "Writer ran into a problem", "Reply failed")
    );
    assert_eq!(
        f.notice(&reply, &result(TurnOutcome::Interrupted, None, None)),
        None
    );
}
