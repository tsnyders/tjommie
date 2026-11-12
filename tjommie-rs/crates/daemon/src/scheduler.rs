//! `scheduler.ts`: starts routine runs when they are due, and records how each one went, plus the `notifications.*`
//! handlers from `daemon.ts`.
//!
//! **Locks.** The runner calls the hook methods ([`Scheduler::run_started`], [`Scheduler::run_ended`],
//! [`Scheduler::retry`]) inside its own locks, with the database it already holds, so those take a `&Connection` (or a
//! [`SchedulerHost`]) and never lock the database themselves. The timer's methods ([`Scheduler::start`],
//! [`Scheduler::tick`], [`Scheduler::recover`]) lock it, and wake work only after letting it go: `deliver` goes to the
//! runner, which takes the database lock itself.
//!
//! **Timers.** TypeScript's `setInterval` and `setTimeout` are tokio tasks, so a test can pause tokio's clock. The work
//! under a timer runs on the blocking pool.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::BoxFuture;
use protocol::ErrorCode;
use protocol::Event;
use protocol::model::{
    Message, MessageData, MessageKind, MessageStatus, Routine, RoutineLabelData, RoutineRun,
    RoutineRunStatus, message_data,
};
use protocol::wire::{Empty, NotificationsAck, Notify, PendingNotification, method};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::bots::get_stored_bot;
use crate::chat::{MessagePatch, NewMessage, add_message, get_message, update_message};
use crate::context::{Context, Db};
use crate::ids::now_iso;
use crate::queue::{
    NewWork, WorkItem, WorkSource, WorkStatus, enqueue, get_work_item, new_chain, set_status,
};
use crate::routines::{
    Schedule, add_run, get_routine, get_run, has_open_run, iso_from_ms, next_run_after, parse_iso,
    set_next_run, set_run_status,
};
use crate::runner::{Broadcast, Core, Log, TurnOutcome, TurnResult};
use crate::server::{Handlers, RpcError};

/// How often due routines are looked for.
pub const TICK_MS: u64 = 30_000;
/// A run later than this counts as missed.
pub const GRACE_MS: i64 = 120_000;

/// `duration`: "1 hour", "2 hours", "1 minute" or "90 minutes".
pub fn duration(minutes: u32) -> String {
    if minutes % 60 == 0 {
        let hours = minutes / 60;
        format!("{hours} {}", if minutes == 60 { "hour" } else { "hours" })
    } else {
        format!("{minutes} {}", if minutes == 1 { "minute" } else { "minutes" })
    }
}

/// What the bot gets when its routine runs.
pub fn routine_prompt(routine: &Routine) -> String {
    let limit = routine
        .time_limit_minutes
        .filter(|&minutes| minutes > 0)
        .map_or_else(String::new, |minutes| {
            format!(" You have {}; then Tjommie stops you.", duration(minutes))
        });
    format!(
        "[Routine \"{}\", started by Tjommie on schedule.{limit}]\n{}",
        routine.name, routine.prompt
    )
}

/// What a retry needs from the runner: the database it holds, and a way to hand it the queued work.
pub trait SchedulerHost {
    fn db(&self) -> &Connection;
    /// `runner.deliver(item)` for work already in the queue.
    fn deliver(&mut self, item: &WorkItem) -> Result<(), String>;
}

