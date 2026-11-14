//! Port of `packages/ui/lib/approval.ts`: the short lines an approval card shows. Claude sends the tool's input as JSON;
//! Codex sends the command or folder as text.

use serde_json::Value;

/// The longest line a card keeps, in UTF-16 units as JavaScript's `slice` counts them.
const LINE_UNITS: usize = 240;

/// The keys of Claude's tool input that name what it acts on, in the order they're looked for.
const DETAIL_KEYS: [&str; 7] = [
    "file_path",
    "path",
    "notebook_path",
    "command",
    "url",
    "pattern",
    "query",
];

/// One line: runs of whitespace become one space, the ends are trimmed, and it's cut to 240 UTF-16 units. A character
/// outside the BMP counts as two; one that would straddle the cut is left out, where JavaScript would split it.
fn one_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut units = 0;
    joined
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= LINE_UNITS
        })
        .collect()
}

/// What an approval card asks about: the path, command or address in Claude's tool input, or Codex's command text.
pub fn approval_detail(detail: &str) -> String {
    if let Ok(Value::Object(record)) = serde_json::from_str::<Value>(detail) {
        for key in DETAIL_KEYS {
            if let Some(Value::String(value)) = record.get(key) {
                return one_line(value);
            }
        }
        // serde_json sorts an object's keys, where JavaScript keeps the input's order, so with no known key this
        // is the first text value by key, not by position. Claude's tools all have a known key.
        if let Some(text) = record.values().find_map(Value::as_str) {
            return one_line(text);
        }
    }
    one_line(detail)
}

/// What "Always allow" saves, in words. Claude's rules read as they are; Codex's are a command prefix
/// (`codex:["npm","test"]`) or an MCP tool (`codex-mcp:browser.browser_navigate`). A Codex list with anything but
/// text in it is shown as it is (JavaScript would join its values; Codex sends only text).
pub fn approval_rule(rule: &str) -> String {
    if let Some(tool) = rule.strip_prefix("codex-mcp:") {
        return one_line(tool);
    }
    if let Some(list) = rule.strip_prefix("codex:")
        && let Ok(Value::Array(items)) = serde_json::from_str::<Value>(list)
        && let Some(words) = items.iter().map(Value::as_str).collect::<Option<Vec<_>>>()
        && !words.is_empty()
    {
        return one_line(&words.join(" "));
    }
    one_line(rule)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// The JSON text an approval's input arrives as: pretty-printed, like the TypeScript's `JSON.stringify(..., null, 2)`.
    fn pretty(value: Value) -> String {
        serde_json::to_string_pretty(&value).unwrap()
    }

    #[test]
    fn picks_the_path_command_or_address_out_of_claudes_tool_input() {
        let path = r"C:\Users\owner\Documents\Job applications\CV-Sabenza-Angular.pdf";
        assert_eq!(
            approval_detail(&pretty(json!({"file_path": path, "content": "..."}))),
            path
        );
        assert_eq!(
            approval_detail(&pretty(
                json!({"command": "npm test", "description": "Run the tests"})
            )),
            "npm test"
        );
        assert_eq!(
            approval_detail(&pretty(json!({"url": "https://pnet.co.za/jobs"}))),
            "https://pnet.co.za/jobs"
        );
    }

    #[test]
    fn keeps_codexs_command_text_on_one_line_and_shortens_very_long_details() {
        assert_eq!(
            approval_detail("Set-Content -LiteralPath 'notes.txt'\n  -Value 'hi'"),
            "Set-Content -LiteralPath 'notes.txt' -Value 'hi'"
        );
        assert_eq!(approval_detail(&"x".repeat(500)).len(), 240);
    }

    #[test]
    fn says_what_always_allow_saves_in_words_including_codexs_command_prefixes_and_tools() {
        assert_eq!(approval_rule("Write"), "Write");
        assert_eq!(approval_rule("Bash(npm test:*)"), "Bash(npm test:*)");
        assert_eq!(
            approval_rule(r#"codex:["Set-Content","-LiteralPath","notes.txt","-Value","hi"]"#),
            "Set-Content -LiteralPath notes.txt -Value hi"
        );
        assert_eq!(
            approval_rule("codex-mcp:browser.browser_navigate"),
            "browser.browser_navigate"
        );
    }

    #[test]
    fn shows_text_that_is_not_a_known_input_as_it_is() {
        // Not an object: the text itself, whatever its JSON shape.
        assert_eq!(approval_detail("[1,2]"), "[1,2]");
        // No known key, so the first text value: a number is skipped.
        assert_eq!(
            approval_detail(r#"{"count": 3, "label": "Scout"}"#),
            "Scout"
        );
        // Codex's list must be a non-empty list of text; anything else is shown as it is.
        assert_eq!(approval_rule("codex:[]"), "codex:[]");
        assert_eq!(approval_rule("codex:not a list"), "codex:not a list");
    }
}
