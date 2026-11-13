//! `routines.ts`: routines (work a bot does on a schedule) and their runs, with the time-zone arithmetic that finds the
//! next run, plus the `routines.*` handlers from `daemon.ts`.
//!
//! Times are milliseconds since the epoch, as `Date.now()` gives them. Stored and sent times are `toISOString()` text.
//! `Intl.DateTimeFormat`'s offsets come from jiff's time zone database here; the arithmetic on top is the TypeScript's.

use std::sync::Arc;

use jiff::civil::DateTime;
use jiff::tz::{Offset, TimeZone as Zone};
use jiff::{Span, Timestamp};
use protocol::model::{
    Routine, RoutineInput, RoutinePatch, RoutineRun, RoutineRunStatus, TimeZone,
};
use protocol::wire::{Empty, Id, method};
use protocol::{ErrorCode, Event};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::bots::{from_text, get_stored_bot, text};
use crate::context::Context;
use crate::ids::{new_id, now_iso};
use crate::queue::{WorkItem, get_work_item};
use crate::runner::{TurnOutcome, TurnResult};
use crate::server::{Handlers, RpcError};

/// The daemon's composed `turnEnded`: ends one work item with a result, telling the runner's hooks, the scheduler and the team.
pub type TurnEnded = Arc<dyn Fn(&WorkItem, &TurnResult) + Send + Sync>;

/// Finished runs kept per routine. Open runs (queued or running) are always kept.
pub const RUNS_KEPT: i64 = 20;
const DAY_MS: i64 = 86_400_000;

/// `Date.now()`.
pub fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}

/// `new Date(ms).toISOString()`: UTC with milliseconds, like [`now_iso`].
pub fn iso_from_ms(ms: i64) -> String {
    Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// `Date.parse` for the ISO times this daemon writes. `None` when the text is not a time.
pub fn parse_iso(text: &str) -> Option<i64> {
    text.parse::<Timestamp>().ok().map(Timestamp::as_millisecond)
}

/// `hostTimeZone`: the PC's IANA zone name.
/// ponytail: a system zone that is only a UTC offset has no IANA name and reads as UTC here, where `Intl` would give
/// the offset; give it the offset spelling if such a PC ever shows up.
pub fn host_time_zone() -> String {
    Zone::system()
        .iana_name()
        .map_or_else(|| "UTC".to_owned(), str::to_owned)
}

/// What decides when a routine runs: the days (0 is Sunday), the "HH:MM" times and the zone they are read in.
pub struct Schedule<'a> {
    pub days: &'a [u8],
    pub times: &'a [String],
    pub time_zone: &'a str,
}

impl<'a> From<&'a Routine> for Schedule<'a> {
    fn from(routine: &'a Routine) -> Self {
        Schedule {
            days: &routine.days,
            times: &routine.times,
            time_zone: routine.time_zone.as_str(),
        }
    }
}

fn bad_params(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::BadParams, message)
}

/// An IANA zone name, or an offset written `±HH`, `±HHMM` or `±HH:MM`: what `Intl.DateTimeFormat` accepts.
/// An unknown zone is the TypeScript's `checkTimeZone` error.
fn zone_of(name: &str) -> Result<Zone, RpcError> {
    let unknown = || bad_params(format!("Unknown time zone {name}"));
    let sign = match name.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Zone::get(name).map_err(|_| unknown()),
    };
    let digits: String = name[1..].chars().filter(|&c| c != ':').collect();
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(unknown());
    }
    let (hours, minutes) = match digits.len() {
        2 => (&digits[..2], "00"),
        4 => (&digits[..2], &digits[2..]),
        _ => return Err(unknown()),
    };
    let seconds = hours.parse::<i32>().map_err(|_| unknown())? * 3600
        + minutes.parse::<i32>().map_err(|_| unknown())? * 60;
    Offset::from_seconds(sign * seconds)
        .map(Zone::fixed)
        .map_err(|_| unknown())
}

