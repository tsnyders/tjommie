//! `mentions.ts`: @mentions in room posts. The daemon uses them to pick who answers, and the UI uses the same rule for its
//! hint and its highlighting, so the two never disagree.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}\p{M}_]").unwrap());

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| WORD.is_match(c.encode_utf8(&mut [0; 4])))
}

/// A mention can't follow an email-like ".", "-", "+" or "%".
fn is_email_end(c: Option<char>) -> bool {
    matches!(c, Some('.' | '-' | '+' | '%'))
}

/// Where a mention is. `name` is `None` for @everyone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionRange {
    pub start: usize,
    pub end: usize,
    pub name: Option<String>,
}

/// Each "@everyone" and "@Name" (whole names, any case, longest first) in the text, in order. A mention can't touch letters,
/// digits or "_" on either side, or follow an email-like ".", "-", "+" or "%".
/// ponytail: offsets are bytes into the lowercased text (TypeScript gives UTF-16 units), so they drift after a character
/// whose lowercase is longer. Map them to the original text if a Rust UI highlights non-ASCII names.
pub fn mention_ranges(text: &str, names: &[&str]) -> Vec<MentionRange> {
    let mut rest = text.to_lowercase();
    let mut ranges = Vec::new();
    let mut take = |tag: &str, name: Option<&str>| {
        let mut from = 0;
        while let Some(found) = rest[from..].find(tag) {
            let (at, end) = (from + found, from + found + tag.len());
            from = end;
            let (before, after) = (rest[..at].chars().next_back(), rest[end..].chars().next());
            if is_word(before) || is_email_end(before) || is_word(after) {
                continue;
            }
            rest.replace_range(at..end, &" ".repeat(tag.len()));
            ranges.push(MentionRange {
                start: at,
                end,
                name: name.map(str::to_owned),
            });
        }
    };
    take("@everyone", None);
    let mut longest_first = names.to_vec();
    longest_first.sort_by_key(|name| std::cmp::Reverse(name.encode_utf16().count()));
    for name in longest_first {
        take(&format!("@{}", name.to_lowercase()), Some(name));
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

/// The ids of the members a post mentions, in the order given. `bots` and `members` are `(id, name)` pairs; in a room the
/// members are its bots. Names are read against every bot, so "@Bot 2" is never taken for "@Bot" in a room without Bot 2.
/// "@everyone" mentions every member.
pub fn find_mentions(text: &str, bots: &[(&str, &str)], members: &[(&str, &str)]) -> Vec<String> {
    let names: Vec<&str> = bots.iter().map(|(_, name)| *name).collect();
    let ranges = mention_ranges(text, &names);
    let everyone = ranges.iter().any(|range| range.name.is_none());
    let named: HashSet<&str> = ranges
        .iter()
        .filter_map(|range| range.name.as_deref())
        .collect();
    members
        .iter()
        .filter(|(_, name)| everyone || named.contains(name))
        .map(|(id, _)| (*id).to_owned())
        .collect()
}
