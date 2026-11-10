//! One line of `claude -p --output-format stream-json` output, reduced to what Tjommie uses.

use serde_json::{Map, Value};

use super::activity_labels::activity_label;
use super::{Limit, LimitState, LimitWindow, TokenUsage, js};

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Permission {
    pub request_id: String,
    pub tool_name: String,
    pub input: Map<String, Value>,
    pub description: String,
    pub rule: String,
    pub suggestion: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultLine {
    pub event_id: Option<String>,
    pub ok: bool,
    pub error: Option<String>,
    pub total_cost_usd: Option<f64>,
    pub model: Option<String>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeLine {
    Init {
        session_id: String,
    },
    TextDelta {
        text: String,
    },
    Assistant {
        texts: Vec<String>,
        tools: Vec<ToolUse>,
    },
    /// `(tool use id, ok)`.
    ToolResults {
        results: Vec<(String, bool)>,
    },
    Permission(Permission),
    /// Its `limit_id` is always `None`.
    RateLimit(Limit),
    Result(ResultLine),
    Other,
}

fn object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

fn number_or_zero(map: &Map<String, Value>, key: &str) -> f64 {
    map.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn limit_state(reached: bool, percent: Option<f64>) -> LimitState {
    match percent {
        _ if reached => LimitState::Reached,
        Some(percent) if percent >= 100.0 => LimitState::Reached,
        Some(percent) if percent >= 90.0 => LimitState::Near,
        _ => LimitState::Ok,
    }
}

pub fn parse_claude_line(line: &str) -> ClaudeLine {
    let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(line) else {
        return ClaudeLine::Other;
    };

    // Subagent traffic stays inside its tool call and never reaches the chat.
    let from_subagent = !matches!(parsed.get("parent_tool_use_id"), None | Some(Value::Null));
    let empty = Map::new();

    match text(&parsed, "type") {
        Some("system") => match (text(&parsed, "subtype"), text(&parsed, "session_id")) {
            (Some("init"), Some(session_id)) => ClaudeLine::Init {
                session_id: session_id.into(),
            },
            _ => ClaudeLine::Other,
        },
        Some("stream_event") => {
            let Some(event) = object(parsed.get("event")).filter(|_| !from_subagent) else {
                return ClaudeLine::Other;
            };
            let Some(delta) = object(event.get("delta")) else {
                return ClaudeLine::Other;
            };
            match (
                text(event, "type"),
                text(delta, "type"),
                text(delta, "text"),
            ) {
                (Some("content_block_delta"), Some("text_delta"), Some(text)) => {
                    ClaudeLine::TextDelta { text: text.into() }
                }
                _ => ClaudeLine::Other,
            }
        }
        Some("assistant") => {
            if from_subagent {
                return ClaudeLine::Other;
            }
            let message = object(parsed.get("message")).unwrap_or(&empty);
            let content = message
                .get("content")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            let (mut texts, mut tools) = (Vec::new(), Vec::new());
            for block in content.iter().filter_map(Value::as_object) {
                match (
                    text(block, "type"),
                    text(block, "text"),
                    text(block, "id"),
                    text(block, "name"),
                ) {
                    (Some("text"), Some(text), _, _) => texts.push(text.to_string()),
                    (Some("tool_use"), _, Some(id), Some(name)) => tools.push(ToolUse {
                        id: id.into(),
                        name: name.into(),
                        input: object(block.get("input")).cloned().unwrap_or_default(),
                    }),
                    _ => {}
                }
            }
            ClaudeLine::Assistant { texts, tools }
        }
        Some("user") => {
            let content = object(parsed.get("message"))
                .and_then(|message| message.get("content")?.as_array());
            let Some(content) = content.filter(|_| !from_subagent) else {
                return ClaudeLine::Other;
            };
            let results: Vec<(String, bool)> = content
                .iter()
                .filter_map(Value::as_object)
                .filter(|block| text(block, "type") == Some("tool_result"))
                .filter_map(|block| {
                    Some((
                        text(block, "tool_use_id")?.to_string(),
                        block.get("is_error") != Some(&Value::Bool(true)),
                    ))
                })
                .collect();
            if results.is_empty() {
                ClaudeLine::Other
            } else {
                ClaudeLine::ToolResults { results }
            }
        }
        Some("control_request") => {
            let Some(request) = object(parsed.get("request"))
                .filter(|request| text(request, "subtype") == Some("can_use_tool"))
            else {
                return ClaudeLine::Other;
            };
            let (Some(request_id), Some(tool_name)) =
                (text(&parsed, "request_id"), text(request, "tool_name"))
            else {
                return ClaudeLine::Other;
            };
            let suggestion = request
                .get("permission_suggestions")
                .and_then(Value::as_array)
                .and_then(|suggestions| {
                    suggestions
                        .iter()
                        .filter_map(Value::as_object)
                        .find(|candidate| {
                            text(candidate, "type") == Some("addRules")
                                && text(candidate, "behavior") == Some("allow")
                        })
                })
                .cloned();
            let first = suggestion
                .as_ref()
                .and_then(|suggestion| object(suggestion.get("rules")?.as_array()?.first()));
            let first_tool_name = first
                .and_then(|first| text(first, "toolName"))
                .filter(|name| !name.is_empty());
            let first_rule_content = first
                .and_then(|first| text(first, "ruleContent"))
                .filter(|content| !content.is_empty());
            let rule = match (first_tool_name, first_rule_content) {
                (Some(name), Some(content)) => format!("{name}({content})"),
                (Some(name), None) => name.into(),
                (None, _) => tool_name.into(),
            };
            ClaudeLine::Permission(Permission {
                request_id: request_id.into(),
                tool_name: tool_name.into(),
                input: object(request.get("input")).cloned().unwrap_or_default(),
                description: text(request, "description").unwrap_or(tool_name).into(),
                rule,
                suggestion,
            })
        }
        Some("rate_limit_event") => {
            let info = object(parsed.get("rate_limit_info")).unwrap_or(&empty);
            let windows = object(info.get("unifiedWindows")).unwrap_or(&empty);
            let limit_type = text(info, "rateLimitType");
            let rejected = text(info, "status") == Some("rejected");
            let selected = limit_type.and_then(|limit_type| object(windows.get(limit_type)));
            let all_windows: Vec<LimitWindow> = windows
                .iter()
                .filter_map(|(window_key, value)| Some((window_key, value.as_object()?)))
                .map(|(window_key, value)| {
                    let percent = js::finite(value.get("utilization"))
                        .map(|used| js::round(used * 100.0).clamp(0.0, 100.0));
                    let resets_at = value
                        .get("resetsAt")
                        .and_then(Value::as_f64)
                        .and_then(|seconds| js::iso_from_millis(seconds * 1000.0));
                    LimitWindow {
                        window_key: window_key.clone(),
                        duration_minutes: match window_key.as_str() {
                            "five_hour" => Some(300.0),
                            "seven_day" => Some(10080.0),
                            _ => None,
                        },
                        used_percent: percent,
                        resets_at,
                        state: limit_state(
                            Some(window_key.as_str()) == limit_type && rejected,
                            percent,
                        ),
                    }
                })
                .collect();
            ClaudeLine::RateLimit(Limit {
                state: match text(info, "status") {
                    Some("rejected") => LimitState::Reached,
                    Some("allowed_warning") => LimitState::Near,
                    _ => LimitState::Ok,
                },
                used_percent: selected
                    .and_then(|window| window.get("utilization")?.as_f64())
                    .map(|used| js::round(used * 100.0)),
                // TypeScript throws on a reset outside the Date range; this reports none.
                resets_at: info
                    .get("resetsAt")
                    .and_then(Value::as_f64)
                    .and_then(|seconds| js::iso_from_millis(seconds * 1000.0)),
                limit_id: None,
                windows: Some(all_windows).filter(|windows| !windows.is_empty()),
            })
        }
        Some("result") => {
            let usage = object(parsed.get("usage")).unwrap_or(&empty);
            let output_details = object(usage.get("output_tokens_details")).unwrap_or(&empty);
            let failed = parsed.get("is_error") == Some(&Value::Bool(true))
                || text(&parsed, "subtype") != Some("success");
            let error = ["result", "terminal_reason", "subtype"]
                .into_iter()
                .map(|key| parsed.get(key))
                .find(|value| js::truthy(*value))
                .flatten()
                .map_or_else(|| "The turn failed".to_string(), js::string);
            ClaudeLine::Result(ResultLine {
                event_id: text(&parsed, "uuid").map(str::to_string),
                ok: !failed,
                error: failed.then_some(error),
                total_cost_usd: parsed.get("total_cost_usd").and_then(Value::as_f64),
                model: object(parsed.get("modelUsage"))
                    .and_then(|models| models.keys().next().cloned()),
                usage: TokenUsage {
                    input: number_or_zero(usage, "input_tokens")
                        + number_or_zero(usage, "cache_creation_input_tokens"),
                    cached_input: number_or_zero(usage, "cache_read_input_tokens"),
                    output: number_or_zero(usage, "output_tokens"),
                    reasoning: number_or_zero(output_details, "thinking_tokens"),
                },
            })
        }
        _ => ClaudeLine::Other,
    }
}

/// A one-line description of a tool call for the chat's activity row.
pub fn tool_summary(name: &str, input: &Map<String, Value>) -> Option<String> {
    activity_label(name, input, None)
}
