//! Port of `packages/daemon/src/providers/codex-parse.test.ts`, with the same recorded app-server lines.

use std::collections::BTreeMap;

use daemon::providers::codex::{CODEX_FEATURES_OFF, codex_config, codex_policy};
use daemon::providers::codex_parse::{map_codex_elicitation, map_codex_notification, map_codex_request};
use daemon::providers::codex_rpc::RpcId;
use daemon::providers::{Approval, Limit, LimitState, LimitWindow, McpServer, ProviderEvent, StartOptions, TokenUsage, TurnResult, Usage};
use protocol::model::Autonomy;
use serde_json::{Value, json};

/// `(method, params)`, or `(method, params, id)` for a request.
fn parse(line: &str) -> (String, Value, Value) {
    let value: Value = serde_json::from_str(line).unwrap();
    assert!(value["method"].is_string() && value["params"].is_object(), "Expected an RPC fixture");
    (value["method"].as_str().unwrap().into(), value["params"].clone(), value["id"].clone())
}

fn id(value: &Value) -> RpcId {
    RpcId::from_value(value).expect("an RPC id")
}

const MODEL: &str = "gpt-6-sol";

const DELTA: &str = // built from the schema, real ids
    r#"{"method":"item/agentMessage/delta","params":{"threadId":"01a0d7cb-a738-7121-bc46-cb16f5896e3c","turnId":"01a0d7cb-b0ed-7a02-bc8c-042988202fa1","itemId":"msg_061da235ca62de87016ab63848eab087d0862ec7c6a83415fe","delta":"I will run"}}"#;
const COMMAND_STARTED: &str = // trimmed: the PowerShell wrapper command, cwd and process fields removed
    r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974","source":"unifiedExecStartup","status":"inProgress","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath 'inside.txt' -Value 'hi' -NoNewline; Get-Content -LiteralPath 'inside.txt' -Raw"}],"aggregatedOutput":null,"exitCode":null,"durationMs":null},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","startedAtMs":1790327506815},"emittedAtMs":1790327506815}"#;
const COMMAND_COMPLETED: &str = // trimmed like COMMAND_STARTED
    r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974","status":"completed","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath 'inside.txt' -Value 'hi' -NoNewline; Get-Content -LiteralPath 'inside.txt' -Raw"}],"aggregatedOutput":"hi","exitCode":0,"durationMs":398},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","completedAtMs":1790327507214},"emittedAtMs":1790327507311}"#;
const AGENT_COMPLETED: &str = r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_04cbf98466447733016ab63af82afc87d092e78611119924a6","text":"1. `inside.txt`: wrote `hi` and read it back successfully.","phase":"final_answer","memoryCitation":null,"delivery":null,"questions":null},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5"}}"#;
const TOKEN_USAGE: &str = r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"01a0d7cb-a738-7121-bc46-cb16f5896e3c","turnId":"01a0d7cb-b0ed-7a02-bc8c-042988202fa1","tokenUsage":{"total":{"totalTokens":52111,"inputTokens":51830,"cachedInputTokens":25600,"cacheWriteInputTokens":0,"outputTokens":281,"reasoningOutputTokens":35},"last":{"totalTokens":26163,"inputTokens":26025,"cachedInputTokens":25600,"cacheWriteInputTokens":0,"outputTokens":138,"reasoningOutputTokens":9},"modelContextWindow":258400}},"emittedAtMs":1790326878417}"#;
const RATE_LIMITS: &str = r#"{"method":"account/rateLimits/updated","params":{"rateLimits":{"limitId":"codex","limitName":null,"normalModelSlug":null,"primary":{"usedPercent":25,"windowDurationMins":10080,"resetsAt":1790438506},"secondary":null,"credits":{"hasCredits":false,"unlimited":false,"balance":"0"},"individualLimit":null,"spendControlReached":null,"planType":"pro","rateLimitReachedType":null}},"emittedAtMs":1790326867549}"#;
const TURN_COMPLETED: &str = r#"{"method":"turn/completed","params":{"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turn":{"id":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","items":[],"itemsView":"summary","status":"completed","error":null,"startedAt":1790327484,"completedAt":1790327547,"durationMs":62249}},"emittedAtMs":1790327547033}"#;
const APPROVAL: &str = // trimmed: the PowerShell path in "command" shortened to powershell.exe
    r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","itemId":"exec-0d7b2fc4-81aa-4a06-98bf-ef0ccc6c31bf","startedAtMs":1790327515964,"environmentId":"local","reason":"Allow writing and reading outside-netconfig.txt in the parent folder as requested?","command":"powershell.exe -Command \"Set-Content -LiteralPath 'C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\codex\\outside-netconfig.txt' -Value 'hi' -NoNewline\"","cwd":"C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\codex\\netconfig","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath 'C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\codex\\outside-netconfig.txt' -Value 'hi' -NoNewline"}],"proposedExecpolicyAmendment":["Set-Content","-LiteralPath","C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\codex\\outside-netconfig.txt","-Value","hi","-NoNewline"],"availableDecisions":["accept","cancel"]}}"#;
