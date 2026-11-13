//! `routines.test.ts`: every case, in the same order, with the same data and expected values. Test names are the TS
//! titles in snake_case. Each `describe` is a module, and each `it.each` is a loop with the row in its messages.
//!
//! Where the TS cannot be copied literally:
//! - No fake clock: `vi.useFakeTimers` and `vi.setSystemTime` do not exist here. Calls that take `now` get the TS value.
//!   Stamps the store writes from the real clock (`started_at`, `ended_at`, `created_at`) are checked to fall between
//!   two real clock reads around the call, and the creation-tie test writes the shared `created_at` into the rows.
//! - `Mars/Olympus` cannot reach a call, because `TimeZone` is validated. The tests check that the type and the wire
//!   form refuse it with a "time zone" error. The TS `updateRoutine` on an unknown zone needs no call at all.
//! - `hostTimeZone()` is compared with `Intl` in the TS. `Intl` is not reachable from Rust, so the test checks the stored
//!   default against `host_time_zone()`, and that the name passes the protocol's zone check that `getRoutine` relies on.
//! - The last test is not a TS case. It is the direct refusal of an unknown zone by `next_run_after` and
//!   `wall_time_to_utc` that the brief asked for.

use daemon::bots::create_bot;
use daemon::queue::{NewWork, WorkSource, WorkStatus, enqueue, get_work_item, set_status};
use daemon::routines::{
    RUNS_KEPT, Schedule, add_run, create_routine, delete_routine, get_routine, get_run,
    has_open_run, host_time_zone, iso_from_ms, list_routines, list_runs, next_run_after, now_ms,
    parse_iso, set_next_run, set_run_status, update_routine, wall_time_to_utc,
};
use protocol::ErrorCode;
use protocol::model::{
    BotInput, Routine, RoutineInput, RoutinePatch, RoutineRun, RoutineRunStatus, TimeZone,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::TempDir;

const EVERY_DAY: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];
const WEEKDAYS: [u8; 5] = [1, 2, 3, 4, 5];

mod next_run_after_tests {
    use super::*;

    const JOHANNESBURG: &str = "Africa/Johannesburg";
    const NEW_YORK: &str = "America/New_York";
    const NUUK: &str = "America/Nuuk";
    const LORD_HOWE: &str = "Australia/Lord_Howe";
    const KATHMANDU: &str = "Asia/Kathmandu";

    /// `nextRunAfter({ days, times, timeZone }, after)`, unwrapped: every schedule in this module is valid.
    fn next_run(days: &[u8], times: &[&str], time_zone: &str, after_ms: i64) -> Option<i64> {
        let times: Vec<String> = times.iter().map(|time| (*time).to_owned()).collect();
        let schedule = Schedule {
            days,
            times: &times[..],
            time_zone,
        };
        next_run_after(&schedule, after_ms).unwrap()
    }

    #[test]
    fn finds_the_next_time_today_or_the_next_matching_day_2026_09_25_is_a_friday() {
        let friday_morning = parse_iso("2026-09-25T07:00:00Z").unwrap();
        // 09:00 in Johannesburg
        assert_eq!(
            next_run(&WEEKDAYS, &["08:48", "12:47"], JOHANNESBURG, friday_morning)
                .map(iso_from_ms)
                .as_deref(),
            Some("2026-09-25T10:47:00.000Z")
        );
        // Monday 08:48
        assert_eq!(
            next_run(
                &WEEKDAYS,
                &["08:48", "12:47"],
                JOHANNESBURG,
                parse_iso("2026-09-25T11:00:00Z").unwrap()
            )
            .map(iso_from_ms)
            .as_deref(),
            Some("2026-09-28T06:48:00.000Z")
        );
        assert_eq!(
            next_run(&[5], &["08:00"], JOHANNESBURG, friday_morning)
                .map(iso_from_ms)
                .as_deref(),
            Some("2026-10-02T06:00:00.000Z")
        );
    }

    #[test]
    fn moves_a_time_skipped_by_the_spring_clock_change_forward_by_the_gap() {
        // 03:30 EDT
        assert_eq!(
            next_run(
                &EVERY_DAY,
                &["02:30"],
                NEW_YORK,
                parse_iso("2026-03-08T05:00:00Z").unwrap()
            )
            .map(iso_from_ms)
            .as_deref(),
            Some("2026-03-08T07:30:00.000Z")
        );
    }

    #[test]
    fn runs_a_time_that_happens_twice_in_the_autumn_only_once() {
        let first = next_run(
            &EVERY_DAY,
            &["01:30"],
            NEW_YORK,
            parse_iso("2026-11-01T04:00:00Z").unwrap(),
        );
        // 01:30 EDT, the first of the two
        assert_eq!(
            first.map(iso_from_ms).as_deref(),
            Some("2026-11-01T05:30:00.000Z")
        );
        // The next day, not 01:30 EST
        assert_eq!(
            next_run(&EVERY_DAY, &["01:30"], NEW_YORK, first.unwrap())
                .map(iso_from_ms)
                .as_deref(),
            Some("2026-11-02T06:30:00.000Z")
        );
    }