/// Milliseconds to add to a UTC instant to get the wall-clock time in `zone` at that instant.
/// ponytail: an instant outside jiff's range (past year 9999) reads as UTC; no schedule reaches it.
fn offset_at(utc_ms: i64, zone: &Zone) -> i64 {
    Timestamp::from_millisecond(utc_ms)
        .map_or(0, |at| i64::from(zone.to_offset(at).seconds()) * 1000)
}

/// `Date.UTC(year, month - 1, day, hour, minute)`: a wall-clock time read as if it were UTC. `None` for a time that
/// does not exist.
fn wall_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> Option<i64> {
    let wall = DateTime::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        0,
        0,
    )
    .ok()?;
    Offset::UTC
        .to_timestamp(wall)
        .ok()
        .map(Timestamp::as_millisecond)
}

/// The UTC instant of a wall-clock time. A time that a clock change skips moves forward by the gap (02:30 becomes
/// 03:30); a time that happens twice when clocks go back uses the first one.
fn to_utc(zone: &Zone, wall: i64) -> i64 {
    let candidates = [
        wall - offset_at(wall - DAY_MS, zone),
        wall - offset_at(wall + DAY_MS, zone),
    ];
    candidates
        .iter()
        .copied()
        .filter(|&at| at + offset_at(at, zone) == wall)
        .min()
        .unwrap_or_else(|| candidates[0].max(candidates[1]))
}

/// `wallTimeToUtc`.
pub fn wall_time_to_utc(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    time_zone: &str,
) -> Result<i64, RpcError> {
    let zone = zone_of(time_zone)?;
    let wall = wall_ms(year, month, day, hour, minute)
        .ok_or_else(|| bad_params(format!("{year}-{month}-{day} {hour}:{minute} is not a time")))?;
    Ok(to_utc(&zone, wall))
}

/// `nextRunAfter`: the first run time strictly after `after_ms`, or `None` when the schedule has no days or no times.
pub fn next_run_after(
    schedule: &Schedule<'_>,
    after_ms: i64,
) -> Result<Option<i64>, RpcError> {
    let zone = zone_of(schedule.time_zone)?;
    let local = Timestamp::from_millisecond(after_ms + offset_at(after_ms, &zone))
        .map_err(|error| bad_params(error.to_string()))?;
    let today = Offset::UTC.to_datetime(local).date();
    let mut next: Option<i64> = None;
    // A spring gap can move yesterday's scheduled time into today (for example in Nuuk). Compare across dates because
    // that shifted run can come after an ordinary time today.
    for offset in -1..=7i64 {
        let Ok(date) = today.checked_add(Span::new().days(offset)) else {
            continue;
        };
        // Sunday is day 0, as `getUTCDay` has it.
        let weekday = date.weekday().to_sunday_zero_offset();
        if !u8::try_from(weekday).is_ok_and(|weekday| schedule.days.contains(&weekday)) {
            continue;
        }
        let upcoming = schedule
            .times
            .iter()
            .filter_map(|time| {
                let (hour, minute) = (time.get(..2)?.parse().ok()?, time.get(3..)?.parse().ok()?);
                let wall = wall_ms(
                    i32::from(date.year()),
                    date.month() as u32,
                    date.day() as u32,
                    hour,
                    minute,
                )?;
                Some(to_utc(&zone, wall))
            })
            .filter(|&at| at > after_ms)
            .min();
        if let Some(candidate) = upcoming {
            next = Some(next.map_or(candidate, |best| best.min(candidate)));
        }
    }
    Ok(next)
}

/// The next run as ISO text, or `None` while the routine is disabled or has none.
fn next_run_iso(routine: &Routine, now_ms: i64) -> Result<Option<String>, RpcError> {
    if !routine.enabled {
        return Ok(None);
    }
    Ok(next_run_after(&Schedule::from(routine), now_ms)?.map(iso_from_ms))
}

const ROUTINE_COLUMNS: &str = "id, bot_id, name, prompt, days, times, time_zone, time_limit_minutes, enabled, next_run_at, created_at";

const RUN_SELECT: &str = "SELECT routine_runs.id, routines.bot_id, routine_runs.routine_id, routine_runs.due_at, routine_runs.started_at, routine_runs.ended_at, routine_runs.status, routine_runs.note FROM routine_runs JOIN routines ON routines.id = routine_runs.routine_id";

