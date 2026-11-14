//! Port of `packages/ui/lib/routine-form.ts` (T1.4): the routine editor's rules and the Recent runs rows. Not ported:
//! `localZone`, which reads this PC's zone through `Intl`; the screen passes the zone in. `zone_line` takes the zone's long
//! name the same way, because `Intl`'s name lookup is not in Rust.

use jiff::{Timestamp, Zoned};
use protocol::model::{Routine, RoutineRun, RoutineRunStatus};

use super::bot_form::RoutineDraft;
use super::format::{clock, day_name, duration};

/// A day of the week as the editor shows it: `letter` for the button, `name` for the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeekDay {
    pub day: u8,
    pub letter: char,
    pub name: &'static str,
}

/// The week as the editor shows it, Monday first. Days are 0 (Sunday) to 6.
pub const WEEK: [WeekDay; 7] = [
    WeekDay {
        day: 1,
        letter: 'M',
        name: "Monday",
    },
    WeekDay {
        day: 2,
        letter: 'T',
        name: "Tuesday",
    },
    WeekDay {
        day: 3,
        letter: 'W',
        name: "Wednesday",
    },
    WeekDay {
        day: 4,
        letter: 'T',
        name: "Thursday",
    },
    WeekDay {
        day: 5,
        letter: 'F',
        name: "Friday",
    },
    WeekDay {
        day: 6,
        letter: 'S',
        name: "Saturday",
    },
    WeekDay {
        day: 0,
        letter: 'S',
        name: "Sunday",
    },
];

/// A routine the editor starts from (`EMPTY_ROUTINE`): weekdays at 9:00, enabled, no limit.
pub fn empty_routine() -> RoutineDraft {
    RoutineDraft {
        name: String::new(),
        prompt: String::new(),
        days: vec![1, 2, 3, 4, 5],
        times: vec!["09:00".to_owned()],
        time_limit_minutes: None,
        enabled: true,
    }
}

/// The editor's draft of a saved routine (`draftFrom`).
pub fn draft_from(routine: &Routine) -> RoutineDraft {
    RoutineDraft {
        name: routine.name.clone(),
        prompt: routine.prompt.clone(),
        days: routine.days.clone(),
        times: routine.times.clone(),
        time_limit_minutes: routine.time_limit_minutes,
        enabled: routine.enabled,
    }
}

/// A string that JavaScript's truthiness takes as present: an empty one counts as missing.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// Adds the day if it's not there, takes it out if it is. Days stay in order.
pub fn toggle_day(days: &[u8], day: u8) -> Vec<u8> {
    if days.contains(&day) {
        days.iter().copied().filter(|each| *each != day).collect()
    } else {
        let mut next = days.to_vec();
        next.push(day);
        next.sort_unstable();
        next
    }
}

/// `/^([01]\d|2[0-3]):[0-5]\d$/`: a 24-hour "HH:MM".
fn is_clock_time(time: &str) -> bool {
    match time.as_bytes() {
        [h1, h2, b':', m1, m2] => {
            let hour = matches!(h1, b'0' | b'1') && h2.is_ascii_digit()
                || matches!((h1, h2), (b'2', b'0'..=b'3'));
            hour && matches!(m1, b'0'..=b'5') && m2.is_ascii_digit()
        }
        _ => false,
    }
}

/// Adds an "HH:MM" time once, in order. Anything else leaves the times as they were.
pub fn add_time(times: &[String], time: &str) -> Vec<String> {
    if !is_clock_time(time) || times.iter().any(|each| each.as_str() == time) {
        return times.to_vec();
    }
    let mut next = times.to_vec();
    next.push(time.to_owned());
    next.sort_unstable();
    next
}

/// Why the routine can't be saved yet, or `None`. The daemon checks again. The name's length is counted in JavaScript's
/// UTF-16 units, as `String.length` counts them.
pub fn routine_problem(draft: &RoutineDraft) -> Option<String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Some("Give the routine a name.".to_owned());
    }
    if name.encode_utf16().count() > 60 {
        return Some("Keep the name to 60 characters or fewer.".to_owned());
    }
    if draft.prompt.trim().is_empty() {
        return Some("Say what the agent should do.".to_owned());
    }
    if draft.days.is_empty() {
        return Some("Pick at least one day.".to_owned());
    }
    if draft.times.is_empty() {
        return Some("Add at least one time.".to_owned());
    }
    if draft.times.len() > 12 {
        return Some("Keep it to 12 times.".to_owned());
    }
    None
}

/// A "Stop after" choice: `value` is what the select holds, `label` what it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopOption {
    pub value: String,
    pub label: String,
}

/// Stop after (WF7). A limit set another way, like a bot's proposal, gets an option of its own.
pub fn stop_after_options(limit: Option<u32>) -> Vec<StopOption> {
    let option = |value: &str, label: &str| StopOption {
        value: value.to_owned(),
        label: label.to_owned(),
    };
    let mut options = vec![
        option("none", "No limit"),
        option("60", "1 hour"),
        option("120", "2 hours"),
        option("180", "3 hours"),
    ];
    if let Some(limit) = limit {
        let value = limit.to_string();
        if !options.iter().any(|known| known.value == value) {
            let label = duration(i64::from(limit) * 60_000);
            options.push(StopOption { value, label });
        }
    }
    options
}

