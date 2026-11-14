//! Port of `packages/ui/lib/format.ts`: times, days, schedules, sizes. Timestamps are the wire's ISO 8601 strings; a value
//! that doesn't parse gives an empty string (JavaScript's `Invalid Date`). Days are calendar days in the zone of `now`.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const DAY_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn at(value: &str, zone: &TimeZone) -> Option<Zoned> {
    value
        .parse::<Timestamp>()
        .ok()
        .map(|timestamp| timestamp.to_zoned(zone.clone()))
}

/// `Sunday` is 0, like JavaScript's `getDay()`.
fn weekday_index(date: Date) -> usize {
    usize::try_from(date.weekday().to_sunday_zero_offset()).unwrap_or(0)
}

fn clock_hm(hour: i8, minute: i8) -> String {
    let twelve = match hour % 12 {
        0 => 12,
        other => other,
    };
    let half = if hour < 12 { "AM" } else { "PM" };
    format!("{twelve}:{minute:02} {half}")
}

/// "9:14 AM", with a plain space before AM or PM.
pub fn clock(value: &str, zone: &TimeZone) -> String {
    at(value, zone)
        .map(|time| clock_hm(time.hour(), time.minute()))
        .unwrap_or_default()
}

fn names(date: Date, list: &[&'static str; 7]) -> &'static str {
    list.get(weekday_index(date)).copied().unwrap_or("")
}

fn day_month(date: Date) -> String {
    let month = usize::try_from(date.month() - 1)
        .ok()
        .and_then(|index| MONTHS.get(index))
        .copied()
        .unwrap_or("");
    format!("{} {month}", date.day())
}

/// "Today", "Yesterday", "Tomorrow", a weekday within a week either way ("Friday", or "Fri" when `short`), otherwise
/// "21 September".
pub fn day_name(value: &str, now: &Zoned, short: bool) -> String {
    let Some(date) = at(value, now.time_zone()) else {
        return String::new();
    };
    let days = date
        .date()
        .until(now.date())
        .map(|span| span.get_days())
        .unwrap_or(0);
    match days {
        0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        -1 => "Tomorrow".to_owned(),
        -6..=6 => names(date.date(), if short { &DAY_SHORT } else { &WEEKDAYS }).to_owned(),
        _ => day_month(date.date()),
    }
}

/// "Monday 21 September".
pub fn long_date(value: &str, zone: &TimeZone) -> String {
    at(value, zone)
        .map(|time| {
            format!(
                "{} {}",
                names(time.date(), &WEEKDAYS),
                day_month(time.date())
            )
        })
        .unwrap_or_default()
}

/// "Every day", "Weekdays", "Weekends" or "Mon, Wed". `days` are 0 (Sunday) to 6.
pub fn days_label(days: &[u8]) -> String {
    let mut sorted = days.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    match sorted.as_slice() {
        [0, 1, 2, 3, 4, 5, 6] => "Every day".to_owned(),
        [1, 2, 3, 4, 5] => "Weekdays".to_owned(),
        [0, 6] => "Weekends".to_owned(),
        _ => sorted
            .iter()
            .map(|day| DAY_SHORT.get(usize::from(*day)).copied().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// "8:48 AM, 4:45 PM" from "HH:MM" times. A time that isn't "HH:MM" is left out.
pub fn times_label(times: &[&str]) -> String {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .iter()
        .filter_map(|time| {
            let hour = time.get(..2)?.parse::<i8>().ok()?;
            let minute = time.get(3..)?.parse::<i8>().ok()?;
            ((0..24).contains(&hour) && (0..60).contains(&minute)).then(|| clock_hm(hour, minute))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// "Weekdays at 8:48 AM", "Every day at 7:00 AM, 5:00 PM", "Mon, Wed at 9:00 AM".
pub fn schedule(days: &[u8], times: &[&str]) -> String {
    format!("{} at {}", days_label(days), times_label(times))
}

/// "just now", "4 minutes ago", "2 hours ago", "yesterday", or "on Monday".
pub fn time_ago(value: &str, now: &Zoned) -> String {
    let Ok(then) = value.parse::<Timestamp>() else {
        return String::new();
    };
    let minutes = (now.timestamp().as_millisecond() - then.as_millisecond()).div_euclid(60_000);
    if minutes < 1 {
        return "just now".to_owned();
    }
    if minutes < 60 {
        return format!("{minutes} {} ago", plural(minutes, "minute"));
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} {} ago", plural(hours, "hour"));
    }
    match day_name(value, now, false).as_str() {
        "Yesterday" => "yesterday".to_owned(),
        day => format!("on {day}"),
    }
}

fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        unit.to_owned()
    } else {
        format!("{unit}s")
    }
}

/// How long something ran: "2h 51m", "3h", "4m" or "under a minute".
pub fn duration(milliseconds: i64) -> String {
    // Math.round: halves go up.
    let minutes = (milliseconds as f64 / 60_000.0 + 0.5).floor() as i64;
    if minutes < 1 {
        return "under a minute".to_owned();
    }
    let (hours, rest) = (minutes / 60, minutes % 60);
    match (hours, rest) {
        (0, rest) => format!("{rest}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, rest) => format!("{hours}h {rest}m"),
    }
}

/// "Today 9:14 AM": the time labels between messages.
pub fn stamp(value: &str, now: &Zoned) -> String {
    format!(
        "{} {}",
        day_name(value, now, false),
        clock(value, now.time_zone())
    )
}

/// "512 bytes", "184 KB", "1.5 MB".
pub fn file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < KB {
        return format!("{bytes} bytes");
    }
    if bytes < MB {
        // Math.round: halves go up.
        return format!("{} KB", (bytes + KB / 2) / KB);
    }
    // toFixed(1): halves go up.
    let tenths = (u128::from(bytes) * 10 + u128::from(MB / 2)) / u128::from(MB);
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

/// "Pnet apply 9am (3h)": a routine's name with its time limit.
pub fn routine_title(name: &str, limit_minutes: Option<u32>) -> String {
    match limit_minutes {
        None | Some(0) => name.to_owned(),
        Some(limit) if limit % 60 == 0 => format!("{name} ({}h)", limit / 60),
        Some(limit) => format!("{name} ({limit}m)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sunday 27 September 2026, 2:00 PM. The zone is fixed so the days don't depend on this machine's.
    fn now() -> Zoned {
        "2026-09-27T14:00:00[UTC]".parse().unwrap()
    }

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    /// `new Date(2026, 8, day, hour, minute)` in the tests' zone, as the wire writes it.
    fn on(day: i8, hour: i8, minute: i8) -> String {
        format!("2026-09-{day:02}T{hour:02}:{minute:02}:00Z")
    }

    #[test]
    fn writes_times_the_way_the_wireframes_do() {
        assert_eq!(clock(&on(27, 9, 14), &utc()), "9:14 AM");
        assert_eq!(clock(&on(27, 17, 0), &utc()), "5:00 PM");
        assert_eq!(stamp(&on(27, 9, 14), &now()), "Today 9:14 AM");
    }

    #[test]
    fn names_days_today_yesterday_a_weekday_this_week_else_the_date() {
        assert_eq!(day_name(&on(26, 22, 0), &now(), false), "Yesterday");
        assert_eq!(day_name(&on(22, 9, 0), &now(), false), "Tuesday");
        assert_eq!(day_name(&on(20, 9, 0), &now(), false), "20 September");
        assert_eq!(day_name(&on(18, 9, 0), &now(), false), "18 September");
    }

    #[test]
    fn sizes_files_and_describes_routine_schedules() {
        assert_eq!(file_size(512), "512 bytes");
        assert_eq!(file_size(184 * 1024), "184 KB");
        assert_eq!(file_size(3 * 1024 * 1024 / 2), "1.5 MB");
        assert_eq!(
            schedule(&[1, 2, 3, 4, 5], &["08:48"]),
            "Weekdays at 8:48 AM"
        );
        assert_eq!(
            schedule(&[0, 1, 2, 3, 4, 5, 6], &["07:00", "17:00"]),
            "Every day at 7:00 AM, 5:00 PM"
        );
        assert_eq!(schedule(&[3, 1], &["09:00"]), "Mon, Wed at 9:00 AM");
        assert_eq!(schedule(&[0, 6], &["09:00"]), "Weekends at 9:00 AM");
        assert_eq!(
            routine_title("Pnet apply 9am", Some(180)),
            "Pnet apply 9am (3h)"
        );
        assert_eq!(
            routine_title("Night digest", Some(90)),
            "Night digest (90m)"
        );
        assert_eq!(routine_title("Night digest", None), "Night digest");
    }

    #[test]
    fn names_tomorrow_and_the_coming_week_and_writes_long_dates() {
        assert_eq!(day_name(&on(28, 9, 0), &now(), false), "Tomorrow");
        assert_eq!(day_name(&on(30, 9, 0), &now(), false), "Wednesday");
        assert_eq!(day_name(&on(30, 9, 0), &now(), true), "Wed");
        assert_eq!(long_date(&on(21, 10, 0), &utc()), "Monday 21 September");
    }

    #[test]
    fn says_how_long_ago_and_how_long() {
        assert_eq!(time_ago("2026-09-27T13:59:30Z", &now()), "just now");
        assert_eq!(time_ago(&on(27, 13, 56), &now()), "4 minutes ago");
        assert_eq!(time_ago(&on(27, 12, 0), &now()), "2 hours ago");
        assert_eq!(time_ago(&on(26, 9, 0), &now()), "yesterday");
        assert_eq!(duration((2 * 60 + 51) * 60_000), "2h 51m");
        assert_eq!(duration(3 * 3_600_000), "3h");
        assert_eq!(duration(4 * 60_000), "4m");
        assert_eq!(duration(20_000), "under a minute");
        assert_eq!(days_label(&[6, 0]), "Weekends");
        assert_eq!(
            times_label(&["16:45", "08:48", "16:45"]),
            "8:48 AM, 4:45 PM"
        );
    }
}