const ELICITATION: &str = // the log cut the line off after "requestedSchema"; its end is as in the run's other requests
    r#"{"method":"mcpServer/elicitation/request","id":0,"params":{"threadId":"01a0d957-abb2-79b3-a19e-4293c113bea1","turnId":"01a0d957-adfa-7f20-8558-a1eb40e165f2","serverName":"browser","mode":"form","_meta":{"codex_approval_kind":"mcp_tool_call","persist":["session","always"],"tool_description":"Navigate to a URL","tool_params":{"url":"https://example.com"},"tool_params_display":[{"name":"url","value":"https://example.com","display_name":"url"}]},"message":"Allow the browser MCP server to run tool \"browser_navigate\"?","requestedSchema":{"type":"object","properties":{}}}}"#;

fn turn_limited(code: &str) -> Value {
    // built from the schema; "UsageLimitExceeded" is the spec spelling
    json!({ "threadId": "t", "turn": { "id": "u", "items": [], "status": "failed", "error": { "message": "You've hit your usage limit.", "codexErrorInfo": code } } })
}

fn notify(line: &str) -> Vec<ProviderEvent> {
    let (method, params, _) = parse(line);
    map_codex_notification(&method, &params, MODEL)
}

fn only_limit(events: Vec<ProviderEvent>) -> Limit {
    match <[ProviderEvent; 1]>::try_from(events) {
        Ok([ProviderEvent::Limit(limit)]) => limit,
        other => panic!("expected one limit, got {other:?}"),
    }
}

fn tool(id: &str, name: &str, summary: &str) -> ProviderEvent {
    ProviderEvent::Tool { id: id.into(), name: name.into(), summary: summary.into() }
}

// describe("mapCodexNotification")

#[test]
fn streams_agent_text_and_closes_it_when_the_message_completes() {
    assert_eq!(notify(DELTA), vec![ProviderEvent::Text { delta: "I will run".into() }]);
    assert_eq!(notify(AGENT_COMPLETED), vec![ProviderEvent::TextEnd]);
}

#[test]
fn reports_a_shell_command_as_a_tool_summarized_by_its_inner_command() {
    assert_eq!(
        notify(COMMAND_STARTED),
        vec![tool(
            "exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974",
            "Shell",
            "Set-Content -LiteralPath 'inside.txt' -Value 'hi' -NoNewline; Get-Content -LiteralPath 'inside.txt' -Raw"
        )]
    );
    assert_eq!(notify(COMMAND_COMPLETED), vec![ProviderEvent::ToolDone { id: "exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974".into(), ok: true }]);
}

#[test]
fn keeps_double_underscores_in_a_shell_command() {
    let command = "python -c 'print(__name__)'";
    let params = json!({ "item": { "type": "commandExecution", "id": "shell-1", "commandActions": [{ "command": command }] } });
    assert_eq!(map_codex_notification("item/started", &params, MODEL), vec![tool("shell-1", "Shell", command)]);
}

#[test]
fn gives_codex_mcp_tools_a_plain_activity_label() {
    for (server, tool_name, args, label) in [
        ("tjommie", "list_bots", json!({}), "Looked at the team"),
        ("tjommie", "message_bot", json!({ "bot": "Writer" }), "Messaged Writer"),
        ("tjommie", "share_file", json!({ "path": "output/notes.md" }), "Shared notes.md"),
        ("tjommie", "add_routine", json!({ "name": "Morning check" }), "Proposed a routine: Morning check"),
        ("browser", "browser_navigate", json!({ "url": "https://pnet.co.za/jobs" }), "Browser: opened pnet.co.za"),
        ("browser", "browser_click", json!({ "element": "Apply" }), "Browser: clicked Apply"),
        ("weather", "check_forecast", json!({}), "check forecast"),
    ] {
        let params = json!({ "item": { "type": "mcpToolCall", "id": "mcp-1", "server": server, "tool": tool_name, "arguments": args } });
        let events = map_codex_notification("item/started", &params, MODEL);
        assert_eq!(events, vec![tool("mcp-1", label, label)], "{server} {tool_name}");
        assert!(!label.contains("mcp__") && !label.contains("select:"));
    }
}