    #[test]
    fn uses_sunday_as_day_zero_and_excludes_the_exact_boundary_down_to_the_millisecond() {
        let due = parse_iso("2026-09-26T22:00:00Z").unwrap();
        assert_eq!(next_run(&[0], &["00:00"], JOHANNESBURG, due - 1), Some(due));
        assert_eq!(
            next_run(&[0], &["00:00"], JOHANNESBURG, due)
                .map(iso_from_ms)
                .as_deref(),
            Some("2026-10-03T22:00:00.000Z")
        );
    }

    #[test]
    fn returns_null_for_an_empty_schedule() {
        let friday_morning = parse_iso("2026-09-25T07:00:00Z").unwrap();
        assert_eq!(
            next_run(&[], &["08:48", "12:47"], JOHANNESBURG, friday_morning),
            None
        );
        assert_eq!(next_run(&WEEKDAYS, &[], JOHANNESBURG, friday_morning), None);
    }

    #[test]
    fn selects_the_earliest_instant_when_a_gap_reorders_or_merges_scheduled_times() {
        let times = ["02:30", "03:15", "03:30"];
        let first = next_run(
            &EVERY_DAY,
            &times,
            NEW_YORK,
            parse_iso("2026-03-08T05:00:00Z").unwrap(),
        );
        assert_eq!(
            first.map(iso_from_ms).as_deref(),
            Some("2026-03-08T07:15:00.000Z")
        );
        let shifted = next_run(&EVERY_DAY, &times, NEW_YORK, first.unwrap());
        assert_eq!(
            shifted.map(iso_from_ms).as_deref(),
            Some("2026-03-08T07:30:00.000Z")
        );
        assert_eq!(
            next_run(&EVERY_DAY, &times, NEW_YORK, shifted.unwrap())
                .map(iso_from_ms)
                .as_deref(),
            Some("2026-03-09T06:30:00.000Z")
        );
    }

    #[test]
    fn skips_the_second_occurrence_even_when_called_between_the_two_autumn_occurrences() {
        assert_eq!(
            next_run(
                &EVERY_DAY,
                &["01:30"],
                NEW_YORK,
                parse_iso("2026-11-01T06:00:00Z").unwrap()
            )
            .map(iso_from_ms)
            .as_deref(),
            Some("2026-11-02T06:30:00.000Z")
        );
    }

    #[test]
    fn handles_lord_howes_half_hour_spring_gap_and_autumn_fold() {
        assert_eq!(
            next_run(
                &EVERY_DAY,
                &["02:15"],
                LORD_HOWE,
                parse_iso("2026-10-03T14:00:00Z").unwrap()
            )
            .map(iso_from_ms)
            .as_deref(),
            Some("2026-10-03T15:45:00.000Z")
        );
        // The autumn routine is the same routine with the time 01:45.
        let first = next_run(
            &EVERY_DAY,
            &["01:45"],
            LORD_HOWE,
            parse_iso("2026-04-04T13:00:00Z").unwrap(),
        );
        assert_eq!(
            first.map(iso_from_ms).as_deref(),
            Some("2026-04-04T14:45:00.000Z")
        );
        assert_eq!(
            next_run(&EVERY_DAY, &["01:45"], LORD_HOWE, first.unwrap())
                .map(iso_from_ms)
                .as_deref(),
            Some("2026-04-05T15:15:00.000Z")
        );
    }

    #[test]
    fn finds_a_previous_days_run_shifted_past_midnight_by_a_spring_gap() {
        let first = next_run(
            &[6],
            &["23:30"],
            NUUK,
            parse_iso("2026-03-29T01:10:00Z").unwrap(),
        );
        assert_eq!(
            first.map(iso_from_ms).as_deref(),
            Some("2026-03-29T01:30:00.000Z")
        );
        assert_eq!(
            next_run(
                &[6],
                &["23:30"],
                NUUK,
                parse_iso("2026-03-29T01:30:00Z").unwrap()
            )
            .map(iso_from_ms)
            .as_deref(),
            Some("2026-04-05T00:30:00.000Z")
        );
    }

    #[test]
    fn orders_a_previous_days_shifted_run_against_todays_scheduled_times() {
        assert_eq!(
            next_run(
                &[0, 6],
                &["00:15", "23:30"],
                NUUK,
                parse_iso("2026-03-29T01:10:00Z").unwrap()
            )
            .map(iso_from_ms)
            .as_deref(),
            Some("2026-03-29T01:15:00.000Z")
        );
    }

    #[test]
    fn handles_a_fractional_offset_and_a_year_boundary_without_dst() {
        assert_eq!(
            next_run(
                &EVERY_DAY,
                &["00:15"],
                KATHMANDU,
                parse_iso("2026-12-31T18:31:00Z").unwrap()
            )
            .map(iso_from_ms)
            .as_deref(),
            Some("2027-01-01T18:30:00.000Z")
        );
    }
}