fn invalid_column(column: usize, message: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::other(message.to_string())),
    )
}

fn json_column<T: serde::de::DeserializeOwned>(row: &Row<'_>, column: usize) -> rusqlite::Result<T> {
    let stored: String = row.get(column)?;
    serde_json::from_str(&stored).map_err(|error| invalid_column(column, error))
}

fn to_routine(row: &Row<'_>) -> rusqlite::Result<Routine> {
    let zone: String = row.get(6)?;
    Ok(Routine {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        name: row.get(2)?,
        prompt: row.get(3)?,
        days: json_column(row, 4)?,
        times: json_column(row, 5)?,
        time_zone: TimeZone::new(&zone).map_err(|error| invalid_column(6, error))?,
        time_limit_minutes: row.get(7)?,
        enabled: row.get::<_, i64>(8)? == 1,
        next_run_at: row.get(9)?,
        created_at: row.get(10)?,
    })
}

fn to_run(row: &Row<'_>) -> rusqlite::Result<RoutineRun> {
    Ok(RoutineRun {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        routine_id: row.get(2)?,
        due_at: row.get(3)?,
        started_at: row.get(4)?,
        ended_at: row.get(5)?,
        status: from_text(row, 6)?,
        note: row.get(7)?,
    })
}

pub fn get_routine(db: &Connection, id: &str) -> Result<Routine, RpcError> {
    db.query_row(
        &format!("SELECT {ROUTINE_COLUMNS} FROM routines WHERE id = ?1"),
        [id],
        to_routine,
    )
    .optional()?
    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such routine"))
}

/// Every routine, or one bot's, in creation order.
pub fn list_routines(db: &Connection, bot_id: Option<&str>) -> Result<Vec<Routine>, RpcError> {
    let mut statement = db.prepare(&format!(
        "SELECT {ROUTINE_COLUMNS} FROM routines WHERE (?1 IS NULL OR bot_id = ?1) ORDER BY created_at, rowid"
    ))?;
    let routines = statement
        .query_map([bot_id], to_routine)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(routines)
}

/// Days ascending and times in text order, each once.
fn sorted_days(days: &[u8]) -> Vec<u8> {
    let mut days = days.to_vec();
    days.sort_unstable();
    days.dedup();
    days
}

fn sorted_times(times: &[String]) -> Vec<String> {
    let mut times = times.to_vec();
    times.sort_unstable();
    times.dedup();
    times
}

fn json_text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("days and times serialize")
}

pub fn create_routine(
    db: &Connection,
    input: &RoutineInput,
    now_ms: i64,
) -> Result<Routine, RpcError> {
    get_stored_bot(db, &input.bot_id)?;
    let id = new_id();
    let days = sorted_days(&input.days);
    let times = sorted_times(&input.times);
    let zone = input
        .time_zone
        .as_ref()
        .map_or_else(host_time_zone, |zone| zone.as_str().to_owned());
    zone_of(&zone)?;
    db.execute(
        "INSERT INTO routines (id, bot_id, name, prompt, days, times, time_zone, time_limit_minutes, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            input.bot_id,
            input.name,
            input.prompt,
            json_text(&days),
            json_text(&times),
            zone,
            input.time_limit_minutes.flatten(),
            i64::from(input.enabled != Some(false)),
            now_iso(),
        ],
    )?;
    let routine = get_routine(db, &id)?;
    set_next_run(db, &id, next_run_iso(&routine, now_ms)?.as_deref())?;
    get_routine(db, &id)
}

