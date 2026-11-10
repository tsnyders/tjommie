//! Port of `packages/daemon/src/providers/claude-parse.test.ts`, with the same recorded stream-json lines.

use daemon::providers::claude_parse::{
    ClaudeLine, ResultLine, ToolUse, parse_claude_line, tool_summary,
};
use daemon::providers::{LimitState, LimitWindow, TokenUsage};
use serde_json::{Map, Value, json};

const INIT: &str = // trimmed: tools list shortened
    r#"{"type":"system","subtype":"init","cwd":"C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\claude-steer","session_id":"f472038e-3221-4314-af40-2fbc326b6209","tools":["Bash","Read","Write"],"mcp_servers":[],"model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none"}"#;
const TEXT_DELTA: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The"}},"session_id":"f472038e-3221-4314-af40-2fbc326b6209","parent_tool_use_id":null,"uuid":"39c0c5e1-590b-4623-9a36-397fccd50230"}"#;
const THINKING_DELTA: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":null}},"session_id":"f472038e-3221-4314-af40-2fbc326b6209","parent_tool_use_id":null,"uuid":"5253cd2b-b1fd-4c18-afb2-69429037b323"}"#;
const ASSISTANT_TOOL: &str = // trimmed: usage removed
    r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011CfPso6TEaKSahfBEtfH1m","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_0157kDBqT6btcG5vksNWWWi7","name":"Bash","input":{"command":"sleep 8 && echo first","description":"Sleep for 8 seconds then print \"first\""},"caller":{"type":"direct"}}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}"#;
const ASSISTANT_TEXT: &str = // trimmed: usage removed
    r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011CfPspSLbBqh1Mi18BxVgT","type":"message","role":"assistant","content":[{"type":"text","text":"The command printed \"first\" after the 8-second delay. BANANA"}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}"#;
const TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_0157kDBqT6btcG5vksNWWWi7","type":"tool_result","content":"first","is_error":false}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}"#;
const TOOL_DENIED: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"The user denied this.","is_error":true,"tool_use_id":"toolu_01UKHn8SRaUHCyDtvGNiZCEX"}]},"parent_tool_use_id":null,"session_id":"d2b2b825-2396-4a10-a648-27605e94cc1d"}"#;
const ASK_BASH: &str = // trimmed: one of three suggestions removed
    r#"{"type":"control_request","request_id":"8e4e2307-ddb2-47b6-a3df-2c08568d70bc","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"mkdir made-by-bash","description":"Create directory named made-by-bash"},"description":"Create directory named made-by-bash","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"mkdir made-by-bash *"}],"behavior":"allow","destination":"localSettings"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"blocked_path":"C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\claude-approve\\made-by-bash","tool_use_id":"toolu_01UKHn8SRaUHCyDtvGNiZCEX"}}"#;
const ASK_WRITE: &str = r#"{"type":"control_request","request_id":"b3716c47-c14d-4ae1-a6d5-06f95632615b","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\claude-approve\\hello.txt","content":"hi"},"description":"hello.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_01QMRGxpAmYkosuW5kse1e2w"}}"#;
const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1790341200,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.02,"resetsAt":1790341200},"seven_day":{"utilization":0.07,"resetsAt":1790802000}}},"uuid":"b883d966-1eba-4099-b9b6-229317053327","session_id":"f472038e-3221-4314-af40-2fbc326b6209"}"#;
const RESULT: &str = // trimmed: iterations and subagent stats removed
    r#"{"type":"result","subtype":"success","is_error":false,"result":"The command printed \"first\" after the 8-second delay. BANANA","session_id":"f472038e-3221-4314-af40-2fbc326b6209","total_cost_usd":0.0337045,"usage":{"input_tokens":18,"cache_creation_input_tokens":13868,"cache_read_input_tokens":48655,"output_tokens":217,"output_tokens_details":{"thinking_tokens":107}},"modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":18,"outputTokens":217,"cacheReadInputTokens":48655,"cacheCreationInputTokens":13868,"costUSD":0.0337045}},"terminal_reason":"completed","num_turns":2}"#;