#[test]
fn normalizes_a_prefixed_codex_mcp_tool_name_and_hides_selector_calls() {
    let prefixed = json!({ "item": { "type": "mcpToolCall", "id": "mcp-2", "server": "tjommie", "tool": "mcp__tjommie__add_routine", "arguments": { "name": "Morning check" } } });
    assert_eq!(
        map_codex_notification("item/started", &prefixed, MODEL),
        vec![tool("mcp-2", "Proposed a routine: Morning check", "Proposed a routine: Morning check")]
    );
    let selector = json!({ "item": { "type": "mcpToolCall", "id": "mcp-3", "server": "tjommie", "tool": "ToolSearch", "arguments": { "query": "select:mcp__tjommie__add_routine" } } });
    assert_eq!(map_codex_notification("item/started", &selector, MODEL), vec![]);
}

#[test]
fn reports_each_model_calls_tokens_uncached_input_separately() {
    assert_eq!(
        notify(TOKEN_USAGE),
        vec![ProviderEvent::Usage(Usage {
            // The same id TypeScript computes for this line: the ids of saved usage stay stable across the rewrite.
            event_id: Some("2782856518d8eb4bf36b1b687a2f419ac5dd30749093d2d125eeab3074747c86".into()),
            model: MODEL.into(),
            tokens: TokenUsage { input: 26025.0 - 25600.0, cached_input: 25600.0, output: 138.0, reasoning: 9.0 },
            cost_usd: None,
            total_cost_usd: None,
        })]
    );
}

#[test]
fn reports_rate_limits_with_percent_and_reset_time() {
    let limit = only_limit(notify(RATE_LIMITS));
    assert_eq!(
        (limit.state, limit.used_percent, limit.resets_at.as_deref()),
        (LimitState::Ok, Some(25.0), Some("2026-09-26T16:01:46.000Z")) // new Date(1790438506 * 1000).toISOString()
    );
}

#[test]
fn reports_the_restrictive_quota_window_with_its_own_reset() {
    for (primary, secondary, state, used, reset) in [
        (json!({ "usedPercent": 25, "resetsAt": 100 }), json!({ "usedPercent": 100, "resetsAt": 200 }), LimitState::Reached, 100.0, "1970-01-01T00:03:20.000Z"),
        (json!({ "usedPercent": 96, "resetsAt": 100 }), json!({ "usedPercent": 20, "resetsAt": 200 }), LimitState::Near, 96.0, "1970-01-01T00:01:40.000Z"),
        (json!({ "usedPercent": 100, "resetsAt": 300 }), json!({ "usedPercent": 100, "resetsAt": 200 }), LimitState::Reached, 100.0, "1970-01-01T00:05:00.000Z"),
        (json!(null), json!({ "usedPercent": 94, "resetsAt": 200 }), LimitState::Near, 94.0, "1970-01-01T00:03:20.000Z"),
        (json!({ "usedPercent": 25, "resetsAt": 100 }), json!({ "usedPercent": 95, "resetsAt": 200 }), LimitState::Near, 95.0, "1970-01-01T00:03:20.000Z"),
    ] {
        let params = json!({ "rateLimits": { "primary": primary, "secondary": secondary } });
        let limit = only_limit(map_codex_notification("account/rateLimits/updated", &params, MODEL));
        assert_eq!((limit.state, limit.used_percent, limit.resets_at.as_deref()), (state, Some(used), Some(reset)));
    }
}

#[test]
fn keeps_account_snapshots_independent_and_handles_invalid_quota_values() {
    let limit = |rate_limits: Value| {
        let limit = only_limit(map_codex_notification("account/rateLimits/updated", &json!({ "rateLimits": rate_limits }), MODEL));
        (limit.state, limit.used_percent, limit.resets_at)
    };
    assert_eq!(
        limit(json!({ "secondary": { "usedPercent": 100, "resetsAt": 200 } })),
        (LimitState::Reached, Some(100.0), Some("1970-01-01T00:03:20.000Z".into()))
    );
    assert_eq!(
        limit(json!({ "primary": { "usedPercent": 10, "resetsAt": 100 } })),
        (LimitState::Ok, Some(10.0), Some("1970-01-01T00:01:40.000Z".into()))
    );
    // TypeScript passes NaN and Infinity, which JSON can't carry: they arrive as null.
    assert_eq!(
        limit(json!({ "primary": { "usedPercent": f64::NAN, "resetsAt": f64::INFINITY }, "secondary": { "usedPercent": "100", "resetsAt": 1e30 } })),
        (LimitState::Ok, None, None)
    );
}