/// "Times are in South Africa Standard Time (Africa/Johannesburg)." `long_name` is the zone's long name, if the platform
/// has one; without a different one, the zone's own name is used.
pub fn zone_line(time_zone: &str, long_name: Option<&str>) -> String {
    match non_empty(long_name).filter(|name| *name != time_zone) {
        Some(name) => format!("Times are in {name} ({time_zone})."),
        None => format!("Times are in {time_zone}."),
    }
}

/// What a Recent runs row shows its icon for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunIcon {
    Done,
    Stopped,
    Failed,
    Missed,
    Skipped,
    Running,
}

/// One row of Recent runs (WF7): when it was due ("Fri 8:48 AM"), what happened, and how long it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLine {
    pub icon: RunIcon,
    pub when: String,
    pub text: String,
    pub length: Option<String>,
}

/// One row of Recent runs. `now` is a parameter, and the times are read in its zone.
pub fn run_line(run: &RoutineRun, now: &Zoned) -> RunLine {
    let when = format!(
        "{} {}",
        day_name(&run.due_at, now, true),
        clock(&run.due_at, now.time_zone())
    );
    let length = run_length(run.started_at.as_deref(), run.ended_at.as_deref());
    // The note loses one trailing full stop, as the TypeScript's `replace(/\.$/, "")` does.
    let note = non_empty(run.note.as_deref()).map(|note| note.strip_suffix('.').unwrap_or(note));
    let line = |icon, text: String, length| RunLine {
        icon,
        when: when.clone(),
        text,
        length,
    };
    match run.status {
        RoutineRunStatus::Done => line(RunIcon::Done, "Done".to_owned(), length),
        RoutineRunStatus::Stopped => line(
            RunIcon::Stopped,
            note.unwrap_or("Stopped").to_owned(),
            length,
        ),
        RoutineRunStatus::Failed => line(
            RunIcon::Failed,
            match non_empty(note) {
                Some(note) => format!("Failed: {note}"),
                None => "Failed".to_owned(),
            },
            length,
        ),
        RoutineRunStatus::Missed => {
            line(RunIcon::Missed, "Missed: the PC was off".to_owned(), None)
        }
        RoutineRunStatus::Skipped => line(
            RunIcon::Skipped,
            match non_empty(note) {
                Some(note) => format!("Skipped: {note}"),
                None => "Skipped".to_owned(),
            },
            None,
        ),
        RoutineRunStatus::Queued => line(RunIcon::Running, "Waiting to start".to_owned(), None),
        RoutineRunStatus::Running => line(RunIcon::Running, "Running".to_owned(), None),
    }
}