mod wall_time_to_utc_tests {
    use super::*;

    /// `it.each`: the title's placeholders are the row's year, month, day, hour, minute and zone.
    #[test]
    fn converts_wall_clock_times_to_utc_in_their_zones() {
        let rows: [(i32, u32, u32, u32, u32, &str, &str); 8] = [
            (
                2026,
                9,
                25,
                8,
                48,
                "Africa/Johannesburg",
                "2026-09-25T06:48:00.000Z",
            ),
            (
                2026,
                9,
                25,
                8,
                15,
                "Asia/Kathmandu",
                "2026-09-25T02:30:00.000Z",
            ),
            (
                2026,
                3,
                8,
                2,
                30,
                "America/New_York",
                "2026-03-08T07:30:00.000Z",
            ),
            (
                2026,
                11,
                1,
                1,
                30,
                "America/New_York",
                "2026-11-01T05:30:00.000Z",
            ),
            (
                2026,
                10,
                4,
                2,
                15,
                "Australia/Lord_Howe",
                "2026-10-03T15:45:00.000Z",
            ),
            (
                2026,
                4,
                5,
                1,
                45,
                "Australia/Lord_Howe",
                "2026-04-04T14:45:00.000Z",
            ),
            (
                2026,
                9,
                6,
                0,
                30,
                "America/Santiago",
                "2026-09-06T04:30:00.000Z",
            ),
            (
                2026,
                3,
                28,
                23,
                30,
                "America/Nuuk",
                "2026-03-29T01:30:00.000Z",
            ),
        ];
        for (year, month, day, hour, minute, zone, expected) in rows {
            let utc = wall_time_to_utc(year, month, day, hour, minute, zone).unwrap();
            assert_eq!(
                iso_from_ms(utc),
                expected,
                "converts {year}-{month}-{day} {hour}:{minute} in {zone}"
            );
        }
    }
}

mod routines_store_tests {
    use super::*;

    /// `beforeEach`: a fresh database and one bot, in a new folder. `db` is declared first so it closes before the folder
    /// is removed. `_root` keeps the folder alive.
    struct Fixture {
        db: Connection,
        bot_id: String,
        _root: TempDir,
    }

    fn fixture() -> Fixture {
        let root = tempfile::Builder::new()
            .prefix("tjommie routines test ")
            .tempdir()
            .unwrap();
        let db = daemon::db::open(&root.path().join("t.db")).unwrap();
        let bots_dir = root.path().join("bots");
        let bot = create_bot(&db, &bots_dir, &bot_input()).unwrap();
        Fixture {
            db,
            bot_id: bot.id,
            _root: root,
        }
    }

    fn bot_input() -> BotInput {
        serde_json::from_value(json!({
            "name": "Pnet apply",
            "color": "green",
            "shape": "blob",
            "instructions": "",
            "provider": "claude",
            "model": "sonnet",
        }))
        .unwrap()
    }

    /// The TS `input()` as wire JSON: a Johannesburg routine for `bot_id`, with `changes` merged over it.
    fn input_value(bot_id: &str, changes: Value) -> Value {
        let mut value = json!({
            "botId": bot_id,
            "name": "Morning check",
            "prompt": "Check pnet.co.za",
            "days": [5, 1, 1],
            "times": ["12:47", "08:48"],
            "timeZone": "Africa/Johannesburg",
        });
        if let (Some(base), Value::Object(extra)) = (value.as_object_mut(), changes) {
            for (key, field) in extra {
                base.insert(key, field);
            }
        }
        value
    }

    fn input_with(bot_id: &str, changes: Value) -> RoutineInput {
        serde_json::from_value(input_value(bot_id, changes)).unwrap()
    }

    fn input(bot_id: &str) -> RoutineInput {
        input_with(bot_id, json!({}))
    }

    fn patch(value: Value) -> RoutinePatch {
        serde_json::from_value(value).unwrap()
    }

    /// The store stamps the real clock, so a stamp must fall between two reads taken around the call.
    fn assert_stamped_between(stamp: &str, before: i64, after: i64) {
        let at = parse_iso(stamp).unwrap_or_else(|| panic!("{stamp} is not a time"));
        assert!(
            before <= at && at <= after,
            "{stamp} is outside {} to {}",
            iso_from_ms(before),
            iso_from_ms(after)
        );
    }

