//! `scheduler.test.ts`: the cases, in the same order, with the same data and expected values. Test names are the TS
//! titles in snake_case; each `describe` is a module, and each `it.each` is a loop with the row in its messages.
//!
//! Where the TS cannot be copied literally:
//! - The case that drives the real `Runner` is in `scheduler_runner.rs`, with the M4 harness it needs.
//! - `vi.useFakeTimers` / `vi.setSystemTime`: the clock is an `AtomicI64` the test sets (`SchedulerDeps::clock`). Cases
//!   that advance timers use `#[tokio::test(start_paused = true)]` and `tokio::time::advance`. Work under a timer runs on
//!   the blocking pool, so after an advance the test waits, bounded, for its effect (`settle_until`).
//! - `start()` and interval cases use real time with a 30 ms `tick_every`. `vi.getTimerCount()` has no equivalent, so
//!   they check that the tick count stops rising after `stop()`. `TICK_MS` and `GRACE_MS` are asserted in their own case.
//! - `vi.spyOn(console, "error")` is the `log` recorder: `(context, error)` pairs.
//! - Injected failures are `RAISE(ABORT)` triggers, as in the TS. Where the TS mocks a method (`s.tick` once to throw),
//!   the `routines` table is renamed away for that tick and put back, since there is no method to mock.
//! - "process stopped" in the restart case is a panic in the broadcast, caught with `catch_unwind`: `flush` logs a
//!   broadcast error and carries on, so the TS per-routine catch has no direct counterpart to test.
//! - In the "throw" stop case a panic inside `stop_item` reaches the log only as tokio's `JoinError`, so that case checks
//!   the log context and the recorded outcome, not the message.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use daemon::bots::{create_bot, set_paused};
use daemon::chat::history;
use daemon::context::Db;
use daemon::queue::{
    NewWork, WorkItem, WorkSource, WorkStatus, enqueue, get_work_item, next_queued,
    recover_after_restart, set_status,
};
use daemon::routines::{
    add_run, create_routine, delete_routine, get_routine, get_run, has_open_run, list_runs,
    parse_iso, update_routine,
};
use daemon::runner::{Broadcast, Log, TurnOutcome, TurnResult};
use daemon::scheduler::{
    Deliver, GRACE_MS, Scheduler, SchedulerDeps, SchedulerHost, StopItem, TICK_MS,
    acknowledge_failure, duration, pending_failures, routine_prompt,
};
use daemon::server::RpcError;
use futures_util::future::BoxFuture;
use protocol::ErrorCode;
use protocol::Event;
use protocol::model::{
    BotInput, Message, MessageData, MessageKind, Routine, RoutineInput, RoutineLabelData,
    RoutinePatch, RoutineRun, RoutineRunStatus, message_data,
};
use protocol::model::RoutineRunStatus::{Done, Failed, Missed, Queued, Running, Skipped, Stopped};
use protocol::wire::{Notify, PendingNotification};
use rusqlite::{Connection, Row, params};
use serde_json::{Value, json};
use tempfile::TempDir;

fn at(iso: &str) -> i64 {
    parse_iso(iso).unwrap_or_else(|| panic!("not a time: {iso}"))
}

fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

/// `vi.advanceTimersByTime` on tokio's paused clock. A limit that `run_started` spawned registers its sleep only once
/// it is polled, so the task runs before the clock moves; otherwise its deadline would land after the advance.
async fn advance(by: Duration) {
    tokio::task::yield_now().await;
    tokio::time::advance(by).await;
}

/// The database lock. A panic inside a tick poisons it, but the data is the committed state, so read through it.
fn lock(db: &Db) -> MutexGuard<'_, Connection> {
    db.lock().unwrap_or_else(PoisonError::into_inner)
}

fn bot_input() -> BotInput {
    serde_json::from_value(json!({
        "name": "Night Shift",
        "instructions": "",
        "color": "purple",
        "shape": "cloud",
        "provider": "codex",
        "model": "gpt-5.6-sol",
    }))
    .unwrap()
}

/// `beforeEach`. `db` comes first so it closes before `root` is removed; `root` keeps the folder alive.
struct Fixture {
    db: Db,
    bot_id: String,
    clock: Arc<AtomicI64>,
    events: Arc<Mutex<Vec<Event>>>,
    delivered: Arc<Mutex<Vec<WorkItem>>>,
    stopped: Arc<Mutex<Vec<(String, String)>>>,
    logs: Arc<Mutex<Vec<(String, String)>>>,
    root: TempDir,
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.root.path().join("t.db")
    }

    /// `db.close(); db = openDb(...)`. The old connection closes once no scheduler still holds it.
    fn reopen(&mut self) {
        self.db = Arc::new(Mutex::new(daemon::db::open(&self.path()).unwrap()));
    }
}

fn fixture() -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap();
    let db: Db = Arc::new(Mutex::new(
        daemon::db::open(&root.path().join("t.db")).unwrap(),
    ));
    let bot = create_bot(&lock(&db), &root.path().join("bots"), &bot_input()).unwrap();
    Fixture {
        db,
        bot_id: bot.id,
        clock: Arc::new(AtomicI64::new(0)),
        events: Arc::default(),
        delivered: Arc::default(),
        stopped: Arc::default(),
        logs: Arc::default(),
        root,
    }
}

fn recording_broadcast(events: &Arc<Mutex<Vec<Event>>>) -> Broadcast {
    let events = events.clone();
    Arc::new(move |event: Event| -> Result<(), String> {
        events.lock().unwrap().push(event);
        Ok(())
    })
}

fn recording_deliver(delivered: &Arc<Mutex<Vec<WorkItem>>>) -> Deliver {
    let delivered = delivered.clone();
    Arc::new(move |work: &WorkItem| -> Result<(), String> {
        delivered.lock().unwrap().push(work.clone());
        Ok(())
    })
}

fn recording_stop(stopped: &Arc<Mutex<Vec<(String, String)>>>) -> StopItem {
    let stopped = stopped.clone();
    Arc::new(move |item: &str, notice: &str| -> BoxFuture<'static, Result<(), String>> {
        stopped
            .lock()
            .unwrap()
            .push((item.to_owned(), notice.to_owned()));
        futures_util::FutureExt::boxed(async { Ok::<(), String>(()) })
    })
}

fn recording_log(logs: &Arc<Mutex<Vec<(String, String)>>>) -> Log {
    let logs = logs.clone();
    Arc::new(move |context: &str, error: &str| {
        logs.lock()
            .unwrap()
            .push((context.to_owned(), error.to_owned()));
    })
}

fn clock_of(clock: &Arc<AtomicI64>) -> Arc<dyn Fn() -> i64 + Send + Sync> {
    let clock = clock.clone();
    Arc::new(move || clock.load(SeqCst))
}

/// The scheduler's dependencies, recording into the fixture. Tests override the fields they need with struct update.
fn deps(f: &Fixture) -> SchedulerDeps {
    SchedulerDeps {
        db: f.db.clone(),
        broadcast: recording_broadcast(&f.events),
        deliver: recording_deliver(&f.delivered),
        stop_item: recording_stop(&f.stopped),
        log: recording_log(&f.logs),
        clock: clock_of(&f.clock),
        tick_every: Duration::from_secs(3600),
    }
}

fn scheduler(f: &Fixture) -> Scheduler {
    Scheduler::new(deps(f))
}

/// Every day at 08:00 and 17:00 UTC; the first run is due at 2026-09-20 08:00.
fn digest(f: &Fixture) -> Routine {
    digest_with(f, json!({}))
}

/// `digest(extra)`: the TS defaults with `extra` laid over them.
fn digest_with(f: &Fixture, extra: Value) -> Routine {
    let mut body = json!({
        "botId": f.bot_id,
        "name": "Digest",
        "prompt": "Summarize my inbox",
        "days": [0, 1, 2, 3, 4, 5, 6],
        "times": ["08:00", "17:00"],
        "timeZone": "UTC",
    });
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    let input: RoutineInput = serde_json::from_value(body).unwrap();
    create_routine(&lock(&f.db), &input, at("2026-09-20T00:00:00Z")).unwrap()
}

fn turn(outcome: TurnOutcome, error: Option<&str>) -> TurnResult {
    TurnResult {
        outcome,
        text: None,
        text_message_id: None,
        error: error.map(str::to_owned),
    }
}