#[test]
fn ends_turns_and_reports_either_usage_limit_spelling_as_a_reached_limit_first() {
    assert_eq!(notify(TURN_COMPLETED), vec![ProviderEvent::TurnDone(TurnResult { ok: true, error: None })]);
    for spelling in ["usageLimitExceeded", "UsageLimitExceeded"] {
        assert_eq!(
            map_codex_notification("turn/completed", &turn_limited(spelling), MODEL),
            vec![
                ProviderEvent::Limit(Limit { state: LimitState::Reached, used_percent: Some(100.0), resets_at: None, limit_id: None, windows: None }),
                ProviderEvent::TurnDone(TurnResult { ok: false, error: Some("You've hit your usage limit.".into()) }),
            ]
        );
    }
}

#[test]
fn ignores_notifications_tjommie_doesnt_use() {
    let params = json!({ "name": "codex_apps", "status": "ready" });
    assert_eq!(map_codex_notification("mcpServer/startupStatus/updated", &params, MODEL), vec![]);
}

// describe("mapCodexRequest")

#[test]
fn turns_a_command_approval_into_an_approval_card_with_a_reusable_rule() {
    let (method, params, request_id) = parse(APPROVAL);
    assert_eq!(
        map_codex_request(&id(&request_id), &method, &params),
        Some(Approval {
            id: "0".into(),
            tool: "Shell".into(),
            summary: "Allow writing and reading outside-netconfig.txt in the parent folder as requested?".into(),
            detail: "Set-Content -LiteralPath 'C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\codex\\outside-netconfig.txt' -Value 'hi' -NoNewline".into(),
            rule: Some(format!("codex:{}", serde_json::to_string(&params["proposedExecpolicyAmendment"]).unwrap())),
        })
    );
}

#[test]
fn turns_a_file_change_approval_into_a_card_without_a_rule_and_refuses_unknown_requests() {
    let file_approval = json!({ "reason": null, "grantRoot": "C:\\work", "itemId": "i", "threadId": "t", "turnId": "u", "startedAtMs": 1 }); // built from the schema
    assert_eq!(
        map_codex_request(&RpcId::from(4), "item/fileChange/requestApproval", &file_approval),
        Some(Approval { id: "4".into(), tool: "Edit".into(), summary: "Change files".into(), detail: "C:\\work".into(), rule: None })
    );
    assert_eq!(map_codex_request(&RpcId::from(5), "item/tool/requestUserInput", &json!({})), None);
}

#[test]
fn turns_the_question_before_an_mcp_tool_call_into_a_card_with_a_reusable_rule() {
    let (_, params, request_id) = parse(ELICITATION);
    assert_eq!(
        map_codex_elicitation(&id(&request_id), &params),
        Some(Approval {
            id: "0".into(),
            tool: "browser.browser_navigate".into(),
            summary: "Navigate to a URL".into(),
            detail: "{\n  \"url\": \"https://example.com\"\n}".into(), // JSON.stringify({ url }, null, 2)
            rule: Some("codex-mcp:browser.browser_navigate".into()),
        })
    );
}

#[test]
fn refuses_malformed_mcp_tool_call_elicitations() {
    let (_, params, _) = parse(ELICITATION);
    let with = |key: &str, value: Value| {
        let mut params = params.clone();
        params[key] = value;
        params
    };
    assert_eq!(map_codex_elicitation(&RpcId::from(1), &with("serverName", json!(3))), None);
    let bad_meta = json!({ "codex_approval_kind": "mcp_tool_call", "tool_description": "Navigate", "tool_params": "bad" });
    assert_eq!(map_codex_elicitation(&RpcId::from(2), &with("_meta", bad_meta)), None);
    assert_eq!(map_codex_elicitation(&RpcId::from(3), &with("message", json!("May I ask a question?"))), None);
}

// describe("codex policy and config")