    #[test]
    fn stores_days_and_times_sorted_and_computes_the_next_run() {
        let f = fixture();
        let routine = create_routine(
            &f.db,
            &input(&f.bot_id),
            parse_iso("2026-09-25T07:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(routine.days, vec![1, 5]);
        assert_eq!(routine.times, vec!["08:48", "12:47"]);
        assert!(routine.enabled);
        assert_eq!(routine.time_limit_minutes, None);
        assert_eq!(
            routine.next_run_at.as_deref(),
            Some("2026-09-25T10:47:00.000Z")
        );
    }

    #[test]
    fn clears_the_next_run_while_disabled_and_refuses_an_unknown_time_zone() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(
            db,
            &input(&f.bot_id),
            parse_iso("2026-09-25T07:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(
            update_routine(db, &routine.id, &patch(json!({"enabled": false})), now_ms())
                .unwrap()
                .next_run_at,
            None
        );
        assert_eq!(
            update_routine(
                db,
                &routine.id,
                &patch(json!({"enabled": true})),
                parse_iso("2026-09-25T11:00:00Z").unwrap()
            )
            .unwrap()
            .next_run_at
            .as_deref(),
            Some("2026-09-28T06:48:00.000Z")
        );
        // The TS `createRoutine` with "Mars/Olympus" throws /time zone/. The zone is a validated type, so the call cannot
        // be written: the type refuses the value, and the wire form refuses the same input.
        assert!(
            TimeZone::new("Mars/Olympus")
                .unwrap_err()
                .to_string()
                .contains("time zone")
        );
        let error = serde_json::from_value::<RoutineInput>(input_value(
            &f.bot_id,
            json!({"timeZone": "Mars/Olympus"}),
        ))
        .unwrap_err();
        assert!(error.to_string().contains("time zone"), "{error}");
    }

    #[test]
    fn preserves_an_already_due_occurrence_when_the_schedule_has_not_changed() {
        let rows = [
            json!({"name": "Renamed"}),
            json!({"prompt": "Updated instructions"}),
            json!({"timeLimitMinutes": 15}),
            json!({
                "days": [5, 1, 5],
                "times": ["12:47", "08:48"],
                "timeZone": "Africa/Johannesburg",
                "enabled": true,
            }),
        ];
        for row in rows {
            // Each row gets its own `beforeEach`, so its own database.
            let f = fixture();
            let routine = create_routine(
                &f.db,
                &input(&f.bot_id),
                parse_iso("2026-09-25T07:00:00Z").unwrap(),
            )
            .unwrap();
            let updated = update_routine(
                &f.db,
                &routine.id,
                &patch(row.clone()),
                parse_iso("2026-09-25T10:47:15Z").unwrap(),
            )
            .unwrap();
            assert_eq!(updated.next_run_at, routine.next_run_at, "patch {row}");
        }
    }

    #[test]
    fn keeps_the_newest_20_runs() {
        let f = fixture();
        let routine = create_routine(&f.db, &input(&f.bot_id), now_ms()).unwrap();
        for day in 1..=25 {
            add_run(
                &f.db,
                &routine.id,
                &format!("2026-08-{day:02}T06:48:00.000Z"),
                RoutineRunStatus::Done,
                None,
            )
            .unwrap();
        }
        let runs = list_runs(&f.db, &routine.id).unwrap();
        assert_eq!(runs.len(), 20);
        assert_eq!(runs[0].due_at, "2026-08-25T06:48:00.000Z");
    }

    #[test]
    fn deletes_a_routine_with_its_runs_and_any_run_still_waiting_in_the_queue() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let run = add_run(
            db,
            &routine.id,
            "2026-09-25T06:48:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        enqueue(
            db,
            &NewWork {
                routine_run_id: Some(run.id.clone()),
                ..NewWork::new(&f.bot_id, WorkSource::Routine, "Check pnet.co.za")
            },
        )
        .unwrap();
        delete_routine(db, &routine.id).unwrap();
        let work_items: i64 = db
            .query_row("SELECT COUNT(*) AS n FROM work_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(work_items, 0);
        let routine_runs: i64 = db
            .query_row("SELECT COUNT(*) AS n FROM routine_runs", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(routine_runs, 0);
    }

    #[test]
    fn defaults_to_the_hosts_zone_and_persists_explicit_options() {
        let f = fixture();
        let db = &f.db;
        // The TS compares with `Intl.DateTimeFormat().resolvedOptions().timeZone`, which Rust cannot reach. Here the
        // host name must pass the protocol's zone check, because `getRoutine` reads the stored zone back through it.
        let host = host_time_zone();
        assert!(
            TimeZone::new(&host).is_ok(),
            "host zone {host} is not a protocol time zone"
        );
        // The TS input without `timeZone`, with `enabled: false` and `timeLimitMinutes: 25`.
        let without_zone: RoutineInput = serde_json::from_value(json!({
            "botId": f.bot_id,
            "name": "Morning check",
            "prompt": "Check pnet.co.za",
            "days": [5, 1, 1],
            "times": ["12:47", "08:48"],
            "enabled": false,
            "timeLimitMinutes": 25,
        }))
        .unwrap();
        let routine = create_routine(db, &without_zone, now_ms()).unwrap();
        assert_eq!(get_routine(db, &routine.id).unwrap(), routine);
        assert_eq!(routine.time_zone.as_str(), host.as_str());
        assert!(!routine.enabled);
        assert_eq!(routine.time_limit_minutes, Some(25));
        assert_eq!(routine.next_run_at, None);
    }

    #[test]
    fn normalizes_duplicate_times_on_creation_and_updates_the_schedule_and_fields_together() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(
            db,
            &input_with(&f.bot_id, json!({"times": ["12:47", "08:48", "12:47"]})),
            now_ms(),
        )
        .unwrap();
        assert_eq!(routine.times, vec!["08:48", "12:47"]);
        let changed = update_routine(
            db,
            &routine.id,
            &patch(json!({
                "name": "Evening check",
                "prompt": "Check again",
                "days": [6, 0, 6],
                "times": ["19:00", "08:00", "19:00"],
                "timeZone": "America/New_York",
                "timeLimitMinutes": 15,
            })),
            parse_iso("2026-09-25T07:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(
            changed,
            Routine {
                name: "Evening check".to_owned(),
                prompt: "Check again".to_owned(),
                days: vec![0, 6],
                times: vec!["08:00".to_owned(), "19:00".to_owned()],
                time_zone: TimeZone::new("America/New_York").unwrap(),
                time_limit_minutes: Some(15),
                next_run_at: Some("2026-09-26T12:00:00.000Z".to_owned()),
                ..routine.clone()
            }
        );
        assert_eq!(
            update_routine(
                db,
                &routine.id,
                &patch(json!({"timeLimitMinutes": null})),
                now_ms()
            )
            .unwrap()
            .time_limit_minutes,
            None
        );
    }

    #[test]
    fn rejects_unknown_zones_on_update_without_changing_the_stored_routine() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        // The TS `updateRoutine` with "Mars/Olympus" throws /time zone/ and changes nothing. The zone is a validated
        // type, so no call can be made with it: the patch fails to deserialize, and nothing reaches the store.
        assert!(
            TimeZone::new("Mars/Olympus")
                .unwrap_err()
                .to_string()
                .contains("time zone")
        );
        let error = serde_json::from_value::<RoutinePatch>(json!({
            "name": "Bad update",
            "timeZone": "Mars/Olympus",
        }))
        .unwrap_err();
        assert!(error.to_string().contains("time zone"), "{error}");
        assert_eq!(get_routine(db, &routine.id).unwrap(), routine);
        let error = serde_json::from_value::<RoutineInput>(input_value(
            &f.bot_id,
            json!({"timeZone": "Mars/Olympus"}),
        ))
        .unwrap_err();
        assert!(error.to_string().contains("time zone"), "{error}");
        assert_eq!(
            list_routines(db, Some(f.bot_id.as_str())).unwrap(),
            vec![routine]
        );
    }

    #[test]
    fn lists_only_the_bots_routines_in_insertion_order_when_creation_times_tie() {
        let f = fixture();
        let db = &f.db;
        let first = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let second = create_routine(
            db,
            &input_with(&f.bot_id, json!({"name": "Second"})),
            now_ms(),
        )
        .unwrap();
        // The TS fakes the clock so both rows share one `createdAt`. The store stamps the real clock, so the tie is
        // written into the rows here, and the routines are read back afterwards.
        let tie = iso_from_ms(parse_iso("2026-09-25T07:00:00Z").unwrap());
        db.execute("UPDATE routines SET created_at = ?1", [&tie])
            .unwrap();
        let first = get_routine(db, &first.id).unwrap();
        let second = get_routine(db, &second.id).unwrap();
        assert_eq!(second.created_at, first.created_at);
        assert_eq!(
            list_routines(db, Some(f.bot_id.as_str())).unwrap(),
            vec![first, second]
        );
        assert!(list_routines(db, Some("no-such-bot")).unwrap().is_empty());
    }

    #[test]
    fn reports_missing_bots_and_routines_and_returns_null_for_missing_runs() {
        let f = fixture();
        let db = &f.db;
        let error =
            create_routine(db, &input_with("no-such-bot", json!({})), now_ms()).unwrap_err();
        assert!(error.message.contains("No such bot"), "{}", error.message);
        assert!(
            get_routine(db, "missing")
                .unwrap_err()
                .message
                .contains("No such routine")
        );
        assert!(
            update_routine(db, "missing", &RoutinePatch::default(), now_ms())
                .unwrap_err()
                .message
                .contains("No such routine")
        );
        assert!(
            delete_routine(db, "missing")
                .unwrap_err()
                .message
                .contains("No such routine")
        );
        assert_eq!(get_run(db, "missing").unwrap(), None);
        assert_eq!(
            set_run_status(db, "missing", RoutineRunStatus::Done, None).unwrap(),
            None
        );
        assert!(!has_open_run(db, "missing").unwrap());
    }

    #[test]
    fn lets_the_scheduler_set_and_clear_the_next_run() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        set_next_run(db, &routine.id, Some("2026-09-28T06:48:00.000Z")).unwrap();
        assert_eq!(
            get_routine(db, &routine.id).unwrap().next_run_at.as_deref(),
            Some("2026-09-28T06:48:00.000Z")
        );
        set_next_run(db, &routine.id, None).unwrap();
        assert_eq!(get_routine(db, &routine.id).unwrap().next_run_at, None);
    }

    #[test]
    fn records_run_lifecycle_timestamps_notes_and_open_status() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let other = create_routine(
            db,
            &input_with(&f.bot_id, json!({"name": "Other"})),
            now_ms(),
        )
        .unwrap();
        assert!(!has_open_run(db, &routine.id).unwrap());
        let run = add_run(
            db,
            &routine.id,
            "2026-09-25T06:48:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        assert_eq!(
            get_run(db, &run.id).unwrap(),
            Some(RoutineRun {
                started_at: None,
                ended_at: None,
                note: None,
                status: RoutineRunStatus::Queued,
                ..run.clone()
            })
        );
        assert!(has_open_run(db, &routine.id).unwrap());
        assert!(!has_open_run(db, &other.id).unwrap());

        let before = now_ms();
        let running = set_run_status(db, &run.id, RoutineRunStatus::Running, None)
            .unwrap()
            .unwrap();
        let after = now_ms();
        let started = running.started_at.clone().unwrap();
        assert_stamped_between(&started, before, after);
        assert_eq!(
            running,
            RoutineRun {
                status: RoutineRunStatus::Running,
                started_at: Some(started.clone()),
                ..run.clone()
            }
        );
        assert!(has_open_run(db, &routine.id).unwrap());

        let before = now_ms();
        let done = set_run_status(db, &run.id, RoutineRunStatus::Done, Some("Checked"))
            .unwrap()
            .unwrap();
        let after = now_ms();
        let ended = done.ended_at.clone().unwrap();
        assert_stamped_between(&ended, before, after);
        assert!(parse_iso(&started).unwrap() <= parse_iso(&ended).unwrap());
        assert_eq!(
            done,
            RoutineRun {
                status: RoutineRunStatus::Done,
                started_at: Some(started.clone()),
                ended_at: Some(ended.clone()),
                note: Some("Checked".to_owned()),
                ..run.clone()
            }
        );
        assert!(!has_open_run(db, &routine.id).unwrap());
    }