/// `runner`'s side of a retry: the database it holds and the work it hands over.
struct Host<'a> {
    db: &'a Connection,
    delivered: Vec<WorkItem>,
    fail: Option<String>,
}

impl SchedulerHost for Host<'_> {
    fn db(&self) -> &Connection {
        self.db
    }

    fn deliver(&mut self, item: &WorkItem) -> Result<(), String> {
        if let Some(error) = &self.fail {
            return Err(error.clone());
        }
        self.delivered.push(item.clone());
        Ok(())
    }
}

/// `s.retry(message)`. The database is locked only for the call, so the caller can read afterwards. Returns what the
/// retry did and what it handed to the runner.
fn retry(s: &Scheduler, f: &Fixture, message: &Message) -> (Result<Message, RpcError>, Vec<WorkItem>) {
    let guard = lock(&f.db);
    let mut host = Host {
        db: &guard,
        delivered: Vec::new(),
        fail: None,
    };
    let result = s.retry(&mut host, message);
    (result, host.delivered)
}

fn runs(db: &Db, routine_id: &str) -> Vec<RoutineRun> {
    list_runs(&lock(db), routine_id).unwrap()
}

fn statuses(db: &Db, routine_id: &str) -> Vec<RoutineRunStatus> {
    runs(db, routine_id).into_iter().map(|run| run.status).collect()
}

fn status_due(db: &Db, routine_id: &str) -> Vec<(RoutineRunStatus, String)> {
    runs(db, routine_id)
        .into_iter()
        .map(|run| (run.status, run.due_at))
        .collect()
}

fn status_notes(db: &Db, routine_id: &str) -> Vec<(RoutineRunStatus, Option<String>)> {
    runs(db, routine_id)
        .into_iter()
        .map(|run| (run.status, run.note))
        .collect()
}

fn run_of(db: &Db, id: &str) -> Option<RoutineRun> {
    get_run(&lock(db), id).unwrap()
}

fn routine_of(db: &Db, id: &str) -> Routine {
    get_routine(&lock(db), id).unwrap()
}

fn next_run(db: &Db, routine_id: &str) -> Option<String> {
    routine_of(db, routine_id).next_run_at
}

fn work_of(db: &Db, id: &str) -> WorkItem {
    get_work_item(&lock(db), id).unwrap()
}

fn open_run(db: &Db, routine_id: &str) -> bool {
    has_open_run(&lock(db), routine_id).unwrap()
}

fn pending(db: &Db) -> Vec<PendingNotification> {
    pending_failures(&lock(db)).unwrap()
}

fn chat(db: &Db, bot_id: &str) -> Vec<Message> {
    history(&lock(db), bot_id, None, 10).unwrap()
}

fn label_data(message: &Message) -> RoutineLabelData {
    match message_data(message).unwrap() {
        MessageData::Routine(data) => data,
        other => panic!("not a routine label: {other:?}"),
    }
}