fn options() -> StartOptions {
    StartOptions {
        cwd: "C:\\p".into(),
        extra_dirs: vec!["C:\\home".into()],
        model: String::new(),
        instructions: String::new(),
        autonomy: Autonomy::Folder,
        codex_sandbox_ready: false,
        allow_rules: vec![],
        resume_id: None,
        resume_cost_usd: None,
        env: BTreeMap::new(),
        api_key: None,
        max_budget_usd: None,
        mcp_servers: vec![],
    }
}

#[test]
fn maps_autonomy_to_approval_policy_and_sandbox() {
    assert_eq!(codex_policy(Autonomy::Ask), json!({ "approvalPolicy": "untrusted", "sandbox": "read-only" }));
    assert_eq!(codex_policy(Autonomy::Folder), json!({ "approvalPolicy": "on-request", "sandbox": "workspace-write" }));
    assert_eq!(codex_policy(Autonomy::Full), json!({ "approvalPolicy": "never", "sandbox": "danger-full-access" }));
}

#[test]
fn chooses_elevated_for_own_folder_elevated_for_ready_always_ask_and_leaves_full_auto_as_is() {
    let features = json!({
        "apps": false, "browser_use": false, "browser_use_external": false, "computer_use": false, "in_app_browser": false,
        "plugins": false, "remote_plugin": false, "memories": false, "tool_suggest": false, "skill_mcp_dependency_install": false,
    });
    let common = json!({ "sandbox_workspace_write": { "network_access": false, "writable_roots": ["C:\\home"] }, "features": features, "mcp_servers": {} });
    let windows = |sandbox: &str| {
        let mut config = common.clone();
        config["windows"] = json!({ "sandbox": sandbox });
        config
    };
    let with = |autonomy, codex_sandbox_ready| StartOptions { autonomy, codex_sandbox_ready, ..options() };
    assert_eq!(codex_config(&options(), true), windows("elevated"));
    assert_eq!(codex_config(&with(Autonomy::Ask, false), true), windows("unelevated"));
    assert_eq!(codex_config(&with(Autonomy::Ask, true), true), windows("elevated"));
    assert_eq!(codex_config(&with(Autonomy::Full, true), true), windows("unelevated"));
    assert_eq!(codex_config(&options(), false), common);
    let features_off: Value = CODEX_FEATURES_OFF.iter().map(|feature| (feature.to_string(), json!(false))).collect::<serde_json::Map<_, _>>().into();
    assert_eq!(features_off, features);
}

#[test]
fn attaches_tjommies_mcp_servers_to_the_thread() {
    let options = StartOptions {
        mcp_servers: vec![
            McpServer::Http {
                name: "tjommie".into(),
                url: "http://127.0.0.1:47100/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer t0k".into())]),
                auto_allow: true,
            },
            McpServer::Stdio {
                name: "browser".into(),
                command: "C:\\node.exe".into(),
                args: vec!["cli.js".into()],
                env: BTreeMap::from([("ELECTRON_RUN_AS_NODE".into(), "1".into())]),
                auto_allow: true,
            },
        ],
        ..options()
    };
    assert_eq!(
        codex_config(&options, true)["mcp_servers"],
        json!({
            "tjommie": { "url": "http://127.0.0.1:47100/mcp", "http_headers": { "Authorization": "Bearer t0k" } },
            "browser": { "command": "C:\\node.exe", "args": ["cli.js"], "env": { "ELECTRON_RUN_AS_NODE": "1" } },
        })
    );
}

#[test]
fn preserves_independent_codex_quota_windows_including_an_exhausted_unknown_reset() {
    let params = json!({ "rateLimits": { "limitId": "codex",
        "primary": { "usedPercent": 100, "windowDurationMins": 300, "resetsAt": null },
        "secondary": { "usedPercent": 92, "windowDurationMins": 10080, "resetsAt": 200 } } });
    let limit = only_limit(map_codex_notification("account/rateLimits/updated", &params, MODEL));
    assert_eq!((limit.state, limit.resets_at, limit.limit_id.as_deref()), (LimitState::Reached, None, Some("codex")));
    assert_eq!(
        limit.windows,
        Some(vec![
            LimitWindow { window_key: "primary".into(), used_percent: Some(100.0), duration_minutes: Some(300.0), resets_at: None, state: LimitState::Reached },
            LimitWindow {
                window_key: "secondary".into(),
                used_percent: Some(92.0),
                duration_minutes: Some(10080.0),
                resets_at: Some("1970-01-01T00:03:20.000Z".into()),
                state: LimitState::Near,
            },
        ])
    );
}
