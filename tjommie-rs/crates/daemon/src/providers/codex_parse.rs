//! Maps `codex app-server` notifications and requests to Tjommie's provider events.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::activity_labels::{activity_label, plain_activity_text};
use super::codex_rpc::RpcId;
use super::{Approval, Limit, LimitState, LimitWindow, ProviderEvent, TokenUsage, TurnResult, Usage, js};

const TOOL_ITEMS: [&str; 4] = ["commandExecution", "fileChange", "mcpToolCall", "webSearch"];
static TOOL_IN_MESSAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"tool "([^"]+)""#).unwrap());

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

/// The value of `key` unless it is null or missing (JavaScript's `??`).
fn present<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !value.is_null())
}

/// `(ms, ISO)` for a reset time in seconds.
fn to_iso(seconds: Option<&Value>) -> Option<(f64, String)> {
    let ms = (js::finite(seconds)? * 1000.0).trunc();
    Some((ms, js::iso_from_millis(ms)?))
}

/// `String(value)`, where a missing value is `undefined`.
fn string_or_undefined(value: Option<&Value>) -> String {
    value.map_or_else(|| "undefined".into(), js::string)
}

fn first_command(item: &Map<String, Value>) -> Option<&Value> {
    let first = item.get("commandActions").and_then(Value::as_array).and_then(|actions| actions.first()?.as_object());
    first.and_then(|first| present(first, "command")).or_else(|| item.get("command"))
}

fn limit_state(percent: Option<f64>) -> LimitState {
    match percent {
        Some(percent) if percent >= 100.0 => LimitState::Reached,
        Some(percent) if percent >= 90.0 => LimitState::Near,
        _ => LimitState::Ok,
    }
}

fn tool(id: &str, name: &str, summary: String) -> ProviderEvent {
    ProviderEvent::Tool { id: id.into(), name: name.into(), summary }
}

fn tool_started(value: Option<&Value>) -> Vec<ProviderEvent> {
    let Some(item) = value.and_then(Value::as_object) else { return vec![] };
    let (Some(item_type), Some(id)) = (text(item, "type"), text(item, "id")) else { return vec![] };

    match item_type {
        "commandExecution" => vec![tool(id, "Shell", plain_activity_text(&string_or_undefined(first_command(item))))],
        "fileChange" => {
            let changes = item.get("changes").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
            let paths: Vec<String> = changes
                .iter()
                .map(|change| change.as_object().and_then(|change| present(change, "path")).map_or_else(String::new, js::string))
                .collect();
            let summary = plain_activity_text(js::slice(&paths.join(", "), 200));
            vec![tool(id, "Edit", if summary.is_empty() { "Change files".into() } else { summary })]
        }
        "mcpToolCall" => {
            let empty = Map::new();
            let arguments = item.get("arguments").and_then(Value::as_object).unwrap_or(&empty);
            let (Some(tool_name), Some(server)) = (text(item, "tool"), text(item, "server")) else { return vec![] };
            match activity_label(tool_name, arguments, Some(server)).filter(|label| !label.is_empty()) {
                Some(label) => vec![tool(id, &label, label.clone())],
                None => vec![],
            }
        }
        "webSearch" => {
            let query = present(item, "query").map_or_else(|| "Search the web".into(), js::string);
            vec![tool(id, "Web search", plain_activity_text(&query))]
        }
        _ => vec![],
    }
}