pub fn update_routine(
    db: &Connection,
    id: &str,
    patch: &RoutinePatch,
    now_ms: i64,
) -> Result<Routine, RpcError> {
    let previous = get_routine(db, id)?;
    let mut next = previous.clone();
    if let Some(name) = &patch.name {
        next.name.clone_from(name);
    }
    if let Some(prompt) = &patch.prompt {
        next.prompt.clone_from(prompt);
    }
    if let Some(days) = &patch.days {
        next.days.clone_from(days);
    }
    if let Some(times) = &patch.times {
        next.times.clone_from(times);
    }
    if let Some(zone) = &patch.time_zone {
        next.time_zone = zone.clone();
    }
    if let Some(limit) = patch.time_limit_minutes {
        next.time_limit_minutes = limit;
    }
    if let Some(enabled) = patch.enabled {
        next.enabled = enabled;
    }
    next.days = sorted_days(&next.days);
    next.times = sorted_times(&next.times);
    // Checked before anything is written, so a refused zone changes nothing.
    zone_of(next.time_zone.as_str())?;
    db.execute(
        "UPDATE routines SET name = ?1, prompt = ?2, days = ?3, times = ?4, time_zone = ?5, time_limit_minutes = ?6, enabled = ?7 WHERE id = ?8",
        params![
            next.name,
            next.prompt,
            json_text(&next.days),
            json_text(&next.times),
            next.time_zone.as_str(),
            next.time_limit_minutes,
            i64::from(next.enabled),
            id,
        ],
    )?;
    if next.enabled != previous.enabled
        || next.time_zone != previous.time_zone
        || next.days != previous.days
        || next.times != previous.times
    {
        set_next_run(db, id, next_run_iso(&next, now_ms)?.as_deref())?;
    }
    get_routine(db, id)
}

/// Deletes the routine, its run history and any run still waiting in the bot's queue.
pub fn delete_routine(db: &Connection, id: &str) -> Result<(), RpcError> {
    get_routine(db, id)?;
    let transaction = db.unchecked_transaction()?;
    transaction.execute(
        "DELETE FROM work_items WHERE status = 'queued' AND routine_run_id IN (SELECT id FROM routine_runs WHERE routine_id = ?1)",
        [id],
    )?;
    transaction.execute("DELETE FROM routines WHERE id = ?1", [id])?;
    transaction.commit()?;
    Ok(())
}

pub fn set_next_run(db: &Connection, id: &str, next_run_at: Option<&str>) -> Result<(), RpcError> {
    db.execute(
        "UPDATE routines SET next_run_at = ?1 WHERE id = ?2",
        params![next_run_at, id],
    )?;
    Ok(())
}

pub fn get_run(db: &Connection, id: &str) -> Result<Option<RoutineRun>, RpcError> {
    Ok(db
        .query_row(
            &format!("{RUN_SELECT} WHERE routine_runs.id = ?1"),
            [id],
            to_run,
        )
        .optional()?)
}

/// All open runs and the newest [`RUNS_KEPT`] finished runs, newest first.
pub fn list_runs(db: &Connection, routine_id: &str) -> Result<Vec<RoutineRun>, RpcError> {
    let mut statement = db.prepare(&format!(
        "{RUN_SELECT} WHERE routine_id = ?1 ORDER BY due_at DESC, routine_runs.rowid DESC"
    ))?;
    let runs = statement
        .query_map([routine_id], to_run)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(runs)
}

fn prune_finished_runs(db: &Connection, routine_id: &str) -> Result<(), RpcError> {
    db.execute(
        "DELETE FROM routine_runs WHERE routine_id = ?1 AND status NOT IN ('queued', 'running') AND id NOT IN
           (SELECT id FROM routine_runs WHERE routine_id = ?1 AND status NOT IN ('queued', 'running')
            ORDER BY due_at DESC, rowid DESC LIMIT ?2)",
        params![routine_id, RUNS_KEPT],
    )?;
    Ok(())
}

/// Records a run, preserving every open run plus the newest [`RUNS_KEPT`] finished runs. A missed or skipped run is
/// recorded as already ended.
pub fn add_run(
    db: &Connection,
    routine_id: &str,
    due_at: &str,
    status: RoutineRunStatus,
    note: Option<&str>,
) -> Result<RoutineRun, RpcError> {
    let id = new_id();
    let ended = matches!(
        status,
        RoutineRunStatus::Missed | RoutineRunStatus::Skipped
    )
    .then(now_iso);
    db.execute(
        "INSERT INTO routine_runs (id, routine_id, due_at, ended_at, status, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, routine_id, due_at, ended, text(&status), note],
    )?;
    prune_finished_runs(db, routine_id)?;
    // An older due time can put the new record outside the retained history immediately.
    Ok(RoutineRun {
        id,
        bot_id: get_routine(db, routine_id)?.bot_id,
        routine_id: routine_id.to_owned(),
        due_at: due_at.to_owned(),
        started_at: None,
        ended_at: ended,
        status,
        note: note.map(str::to_owned),
    })
}