impl SchedulerHost for Core<'_> {
    fn db(&self) -> &Connection {
        Core::db(self)
    }

    fn deliver(&mut self, item: &WorkItem) -> Result<(), String> {
        Core::deliver(self, item.clone())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// `runner.deliver` for the timer: work already in the queue. It takes the runner's locks itself.
pub type Deliver = Arc<dyn Fn(&WorkItem) -> Result<(), String> + Send + Sync>;
/// `runner.stopItem`: stops one work item, saying why. Resolves when it has stopped.
pub type StopItem =
    Arc<dyn Fn(&str, &str) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

pub struct SchedulerDeps {
    pub db: Db,
    pub broadcast: Broadcast,
    pub deliver: Deliver,
    pub stop_item: StopItem,
    pub log: Log,
    /// `Date.now()` for the timer's ticks: [`now_ms`] in the daemon. A test sets it, as `vi.setSystemTime` does.
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// How often the timer ticks: `Duration::from_millis(TICK_MS)` in the daemon.
    pub tick_every: Duration,
}

#[derive(Default)]
struct State {
    timer: Option<JoinHandle<()>>,
    /// Run id -> its time limit.
    limits: HashMap<String, JoinHandle<()>>,
    timed_out: HashSet<String>,
}

struct Inner {
    deps: SchedulerDeps,
    state: Mutex<State>,
}

/// Starts routine runs when they are due, and records how each one went. Cheap to clone: every clone is the same
/// scheduler.
#[derive(Clone)]
pub struct Scheduler(Arc<Inner>);

fn internal(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::Internal, message)
}

/// The label message's data, `{ routineId, runId, status, note }`.
fn label_data(routine: &Routine, run: &RoutineRun) -> Map<String, Value> {
    let data = RoutineLabelData {
        routine_id: routine.id.clone(),
        run_id: run.id.clone(),
        status: run.status,
        note: run.note.clone(),
    };
    match serde_json::to_value(data).expect("a routine label serializes") {
        Value::Object(map) => map,
        _ => unreachable!("a routine label is an object"),
    }
}