    #[test]
    fn records_missed_and_skipped_runs_as_ended_without_starting_them() {
        for status in [RoutineRunStatus::Missed, RoutineRunStatus::Skipped] {
            let f = fixture();
            let db = &f.db;
            let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
            let before = now_ms();
            let run = add_run(
                db,
                &routine.id,
                "2026-09-25T06:48:00.000Z",
                status,
                Some("Not run"),
            )
            .unwrap();
            let after = now_ms();
            assert_eq!(run.status, status, "{status:?}");
            assert_eq!(run.note.as_deref(), Some("Not run"), "{status:?}");
            assert_eq!(run.started_at, None, "{status:?}");
            assert_stamped_between(run.ended_at.as_deref().unwrap(), before, after);
            assert!(!has_open_run(db, &routine.id).unwrap(), "{status:?}");
        }
    }

    #[test]
    fn retains_exactly_20_newest_runs_per_routine_and_breaks_due_time_ties_by_insertion_order() {
        assert_eq!(RUNS_KEPT, 20);
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let other = create_routine(
            db,
            &input_with(&f.bot_id, json!({"name": "Other"})),
            now_ms(),
        )
        .unwrap();
        let independent = add_run(
            db,
            &other.id,
            "2026-08-01T06:48:00.000Z",
            RoutineRunStatus::Done,
            None,
        )
        .unwrap();
        let runs: Vec<RoutineRun> = (0..25)
            .map(|_| {
                add_run(
                    db,
                    &routine.id,
                    "2026-09-25T06:48:00.000Z",
                    RoutineRunStatus::Done,
                    None,
                )
                .unwrap()
            })
            .collect();
        let expected: Vec<RoutineRun> = runs[5..].iter().rev().cloned().collect();
        assert_eq!(list_runs(db, &routine.id).unwrap(), expected);
        assert_eq!(get_run(db, &runs[0].id).unwrap(), None);
        assert_eq!(list_runs(db, &other.id).unwrap(), vec![independent]);
    }