/// Maps one app-server notification to Tjommie's provider events. `model` is the thread's model.
pub fn map_codex_notification(method: &str, params: &Value, model: &str) -> Vec<ProviderEvent> {
    let Some(params) = params.as_object() else { return vec![] };

    match method {
        "item/agentMessage/delta" => text(params, "delta").map(|delta| ProviderEvent::Text { delta: delta.into() }).into_iter().collect(),
        "item/started" => tool_started(params.get("item")),
        "item/completed" => {
            let Some(item) = params.get("item").and_then(Value::as_object) else { return vec![] };
            match (text(item, "type"), text(item, "id")) {
                (Some("agentMessage"), _) => vec![ProviderEvent::TextEnd],
                // Web searches carry no status; they only complete when they worked.
                (Some(item_type), Some(id)) if TOOL_ITEMS.contains(&item_type) => {
                    let ok = item.get("status").is_none() || text(item, "status") == Some("completed");
                    vec![ProviderEvent::ToolDone { id: id.into(), ok }]
                }
                _ => vec![],
            }
        }
        "thread/tokenUsage/updated" => {
            let Some(token_usage) = params.get("tokenUsage").filter(|usage| usage.is_object()) else { return vec![] };
            let Some(last) = token_usage.get("last").and_then(Value::as_object) else { return vec![] };
            let number = |key| last.get(key).and_then(Value::as_f64);
            let (Some(input), Some(cached), Some(output), Some(reasoning)) =
                (number("inputTokens"), number("cachedInputTokens"), number("outputTokens"), number("reasoningOutputTokens"))
            else {
                return vec![];
            };
            // JSON.stringify([threadId, turnId, tokenUsage]): serde_json writes the same text, in the same key order.
            let identity = json!([params.get("threadId"), params.get("turnId"), token_usage]);
            let digest = Sha256::digest(serde_json::to_string(&identity).expect("JSON values serialize"));
            vec![ProviderEvent::Usage(Usage {
                event_id: Some(digest.iter().map(|byte| format!("{byte:02x}")).collect()),
                model: model.into(),
                tokens: TokenUsage { input: input - cached, cached_input: cached, output, reasoning },
                cost_usd: None, // Codex reports no cost; Phase 2 prices tokens from a price list
                total_cost_usd: None,
            })]
        }
        "account/rateLimits/updated" => {
            let Some(limits) = params.get("rateLimits").and_then(Value::as_object) else { return vec![] };
            let mut detailed: Vec<LimitWindow> = ["primary", "secondary"]
                .into_iter()
                .filter_map(|window_key| {
                    let value = limits.get(window_key)?.as_object()?;
                    let used_percent = js::finite(value.get("usedPercent")).map(|used| used.clamp(0.0, 100.0));
                    Some(LimitWindow {
                        window_key: window_key.into(),
                        duration_minutes: js::finite(value.get("windowDurationMins")).filter(|minutes| *minutes > 0.0),
                        used_percent,
                        resets_at: to_iso(value.get("resetsAt")).map(|(_, iso)| iso),
                        state: limit_state(used_percent),
                    })
                })
                .collect();
            let reached_type = js::truthy(limits.get("rateLimitReachedType"));
            if reached_type && !detailed.iter().any(|window| window.state == LimitState::Reached) {
                detailed.push(LimitWindow {
                    window_key: "provider".into(),
                    duration_minutes: None,
                    used_percent: None,
                    resets_at: None,
                    state: LimitState::Reached,
                });
            }
            // Keep the single-window provider contract: percentages and resets always describe the same
            // window. If both are exhausted, work cannot resume until the later reset.
            let mut windows: Vec<(Option<f64>, Option<(f64, String)>)> = [limits.get("primary"), limits.get("secondary")]
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .map(|window| (js::finite(window.get("usedPercent")), to_iso(window.get("resetsAt"))))
                .collect();
            let exhausted = |used: Option<f64>| used.is_some_and(|used| used >= 100.0);
            windows.sort_by(|(a_used, a_reset), (b_used, b_reset)| {
                if exhausted(*a_used) && exhausted(*b_used) {
                    // An unknown reset for an exhausted window must not borrow another window's timestamp.
                    return match (a_reset, b_reset) {
                        (None, None) => Ordering::Equal,
                        (None, _) => Ordering::Less,
                        (_, None) => Ordering::Greater,
                        (Some((a, _)), Some((b, _))) => b.total_cmp(a),
                    };
                }
                b_used.unwrap_or(-1.0).total_cmp(&a_used.unwrap_or(-1.0))
            });
            let (used, reset) = windows.into_iter().next().unwrap_or((None, None));
            let limit_id = text(limits, "limitId").filter(|id| (1..=100).contains(&js::utf16_len(id))).unwrap_or("codex");
            vec![ProviderEvent::Limit(Limit {
                state: if reached_type { LimitState::Reached } else { limit_state(used) },
                used_percent: used,
                resets_at: reset.map(|(_, iso)| iso),
                limit_id: Some(limit_id.into()),
                windows: Some(detailed),
            })]
        }
        "turn/completed" => {
            let Some(turn) = params.get("turn").and_then(Value::as_object) else { return vec![] };
            let error = turn.get("error").and_then(Value::as_object);
            let error_info = error.and_then(|error| text(error, "codexErrorInfo"));
            let limited = matches!(error_info, Some("usageLimitExceeded" | "UsageLimitExceeded"));
            let ok = text(turn, "status") == Some("completed");
            let message = error
                .and_then(|error| text(error, "message"))
                .map_or_else(|| format!("The turn ended: {}", string_or_undefined(turn.get("status"))), str::to_string);
            let limit = limited.then(|| {
                ProviderEvent::Limit(Limit { state: LimitState::Reached, used_percent: Some(100.0), resets_at: None, limit_id: None, windows: None })
            });
            limit.into_iter().chain([ProviderEvent::TurnDone(TurnResult { ok, error: (!ok).then_some(message) })]).collect()
        }
        _ => vec![],
    }
}