/// `SELECT ... FROM <table>`-style reads: every row, read by `read`.
fn rows<T>(
    db: &Connection,
    sql: &str,
    read: impl Fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Vec<T> {
    let mut statement = db.prepare(sql).unwrap();
    statement
        .query_map([], |row| read(row))
        .unwrap()
        .collect::<rusqlite::Result<Vec<T>>>()
        .unwrap()
}

fn count(db: &Db, table: &str) -> i64 {
    lock(db)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
        .unwrap()
}

fn chain_rows(db: &Db) -> Vec<(String, i64)> {
    rows(
        &lock(db),
        "SELECT id, requests_made FROM chains ORDER BY id",
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

fn chain_ids(db: &Connection) -> Vec<String> {
    rows(db, "SELECT id FROM chains ORDER BY id", |row| row.get(0))
}

/// Everything a refused retry must leave alone.
#[derive(Debug, PartialEq)]
struct Snapshot {
    routine: Routine,
    runs: Vec<RoutineRun>,
    messages: Vec<Message>,
    chains: Vec<(String, i64, String)>,
    work: Vec<WorkItem>,
}

fn snapshot(db: &Db, routine_id: &str, bot_id: &str) -> Snapshot {
    let db = lock(db);
    let work_ids: Vec<String> = rows(&db, "SELECT id FROM work_items ORDER BY id", |row| {
        row.get(0)
    });
    Snapshot {
        routine: get_routine(&db, routine_id).unwrap(),
        runs: list_runs(&db, routine_id).unwrap(),
        messages: history(&db, bot_id, None, 10).unwrap(),
        chains: rows(
            &db,
            "SELECT id, requests_made, created_at FROM chains ORDER BY id",
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ),
        work: work_ids
            .iter()
            .map(|id| get_work_item(&db, id).unwrap())
            .collect(),
    }
}

fn events_of(f: &Fixture) -> Vec<Event> {
    f.events.lock().unwrap().clone()
}

/// The wire names of the broadcast events, as the TS asserts them (`events.map(e => e.event)`).
fn event_names(f: &Fixture) -> Vec<String> {
    events_of(f)
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn notices(f: &Fixture) -> Vec<Notify> {
    events_of(f)
        .into_iter()
        .filter_map(|event| match event {
            Event::Notify(notice) => Some(notice),
            _ => None,
        })
        .collect()
}

fn first_notice(f: &Fixture) -> Notify {
    notices(f).into_iter().next().expect("a notify event")
}

fn stopped(f: &Fixture) -> Vec<(String, String)> {
    f.stopped.lock().unwrap().clone()
}

fn delivered_items(f: &Fixture) -> Vec<WorkItem> {
    f.delivered.lock().unwrap().clone()
}

fn delivered_item(f: &Fixture, index: usize) -> WorkItem {
    delivered_items(f)[index].clone()
}

fn logs(f: &Fixture) -> Vec<(String, String)> {
    f.logs.lock().unwrap().clone()
}

/// The TS `toHaveBeenCalledWith(context, objectContaining({ message }))`: a log with that exact context whose error
/// text contains `error`.
fn logged_error(f: &Fixture, context: &str, error: &str) -> bool {
    logs(f)
        .iter()
        .any(|(logged, text)| logged == context && text.contains(error))
}

fn logged_context(f: &Fixture, context: &str) -> bool {
    logs(f).iter().any(|(logged, _)| logged == context)
}

/// What the TS `observe` checks: everything a tick wrote is committed before any broadcast or wake.
fn assert_committed(reader: &Db, routine_id: &str, bot_id: &str) {
    let db = lock(reader);
    assert_eq!(
        get_routine(&db, routine_id).unwrap().next_run_at.as_deref(),
        Some("2026-09-20T17:00:00.000Z")
    );
    let run = list_runs(&db, routine_id).unwrap().remove(0);
    let item = next_queued(&db, bot_id).unwrap().expect("queued work");
    assert_eq!(item.routine_run_id.as_deref(), Some(run.id.as_str()));
    let labels = history(&db, bot_id, None, 10).unwrap();
    assert_eq!(labels.len(), 1);
    assert_eq!(item.message_id.as_deref(), Some(labels[0].id.as_str()));
    assert_eq!(label_data(&labels[0]).run_id, run.id);
    assert_eq!(chain_ids(&db), vec![item.chain_id.clone().unwrap()]);
}

/// Lets the runtime run for about `ms` of real time, so timers and blocking work that are due get to run. Paused
/// time does not move: `yield_now` never auto-advances it.
async fn let_runtime_run(ms: u64) {
    for _ in 0..(ms / 10) {
        std::thread::sleep(Duration::from_millis(10));
        tokio::task::yield_now().await;
    }
}

/// Like [`let_runtime_run`], but returns as soon as `condition` holds. Gives up after 2 s and lets the assertion fail.
async fn settle_until(condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
        tokio::task::yield_now().await;
    }
}

fn stop_that_throws(_item: &str, _notice: &str) -> BoxFuture<'static, Result<(), String>> {
    panic!("Could not interrupt")
}

fn stop_that_rejects(_item: &str, _notice: &str) -> BoxFuture<'static, Result<(), String>> {
    futures_util::FutureExt::boxed(async { Err("Could not interrupt".to_owned()) })
}

mod scheduler {
    use super::*;

    #[test]
    fn retains_startup_failures_across_database_reopen_until_explicitly_acknowledged() {
        let mut f = fixture();
        let routine = digest(&f);
        let orphan = add_run(
            &lock(&f.db),
            &routine.id,
            "2026-09-19T17:00:00.000Z",
            Queued,
            None,
        )
        .unwrap();
        let first = scheduler(&f);
        first.recover().unwrap();
        drop(first);
        f.reopen();
        let restarted = scheduler(&f);
        restarted.recover().unwrap();
        let expected = vec![PendingNotification::RoutineFailed {
            bot_id: f.bot_id.clone(),
            routine_id: routine.id.clone(),
            run_id: orphan.id.clone(),
            title: "Night Shift's routine failed".to_owned(),
            body: "Digest: Tjommie restarted before this routine run could finish.".to_owned(),
        }];
        assert_eq!(pending(&f.db), expected);
        assert_eq!(pending(&f.db), expected); // fetching is not delivery
        assert_eq!(chat(&f.db, &f.bot_id).len(), 1);
        acknowledge_failure(&lock(&f.db), &orphan.id).unwrap();
        acknowledge_failure(&lock(&f.db), &orphan.id).unwrap(); // an acknowledgement can be retried
        drop(restarted);
        f.reopen();
        let again = scheduler(&f);
        again.recover().unwrap();
        assert!(pending(&f.db).is_empty());
        assert_eq!(run_of(&f.db, &orphan.id).unwrap().status, Failed);
        assert_eq!(chat(&f.db, &f.bot_id).len(), 1);
    }

    #[test]
    fn rolls_back_recovery_status_and_history_if_its_durable_notification_cannot_be_stored() {
        let f = fixture();
        let routine = digest(&f);
        let orphan = add_run(
            &lock(&f.db),
            &routine.id,
            "2026-09-19T17:00:00.000Z",
            Running,
            None,
        )
        .unwrap();
        let s = scheduler(&f);
        lock(&f.db)
            .execute_batch(
                "CREATE TRIGGER reject_notice BEFORE INSERT ON routine_failure_notifications
                 BEGIN SELECT RAISE(ABORT, 'notice unavailable'); END",
            )
            .unwrap();
        let error = s.recover().unwrap_err();
        assert!(error.message.contains("notice unavailable"), "{error}");
        assert_eq!(run_of(&f.db, &orphan.id).unwrap().status, Running);
        assert!(chat(&f.db, &f.bot_id).is_empty());
        assert!(events_of(&f).is_empty());
        lock(&f.db)
            .execute_batch("DROP TRIGGER reject_notice")
            .unwrap();
        s.recover().unwrap();
        assert_eq!(pending(&f.db).len(), 1);
    }

    #[test]
    fn removes_pending_routine_notices_when_their_routine_is_deleted() {
        let f = fixture();
        let routine = digest(&f);
        add_run(
            &lock(&f.db),
            &routine.id,
            "2026-09-19T17:00:00.000Z",
            Queued,
            None,
        )
        .unwrap();
        let s = scheduler(&f);
        s.recover().unwrap();
        delete_routine(&lock(&f.db), &routine.id).unwrap();
        assert!(pending(&f.db).is_empty());
    }

    #[test]
    fn rolls_back_the_entire_firing_when_persisting_a_table_fails() {
        for (table, verb) in [
            ("routine_runs", "INSERT"),
            ("routines", "UPDATE"),
            ("messages", "INSERT"),
            ("chains", "INSERT"),
            ("work_items", "INSERT"),
        ] {
            let f = fixture();
            let routine = digest(&f);
            let s = scheduler(&f);
            lock(&f.db)
                .execute_batch(&format!(
                    "CREATE TRIGGER fail_schedule BEFORE {verb} ON {table}
                     BEGIN SELECT RAISE(ABORT, 'simulated persistence failure'); END"
                ))
                .unwrap();
            s.tick(at("2026-09-20T08:00:10Z")).unwrap();
            assert!(
                logged_error(
                    &f,
                    &format!("Routine scheduler failed for {}.", routine.id),
                    "simulated persistence failure"
                ),
                "{table}: {:?}",
                logs(&f)
            );
            assert_eq!(routine_of(&f.db, &routine.id), routine, "{table}");
            for name in ["routine_runs", "messages", "chains", "work_items"] {
                assert_eq!(count(&f.db, name), 0, "{table}: {name} is not empty");
            }
            assert!(events_of(&f).is_empty(), "{table}");
            assert!(delivered_items(&f).is_empty(), "{table}");
            lock(&f.db)
                .execute_batch("DROP TRIGGER fail_schedule")
                .unwrap();
            s.tick(at("2026-09-20T08:00:30Z")).unwrap();
            assert_eq!(runs(&f.db, &routine.id).len(), 1, "{table}");
            assert_eq!(delivered_items(&f).len(), 1, "{table}");
        }
    }

    #[test]
    fn keeps_firing_later_due_routines_while_one_routine_repeatedly_fails_to_persist() {
        let f = fixture();
        let failing = digest(&f);
        let later = digest_with(&f, json!({ "times": ["08:01", "17:01"] }));
        let s = scheduler(&f);
        lock(&f.db)
            .execute_batch(&format!(
                "CREATE TRIGGER fail_one_routine BEFORE INSERT ON work_items
                 WHEN (SELECT routine_id FROM routine_runs WHERE id = NEW.routine_run_id) = '{}'
                 BEGIN SELECT RAISE(ABORT, 'one routine unavailable'); END",
                failing.id
            ))
            .unwrap();
        s.tick(at("2026-09-20T08:01:10Z")).unwrap();
        assert_eq!(routine_of(&f.db, &failing.id), failing);
        assert!(runs(&f.db, &failing.id).is_empty());
        assert_eq!(statuses(&f.db, &later.id), vec![Queued]);
        let items = delivered_items(&f);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].routine_run_id,
            Some(runs(&f.db, &later.id)[0].id.clone())
        );
        assert_eq!(count(&f.db, "chains"), 1);
        assert_eq!(chat(&f.db, &f.bot_id).len(), 1);
        s.tick(at("2026-09-20T08:01:30Z")).unwrap();
        assert_eq!(logs(&f).len(), 2);
        assert_eq!(routine_of(&f.db, &failing.id), failing);
        assert!(runs(&f.db, &failing.id).is_empty());
        assert_eq!(delivered_items(&f).len(), 1);
        lock(&f.db)
            .execute_batch("DROP TRIGGER fail_one_routine")
            .unwrap();
        s.tick(at("2026-09-20T08:01:45Z")).unwrap();
        assert_eq!(statuses(&f.db, &failing.id), vec![Queued]);
        assert_eq!(delivered_items(&f).len(), 2);
    }

    #[test]
    fn commits_the_schedule_label_and_work_before_broadcasting_or_waking_the_runner() {
        let f = fixture();
        let routine = digest(&f);
        // The TS `reader` is a second connection, so the observer reads only what the tick has committed.
        let reader: Db = Arc::new(Mutex::new(daemon::db::open(&f.path()).unwrap()));
        let broadcast: Broadcast = {
            let (reader, routine_id, bot_id) =
                (reader.clone(), routine.id.clone(), f.bot_id.clone());
            Arc::new(move |_event: Event| -> Result<(), String> {
                assert_committed(&reader, &routine_id, &bot_id);
                Ok(())
            })
        };
        let deliver: Deliver = {
            let (reader, routine_id, bot_id) =
                (reader.clone(), routine.id.clone(), f.bot_id.clone());
            Arc::new(move |work: &WorkItem| -> Result<(), String> {
                assert_committed(&reader, &routine_id, &bot_id);
                assert_eq!(get_work_item(&lock(&reader), &work.id).unwrap(), *work);
                Ok(())
            })
        };
        let s = Scheduler::new(SchedulerDeps {
            broadcast,
            deliver,
            ..deps(&f)
        });
        s.tick(at("2026-09-20T08:00:10Z")).unwrap();
        assert_eq!(count(&f.db, "work_items"), 1);
    }

    #[test]
    fn retains_committed_work_across_a_restart_before_the_runner_is_woken_without_firing_twice() {
        let mut f = fixture();
        let routine = digest(&f);
        // Terminated after COMMIT, at the first external effect and before deliver. A panic in the broadcast stands
        // in for the crash.
        let crashing = Scheduler::new(SchedulerDeps {
            broadcast: Arc::new(|_event: Event| -> Result<(), String> {
                panic!("process stopped")
            }),
            deliver: Arc::new(|_work: &WorkItem| -> Result<(), String> {
                panic!("must not wake")
            }),
            ..deps(&f)
        });
        let crashed = catch_unwind(AssertUnwindSafe(|| {
            crashing.tick(at("2026-09-20T08:00:10Z"))
        }));
        let payload = crashed.expect_err("the tick stops at its first broadcast");
        assert_eq!(
            payload.downcast_ref::<&str>().copied(),
            Some("process stopped")
        );
        let item = next_queued(&lock(&f.db), &f.bot_id)
            .unwrap()
            .expect("committed work survives the crash");
        drop(crashing);
        f.reopen();
        assert!(recover_after_restart(&lock(&f.db)).unwrap().is_empty());
        let restarted = scheduler(&f);
        restarted.recover().unwrap();
        restarted.tick(at("2026-09-20T08:00:30Z")).unwrap();
        assert_eq!(next_queued(&lock(&f.db), &f.bot_id).unwrap(), Some(item.clone()));
        let listed = runs(&f.db, &routine.id);
        assert_eq!(listed.len(), 1);
        assert_eq!(Some(listed[0].id.clone()), item.routine_run_id);
        assert_eq!(listed[0].status, Queued);
        assert!(delivered_items(&f).is_empty());
        assert!(events_of(&f).is_empty());
    }

    #[tokio::test]
    async fn settles_a_pre_existing_orphan_run_on_startup_and_releases_future_scheduling() {
        for status in [Queued, Running] {
            let f = fixture();
            let routine = digest(&f);
            let orphan = add_run(
                &lock(&f.db),
                &routine.id,
                "2026-09-19T17:00:00.000Z",
                status,
                None,
            )
            .unwrap();
            f.clock.store(at("2026-09-20T07:59:00Z"), SeqCst);
            let s = scheduler(&f);
            s.start().unwrap();
            let settled = run_of(&f.db, &orphan.id).unwrap();
            assert_eq!(settled.status, Failed, "{status:?}");
            assert!(settled.ended_at.is_some(), "{status:?}");
            assert!(
                settled.note.as_deref().unwrap_or_default().contains("restarted"),
                "{status:?}"
            );
            assert!(!open_run(&f.db, &routine.id), "{status:?}");
            let labels = chat(&f.db, &f.bot_id);
            assert_eq!(labels.len(), 1, "{status:?}");
            let data = label_data(&labels[0]);
            assert_eq!(data.run_id, orphan.id, "{status:?}");
            assert_eq!(data.status, Failed, "{status:?}");
            assert_eq!(notices(&f).len(), 1, "{status:?}");
            f.events.lock().unwrap().clear();
            s.recover().unwrap();
            assert!(events_of(&f).is_empty(), "{status:?}");
            s.tick(at("2026-09-20T08:00:10Z")).unwrap();
            assert_eq!(delivered_items(&f).len(), 1, "{status:?}");
            assert_eq!(statuses(&f.db, &routine.id), vec![Queued, Failed], "{status:?}");
            s.stop();
        }
    }

    #[test]
    fn settles_an_open_run_left_behind_after_its_work_became_done_failed_or_interrupted() {
        for status in [WorkStatus::Done, WorkStatus::Failed, WorkStatus::Interrupted] {
            let f = fixture();
            let routine = digest(&f);
            let s = scheduler(&f);
            s.tick(at("2026-09-20T08:00:10Z")).unwrap();
            let item = delivered_item(&f, 0);
            set_status(&lock(&f.db), &item.id, status).unwrap();
            s.recover().unwrap();
            let run_id = item.routine_run_id.as_deref().unwrap();
            assert_eq!(run_of(&f.db, run_id).unwrap().status, Failed, "{status:?}");
            let labels = chat(&f.db, &f.bot_id);
            assert_eq!(labels.len(), 1, "{status:?}");
            assert_eq!(label_data(&labels[0]).status, Failed, "{status:?}");
            assert!(!open_run(&f.db, &routine.id), "{status:?}");
        }
    }

    #[test]
    fn preserves_open_routine_runs_with_queued_or_running_work_during_reconciliation() {
        for status in [WorkStatus::Queued, WorkStatus::Running] {
            let f = fixture();
            let routine = digest(&f);
            let s = scheduler(&f);
            s.tick(at("2026-09-20T08:00:10Z")).unwrap();
            let item = delivered_item(&f, 0);
            // running + queued is also valid during a usage-limit retry.
            s.run_started(&lock(&f.db), &item).unwrap();
            set_status(&lock(&f.db), &item.id, status).unwrap();
            let before = runs(&f.db, &routine.id);
            f.events.lock().unwrap().clear();
            s.recover().unwrap();
            assert_eq!(runs(&f.db, &routine.id), before, "{status:?}");
            assert_eq!(work_of(&f.db, &item.id).status, status, "{status:?}");
            assert!(events_of(&f).is_empty(), "{status:?}");
        }
    }

    #[test]
    fn rolls_back_a_manual_retry_when_queue_persistence_fails() {
        let f = fixture();
        let routine = digest(&f);
        set_paused(&lock(&f.db), &f.bot_id, true).unwrap();
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:10Z")).unwrap();
        set_paused(&lock(&f.db), &f.bot_id, false).unwrap();
        let label = chat(&f.db, &f.bot_id).remove(0);
        let before = runs(&f.db, &routine.id);
        f.events.lock().unwrap().clear();
        lock(&f.db)
            .execute_batch(
                "CREATE TRIGGER fail_retry BEFORE INSERT ON work_items
                 BEGIN SELECT RAISE(ABORT, 'queue unavailable'); END",
            )
            .unwrap();
        let (result, _) = retry(&s, &f, &label);
        assert!(result.unwrap_err().message.contains("queue unavailable"));
        assert_eq!(runs(&f.db, &routine.id), before);
        assert_eq!(chat(&f.db, &f.bot_id), vec![label.clone()]);
        assert_eq!(count(&f.db, "chains"), 0);
        assert_eq!(count(&f.db, "work_items"), 0);
        assert!(events_of(&f).is_empty());
        assert!(delivered_items(&f).is_empty());
        lock(&f.db)
            .execute_batch("DROP TRIGGER fail_retry")
            .unwrap();
        let (retried, by_retry) = retry(&s, &f, &label);
        let retried = retried.unwrap();
        assert_eq!(by_retry.len(), 1);
        assert_eq!(
            work_of(&f.db, &by_retry[0].id).message_id,
            Some(retried.id.clone())
        );
    }

    #[test]
    fn publishes_one_final_label_when_a_due_run_is_missed_paused_running_or_queued() {
        let rows = [
            (
                "missed",
                Missed,
                "The PC was off or asleep, or Tjommie wasn't running.",
                "2026-09-20T08:03:00Z",
            ),
            ("paused", Skipped, "Night Shift was paused.", "2026-09-20T08:00:10Z"),
            (
                "running",
                Skipped,
                "The previous run was still going.",
                "2026-09-20T08:00:10Z",
            ),
            (
                "queued",
                Skipped,
                "The previous run was still going.",
                "2026-09-20T08:00:10Z",
            ),
        ];
        for (reason, status, note, tick) in rows {
            let f = fixture();
            let routine = digest(&f);
            if reason == "paused" {
                set_paused(&lock(&f.db), &f.bot_id, true).unwrap();
            }
            if reason == "running" || reason == "queued" {
                let open = if reason == "running" { Running } else { Queued };
                add_run(
                    &lock(&f.db),
                    &routine.id,
                    "2026-09-19T17:00:00.000Z",
                    open,
                    None,
                )
                .unwrap();
            }
            scheduler(&f).tick(at(tick)).unwrap();
            let labels = chat(&f.db, &f.bot_id);
            assert_eq!(labels.len(), 1, "{reason}");
            assert_eq!(labels[0].kind, MessageKind::Routine, "{reason}");
            let data = label_data(&labels[0]);
            assert_eq!(data.status, status, "{reason}");
            assert_eq!(data.note.as_deref(), Some(note), "{reason}");
            let label_events: Vec<Event> = events_of(&f)
                .into_iter()
                .filter(|event| {
                    matches!(event, Event::MessageAdded(_) | Event::MessageUpdated(_))
                })
                .collect();
            assert_eq!(
                label_events,
                vec![Event::MessageAdded(labels[0].clone())],
                "{reason}"
            );
            assert!(delivered_items(&f).is_empty(), "{reason}");
        }
    }

    #[test]
    fn refuses_a_routine_retry_for_a_paused_bot_or_disabled_routine_before_creating_work() {
        let rows = [
            ("paused bot", "Unpause this bot before retrying its routine"),
            ("disabled routine", "Enable this routine before retrying it"),
        ];
        for (reason, message) in rows {
            let f = fixture();
            let routine = digest(&f);
            let s = scheduler(&f);
            s.tick(at("2026-09-20T08:00:10Z")).unwrap();
            let item = work_of(&f.db, &delivered_item(&f, 0).id);
            s.run_started(&lock(&f.db), &item).unwrap();
            set_status(&lock(&f.db), &item.id, WorkStatus::Failed).unwrap();
            s.run_ended(
                &lock(&f.db),
                &item,
                &turn(TurnOutcome::Failed, Some("CLI exited")),
            )
            .unwrap();
            if reason == "paused bot" {
                set_paused(&lock(&f.db), &f.bot_id, true).unwrap();
            } else {
                update_routine(
                    &lock(&f.db),
                    &routine.id,
                    &RoutinePatch {
                        enabled: Some(false),
                        ..RoutinePatch::default()
                    },
                    at("2026-09-20T08:00:10Z"),
                )
                .unwrap();
            }
            let before = snapshot(&f.db, &routine.id, &f.bot_id);
            f.delivered.lock().unwrap().clear();
            f.events.lock().unwrap().clear();
            let label = before.messages[0].clone();
            let (result, by_retry) = retry(&s, &f, &label);
            let error = result.unwrap_err();
            assert_eq!(error.code, ErrorCode::BadState, "{reason}");
            assert_eq!(error.message, message, "{reason}");
            assert_eq!(snapshot(&f.db, &routine.id, &f.bot_id), before, "{reason}");
            assert!(by_retry.is_empty(), "{reason}");
            assert!(delivered_items(&f).is_empty(), "{reason}");
            assert!(events_of(&f).is_empty(), "{reason}");
        }
    }

    #[test]
    fn keeps_one_routine_label_current_through_done_failed_or_interrupted() {
        for outcome in [
            TurnOutcome::Done,
            TurnOutcome::Failed,
            TurnOutcome::Interrupted,
        ] {
            let f = fixture();
            let routine = digest(&f);
            let s = scheduler(&f);
            s.tick(at("2026-09-20T08:00:10Z")).unwrap();
            let label = chat(&f.db, &f.bot_id).remove(0);
            let run = runs(&f.db, &routine.id).remove(0);
            assert_eq!(label.kind, MessageKind::Routine, "{outcome:?}");
            let first = label_data(&label);
            assert_eq!(first.run_id, run.id, "{outcome:?}");
            assert_eq!(first.status, Queued, "{outcome:?}");
            assert_eq!(delivered_item(&f, 0).message_id, Some(label.id.clone()));
            let item = work_of(&f.db, &delivered_item(&f, 0).id);
            s.run_started(&lock(&f.db), &item).unwrap();
            let error = (outcome == TurnOutcome::Failed).then_some("CLI exited");
            s.run_ended(&lock(&f.db), &item, &turn(outcome, error)).unwrap();
            let (status, note) = match outcome {
                TurnOutcome::Done => (Done, None),
                TurnOutcome::Interrupted => (Stopped, Some("Stopped by you.")),
                TurnOutcome::Failed => (Failed, Some("CLI exited")),
            };
            let messages = chat(&f.db, &f.bot_id);
            assert_eq!(messages.len(), 1, "{outcome:?}");
            assert_eq!(messages[0].id, label.id, "{outcome:?}");
            let data = label_data(&messages[0]);
            assert_eq!(data.status, status, "{outcome:?}");
            assert_eq!(data.note.as_deref(), note, "{outcome:?}");
            assert!(
                events_of(&f).contains(&Event::MessageUpdated(messages[0].clone())),
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn queues_a_due_run_for_the_bot_with_a_chain_of_its_own() {
        let f = fixture();
        let routine = digest(&f);
        scheduler(&f).tick(at("2026-09-20T08:00:10Z")).unwrap();
        let run = runs(&f.db, &routine.id).remove(0);
        assert_eq!(run.status, Queued);
        assert_eq!(run.due_at, "2026-09-20T08:00:00.000Z");
        let items = delivered_items(&f);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.bot_id, f.bot_id);
        assert_eq!(item.source, WorkSource::Routine);
        assert!(item.text.contains("Summarize my inbox"));
        assert!(item.message_id.is_some());
        assert!(item.chain_id.is_some());
        assert_eq!(item.routine_run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(item.status, WorkStatus::Queued);
        assert_eq!(work_of(&f.db, &item.id), *item);
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-20T17:00:00.000Z".to_owned())
        );
        assert_eq!(chain_rows(&f.db), vec![(item.chain_id.clone().unwrap(), 0)]);
        let labels = chat(&f.db, &f.bot_id);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].kind, MessageKind::Routine);
        let data = label_data(&labels[0]);
        assert_eq!(data.run_id, run.id);
        assert_eq!(data.status, Queued);
        assert!(events_of(&f).contains(&Event::RoutineRun(run.clone())));
        assert!(events_of(&f).contains(&Event::RoutineUpdated(routine_of(&f.db, &routine.id))));
    }

    #[test]
    fn records_one_missed_run_when_the_pc_was_off_through_several_run_times_and_starts_nothing() {
        let f = fixture();
        let routine = digest(&f);
        let s = scheduler(&f);
        let missed = vec![(Missed, "2026-09-20T08:00:00.000Z".to_owned())];
        s.tick(at("2026-09-25T09:00:00Z")).unwrap(); // five days and ten run times later
        assert_eq!(status_due(&f.db, &routine.id), missed);
        assert!(delivered_items(&f).is_empty());
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-25T17:00:00.000Z".to_owned())
        );
        for tick_at in [
            "2026-09-25T09:00:30Z",
            "2026-09-25T09:01:00Z",
            "2026-09-25T09:01:30Z",
        ] {
            s.tick(at(tick_at)).unwrap();
            assert_eq!(status_due(&f.db, &routine.id), missed, "{tick_at}");
            assert!(delivered_items(&f).is_empty(), "{tick_at}");
            assert_eq!(
                next_run(&f.db, &routine.id),
                Some("2026-09-25T17:00:00.000Z".to_owned()),
                "{tick_at}"
            );
        }
        let labels = chat(&f.db, &f.bot_id);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].kind, MessageKind::Routine);
        assert_eq!(labels[0].body, "Routine: Digest");
        assert_eq!(label_data(&labels[0]).status, Missed);
        assert_eq!(
            runs(&f.db, &routine.id)[0].note.as_deref(),
            Some("The PC was off or asleep, or Tjommie wasn't running.")
        );
    }

    #[test]
    fn queues_the_within_grace_run_on_the_next_tick_after_recording_the_missed_run() {
        let f = fixture();
        let routine = digest(&f);
        let s = scheduler(&f);
        s.tick(at("2026-09-20T07:59:00Z")).unwrap();
        assert!(runs(&f.db, &routine.id).is_empty());
        assert!(delivered_items(&f).is_empty());

        // The PC sleeps through 08:00 and wakes at 17:01, within the 17:00 run's grace period.
        s.tick(at("2026-09-20T17:01:00Z")).unwrap();
        assert_eq!(
            status_due(&f.db, &routine.id),
            vec![(Missed, "2026-09-20T08:00:00.000Z".to_owned())]
        );
        assert!(delivered_items(&f).is_empty());
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-20T17:00:00.000Z".to_owned())
        );

        s.tick(at("2026-09-20T17:01:30Z")).unwrap();
        let listed = runs(&f.db, &routine.id);
        assert_eq!(
            status_due(&f.db, &routine.id),
            vec![
                (Queued, "2026-09-20T17:00:00.000Z".to_owned()),
                (Missed, "2026-09-20T08:00:00.000Z".to_owned()),
            ]
        );
        let items = delivered_items(&f);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].bot_id, f.bot_id);
        assert_eq!(items[0].source, WorkSource::Routine);
        assert_eq!(items[0].routine_run_id.as_deref(), Some(listed[0].id.as_str()));
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-21T08:00:00.000Z".to_owned())
        );

        s.tick(at("2026-09-20T17:02:00Z")).unwrap();
        assert_eq!(runs(&f.db, &routine.id), listed);
        assert_eq!(delivered_items(&f).len(), 1);
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-21T08:00:00.000Z".to_owned())
        );
    }

    #[test]
    fn skips_a_run_while_the_bot_is_paused_or_while_the_previous_run_is_still_going() {
        let f = fixture();
        let routine = digest(&f);
        set_paused(&lock(&f.db), &f.bot_id, true).unwrap();
        scheduler(&f).tick(at("2026-09-20T08:00:10Z")).unwrap();
        set_paused(&lock(&f.db), &f.bot_id, false).unwrap();
        add_run(
            &lock(&f.db),
            &routine.id,
            "2026-09-20T12:00:00.000Z",
            Running,
            None,
        )
        .unwrap();
        scheduler(&f).tick(at("2026-09-20T17:00:10Z")).unwrap();
        assert_eq!(
            status_notes(&f.db, &routine.id),
            vec![
                (Skipped, Some("The previous run was still going.".to_owned())),
                (Running, None),
                (Skipped, Some("Night Shift was paused.".to_owned())),
            ]
        );
        assert!(delivered_items(&f).is_empty());
        let labels: Vec<(String, RoutineRunStatus, Option<String>)> = chat(&f.db, &f.bot_id)
            .iter()
            .map(|message| {
                let data = label_data(message);
                (message.body.clone(), data.status, data.note)
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                (
                    "Routine: Digest".to_owned(),
                    Skipped,
                    Some("Night Shift was paused.".to_owned())
                ),
                (
                    "Routine: Digest".to_owned(),
                    Skipped,
                    Some("The previous run was still going.".to_owned())
                ),
            ]
        );
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-21T08:00:00.000Z".to_owned())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn labels_a_run_when_it_starts_and_stops_it_at_the_time_limit() {
        let f = fixture();
        digest_with(&f, json!({ "timeLimitMinutes": 180 }));
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:10Z")).unwrap();
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        s.run_started(&lock(&f.db), &item).unwrap();
        let run_id = item.routine_run_id.clone().unwrap();
        assert_eq!(run_of(&f.db, &run_id).unwrap().status, Running);
        let last = chat(&f.db, &f.bot_id).pop().unwrap();
        assert_eq!(last.kind, MessageKind::Routine);
        assert_eq!(last.body, "Routine: Digest");
        let data = label_data(&last);
        assert_eq!(data.run_id, run_id);
        assert_eq!(data.status, Running);

        advance(minutes(180)).await;
        settle_until(|| stopped(&f).len() == 1).await;
        assert_eq!(
            stopped(&f),
            vec![(item.id.clone(), "Stopped at the time limit (3 hours).".to_owned())]
        );
        let current = work_of(&f.db, &item.id);
        s.run_ended(&lock(&f.db), &current, &turn(TurnOutcome::Interrupted, None))
            .unwrap();
        let run = run_of(&f.db, &run_id).unwrap();
        assert_eq!(run.status, Stopped);
        assert_eq!(run.note.as_deref(), Some("Stopped at the time limit."));
        let Notify::RoutineFailed { bot_id, body, .. } = first_notice(&f) else {
            panic!("expected a routine failure notice")
        };
        assert_eq!(bot_id, f.bot_id);
        assert_eq!(body, "Digest: Stopped at the time limit.");
        s.stop();
    }

    #[test]
    fn announces_a_finished_routine_with_the_ending_reply() {
        let f = fixture();
        digest(&f);
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:10Z")).unwrap();
        let item = delivered_item(&f, 0);
        s.run_started(&lock(&f.db), &item).unwrap();
        // 139 "x", then one emoji (a single code point) and a trailing "y" that the 140-character cut drops.
        let text = format!("{}😀y", "x".repeat(139));
        s.run_ended(
            &lock(&f.db),
            &item,
            &TurnResult {
                outcome: TurnOutcome::Done,
                text: Some(text),
                text_message_id: None,
                error: None,
            },
        )
        .unwrap();
        assert_eq!(
            first_notice(&f),
            Notify::Done {
                bot_id: f.bot_id.clone(),
                title: "Night Shift finished \"Digest\"".to_owned(),
                body: format!("{}😀", "x".repeat(139)),
                room_id: None,
            }
        );
    }

    #[test]
    fn notifies_the_person_when_a_run_fails() {
        let f = fixture();
        digest(&f);
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:10Z")).unwrap();
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        s.run_started(&lock(&f.db), &item).unwrap();
        s.run_ended(
            &lock(&f.db),
            &item,
            &turn(TurnOutcome::Failed, Some("Codex exited with code 1")),
        )
        .unwrap();
        let run_id = item.routine_run_id.clone().unwrap();
        let run = run_of(&f.db, &run_id).unwrap();
        assert_eq!(run.status, Failed);
        assert_eq!(run.note.as_deref(), Some("Codex exited with code 1"));
        assert_eq!(
            first_notice(&f),
            Notify::RoutineFailed {
                bot_id: f.bot_id.clone(),
                title: "Night Shift's routine failed".to_owned(),
                body: "Digest: Codex exited with code 1".to_owned(),
                routine_id: Some(run.routine_id.clone()),
                run_id: Some(run_id),
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn records_the_actual_outcome_when_the_time_limit_stop_fails_by_throw_or_reject() {
        for failure in ["throw", "reject"] {
            let f = fixture();
            digest_with(&f, json!({ "timeLimitMinutes": 1 }));
            let stop_item: StopItem = if failure == "throw" {
                Arc::new(stop_that_throws) as StopItem
            } else {
                Arc::new(stop_that_rejects) as StopItem
            };
            let s = Scheduler::new(SchedulerDeps {
                stop_item,
                ..deps(&f)
            });
            s.tick(at("2026-09-20T08:00:10Z")).unwrap();
            let item = work_of(&f.db, &delivered_item(&f, 0).id);
            s.run_started(&lock(&f.db), &item).unwrap();
            advance(minutes(1)).await;
            settle_until(|| logged_context(&f, "Routine time limit failed.")).await;
            let current = work_of(&f.db, &item.id);
            s.run_ended(&lock(&f.db), &current, &turn(TurnOutcome::Done, None))
                .unwrap();
            let run = run_of(&f.db, item.routine_run_id.as_deref().unwrap()).unwrap();
            assert_eq!(run.status, Done, "{failure}");
            assert_eq!(run.note, None, "{failure}");
            assert!(
                logged_context(&f, "Routine time limit failed."),
                "{failure}: {:?}",
                logs(&f)
            );
            if failure == "reject" {
                assert!(
                    logged_error(&f, "Routine time limit failed.", "Could not interrupt"),
                    "{:?}",
                    logs(&f)
                );
            }
            s.stop();
        }
    }

    #[test]
    fn waits_until_the_due_instant_and_queues_a_run_exactly_two_minutes_late() {
        assert_eq!(TICK_MS, 30_000);
        assert_eq!(GRACE_MS, 120_000);
        let f = fixture();
        let routine = digest(&f);
        let s = scheduler(&f);
        s.tick(at("2026-09-20T07:59:59.999Z")).unwrap();
        assert!(runs(&f.db, &routine.id).is_empty());
        s.tick(at("2026-09-20T08:02:00Z")).unwrap();
        assert_eq!(statuses(&f.db, &routine.id), vec![Queued]);
        assert_eq!(delivered_items(&f).len(), 1);
    }

    #[test]
    fn marks_a_run_missed_one_millisecond_past_the_grace_period() {
        let f = fixture();
        let routine = digest(&f);
        scheduler(&f).tick(at("2026-09-20T08:02:00.001Z")).unwrap();
        assert_eq!(statuses(&f.db, &routine.id), vec![Missed]);
        assert!(delivered_items(&f).is_empty());
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-20T17:00:00.000Z".to_owned())
        );
    }

    #[test]
    fn skips_the_next_run_while_the_previous_run_is_still_queued() {
        let f = fixture();
        let routine = digest(&f);
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        s.tick(at("2026-09-20T17:00:00Z")).unwrap();
        assert_eq!(
            status_notes(&f.db, &routine.id),
            vec![
                (Skipped, Some("The previous run was still going.".to_owned())),
                (Queued, None),
            ]
        );
        assert_eq!(delivered_items(&f).len(), 1);
    }

    #[test]
    fn ignores_disabled_routines_and_schedules_without_a_next_run() {
        let f = fixture();
        let disabled = digest_with(&f, json!({ "enabled": false }));
        let empty = digest_with(&f, json!({ "times": [] }));
        scheduler(&f).tick(at("2026-09-25T09:00:00Z")).unwrap();
        assert!(runs(&f.db, &disabled.id).is_empty());
        assert!(runs(&f.db, &empty.id).is_empty());
        assert!(delivered_items(&f).is_empty());
        assert!(events_of(&f).is_empty());
    }

    #[test]
    fn does_not_duplicate_a_due_run_when_days_and_times_contain_duplicates() {
        let f = fixture();
        let routine = digest_with(
            &f,
            json!({
                "days": [0, 0, 1, 2, 3, 4, 5, 6],
                "times": ["08:00", "08:00", "17:00"],
            }),
        );
        // Stored input is normalized today, but the scheduler must also tolerate older duplicate schedules.
        lock(&f.db)
            .execute(
                "UPDATE routines SET days = ?1, times = ?2 WHERE id = ?3",
                params![
                    json!([0, 0, 1, 2, 3, 4, 5, 6]).to_string(),
                    json!(["08:00", "08:00", "17:00"]).to_string(),
                    routine.id
                ],
            )
            .unwrap();
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        s.tick(at("2026-09-20T08:00:30Z")).unwrap();
        assert_eq!(
            status_due(&f.db, &routine.id),
            vec![(Queued, "2026-09-20T08:00:00.000Z".to_owned())]
        );
        assert_eq!(delivered_items(&f).len(), 1);
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-20T17:00:00.000Z".to_owned())
        );
    }

    #[tokio::test]
    async fn ticks_immediately_and_every_thirty_seconds_then_stops_all_timers() {
        // Real time with a 30 ms interval: the tick count is the clock's read count, one per tick.
        let f = fixture();
        digest_with(&f, json!({ "timeLimitMinutes": 1 }));
        f.clock.store(at("2026-09-20T08:00:00Z"), SeqCst);
        let ticks = Arc::new(AtomicUsize::new(0));
        let s = Scheduler::new(SchedulerDeps {
            clock: {
                let (ticks, clock) = (ticks.clone(), f.clock.clone());
                Arc::new(move || {
                    ticks.fetch_add(1, SeqCst);
                    clock.load(SeqCst)
                })
            },
            tick_every: Duration::from_millis(30),
            ..deps(&f)
        });
        s.start().unwrap();
        assert_eq!(ticks.load(SeqCst), 1);
        assert_eq!(delivered_items(&f).len(), 1);
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        s.run_started(&lock(&f.db), &item).unwrap();
        settle_until(|| ticks.load(SeqCst) >= 2).await;
        assert!(ticks.load(SeqCst) >= 2, "the interval tick ran");
        s.stop();
        let_runtime_run(100).await; // an interval tick already on the blocking pool finishes
        let frozen = ticks.load(SeqCst);
        let_runtime_run(150).await;
        assert_eq!(ticks.load(SeqCst), frozen, "no tick after stop");
        assert!(stopped(&f).is_empty());
        assert_eq!(delivered_items(&f).len(), 1);
    }

    #[tokio::test]
    async fn logs_an_immediate_tick_exception_and_still_runs_the_next_interval_tick() {
        let f = fixture();
        let routine = digest(&f);
        f.clock.store(at("2026-09-20T08:00:00Z"), SeqCst);
        let s = Scheduler::new(SchedulerDeps {
            tick_every: Duration::from_millis(30),
            ..deps(&f)
        });
        // The TS makes the immediate tick throw. Renaming the table makes that tick's query fail; the table goes back
        // before the first interval tick.
        lock(&f.db)
            .execute_batch("ALTER TABLE routines RENAME TO routines_offline")
            .unwrap();
        s.start().unwrap();
        lock(&f.db)
            .execute_batch("ALTER TABLE routines_offline RENAME TO routines")
            .unwrap();
        assert!(
            logged_error(&f, "Routine scheduler tick failed.", "no such table"),
            "{:?}",
            logs(&f)
        );
        assert!(runs(&f.db, &routine.id).is_empty());
        settle_until(|| delivered_items(&f).len() == 1).await;
        assert_eq!(statuses(&f.db, &routine.id), vec![Queued]);
        assert_eq!(delivered_items(&f).len(), 1);
        s.stop();
    }

    #[tokio::test]
    async fn logs_an_interval_tick_exception_and_still_runs_the_following_tick() {
        let f = fixture();
        let routine = digest(&f);
        f.clock.store(at("2026-09-20T07:59:30Z"), SeqCst);
        let s = Scheduler::new(SchedulerDeps {
            tick_every: Duration::from_millis(30),
            ..deps(&f)
        });
        s.start().unwrap();
        // The TS makes the next interval tick throw. The table is renamed away until a tick has failed.
        lock(&f.db)
            .execute_batch("ALTER TABLE routines RENAME TO routines_offline")
            .unwrap();
        settle_until(|| logged_error(&f, "Routine scheduler tick failed.", "no such table")).await;
        assert!(
            logged_error(&f, "Routine scheduler tick failed.", "no such table"),
            "{:?}",
            logs(&f)
        );
        assert!(runs(&f.db, &routine.id).is_empty());
        // Due at the next tick, which is the first to see the table again.
        f.clock.store(at("2026-09-20T08:00:00Z"), SeqCst);
        lock(&f.db)
            .execute_batch("ALTER TABLE routines_offline RENAME TO routines")
            .unwrap();
        settle_until(|| delivered_items(&f).len() == 1).await;
        assert_eq!(statuses(&f.db, &routine.id), vec![Queued]);
        assert_eq!(delivered_items(&f).len(), 1);
        s.stop();
    }

    #[test]
    fn marks_a_throwing_delivery_failed_and_persists_the_deterministic_next_run() {
        // The em dash and the siren emoji, as the TS escapes spell them.
        const ERROR: &str = "Provider queue failed \u{2014} retry \u{1F6A8}";
        let f = fixture();
        let routine = digest(&f);
        let calls = Arc::new(AtomicUsize::new(0));
        let deliver: Deliver = {
            let (calls, delivered) = (calls.clone(), f.delivered.clone());
            Arc::new(move |work: &WorkItem| -> Result<(), String> {
                if calls.fetch_add(1, SeqCst) == 0 {
                    return Err(ERROR.to_owned());
                }
                delivered.lock().unwrap().push(work.clone());
                Ok(())
            })
        };
        let s = Scheduler::new(SchedulerDeps {
            deliver,
            ..deps(&f)
        });
        s.tick(at("2026-09-20T08:00:10Z")).unwrap();
        let failed = runs(&f.db, &routine.id).remove(0);
        assert_eq!(failed.status, Failed);
        assert_eq!(failed.due_at, "2026-09-20T08:00:00.000Z");
        assert!(failed.started_at.is_none());
        assert!(failed.ended_at.is_some());
        assert_eq!(
            failed.note.as_deref(),
            Some("Could not queue the routine run.")
        );
        assert!(!open_run(&f.db, &routine.id));
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-20T17:00:00.000Z".to_owned())
        );
        assert!(logged_error(&f, "Routine delivery failed.", ERROR));
        assert!(events_of(&f).contains(&Event::RoutineRun(failed.clone())));
        let label = chat(&f.db, &f.bot_id).remove(0);
        let data = label_data(&label);
        assert_eq!(data.run_id, failed.id);
        assert_eq!(data.status, Failed);
        assert_eq!(data.note, failed.note);
        assert!(events_of(&f).contains(&Event::MessageUpdated(label.clone())));
        s.tick(at("2026-09-20T08:00:30Z")).unwrap();
        assert_eq!(calls.load(SeqCst), 1);
        s.tick(at("2026-09-20T17:00:00Z")).unwrap();
        assert_eq!(calls.load(SeqCst), 2);
        assert_eq!(statuses(&f.db, &routine.id), vec![Queued, Failed]);
        assert_eq!(
            next_run(&f.db, &routine.id),
            Some("2026-09-21T08:00:00.000Z".to_owned())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn clears_the_limit_while_requeued_rearms_the_full_limit_on_restart_and_clears_it_on_completion(
    ) {
        let f = fixture();
        let routine = digest_with(&f, json!({ "timeLimitMinutes": 2 }));
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        let run_id = item.routine_run_id.clone().unwrap();
        s.run_started(&lock(&f.db), &item).unwrap();
        let started = run_of(&f.db, &run_id);
        advance(minutes(1)).await;
        s.run_requeued(&item);
        assert_eq!(run_of(&f.db, &run_id), started);
        assert!(open_run(&f.db, &routine.id));
        // Nothing fires while the item is parked: its limit was cleared.
        advance(Duration::from_secs(300)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        s.run_started(&lock(&f.db), &item).unwrap();
        assert_eq!(chat(&f.db, &f.bot_id).len(), 1);
        advance(Duration::from_millis(119_999)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        s.run_ended(&lock(&f.db), &item, &turn(TurnOutcome::Done, None))
            .unwrap();
        let run = run_of(&f.db, &run_id).unwrap();
        assert_eq!((run.status, run.note), (Done, None));
        assert!(!open_run(&f.db, &routine.id));
        advance(Duration::from_secs(120)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        s.stop();
    }

    #[tokio::test(start_paused = true)]
    async fn stops_a_resumed_run_only_after_its_full_restarted_time_limit() {
        let f = fixture();
        digest_with(&f, json!({ "timeLimitMinutes": 1 }));
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        let run_id = item.routine_run_id.clone().unwrap();
        s.run_started(&lock(&f.db), &item).unwrap();
        advance(Duration::from_millis(59_000)).await;
        s.run_requeued(&item);
        advance(Duration::from_millis(180_000)).await;
        s.run_started(&lock(&f.db), &item).unwrap();
        advance(Duration::from_millis(59_999)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        advance(Duration::from_millis(1)).await;
        settle_until(|| stopped(&f).len() == 1).await;
        assert_eq!(
            stopped(&f),
            vec![(item.id.clone(), "Stopped at the time limit (1 minute).".to_owned())]
        );
        // Two statements: the database lock is not reentrant, so the work is read before the lock is taken.
        let current = work_of(&f.db, &item.id);
        s.run_ended(&lock(&f.db), &current, &turn(TurnOutcome::Interrupted, None))
            .unwrap();
        let run = run_of(&f.db, &run_id).unwrap();
        assert_eq!(run.status, Stopped);
        assert_eq!(run.note.as_deref(), Some("Stopped at the time limit."));
        s.stop();
    }

    #[tokio::test(start_paused = true)]
    async fn settles_the_label_after_a_queued_routine_has_been_deleted() {
        let f = fixture();
        let routine = digest_with(&f, json!({ "timeLimitMinutes": 1 }));
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        let run_id = item.routine_run_id.clone().unwrap();
        delete_routine(&lock(&f.db), &routine.id).unwrap();
        f.events.lock().unwrap().clear();
        s.run_started(&lock(&f.db), &item).unwrap();
        s.run_requeued(&item);
        s.run_ended(&lock(&f.db), &item, &turn(TurnOutcome::Failed, Some("Deleted")))
            .unwrap();
        assert!(run_of(&f.db, &run_id).is_none());
        let labels = chat(&f.db, &f.bot_id);
        assert_eq!(labels.len(), 1);
        let data = label_data(&labels[0]);
        assert_eq!(data.status, Stopped);
        assert_eq!(data.note.as_deref(), Some("The routine was deleted."));
        assert_eq!(event_names(&f), vec!["message.updated".to_owned()]);
        advance(minutes(1)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        s.stop();
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_stop_or_notify_a_running_item_when_its_routine_was_deleted_before_the_limit() {
        let f = fixture();
        let routine = digest_with(&f, json!({ "timeLimitMinutes": 1 }));
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        let run_id = item.routine_run_id.clone().unwrap();
        set_status(&lock(&f.db), &item.id, WorkStatus::Running).unwrap();
        s.run_started(&lock(&f.db), &item).unwrap();
        delete_routine(&lock(&f.db), &routine.id).unwrap();
        f.events.lock().unwrap().clear();
        advance(minutes(1)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        s.run_requeued(&item);
        s.run_started(&lock(&f.db), &item).unwrap();
        s.run_ended(&lock(&f.db), &item, &turn(TurnOutcome::Failed, Some("Deleted")))
            .unwrap();
        assert_eq!(event_names(&f), vec!["message.updated".to_owned()]);
        let labels = chat(&f.db, &f.bot_id);
        assert_eq!(labels.len(), 1);
        let data = label_data(&labels[0]);
        assert_eq!(data.status, Stopped);
        assert_eq!(data.note.as_deref(), Some("The routine was deleted."));
        assert!(run_of(&f.db, &run_id).is_none());
        assert_eq!(work_of(&f.db, &item.id).status, WorkStatus::Running);
        s.stop();
    }

    #[tokio::test(start_paused = true)]
    async fn records_the_outcome_and_clears_its_time_limit() {
        let rows = [
            (TurnOutcome::Done, Done, None),
            (TurnOutcome::Interrupted, Stopped, Some("Stopped by you.")),
            (TurnOutcome::Failed, Failed, Some("Something went wrong.")),
        ];
        for (outcome, status, note) in rows {
            let f = fixture();
            let routine = digest_with(&f, json!({ "timeLimitMinutes": 1 }));
            let s = scheduler(&f);
            s.tick(at("2026-09-20T08:00:00Z")).unwrap();
            let item = work_of(&f.db, &delivered_item(&f, 0).id);
            let run_id = item.routine_run_id.clone().unwrap();
            s.run_started(&lock(&f.db), &item).unwrap();
            s.run_ended(&lock(&f.db), &item, &turn(outcome, None)).unwrap();
            let ended = run_of(&f.db, &run_id).unwrap();
            assert_eq!(ended.status, status, "{outcome:?}");
            assert_eq!(ended.note.as_deref(), note, "{outcome:?}");
            assert!(ended.ended_at.is_some(), "{outcome:?}");
            assert!(
                events_of(&f).contains(&Event::RoutineRun(ended.clone())),
                "{outcome:?}"
            );
            assert!(!open_run(&f.db, &routine.id), "{outcome:?}");
            advance(minutes(1)).await;
            let_runtime_run(50).await;
            assert!(stopped(&f).is_empty(), "{outcome:?}");
            let expected = if outcome == TurnOutcome::Interrupted { 0 } else { 1 };
            assert_eq!(notices(&f).len(), expected, "{outcome:?}");
            s.stop();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_arm_a_timer_for_an_unlimited_routine() {
        let f = fixture();
        digest(&f);
        let s = scheduler(&f);
        s.tick(at("2026-09-20T08:00:00Z")).unwrap();
        let item = work_of(&f.db, &delivered_item(&f, 0).id);
        let run_id = item.routine_run_id.clone().unwrap();
        s.run_started(&lock(&f.db), &item).unwrap();
        advance(Duration::from_secs(24 * 60 * 60)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        let run = run_of(&f.db, &run_id).unwrap();
        assert_eq!(run.status, Running);
        assert!(run.ended_at.is_none());
        s.stop();
    }

    #[tokio::test(start_paused = true)]
    async fn ignores_hooks_for_work_that_is_not_from_a_routine() {
        let f = fixture();
        let s = scheduler(&f);
        let item = enqueue(
            &lock(&f.db),
            &NewWork::new(&f.bot_id, WorkSource::You, "Hello"),
        )
        .unwrap();
        s.run_started(&lock(&f.db), &item).unwrap();
        s.run_requeued(&item);
        s.run_ended(&lock(&f.db), &item, &turn(TurnOutcome::Failed, Some("Failed")))
            .unwrap();
        assert!(events_of(&f).is_empty());
        assert!(stopped(&f).is_empty());
        // The TS checks that no timer is armed; a stop would show here if one were.
        advance(minutes(1)).await;
        let_runtime_run(50).await;
        assert!(stopped(&f).is_empty());
        s.stop();
    }
}

mod routine_prompts {
    use super::*;

    #[test]
    fn formats_minutes_as_a_duration() {
        for (minutes, expected) in [
            (1, "1 minute"),
            (2, "2 minutes"),
            (60, "1 hour"),
            (90, "90 minutes"),
            (180, "3 hours"),
        ] {
            assert_eq!(duration(minutes), expected, "{minutes} minutes");
        }
    }

    #[test]
    fn includes_the_exact_schedule_label_and_time_limit_in_the_prompt() {
        let f = fixture();
        assert_eq!(
            routine_prompt(&digest_with(&f, json!({ "timeLimitMinutes": 180 }))),
            "[Routine \"Digest\", started by Tjommie on schedule. You have 3 hours; then Tjommie stops you.]\nSummarize my inbox"
        );
        assert_eq!(
            routine_prompt(&digest(&f)),
            "[Routine \"Digest\", started by Tjommie on schedule.]\nSummarize my inbox"
        );
    }
}