    #[test]
    fn preserves_the_oldest_queued_run_while_pruning_finished_history_then_deletes_its_queued_work()
    {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let queued = add_run(
            db,
            &routine.id,
            "2026-08-01T06:48:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        let work = enqueue(
            db,
            &NewWork {
                routine_run_id: Some(queued.id.clone()),
                ..NewWork::new(&f.bot_id, WorkSource::Routine, "Waiting")
            },
        )
        .unwrap();
        let finished: Vec<RoutineRun> = (0..RUNS_KEPT)
            .map(|_| {
                add_run(
                    db,
                    &routine.id,
                    "2026-09-25T06:48:00.000Z",
                    RoutineRunStatus::Done,
                    None,
                )
                .unwrap()
            })
            .collect();
        assert_eq!(
            list_runs(db, &routine.id).unwrap().len(),
            usize::try_from(RUNS_KEPT + 1).unwrap()
        );
        let newest = add_run(
            db,
            &routine.id,
            "2026-09-26T06:48:00.000Z",
            RoutineRunStatus::Done,
            None,
        )
        .unwrap();
        let mut expected = vec![newest.clone()];
        expected.extend(finished[1..].iter().rev().cloned());
        expected.push(queued.clone());
        assert_eq!(list_runs(db, &routine.id).unwrap(), expected);
        assert_eq!(get_run(db, &finished[0].id).unwrap(), None);
        assert_eq!(get_run(db, &queued.id).unwrap(), Some(queued.clone()));
        assert!(has_open_run(db, &routine.id).unwrap());
        assert_eq!(
            get_work_item(db, &work.id).unwrap().routine_run_id,
            Some(queued.id.clone())
        );
        delete_routine(db, &routine.id).unwrap();
        assert!(list_runs(db, &routine.id).unwrap().is_empty());
        assert!(get_work_item(db, &work.id).is_err());
    }