const RESULT_STOPPED: &str = // trimmed
    r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"","stop_reason":"tool_use","terminal_reason":"aborted_tools","session_id":"59b5f08f-5b89-444d-9fed-75dce5ec727a","total_cost_usd":0.0181357,"usage":{"input_tokens":18,"cache_creation_input_tokens":5839,"cache_read_input_tokens":48647,"output_tokens":315}}"#;
const CONTROL_RESPONSE: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"int-1","response":{"still_queued":[]}}}"#;

/// Built from the real line, as in TypeScript.
fn rate_limit_rejected() -> String {
    RATE_LIMIT
        .replacen(r#""status":"allowed""#, r#""status":"rejected""#, 1)
        .replacen(r#""utilization":0.02"#, r#""utilization":1"#, 1)
}

fn obj(value: Value) -> Map<String, Value> {
    value.as_object().expect("an object").clone()
}

// describe("parseClaudeLine")

#[test]
fn reads_the_session_id_from_each_turns_init_line() {
    assert_eq!(
        parse_claude_line(INIT),
        ClaudeLine::Init {
            session_id: "f472038e-3221-4314-af40-2fbc326b6209".into()
        }
    );
}

#[test]
fn streams_text_deltas_and_ignores_thinking() {
    assert_eq!(
        parse_claude_line(TEXT_DELTA),
        ClaudeLine::TextDelta { text: "The".into() }
    );
    assert_eq!(parse_claude_line(THINKING_DELTA), ClaudeLine::Other);
}

#[test]
fn splits_assistant_lines_into_text_blocks_and_tool_uses() {
    assert_eq!(
        parse_claude_line(ASSISTANT_TOOL),
        ClaudeLine::Assistant {
            texts: vec![],
            tools: vec![ToolUse {
                id: "toolu_0157kDBqT6btcG5vksNWWWi7".into(),
                name: "Bash".into(),
                input: obj(
                    json!({ "command": "sleep 8 && echo first", "description": "Sleep for 8 seconds then print \"first\"" })
                ),
            }],
        }
    );
    assert_eq!(
        parse_claude_line(ASSISTANT_TEXT),
        ClaudeLine::Assistant {
            texts: vec!["The command printed \"first\" after the 8-second delay. BANANA".into()],
            tools: vec![]
        }
    );
}

#[test]
fn ignores_lines_from_subagents() {
    let line = ASSISTANT_TEXT.replacen(
        r#""parent_tool_use_id":null"#,
        r#""parent_tool_use_id":"toolu_x""#,
        1,
    );
    assert_eq!(parse_claude_line(&line), ClaudeLine::Other);
}

#[test]
fn reports_tool_results_including_denied_ones() {
    let result = |id: &str, ok| ClaudeLine::ToolResults {
        results: vec![(id.into(), ok)],
    };
    assert_eq!(
        parse_claude_line(TOOL_RESULT),
        result("toolu_0157kDBqT6btcG5vksNWWWi7", true)
    );
    assert_eq!(
        parse_claude_line(TOOL_DENIED),
        result("toolu_01UKHn8SRaUHCyDtvGNiZCEX", false)
    );
}

#[test]
fn turns_a_permission_request_into_an_approval_with_a_reusable_rule() {
    let ClaudeLine::Permission(bash) = parse_claude_line(ASK_BASH) else {
        panic!("expected a permission")
    };
    assert_eq!(bash.request_id, "8e4e2307-ddb2-47b6-a3df-2c08568d70bc");
    assert_eq!(bash.tool_name, "Bash");
    assert_eq!(bash.description, "Create directory named made-by-bash");
    assert_eq!(bash.rule, "Bash(mkdir made-by-bash *)");

    let ClaudeLine::Permission(write) = parse_claude_line(ASK_WRITE) else {
        panic!("expected a permission")
    };
    assert_eq!(
        (
            write.tool_name.as_str(),
            write.description.as_str(),
            write.rule.as_str()
        ),
        ("Write", "hello.txt", "Write")
    );
    assert_eq!(write.suggestion, None);
}

#[test]
fn reads_rate_limits_state_percent_of_the_current_window_and_reset_time() {
    let ClaudeLine::RateLimit(limit) = parse_claude_line(RATE_LIMIT) else {
        panic!("expected a rate limit")
    };
    assert_eq!(limit.state, LimitState::Ok);
    assert_eq!(limit.used_percent, Some(2.0));
    assert_eq!(limit.resets_at.as_deref(), Some("2026-09-25T13:00:00.000Z")); // new Date(1790341200 * 1000).toISOString()

    let ClaudeLine::RateLimit(rejected) = parse_claude_line(&rate_limit_rejected()) else {
        panic!("expected a rate limit")
    };
    assert_eq!(
        (rejected.state, rejected.used_percent),
        (LimitState::Reached, Some(100.0))
    );
}

#[test]
fn reads_a_finished_turns_per_turn_usage_and_cumulative_cost() {
    assert_eq!(
        parse_claude_line(RESULT),
        ClaudeLine::Result(ResultLine {
            event_id: None,
            ok: true,
            error: None,
            total_cost_usd: Some(0.0337045),
            model: Some("claude-haiku-4-5-20251001".into()),
            usage: TokenUsage {
                input: 18.0 + 13868.0,
                cached_input: 48655.0,
                output: 217.0,
                reasoning: 107.0
            },
        })
    );
}

#[test]
fn reads_a_stopped_turn_as_failed_with_its_terminal_reason() {
    let ClaudeLine::Result(stopped) = parse_claude_line(RESULT_STOPPED) else {
        panic!("expected a result")
    };
    assert!(!stopped.ok);
    assert_eq!(stopped.error.as_deref(), Some("aborted_tools"));
    assert_eq!(stopped.total_cost_usd, Some(0.0181357));
}

#[test]
fn ignores_control_responses_and_non_json_output() {
    assert_eq!(parse_claude_line(CONTROL_RESPONSE), ClaudeLine::Other);
    assert_eq!(parse_claude_line("Warning: something"), ClaudeLine::Other);
}

#[test]
fn ignores_valid_json_that_is_not_an_object() {
    assert_eq!(parse_claude_line("null"), ClaudeLine::Other);
    assert_eq!(parse_claude_line("42"), ClaudeLine::Other);
}

// describe("toolSummary")

fn summary(name: &str, input: Value) -> Option<String> {
    tool_summary(name, &obj(input))
}

#[test]
fn prefers_the_tools_own_description_then_its_main_argument_then_its_name() {
    assert_eq!(
        summary(
            "Bash",
            json!({ "command": "npm test", "description": "Run the tests" })
        )
        .as_deref(),
        Some("Run the tests")
    );
    assert_eq!(
        summary("Read", json!({ "file_path": "C:\\bots\\notes.md" })).as_deref(),
        Some("C:\\bots\\notes.md")
    );
    assert_eq!(
        summary("TodoWrite", json!({ "todos": [] })).as_deref(),
        Some("TodoWrite")
    );
}

#[test]
fn omits_claudes_tool_loading_steps_while_keeping_other_searches_visible() {
    assert_eq!(
        summary(
            "ToolSearch",
            json!({ "query": "select:mcp__tjommie__add_routine" })
        ),
        None
    );
    assert_eq!(
        summary("ToolSearch", json!({ "query": "other tools" })),
        None
    );
    assert_eq!(
        summary("WebSearch", json!({ "query": "select: best laptops" })).as_deref(),
        Some("best laptops")
    );
}

#[test]
fn keeps_double_underscores_in_ordinary_read_paths() {
    assert_eq!(
        summary("Read", json!({ "file_path": "src/__tests__/chat.test.ts" })).as_deref(),
        Some("src/__tests__/chat.test.ts")
    );
    assert_eq!(
        summary("Read", json!({ "file_path": "C:\\proj\\pkg\\__init__.py" })).as_deref(),
        Some("C:\\proj\\pkg\\__init__.py")
    );
    assert_eq!(
        summary(
            "Read",
            json!({ "description": "Read src/__tests__/chat.test.ts" })
        )
        .as_deref(),
        Some("Read src/__tests__/chat.test.ts")
    );
    assert_eq!(
        summary(
            "Read",
            json!({ "description": "Read C:\\proj\\pkg\\__init__.py" })
        )
        .as_deref(),
        Some("Read C:\\proj\\pkg\\__init__.py")
    );
    assert_eq!(
        summary("Read", json!({ "description": "Read mcp__ notes" })).as_deref(),
        Some("Read notes")
    );
}

#[test]
fn gives_each_tjommie_tool_a_plain_activity_label_from_its_input() {
    assert_eq!(
        summary("mcp__tjommie__list_bots", json!({})).as_deref(),
        Some("Looked at the team")
    );
    assert_eq!(
        summary(
            "mcp__tjommie__message_bot",
            json!({ "bot": "Writer", "message": "Hello" })
        )
        .as_deref(),
        Some("Messaged Writer")
    );
    assert_eq!(
        summary(
            "mcp__tjommie__share_file",
            json!({ "path": "output\\notes.md" })
        )
        .as_deref(),
        Some("Shared notes.md")
    );
    assert_eq!(
        summary(
            "mcp__tjommie__add_routine",
            json!({ "name": "Morning check" })
        )
        .as_deref(),
        Some("Proposed a routine: Morning check")
    );
}

#[test]
fn describes_browser_actions_and_other_mcp_tools_without_protocol_names() {
    assert_eq!(
        summary(
            "mcp__browser__browser_navigate",
            json!({ "url": "https://pnet.co.za/jobs" })
        )
        .as_deref(),
        Some("Browser: opened pnet.co.za")
    );
    assert_eq!(
        summary("mcp__browser__browser_click", json!({ "element": "Apply" })).as_deref(),
        Some("Browser: clicked Apply")
    );
    assert_eq!(
        summary("mcp__weather__check_forecast", json!({})).as_deref(),
        Some("check forecast")
    );
}

#[test]
fn does_not_leak_protocol_markers_through_activity_inputs() {
    assert_eq!(
        summary(
            "mcp__tjommie__message_bot",
            json!({ "bot": "mcp__my_server__Writer" })
        )
        .as_deref(),
        Some("Messaged Writer")
    );
    for label in [
        summary(
            "mcp__tjommie__message_bot",
            json!({ "bot": "select:mcp__team__bot" }),
        ),
        summary(
            "mcp__tjommie__message_bot",
            json!({ "bot": "mcp__my_server__Writer" }),
        ),
        summary(
            "mcp__tjommie__add_routine",
            json!({ "name": "select:mcp__news__brief" }),
        ),
        summary(
            "Bash",
            json!({ "description": "select:mcp__browser__browser_navigate" }),
        ),
    ] {
        let label = label.expect("a label");
        assert!(
            !label.contains("mcp__") && !label.contains("select:"),
            "{label}"
        );
    }
}

#[test]
fn preserves_both_claude_quota_windows_and_stable_result_identities() {
    let ClaudeLine::RateLimit(limit) = parse_claude_line(&rate_limit_rejected()) else {
        panic!("expected a rate limit")
    };
    let windows: Vec<(String, Option<f64>, Option<f64>, LimitState)> = limit
        .windows
        .expect("windows")
        .into_iter()
        .map(
            |LimitWindow {
                 window_key,
                 duration_minutes,
                 used_percent,
                 state,
                 ..
             }| (window_key, duration_minutes, used_percent, state),
        )
        .collect();
    assert_eq!(
        windows,
        vec![
            (
                "five_hour".into(),
                Some(300.0),
                Some(100.0),
                LimitState::Reached
            ),
            ("seven_day".into(), Some(10080.0), Some(7.0), LimitState::Ok),
        ]
    );
    let line = json!({ "type": "result", "uuid": "result-one", "subtype": "success" }).to_string();
    let ClaudeLine::Result(result) = parse_claude_line(&line) else {
        panic!("expected a result")
    };
    assert_eq!(result.event_id.as_deref(), Some("result-one"));
}