/// The "Routine: <name>" row in the bot's chat, above the run or saying why it didn't run. Its event goes to `events`.
fn label(
    db: &Connection,
    routine: &Routine,
    run: &RoutineRun,
    events: &mut Vec<Event>,
) -> Result<Message, RpcError> {
    let data = label_data(routine, run);
    let existing: Option<String> = db
        .query_row(
            "SELECT id FROM messages WHERE conversation_id = ?1 AND kind = 'routine' AND json_extract(data, '$.runId') = ?2 LIMIT 1",
            [&routine.bot_id, &run.id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        let message = update_message(
            db,
            &id,
            &MessagePatch {
                data: Some(Some(data)),
                ..MessagePatch::default()
            },
        )?;
        events.push(Event::MessageUpdated(message.clone()));
        return Ok(message);
    }
    let message = add_message(
        db,
        &NewMessage {
            conversation_id: routine.bot_id.clone(),
            author: "system".to_owned(),
            kind: MessageKind::Routine,
            body: format!("Routine: {}", routine.name),
            data: Some(data),
            status: MessageStatus::Done,
        },
    )?;
    events.push(Event::MessageAdded(message.clone()));
    Ok(message)
}

/// Keeps a failure notice until it is acknowledged, and returns the event that announces it.
fn store_failure(
    db: &Connection,
    routine: &Routine,
    run: &RoutineRun,
    note: &str,
) -> Result<Event, RpcError> {
    let bot = get_stored_bot(db, &routine.bot_id)?;
    let title = format!("{}'s routine failed", bot.name);
    let body = format!("{}: {note}", routine.name);
    db.execute(
        "INSERT INTO routine_failure_notifications (run_id, title, body) VALUES (?1, ?2, ?3)",
        params![run.id, title, body],
    )?;
    Ok(Event::Notify(Notify::RoutineFailed {
        bot_id: routine.bot_id.clone(),
        title,
        body,
        routine_id: Some(routine.id.clone()),
        run_id: Some(run.id.clone()),
    }))
}

/// Only an explicit consumer acknowledgement removes a notice; startup broadcasts have no listeners.
pub fn pending_failures(db: &Connection) -> Result<Vec<PendingNotification>, RpcError> {
    let mut statement = db.prepare(
        "SELECT n.run_id, r.routine_id, t.bot_id, n.title, n.body
         FROM routine_failure_notifications n JOIN routine_runs r ON r.id = n.run_id
         JOIN routines t ON t.id = r.routine_id ORDER BY n.rowid",
    )?;
    let notices = statement
        .query_map([], |row| {
            Ok(PendingNotification::RoutineFailed {
                run_id: row.get(0)?,
                routine_id: row.get(1)?,
                bot_id: row.get(2)?,
                title: row.get(3)?,
                body: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(notices)
}

pub fn acknowledge_failure(db: &Connection, run_id: &str) -> Result<(), RpcError> {
    db.execute(
        "DELETE FROM routine_failure_notifications WHERE run_id = ?1",
        [run_id],
    )?;
    Ok(())
}

impl Scheduler {
    pub fn new(deps: SchedulerDeps) -> Self {
        Scheduler(Arc::new(Inner {
            deps,
            state: Mutex::default(),
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn with_db<T>(&self, work: impl FnOnce(&Connection) -> T) -> T {
        work(&self.0.deps.db.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn log(&self, context: &str, error: &str) {
        (self.0.deps.log)(context, error);
    }

    /// Sends the events of a committed change. A broadcast that fails is logged, not raised.
    fn flush(&self, events: Vec<Event>) {
        for event in events {
            if let Err(error) = (self.0.deps.broadcast)(event) {
                self.log("Tjommie: a Scheduler broadcast failed", &error);
            }
        }
    }

    /// Checks for due runs now and every 30 seconds. Runs that a restart cut off are settled by the runner's restart
    /// recovery, which reports them through [`Scheduler::run_ended`]. Must be called inside a tokio runtime.
    pub fn start(&self) -> Result<(), RpcError> {
        if self.state().timer.is_some() {
            return Ok(());
        }
        self.recover()?;
        self.tick_safely();
        let scheduler = self.clone();
        let timer = tokio::spawn(async move {
            let mut interval = tokio::time::interval(scheduler.0.deps.tick_every);
            // The first tick is immediate and has just been run; like `setInterval`, late ticks don't pile up.
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                let scheduler = scheduler.clone();
                let _ = tokio::task::spawn_blocking(move || scheduler.tick_safely()).await;
            }
        });
        self.state().timer = Some(timer);
        Ok(())
    }

    pub fn stop(&self) {
        let mut state = self.state();
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        for (_, limit) in state.limits.drain() {
            limit.abort();
        }
    }

    /// Runner recovery settles interrupted work first. Preserve live queue entries (including usage retries), but
    /// release runs left open by older non-atomic scheduling or interrupted hooks.
    pub fn recover(&self) -> Result<(), RpcError> {
        let events = self.with_db(|db| -> Result<Vec<Event>, RpcError> {
            let mut events = Vec::new();
            let transaction = db.unchecked_transaction()?;
            let ids = db
                .prepare(
                    "SELECT id FROM routine_runs r WHERE status IN ('queued', 'running')
                     AND NOT EXISTS (SELECT 1 FROM work_items w WHERE w.routine_run_id = r.id AND w.status IN ('queued', 'running'))",
                )?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for id in ids {
                let Some(run) = set_run_status(
                    db,
                    &id,
                    RoutineRunStatus::Failed,
                    Some("Tjommie restarted before this routine run could finish."),
                )?
                else {
                    continue;
                };
                let routine = get_routine(db, &run.routine_id)?;
                label(db, &routine, &run, &mut events)?;
                events.push(Event::RoutineRun(run.clone()));
                let note = run.note.clone().unwrap_or_default();
                events.push(store_failure(db, &routine, &run, &note)?);
            }
            transaction.commit()?;
            Ok(events)
        })?;
        self.flush(events);
        Ok(())
    }

    /// Fires every routine that is due at `now_ms`, then wakes the runner for the work it queued. A routine that fails
    /// to persist stays due for the next tick; the others still fire.
    pub fn tick(&self, now_ms: i64) -> Result<(), RpcError> {
        let items = self.with_db(|db| self.tick_in(db, now_ms))?;
        // ponytail: TypeScript wakes each run as its routine commits; here every wake comes after the whole tick.
        for item in &items {
            self.wake_direct(item);
        }
        Ok(())
    }

    /// [`Scheduler::tick`] without the waking, for a caller that holds the database. It returns the queued work, which
    /// the caller wakes once it has released the lock.
    pub fn tick_in(&self, db: &Connection, now_ms: i64) -> Result<Vec<WorkItem>, RpcError> {
        let due = db
            .prepare("SELECT id FROM routines WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1 ORDER BY next_run_at")?
            .query_map([iso_from_ms(now_ms)], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut queued = Vec::new();
        for id in due {
            // The failed transaction leaves this routine due for retry. Keep processing the others.
            match get_routine(db, &id).and_then(|routine| self.fire(db, &routine, now_ms)) {
                Ok(Some(item)) => queued.push(item),
                Ok(None) => {}
                Err(error) => self.log(
                    &format!("Routine scheduler failed for {id}."),
                    &error.to_string(),
                ),
            }
        }
        Ok(queued)
    }

    fn tick_safely(&self) {
        if let Err(error) = self.tick((self.0.deps.clock)()) {
            self.log("Routine scheduler tick failed.", &error.to_string());
        }
    }

    /// The runner calls this when a routine run's turn starts.
    pub fn run_started(&self, db: &Connection, item: &WorkItem) -> Result<(), RpcError> {
        let Some(id) = &item.routine_run_id else {
            return Ok(());
        };
        let Some(run) = get_run(db, id)? else {
            // The routine was deleted; discard its timer too.
            self.run_requeued(item);
            return Ok(());
        };
        if !matches!(
            run.status,
            RoutineRunStatus::Queued | RoutineRunStatus::Running
        ) {
            return Ok(());
        }
        let routine = get_routine(db, &run.routine_id)?;
        if run.status == RoutineRunStatus::Queued {
            let Some(started) = set_run_status(db, id, RoutineRunStatus::Running, None)? else {
                return Ok(()); // the routine was deleted
            };
            let mut events = Vec::new();
            label(db, &routine, &started, &mut events)?;
            events.push(Event::RoutineRun(started));
            self.flush(events);
        }
        self.clear_limit(id);
        self.state().timed_out.remove(id);
        let Some(minutes) = routine.time_limit_minutes.filter(|&minutes| minutes > 0) else {
            return Ok(());
        };
        // Restart the full budget after a usage limit. A limited routine can run up to about twice its limit overall
        // across the original turn and one retry; time parked in the queue does not count.
        let notice = format!("Stopped at the time limit ({}).", duration(minutes));
        let (scheduler, run_id, item_id) = (self.clone(), id.clone(), item.id.clone());
        let limit = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(minutes) * 60)).await;
            scheduler.limit_reached(run_id, item_id, notice).await;
        });
        self.state().limits.insert(id.clone(), limit);
        Ok(())
    }

    /// A run's time limit passed: stops its item if the run is still going.
    async fn limit_reached(&self, run_id: String, item_id: String, notice: String) {
        let checking = self.clone();
        let (check_run, check_item, check_notice) = (run_id.clone(), item_id, notice);
        let stopping = tokio::task::spawn_blocking(move || {
            checking.state().limits.remove(&check_run);
            let current = checking.with_db(|db| get_run(db, &check_run));
            match current {
                // Deleted or already finished.
                Ok(Some(run)) if run.status == RoutineRunStatus::Running => {
                    checking.state().timed_out.insert(check_run.clone());
                    Ok(Some((checking.0.deps.stop_item)(&check_item, &check_notice)))
                }
                Ok(_) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
        .await;
        let failed = |error: String| {
            self.state().timed_out.remove(&run_id);
            self.log("Routine time limit failed.", &error);
        };
        match stopping {
            Ok(Ok(Some(stopped))) => {
                if let Err(error) = stopped.await {
                    failed(error);
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => failed(error),
            Err(error) => failed(error.to_string()),
        }
    }

    /// The runner calls this when a provider usage limit puts the item back in its queue.
    pub fn run_requeued(&self, item: &WorkItem) {
        let Some(id) = &item.routine_run_id else {
            return;
        };
        self.clear_limit(id);
        self.state().timed_out.remove(id);
        // Keep the run open. `run_started` re-arms its full time limit when work resumes.
    }

    /// The runner calls this when a routine run's turn ends.
    pub fn run_ended(
        &self,
        db: &Connection,
        item: &WorkItem,
        result: &TurnResult,
    ) -> Result<(), RpcError> {
        let Some(id) = &item.routine_run_id else {
            return Ok(());
        };
        self.clear_limit(id);
        let timed_out = self.state().timed_out.remove(id);
        let Some(run) = get_run(db, id)? else {
            self.close_deleted_label(db, item)?;
            return Ok(());
        };
        if !matches!(
            run.status,
            RoutineRunStatus::Queued | RoutineRunStatus::Running
        ) {
            return Ok(());
        }
        let status = if timed_out || result.outcome == TurnOutcome::Interrupted {
            RoutineRunStatus::Stopped
        } else if result.outcome == TurnOutcome::Failed {
            RoutineRunStatus::Failed
        } else {
            RoutineRunStatus::Done
        };
        let note = if timed_out {
            Some("Stopped at the time limit.".to_owned())
        } else if status == RoutineRunStatus::Stopped {
            Some("Stopped by you.".to_owned())
        } else if status == RoutineRunStatus::Failed {
            Some(
                result
                    .error
                    .clone()
                    .unwrap_or_else(|| "Something went wrong.".to_owned()),
            )
        } else {
            None
        };
        let mut events = Vec::new();
        let transaction = db.unchecked_transaction()?;
        if let Some(ended) = set_run_status(db, id, status, note.as_deref())? {
            let routine = get_routine(db, &run.routine_id)?;
            label(db, &routine, &ended, &mut events)?;
            events.push(Event::RoutineRun(ended.clone()));
            if status == RoutineRunStatus::Failed || timed_out {
                let note = note.as_deref().unwrap_or("Something went wrong.");
                events.push(store_failure(db, &routine, &ended, note)?);
            } else if status == RoutineRunStatus::Done {
                let bot = get_stored_bot(db, &routine.bot_id)?;
                events.push(Event::Notify(Notify::Done {
                    bot_id: routine.bot_id.clone(),
                    title: format!("{} finished \"{}\"", bot.name, routine.name),
                    // The first 140 code points, as `Array.from` counts them.
                    body: result
                        .text
                        .as_deref()
                        .map_or_else(String::new, |text| text.chars().take(140).collect()),
                    room_id: None,
                }));
            }
        }
        transaction.commit()?;
        self.flush(events);
        Ok(())
    }

    /// A deleted routine has no run row; its retained label still needs a final state.
    fn close_deleted_label(&self, db: &Connection, item: &WorkItem) -> Result<(), RpcError> {
        let Some(message_id) = &item.message_id else {
            return Ok(());
        };
        let known = db
            .query_row("SELECT 1 FROM messages WHERE id = ?1", [message_id], |_| {
                Ok(())
            })
            .optional()?;
        if known.is_none() {
            return Ok(());
        }
        let message = get_message(db, message_id)?;
        let parsed = message_data(&message).map_err(|error| internal(error.to_string()))?;
        if let MessageData::Routine(data) = parsed
            && matches!(
                data.status,
                RoutineRunStatus::Queued | RoutineRunStatus::Running
            )
        {
            let data = RoutineLabelData {
                status: RoutineRunStatus::Stopped,
                note: Some("The routine was deleted.".to_owned()),
                ..data
            };
            let Value::Object(map) = serde_json::to_value(data).expect("a label serializes")
            else {
                unreachable!("a routine label is an object");
            };
            let updated = update_message(
                db,
                message_id,
                &MessagePatch {
                    data: Some(Some(map)),
                    ..MessagePatch::default()
                },
            )?;
            self.flush(vec![Event::MessageUpdated(updated)]);
        }
        Ok(())
    }

    fn clear_limit(&self, run_id: &str) {
        if let Some(limit) = self.state().limits.remove(run_id) {
            limit.abort();
        }
    }

    /// One due routine: its run, its next time, its label and its work, in one transaction. The work is returned, to be
    /// woken after the lock is released.
    fn fire(
        &self,
        db: &Connection,
        routine: &Routine,
        now_ms: i64,
    ) -> Result<Option<WorkItem>, RpcError> {
        let mut events = Vec::new();
        let transaction = db.unchecked_transaction()?;
        let due_at = routine
            .next_run_at
            .clone()
            .ok_or_else(|| internal(format!("Routine {} has no next run", routine.id)))?;
        let due_ms = parse_iso(&due_at)
            .ok_or_else(|| internal(format!("Routine {} has a bad next run {due_at}", routine.id)))?;
        let bot = get_stored_bot(db, &routine.bot_id)?;
        let run = if now_ms - due_ms > GRACE_MS {
            add_run(
                db,
                &routine.id,
                &due_at,
                RoutineRunStatus::Missed,
                Some("The PC was off or asleep, or Tjommie wasn't running."),
            )?
        } else if bot.paused {
            add_run(
                db,
                &routine.id,
                &due_at,
                RoutineRunStatus::Skipped,
                Some(&format!("{} was paused.", bot.name)),
            )?
        } else if has_open_run(db, &routine.id)? {
            add_run(
                db,
                &routine.id,
                &due_at,
                RoutineRunStatus::Skipped,
                Some("The previous run was still going."),
            )?
        } else {
            add_run(db, &routine.id, &due_at, RoutineRunStatus::Queued, None)?
        };
        // Skip older missed times, preserve times within grace, and advance past this due time to avoid duplicates.
        let next = next_run_after(&Schedule::from(routine), (now_ms - GRACE_MS).max(due_ms))?;
        set_next_run(db, &routine.id, next.map(iso_from_ms).as_deref())?;
        events.push(Event::RoutineUpdated(get_routine(db, &routine.id)?));
        events.push(Event::RoutineRun(run.clone()));
        let label = label(db, routine, &run, &mut events)?;
        let item = if run.status == RoutineRunStatus::Queued {
            Some(self.queue(db, routine, &run, &label)?)
        } else {
            None
        };
        transaction.commit()?;
        self.flush(events);
        Ok(item)
    }

    /// A manual retry is a new run with its own label and chain, leaving the old result intact.
    pub fn retry(
        &self,
        host: &mut dyn SchedulerHost,
        message: &Message,
    ) -> Result<Message, RpcError> {
        let data = match message_data(message) {
            Ok(MessageData::Routine(data)) => data,
            Ok(_) => {
                return Err(RpcError::new(
                    ErrorCode::BadParams,
                    "This message is not a routine label",
                ));
            }
            Err(error) => return Err(internal(error.to_string())),
        };
        let (run, label, item) = {
            let db = host.db();
            let routine = get_routine(db, &data.routine_id)?;
            if !routine.enabled {
                return Err(RpcError::new(
                    ErrorCode::BadState,
                    "Enable this routine before retrying it",
                ));
            }
            if get_stored_bot(db, &routine.bot_id)?.paused {
                return Err(RpcError::new(
                    ErrorCode::BadState,
                    "Unpause this bot before retrying its routine",
                ));
            }
            if has_open_run(db, &routine.id)? {
                return Err(RpcError::new(
                    ErrorCode::Busy,
                    "This routine already has a queued or running run",
                ));
            }
            let transaction = db.unchecked_transaction()?;
            let run = add_run(db, &routine.id, &now_iso(), RoutineRunStatus::Queued, None)?;
            let label = label(db, &routine, &run, &mut Vec::new())?;
            let item = self.queue(db, &routine, &run, &label)?;
            transaction.commit()?;
            (run, label, item)
        };
        self.flush(vec![
            Event::RoutineRun(run),
            Event::MessageAdded(label.clone()),
        ]);
        if let Err(error) = host.deliver(&item) {
            self.log("Routine delivery failed.", &error);
            self.delivery_failed(host.db(), &item);
        }
        get_message(host.db(), &label.id)
    }

    /// Persist only: callers include this in the run/schedule transaction and wake after commit.
    fn queue(
        &self,
        db: &Connection,
        routine: &Routine,
        run: &RoutineRun,
        label: &Message,
    ) -> Result<WorkItem, RpcError> {
        let chain_id = new_chain(db)?;
        enqueue(
            db,
            &NewWork {
                message_id: Some(label.id.clone()),
                chain_id: Some(chain_id),
                routine_run_id: Some(run.id.clone()),
                ..NewWork::new(
                    &routine.bot_id,
                    WorkSource::Routine,
                    &routine_prompt(routine),
                )
            },
        )
    }

    /// Hands queued work to the runner from the timer, which holds no lock.
    fn wake_direct(&self, item: &WorkItem) {
        if let Err(error) = (self.0.deps.deliver)(item) {
            self.log("Routine delivery failed.", &error);
            self.with_db(|db| self.delivery_failed(db, item));
        }
    }

    /// The runner could not take the work: fail the run, unless a callback already started the turn.
    fn delivery_failed(&self, db: &Connection, item: &WorkItem) {
        if let Err(error) = self.fail_undelivered(db, item) {
            self.log("Routine delivery failed.", &error.to_string());
        }
    }

    fn fail_undelivered(&self, db: &Connection, item: &WorkItem) -> Result<(), RpcError> {
        // A callback that already started the turn owns its outcome. Never fail live work here.
        if get_work_item(db, &item.id)?.status != WorkStatus::Queued {
            return Ok(());
        }
        let Some(run_id) = &item.routine_run_id else {
            return Ok(());
        };
        let Some(run) = get_run(db, run_id)? else {
            return Ok(());
        };
        let routine = get_routine(db, &run.routine_id)?;
        self.clear_limit(&run.id);
        self.state().timed_out.remove(&run.id);
        // The next run was computed from this tick before delivery, so failure cannot retry or burst it.
        let note = "Could not queue the routine run.";
        let mut events = Vec::new();
        let transaction = db.unchecked_transaction()?;
        set_status(db, &item.id, WorkStatus::Failed)?;
        if let Some(failed) = set_run_status(db, &run.id, RoutineRunStatus::Failed, Some(note))? {
            events.push(Event::RoutineRun(failed.clone()));
            label(db, &routine, &failed, &mut events)?;
            events.push(store_failure(db, &routine, &failed, note)?);
        }
        transaction.commit()?;
        self.flush(events);
        Ok(())
    }
}

/// The `notifications.*` methods (`daemon.ts:614-618`).
pub fn register(handlers: Handlers, context: &Context) -> Handlers {
    let (pending, ack) = (context.clone(), context.clone());
    handlers
        .on::<method::NotificationsPending, _>(move |_, _| {
            let notices = pending.with_db(pending_failures);
            async move { notices }
        })
        .on::<method::NotificationsAck, _>(move |NotificationsAck { run_id }, _| {
            let acknowledged = ack.with_db(|db| {
                acknowledge_failure(db, &run_id)?;
                Ok(Empty {})
            });
            async move { acknowledged }
        })
}