    #[test]
    fn preserves_multiple_queued_and_running_runs_independently_of_the_finished_history_cap() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let queued = add_run(
            db,
            &routine.id,
            "2026-08-01T06:48:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        let running = add_run(
            db,
            &routine.id,
            "2026-08-02T06:48:00.000Z",
            RoutineRunStatus::Running,
            None,
        )
        .unwrap();
        let also_queued = add_run(
            db,
            &routine.id,
            "2026-08-03T06:48:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        let finished: Vec<RoutineRun> = (0..=RUNS_KEPT)
            .map(|_| {
                add_run(
                    db,
                    &routine.id,
                    "2026-09-25T06:48:00.000Z",
                    RoutineRunStatus::Done,
                    None,
                )
                .unwrap()
            })
            .collect();
        let mut expected: Vec<RoutineRun> = finished[1..].iter().rev().cloned().collect();
        expected.extend([also_queued.clone(), running.clone(), queued.clone()]);
        assert_eq!(list_runs(db, &routine.id).unwrap(), expected);
        assert_eq!(get_run(db, &running.id).unwrap(), Some(running.clone()));
        assert!(has_open_run(db, &routine.id).unwrap());
    }

    #[test]
    fn enforces_the_finished_history_cap_when_an_open_run_becomes_finished() {
        // it.each over "done", "stopped", "failed", "missed", "skipped"
        for status in [
            RoutineRunStatus::Done,
            RoutineRunStatus::Stopped,
            RoutineRunStatus::Failed,
            RoutineRunStatus::Missed,
            RoutineRunStatus::Skipped,
        ] {
            let f = fixture();
            let db = &f.db;
            let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
            let finished: Vec<RoutineRun> = (0..RUNS_KEPT)
                .map(|_| {
                    add_run(
                        db,
                        &routine.id,
                        "2026-09-25T06:48:00.000Z",
                        RoutineRunStatus::Done,
                        None,
                    )
                    .unwrap()
                })
                .collect();
            let open = add_run(
                db,
                &routine.id,
                "2026-09-26T06:48:00.000Z",
                RoutineRunStatus::Running,
                None,
            )
            .unwrap();
            let ended = set_run_status(db, &open.id, status, Some("Finished"))
                .unwrap()
                .unwrap();
            assert_eq!(ended.id, open.id, "{status:?}");
            assert_eq!(ended.status, status, "{status:?}");
            assert_eq!(ended.note.as_deref(), Some("Finished"), "{status:?}");
            assert!(ended.ended_at.is_some(), "{status:?}");
            let mut expected = vec![ended.clone()];
            expected.extend(finished[1..].iter().rev().cloned());
            assert_eq!(list_runs(db, &routine.id).unwrap(), expected, "{status:?}");
            assert_eq!(get_run(db, &finished[0].id).unwrap(), None, "{status:?}");
            assert!(!has_open_run(db, &routine.id).unwrap(), "{status:?}");
        }
    }

    #[test]
    fn returns_the_finished_snapshot_when_an_older_open_run_falls_outside_retained_history() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let open = add_run(
            db,
            &routine.id,
            "2026-08-01T06:48:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        let mut finished: Vec<RoutineRun> = (0..RUNS_KEPT)
            .map(|_| {
                add_run(
                    db,
                    &routine.id,
                    "2026-09-25T06:48:00.000Z",
                    RoutineRunStatus::Done,
                    None,
                )
                .unwrap()
            })
            .collect();
        let stopped = set_run_status(db, &open.id, RoutineRunStatus::Stopped, Some("Cancelled"))
            .unwrap()
            .unwrap();
        assert_eq!(stopped.id, open.id);
        assert_eq!(stopped.status, RoutineRunStatus::Stopped);
        assert_eq!(stopped.note.as_deref(), Some("Cancelled"));
        finished.reverse();
        assert_eq!(list_runs(db, &routine.id).unwrap(), finished);
        assert_eq!(get_run(db, &open.id).unwrap(), None);
        assert!(!has_open_run(db, &routine.id).unwrap());
    }