pub fn set_run_status(
    db: &Connection,
    id: &str,
    status: RoutineRunStatus,
    note: Option<&str>,
) -> Result<Option<RoutineRun>, RpcError> {
    if status == RoutineRunStatus::Running {
        db.execute(
            "UPDATE routine_runs SET status = 'running', started_at = ?1 WHERE id = ?2",
            params![now_iso(), id],
        )?;
    } else {
        db.execute(
            "UPDATE routine_runs SET status = ?1, note = ?2, ended_at = ?3 WHERE id = ?4",
            params![text(&status), note, now_iso(), id],
        )?;
    }
    // The updated run comes back even if finishing it makes it too old for the retained history.
    let run = get_run(db, id)?;
    if let Some(run) = &run
        && !matches!(status, RoutineRunStatus::Queued | RoutineRunStatus::Running)
    {
        prune_finished_runs(db, &run.routine_id)?;
    }
    Ok(run)
}

/// True while an earlier run of the routine is still waiting or running.
pub fn has_open_run(db: &Connection, routine_id: &str) -> Result<bool, RpcError> {
    Ok(db
        .query_row(
            "SELECT 1 FROM routine_runs WHERE routine_id = ?1 AND status IN ('queued', 'running')",
            [routine_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The `routines.*` methods (`daemon.ts:588-613`). `turn_ended` is the daemon's `turnEnded`: it ends the work a deleted
/// routine had queued, so the runner's hooks, the scheduler and the team hear about it. It runs after the database
/// work, outside the lock.
pub fn register(
    handlers: Handlers,
    context: &Context,
    turn_ended: TurnEnded,
) -> Handlers {
    let (list, create, update, delete, runs) = (
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
    );
    handlers
        .on::<method::RoutinesList, _>(move |params, _| {
            let routines = list.with_db(|db| {
                if let Some(bot_id) = &params.bot_id {
                    get_stored_bot(db, bot_id)?;
                }
                list_routines(db, params.bot_id.as_deref())
            });
            async move { routines }
        })
        .on::<method::RoutinesCreate, _>(move |input, _| {
            let routine = create.with_db(|db| create_routine(db, &input, now_ms()));
            if let Ok(routine) = &routine {
                create.events.send(Event::RoutineUpdated(routine.clone()));
            }
            async move { routine }
        })
        .on::<method::RoutinesUpdate, _>(move |params, _| {
            let routine = update.with_db(|db| update_routine(db, &params.id, &params.patch, now_ms()));
            if let Ok(routine) = &routine {
                update.events.send(Event::RoutineUpdated(routine.clone()));
            }
            async move { routine }
        })
        .on::<method::RoutinesDelete, _>(move |Id { id }, _| {
            let dropped = delete.with_db(|db| {
                let queued = db
                    .prepare("SELECT id FROM work_items WHERE status = 'queued' AND routine_run_id IN (SELECT id FROM routine_runs WHERE routine_id = ?1)")?
                    .query_map([&id], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                let dropped = queued
                    .iter()
                    .map(|item| get_work_item(db, item))
                    .collect::<Result<Vec<_>, _>>()?;
                delete_routine(db, &id)?;
                Ok(dropped)
            });
            let deleted = dropped.map(|dropped| {
                let interrupted = TurnResult {
                    outcome: TurnOutcome::Interrupted,
                    text: None,
                    text_message_id: None,
                    error: Some("The routine was deleted.".to_owned()),
                };
                for item in &dropped {
                    turn_ended(item, &interrupted);
                }
                delete.events.send(Event::RoutineDeleted(Id { id: id.clone() }));
                Empty {}
            });
            async move { deleted }
        })
        .on::<method::RoutinesRuns, _>(move |params, _| {
            let found = runs.with_db(|db| {
                get_routine(db, &params.routine_id)?;
                list_runs(db, &params.routine_id)
            });
            async move { found }
        })
}