/// How long a run ran, from its start and end. A time that is missing, empty or unreadable gives no length (where
/// JavaScript's `Date.parse` gives NaN).
fn run_length(started: Option<&str>, ended: Option<&str>) -> Option<String> {
    let started = non_empty(started)?.parse::<Timestamp>().ok()?;
    let ended = non_empty(ended)?.parse::<Timestamp>().ok()?;
    Some(duration(ended.as_millisecond() - started.as_millisecond()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Sunday 27 September 2026, 2:00 PM, in UTC. The zone is fixed so the days don't depend on this machine's.
    fn now() -> Zoned {
        "2026-09-27T14:00:00[UTC]".parse().unwrap()
    }

    /// A UTC time on September `day` of 2026 (the TypeScript builds it in local time).
    fn at(day: u8, hour: u8, minute: u8) -> String {
        format!("2026-09-{day:02}T{hour:02}:{minute:02}:00Z")
    }

    /// A run due at 8:48 on the 27th, done, with nothing else set; `edit` changes it.
    fn run(edit: impl FnOnce(&mut RoutineRun)) -> RoutineRun {
        let mut run: RoutineRun = serde_json::from_value(json!({
            "id": "run1", "botId": "b1", "routineId": "r1", "dueAt": at(27, 8, 48),
            "startedAt": null, "endedAt": null, "status": "done", "note": null
        }))
        .unwrap();
        edit(&mut run);
        run
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn line(icon: RunIcon, text: &str, length: Option<&str>) -> RunLine {
        RunLine {
            icon,
            when: "Today 8:48 AM".to_owned(),
            text: text.to_owned(),
            length: length.map(str::to_owned),
        }
    }

    #[test]
    fn edits_days_and_times() {
        assert_eq!(toggle_day(&[1, 3], 2), vec![1, 2, 3]);
        assert_eq!(toggle_day(&[1, 2, 3], 2), vec![1, 3]);
        assert_eq!(
            add_time(&strings(&["12:47"]), "08:48"),
            strings(&["08:48", "12:47"])
        );
        assert_eq!(add_time(&strings(&["08:48"]), "08:48"), strings(&["08:48"]));
        assert_eq!(add_time(&strings(&["08:48"]), "24:00"), strings(&["08:48"]));
    }

    #[test]
    fn says_whats_missing_before_saving_and_offers_a_proposals_own_time_limit() {
        assert_eq!(
            routine_problem(&empty_routine()),
            Some("Give the routine a name.".to_owned())
        );
        let named = RoutineDraft {
            name: "Check".to_owned(),
            ..empty_routine()
        };
        assert_eq!(
            routine_problem(&named),
            Some("Say what the agent should do.".to_owned())
        );
        let prompted = RoutineDraft {
            prompt: "Check".to_owned(),
            ..named
        };
        assert_eq!(
            routine_problem(&RoutineDraft {
                days: Vec::new(),
                ..prompted.clone()
            }),
            Some("Pick at least one day.".to_owned())
        );
        assert_eq!(
            routine_problem(&RoutineDraft {
                times: Vec::new(),
                ..prompted.clone()
            }),
            Some("Add at least one time.".to_owned())
        );
        let twelve: Vec<String> = (0..12).map(|hour| format!("{hour:02}:00")).collect();
        assert_eq!(
            routine_problem(&RoutineDraft {
                times: twelve.clone(),
                ..prompted.clone()
            }),
            None
        );
        let thirteen = [twelve, vec!["12:00".to_owned()]].concat();
        assert_eq!(
            routine_problem(&RoutineDraft {
                times: thirteen,
                ..prompted.clone()
            }),
            Some("Keep it to 12 times.".to_owned())
        );
        assert_eq!(routine_problem(&prompted), None);
        let labels: Vec<String> = stop_after_options(Some(180))
            .into_iter()
            .map(|option| option.label)
            .collect();
        assert_eq!(
            labels,
            strings(&["No limit", "1 hour", "2 hours", "3 hours"])
        );
        assert_eq!(
            stop_after_options(Some(90)).last(),
            Some(&StopOption {
                value: "90".to_owned(),
                label: "1h 30m".to_owned()
            })
        );
        assert_eq!(
            zone_line("Africa/Johannesburg", Some("South Africa Standard Time")),
            "Times are in South Africa Standard Time (Africa/Johannesburg)."
        );
    }

    #[test]
    fn builds_the_recent_runs_rows() {
        assert_eq!(
            run_line(
                &run(|run| {
                    run.started_at = Some(at(27, 8, 48));
                    run.ended_at = Some(at(27, 11, 39));
                }),
                &now()
            ),
            line(RunIcon::Done, "Done", Some("2h 51m"))
        );
        assert_eq!(
            run_line(
                &run(|run| {
                    run.status = RoutineRunStatus::Stopped;
                    run.note = Some("Stopped at the time limit.".to_owned());
                    run.started_at = Some(at(27, 8, 48));
                    run.ended_at = Some(at(27, 11, 48));
                }),
                &now()
            ),
            line(RunIcon::Stopped, "Stopped at the time limit", Some("3h"))
        );
        assert_eq!(
            run_line(
                &run(|run| {
                    run.status = RoutineRunStatus::Failed;
                    run.note = Some("Claude stopped unexpectedly".to_owned());
                }),
                &now()
            ),
            line(RunIcon::Failed, "Failed: Claude stopped unexpectedly", None)
        );
        assert_eq!(
            run_line(
                &run(|run| {
                    run.status = RoutineRunStatus::Missed;
                    run.due_at = at(25, 8, 48);
                }),
                &now()
            ),
            RunLine {
                icon: RunIcon::Missed,
                when: "Fri 8:48 AM".to_owned(),
                text: "Missed: the PC was off".to_owned(),
                length: None,
            }
        );
        assert_eq!(
            run_line(
                &run(|run| {
                    run.status = RoutineRunStatus::Skipped;
                    run.note = Some("Pnet apply was paused.".to_owned());
                }),
                &now()
            ),
            line(RunIcon::Skipped, "Skipped: Pnet apply was paused", None)
        );
    }

    #[test]
    fn a_run_with_an_empty_or_unreadable_time_or_note_is_as_if_it_had_none() {
        let empty_start = run(|run| {
            run.started_at = Some(String::new());
            run.ended_at = Some(at(27, 11, 39));
        });
        assert_eq!(run_line(&empty_start, &now()).length, None);
        let unreadable_start = run(|run| {
            run.started_at = Some("not a time".to_owned());
            run.ended_at = Some(at(27, 11, 39));
        });
        assert_eq!(run_line(&unreadable_start, &now()).length, None);
        let empty_note = run(|run| {
            run.status = RoutineRunStatus::Failed;
            run.note = Some(String::new());
        });
        assert_eq!(run_line(&empty_note, &now()).text, "Failed");
    }

    #[test]
    fn zone_line_gives_only_the_zone_when_the_name_is_missing_or_the_same() {
        assert_eq!(zone_line("UTC", Some("UTC")), "Times are in UTC.");
        assert_eq!(
            zone_line("Africa/Johannesburg", None),
            "Times are in Africa/Johannesburg."
        );
    }
}