    #[test]
    fn returns_a_recorded_run_even_when_its_older_due_time_puts_it_outside_the_retained_history() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let mut retained: Vec<RoutineRun> = (0..20)
            .map(|_| {
                add_run(
                    db,
                    &routine.id,
                    "2026-09-25T06:48:00.000Z",
                    RoutineRunStatus::Done,
                    None,
                )
                .unwrap()
            })
            .collect();
        let older = add_run(
            db,
            &routine.id,
            "2026-08-01T06:48:00.000Z",
            RoutineRunStatus::Missed,
            Some("Late record"),
        )
        .unwrap();
        assert_eq!(older.routine_id, routine.id);
        assert_eq!(older.due_at, "2026-08-01T06:48:00.000Z");
        assert_eq!(older.status, RoutineRunStatus::Missed);
        assert_eq!(older.note.as_deref(), Some("Late record"));
        retained.reverse();
        assert_eq!(list_runs(db, &routine.id).unwrap(), retained);
        assert_eq!(get_run(db, &older.id).unwrap(), None);
    }

    #[test]
    fn deletes_only_the_routines_queued_work_leaving_active_and_unrelated_work() {
        let f = fixture();
        let db = &f.db;
        let routine = create_routine(db, &input(&f.bot_id), now_ms()).unwrap();
        let other = create_routine(
            db,
            &input_with(&f.bot_id, json!({"name": "Other"})),
            now_ms(),
        )
        .unwrap();
        let run = add_run(
            db,
            &routine.id,
            "2026-09-25T06:48:00.000Z",
            RoutineRunStatus::Running,
            None,
        )
        .unwrap();
        let waiting_run = add_run(
            db,
            &routine.id,
            "2026-09-25T10:47:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        let other_run = add_run(
            db,
            &other.id,
            "2026-09-25T06:48:00.000Z",
            RoutineRunStatus::Queued,
            None,
        )
        .unwrap();
        let active = enqueue(
            db,
            &NewWork {
                routine_run_id: Some(run.id.clone()),
                ..NewWork::new(&f.bot_id, WorkSource::Routine, "Active")
            },
        )
        .unwrap();
        set_status(db, &active.id, WorkStatus::Running).unwrap();
        let queued = enqueue(
            db,
            &NewWork {
                routine_run_id: Some(waiting_run.id.clone()),
                ..NewWork::new(&f.bot_id, WorkSource::Routine, "Waiting")
            },
        )
        .unwrap();
        let unrelated = enqueue(
            db,
            &NewWork {
                routine_run_id: Some(other_run.id.clone()),
                ..NewWork::new(&f.bot_id, WorkSource::Routine, "Other")
            },
        )
        .unwrap();
        let user_work = enqueue(db, &NewWork::new(&f.bot_id, WorkSource::You, "Hello")).unwrap();
        delete_routine(db, &routine.id).unwrap();
        assert!(
            get_routine(db, &routine.id)
                .unwrap_err()
                .message
                .contains("No such routine")
        );
        assert!(list_runs(db, &routine.id).unwrap().is_empty());
        assert!(get_work_item(db, &queued.id).is_err());
        assert_eq!(
            get_work_item(db, &active.id).unwrap().status,
            WorkStatus::Running
        );
        assert_eq!(get_work_item(db, &unrelated.id).unwrap(), unrelated);
        assert_eq!(get_work_item(db, &user_work.id).unwrap(), user_work);
        assert_eq!(get_routine(db, &other.id).unwrap(), other);
        assert_eq!(get_run(db, &other_run.id).unwrap(), Some(other_run));
    }
}

/// Not a TS case. The brief asks for `next_run_after` and `wall_time_to_utc` to refuse an unknown zone string directly:
/// `BadParams`, with a message containing "Unknown time zone".
#[test]
fn unknown_zone_is_refused_by_next_run_after_and_wall_time_to_utc() {
    let times = ["08:48".to_owned()];
    let schedule = Schedule {
        days: &WEEKDAYS,
        times: &times[..],
        time_zone: "Mars/Olympus",
    };
    let error = next_run_after(&schedule, parse_iso("2026-09-25T07:00:00Z").unwrap()).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadParams);
    assert!(
        error.message.contains("Unknown time zone"),
        "{}",
        error.message
    );
    let error = wall_time_to_utc(2026, 9, 25, 8, 48, "Mars/Olympus").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadParams);
    assert!(
        error.message.contains("Unknown time zone"),
        "{}",
        error.message
    );
}
