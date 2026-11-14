//! Activity labels are stored with the message, so both terminal and UI see the same words.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::{Map, Value};
use url::Url;

use super::js;

// `(?i-u)`: JavaScript's `/i` without `/u` folds ASCII only.
static SELECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i-u:select:){}*", js::SPACE_CLASS)).unwrap());
static MCP_TOOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i-u)mcp__[a-z0-9_-]+?__([a-z0-9_-]+)").unwrap());
static MCP: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i-u)mcp__").unwrap());
static SPACES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("{}+", js::SPACE_CLASS)).unwrap());
// `.` in JavaScript stops at every line terminator, not just `\n`.
static FULL_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^mcp__([^\n\r\x{2028}\x{2029}]+?)__([^\n\r\x{2028}\x{2029}]+)$").unwrap()
});

pub fn plain_activity_text(value: &str) -> String {
    let text = SELECT.replace_all(value, "");
    let text = MCP_TOOL.replace_all(&text, |captures: &Captures| captures[1].replace('_', " "));
    let text = MCP.replace_all(&text, "");
    let text = SPACES.replace_all(&text, " ");
    js::slice(js::trim(&text), 200).to_string()
}

fn text_arg(input: &Map<String, Value>, key: &str) -> Option<String> {
    let value = js::trim(input.get(key)?.as_str()?);
    Some(plain_activity_text(value)).filter(|text| !text.is_empty())
}

fn file_name(path: Option<String>) -> String {
    path.and_then(|path| {
        path.split(['\\', '/'])
            .rfind(|part| !part.is_empty())
            .map(str::to_string)
    })
    .unwrap_or_else(|| "a file".into())
}

fn browser_target(input: &Map<String, Value>) -> String {
    let Some(url) = text_arg(input, "url") else {
        return "a page".into();
    };
    match Url::parse(&url) {
        Ok(parsed) => parsed
            .host_str()
            .filter(|host| !host.is_empty())
            .map_or(url.clone(), str::to_string),
        Err(_) => url,
    }
}

fn browser_label(tool: &str, input: &Map<String, Value>) -> String {
    let action = tool.strip_prefix("browser_").unwrap_or(tool);
    let element = || text_arg(input, "element").unwrap_or_else(|| "an element".into());
    match action {
        "navigate" => format!("Browser: opened {}", browser_target(input)),
        "navigate_back" => "Browser: went back".into(),
        "click" => format!("Browser: clicked {}", element()),
        "snapshot" => "Browser: viewed the page".into(),
        "take_screenshot" => "Browser: took a screenshot".into(),
        "type" => "Browser: typed into a field".into(),
        "fill_form" => "Browser: filled a form".into(),
        "press_key" => format!(
            "Browser: pressed {}",
            text_arg(input, "key").unwrap_or_else(|| "a key".into())
        ),
        "select_option" => "Browser: chose an option".into(),
        "hover" => format!("Browser: hovered over {}", element()),
        _ => format!("Browser: {}", action.replace('_', " ")),
    }
}

/// `None` hides the call (Claude's tool loading). An empty label means the same to callers.
pub fn activity_label(
    name: &str,
    input: &Map<String, Value>,
    server_name: Option<&str>,
) -> Option<String> {
    if name.eq_ignore_ascii_case("ToolSearch") {
        return None;
    }
    let full_name = FULL_NAME.captures(name);
    let server = full_name
        .as_ref()
        .map(|captures| captures.get(1).unwrap().as_str())
        .or(server_name);
    let tool = full_name
        .as_ref()
        .map_or(name, |captures| captures.get(2).unwrap().as_str());

    let label = match server {
        Some("tjommie") => match tool {
            "list_bots" => "Looked at the team".into(),
            "message_bot" => format!(
                "Messaged {}",
                text_arg(input, "bot").unwrap_or_else(|| "an agent".into())
            ),
            "share_file" => format!("Shared {}", file_name(text_arg(input, "path"))),
            "add_routine" => format!(
                "Proposed a routine: {}",
                text_arg(input, "name").unwrap_or_else(|| "a routine".into())
            ),
            _ => tool.replace('_', " "),
        },
        Some("browser") => browser_label(tool, input),
        Some(server) if !server.is_empty() => tool.replace('_', " "),
        _ => [
            "description",
            "command",
            "file_path",
            "url",
            "pattern",
            "query",
        ]
        .into_iter()
        .find_map(|key| text_arg(input, key))
        .unwrap_or_else(|| name.to_string()),
    };
    Some(plain_activity_text(&label))
}