/// Maps an approval request from the app-server. The session refuses anything this returns `None` for.
pub fn map_codex_request(id: &RpcId, method: &str, params: &Value) -> Option<Approval> {
    let params = params.as_object()?;

    match method {
        "item/commandExecution/requestApproval" => {
            let command = first_command(params).filter(|command| !command.is_null()).map_or_else(|| "a command".into(), js::string);
            let proposal = params.get("proposedExecpolicyAmendment").filter(|proposal| proposal.is_array());
            Some(Approval {
                id: id.to_string(),
                tool: "Shell".into(),
                summary: text(params, "reason").map_or_else(|| format!("Run {command}"), str::to_string),
                detail: command,
                rule: proposal.map(|proposal| format!("codex:{}", serde_json::to_string(proposal).expect("JSON values serialize"))),
            })
        }
        "item/fileChange/requestApproval" => Some(Approval {
            id: id.to_string(),
            tool: "Edit".into(),
            summary: text(params, "reason").unwrap_or("Change files").into(),
            detail: text(params, "grantRoot").unwrap_or_default().into(),
            rule: None,
        }),
        _ => None,
    }
}

/// The fields of an MCP tool-call question (`isCodexMcpToolCall` in TypeScript).
#[derive(Debug, Clone, PartialEq)]
pub struct CodexMcpToolCall {
    pub server_name: String,
    pub message: String,
    pub tool_description: Option<String>,
    pub tool_params: Option<Map<String, Value>>,
}

/// Validates the tool-call fields independently of the wording used to build an approval card.
pub fn parse_codex_mcp_tool_call(params: &Value) -> Option<CodexMcpToolCall> {
    let params = params.as_object()?;
    let server_name = text(params, "serverName").filter(|name| !name.is_empty())?;
    let message = text(params, "message").filter(|_| text(params, "mode") == Some("form"))?;
    let meta = params.get("_meta")?.as_object().filter(|meta| text(meta, "codex_approval_kind") == Some("mcp_tool_call"))?;
    let tool_description = match meta.get("tool_description") {
        None => None,
        Some(description) => Some(description.as_str()?.to_string()),
    };
    let tool_params = match meta.get("tool_params") {
        None => None,
        Some(tool_params) => Some(tool_params.as_object()?.clone()),
    };
    Some(CodexMcpToolCall { server_name: server_name.into(), message: message.into(), tool_description, tool_params })
}

/// Codex asks before every MCP tool call. For a server that isn't allowed outright this becomes an approval
/// card. The tool's name only appears in the message text.
pub fn map_codex_elicitation(id: &RpcId, params: &Value) -> Option<Approval> {
    let call = parse_codex_mcp_tool_call(params)?;
    let tool = TOOL_IN_MESSAGE.captures(&call.message)?.get(1)?.as_str();
    let tool_params = Value::Object(call.tool_params.unwrap_or_default());
    Some(Approval {
        id: id.to_string(),
        tool: format!("{}.{tool}", call.server_name),
        summary: call.tool_description.unwrap_or_else(|| format!("Use {tool}")),
        detail: js::slice(&js::pretty(&tool_params), 4000).into(),
        rule: Some(format!("codex-mcp:{}.{tool}", call.server_name)),
    })
}
