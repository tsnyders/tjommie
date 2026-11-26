//! Port of `packages/ui/lib/mentions.ts`: typing "@name" in the composer, and the names it can complete to.
//! Offsets are UTF-8 byte offsets into the composer's text, which is what the input's caret is. The TypeScript counts
//! UTF-16 units; the two agree for the ASCII the offsets come from, and the query's length limit counts UTF-16 units.

use protocol::model::Bot;

/// The longest query a mention can have, in UTF-16 units.
const MAX_QUERY_UNITS: usize = 40;

/// The "@..." being typed: where it starts (its "@"), and what follows the "@".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedMention {
    pub start: usize,
    pub query: String,
}

/// The "@..." being typed right before the caret. Names can have spaces, so the query runs to the caret. An "@" inside a
/// word ("me@...") is an email address, not a mention. `None` for a caret that isn't in the text or sits inside a character.
pub fn typed_mention(text: &str, caret: usize) -> Option<TypedMention> {
    let before = text.get(..caret.min(text.len()))?;
    let (head, query) = before.rsplit_once('@')?;
    if query.contains('\n') || query.encode_utf16().count() > MAX_QUERY_UNITS {
        return None;
    }
    // The "@" starts a word: at the start of the text, or after a space or "(". (JavaScript's `\s` and Rust's
    // whitespace differ only on U+FEFF and U+0085, which nobody types before a mention.)
    if let Some(previous) = head.chars().next_back()
        && !(previous.is_whitespace() || previous == '(')
    {
        return None;
    }
    Some(TypedMention {
        start: head.len(),
        query: query.to_owned(),
    })
}

/// A completion: a bot, or `None` for `@everyone`.
#[derive(Debug, Clone, PartialEq)]
pub struct MentionOption<'a> {
    pub name: String,
    pub bot: Option<&'a Bot>,
}

/// The bots whose names start with the query (any case), then `@everyone`. Empty when nothing matches.
pub fn mention_options<'a>(query: &str, bots: &'a [Bot]) -> Vec<MentionOption<'a>> {
    let typed = query.to_lowercase();
    let mut options: Vec<MentionOption<'a>> = bots
        .iter()
        .filter(|bot| bot.name.to_lowercase().starts_with(typed.as_str()))
        .map(|bot| MentionOption {
            name: bot.name.clone(),
            bot: Some(bot),
        })
        .collect();
    if "everyone".starts_with(typed.as_str()) {
        options.push(MentionOption {
            name: "everyone".to_owned(),
            bot: None,
        });
    }
    options
}

/// The text after a mention is put in: the text, and where the caret goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    pub text: String,
    pub caret: usize,
}

/// Swaps the typed "@..." (from `start` to the caret) for the whole mention and a space, and says where the caret goes.
/// `None` when the range isn't in the text: a caret before `start`, or an offset inside a character.
pub fn insert_mention(text: &str, start: usize, caret: usize, name: &str) -> Option<Insertion> {
    if start > caret {
        return None;
    }
    let head = text.get(..start)?;
    let tail = text.get(caret..)?;
    let mention = format!("@{name} ");
    Some(Insertion {
        text: format!("{head}{mention}{tail}"),
        caret: start + mention.len(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn bot(id: &str, name: &str) -> Bot {
        serde_json::from_value(json!({
            "id": id, "name": name, "color": "orange", "shape": "circle", "instructions": "",
            "provider": "claude", "model": "sonnet", "autonomy": "ask", "homeFolder": "C:/b",
            "projectFolder": null, "browser": false, "paused": false, "canMessage": [],
            "state": "idle", "setupReason": null, "limitedUntil": null, "createdAt": "2026-09-27T00:00:00Z"
        }))
        .unwrap()
    }

    fn bots() -> Vec<Bot> {
        vec![
            bot("b1", "Careers24 scout"),
            bot("b2", "Chief of Staff"),
            bot("b3", "CV Writer"),
        ]
    }

    fn names<'a>(options: &'a [MentionOption<'_>]) -> Vec<&'a str> {
        options.iter().map(|option| option.name.as_str()).collect()
    }

    #[test]
    fn finds_the_at_being_typed_and_ignores_email_addresses() {
        assert_eq!(
            typed_mention("Hi @Ca", 6),
            Some(TypedMention {
                start: 3,
                query: "Ca".to_owned()
            })
        );
        assert_eq!(
            typed_mention("@Chief of", 9),
            Some(TypedMention {
                start: 0,
                query: "Chief of".to_owned()
            })
        );
        assert_eq!(typed_mention("mail me@wr", 10), None);
        assert_eq!(typed_mention("no mention", 10), None);
        assert_eq!(typed_mention("@a\nb", 4), None);
    }

    #[test]
    fn reads_carets_the_way_the_composer_gives_them() {
        // A caret past the end reads the whole text, as JavaScript's slice does.
        assert_eq!(typed_mention("Hi @Ca", 99), typed_mention("Hi @Ca", 6));
        // A caret inside "é" (two bytes) is no caret.
        assert_eq!(typed_mention("é@x", 1), None);
        // The query is at most 40 UTF-16 units: an emoji is two.
        let forty = format!("@{}", "a".repeat(40));
        assert!(typed_mention(&forty, forty.len()).is_some());
        let forty_one = format!("@{}", "a".repeat(41));
        assert_eq!(typed_mention(&forty_one, forty_one.len()), None);
        let twenty_emoji = format!("@{}", "😀".repeat(20));
        assert!(typed_mention(&twenty_emoji, twenty_emoji.len()).is_some());
        let twenty_one_emoji = format!("@{}", "😀".repeat(21));
        assert_eq!(
            typed_mention(&twenty_one_emoji, twenty_one_emoji.len()),
            None
        );
    }

    #[test]
    fn offers_the_bots_whose_names_start_with_it_then_everyone() {
        let bots = bots();
        assert_eq!(names(&mention_options("ca", &bots)), ["Careers24 scout"]);
        assert_eq!(
            names(&mention_options("c", &bots)),
            ["Careers24 scout", "Chief of Staff", "CV Writer"]
        );
        assert_eq!(
            names(&mention_options("", &bots)),
            ["Careers24 scout", "Chief of Staff", "CV Writer", "everyone"]
        );
        assert_eq!(
            mention_options("ev", &bots),
            [MentionOption {
                name: "everyone".to_owned(),
                bot: None
            }]
        );
        assert!(mention_options("zz", &bots).is_empty());
    }

    #[test]
    fn puts_the_whole_mention_in_with_a_space_after_it() {
        assert_eq!(
            insert_mention("Hi @Ca please", 3, 6, "Careers24 scout"),
            Some(Insertion {
                text: "Hi @Careers24 scout  please".to_owned(),
                caret: 20
            })
        );
        // A caret before the "@" or past the text is no insertion.
        assert_eq!(insert_mention("Hi @Ca please", 6, 3, "x"), None);
        assert_eq!(insert_mention("Hi", 0, 9, "x"), None);
    }
}
