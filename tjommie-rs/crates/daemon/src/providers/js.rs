//! JavaScript behaviour the providers' output depends on: string lengths in UTF-16 units, `String(value)`, truthiness,
//! `Math.round`, whitespace and `Date#toISOString`.

use serde_json::Value;

/// `text.slice(0, units)`. Never splits a surrogate pair; JavaScript would keep its first half.
pub fn slice(text: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, ch) in text.char_indices() {
        count += ch.len_utf16();
        if count > units {
            return &text[..index];
        }
    }
    text
}

/// `text.slice(-units)`.
pub fn slice_end(text: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, ch) in text.char_indices().rev() {
        count += ch.len_utf16();
        if count > units {
            return &text[index + ch.len_utf8()..];
        }
    }
    text
}

pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `String(value)`.
pub fn string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number
            .as_f64()
            .map_or_else(|| number.to_string(), |number| number.to_string()),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// A finite number, as `typeof value === "number" && Number.isFinite(value)` accepts it.
pub fn finite(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
}

/// `Math.round`: halves round towards positive infinity.
pub fn round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// `\s` and `String#trim` whitespace.
pub fn is_space(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
    ) || matches!(
        ch,
        '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// The regex class for `\s`.
pub const SPACE_CLASS: &str = r"[\t\n\x0B\x0C\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]";

pub fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `new Date(ms).toISOString()`, or `None` for an invalid date. Years past ±9999 are `None` too; JavaScript would
/// write them with six digits.
pub fn iso_from_millis(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return None;
    }
    let timestamp = jiff::Timestamp::from_millisecond(ms.trunc() as i64).ok()?;
    let date = jiff::tz::Offset::UTC.to_datetime(timestamp);
    let year = date.year();
    let year = if year < 0 {
        format!("-{:06}", -i32::from(year))
    } else {
        format!("{year:04}")
    };
    Some(format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        date.month(),
        date.day(),
        date.hour(),
        date.minute(),
        date.second(),
        date.millisecond()
    ))
}

/// `Date.parse` for ISO strings: with an offset or `Z` it is that instant, a date-time without one is local time, and a
/// date alone is UTC midnight. ponytail: the other formats JavaScript engines accept (RFC 2822 and the like) are `None`,
/// JavaScript's `NaN`; add them if a provider ever sends one.
pub fn date_parse(text: &str) -> Option<f64> {
    use jiff::{Timestamp, civil, tz::TimeZone};
    let ms = |instant: Timestamp| instant.as_millisecond() as f64;
    if let Ok(instant) = text.parse::<Timestamp>() {
        return Some(ms(instant));
    }
    let zoned = if text.contains(['T', 't', ' ']) {
        text.parse::<civil::DateTime>()
            .ok()?
            .to_zoned(TimeZone::system())
    } else {
        text.parse::<civil::Date>().ok()?.to_zoned(TimeZone::UTC)
    };
    zoned.ok().map(|zoned| ms(zoned.timestamp()))
}

/// `Number(text)` for decimal text: blank is 0, anything else unparseable is `NaN`. ponytail: decimal only;
/// JavaScript also reads `0x`/`0o`/`0b` prefixes, which no caller's input carries.
pub fn number(text: &str) -> f64 {
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    let numeric = text
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
    if numeric {
        text.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

/// `JSON.stringify(value, null, 2)`.
pub fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON values serialize")
}
