import { describe, expect, it } from "vitest";
import { parseClaudeLine, toolSummary } from "./claude-parse.ts";

const INIT = // trimmed: tools list shortened
  '{"type":"system","subtype":"init","cwd":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\claude-steer","session_id":"f472038e-3221-4314-af40-2fbc326b6209","tools":["Bash","Read","Write"],"mcp_servers":[],"model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none"}';
const TEXT_DELTA =
  '{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The"}},"session_id":"f472038e-3221-4314-af40-2fbc326b6209","parent_tool_use_id":null,"uuid":"39c0c5e1-590b-4623-9a36-397fccd50230"}';
const THINKING_DELTA =
  '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":null}},"session_id":"f472038e-3221-4314-af40-2fbc326b6209","parent_tool_use_id":null,"uuid":"5253cd2b-b1fd-4c18-afb2-69429037b323"}';
const ASSISTANT_TOOL = // trimmed: usage removed
  '{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011CfPso6TEaKSahfBEtfH1m","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_0157kDBqT6btcG5vksNWWWi7","name":"Bash","input":{"command":"sleep 8 && echo first","description":"Sleep for 8 seconds then print \\"first\\""},"caller":{"type":"direct"}}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const ASSISTANT_TEXT = // trimmed: usage removed
  '{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011CfPspSLbBqh1Mi18BxVgT","type":"message","role":"assistant","content":[{"type":"text","text":"The command printed \\"first\\" after the 8-second delay. BANANA"}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const TOOL_RESULT =
  '{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_0157kDBqT6btcG5vksNWWWi7","type":"tool_result","content":"first","is_error":false}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const TOOL_DENIED =
  '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"The user denied this.","is_error":true,"tool_use_id":"toolu_01UKHn8SRaUHCyDtvGNiZCEX"}]},"parent_tool_use_id":null,"session_id":"d2b2b825-2396-4a10-a648-27605e94cc1d"}';
const ASK_BASH = // trimmed: one of three suggestions removed
  '{"type":"control_request","request_id":"8e4e2307-ddb2-47b6-a3df-2c08568d70bc","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"mkdir made-by-bash","description":"Create directory named made-by-bash"},"description":"Create directory named made-by-bash","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"mkdir made-by-bash *"}],"behavior":"allow","destination":"localSettings"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"blocked_path":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\claude-approve\\\\made-by-bash","tool_use_id":"toolu_01UKHn8SRaUHCyDtvGNiZCEX"}}';
const ASK_WRITE =
  '{"type":"control_request","request_id":"b3716c47-c14d-4ae1-a6d5-06f95632615b","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\claude-approve\\\\hello.txt","content":"hi"},"description":"hello.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_01QMRGxpAmYkosuW5kse1e2w"}}';
const RATE_LIMIT =
  '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1790341200,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.02,"resetsAt":1790341200},"seven_day":{"utilization":0.07,"resetsAt":1790802000}}},"uuid":"b883d966-1eba-4099-b9b6-229317053327","session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const RATE_LIMIT_REJECTED = RATE_LIMIT.replace('"status":"allowed"', '"status":"rejected"').replace('"utilization":0.02', '"utilization":1'); // built from the real line
const RESULT = // trimmed: iterations and subagent stats removed
  '{"type":"result","subtype":"success","is_error":false,"result":"The command printed \\"first\\" after the 8-second delay. BANANA","session_id":"f472038e-3221-4314-af40-2fbc326b6209","total_cost_usd":0.0337045,"usage":{"input_tokens":18,"cache_creation_input_tokens":13868,"cache_read_input_tokens":48655,"output_tokens":217,"output_tokens_details":{"thinking_tokens":107}},"modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":18,"outputTokens":217,"cacheReadInputTokens":48655,"cacheCreationInputTokens":13868,"costUSD":0.0337045}},"terminal_reason":"completed","num_turns":2}';
const RESULT_STOPPED = // trimmed
  '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"","stop_reason":"tool_use","terminal_reason":"aborted_tools","session_id":"59b5f08f-5b89-444d-9fed-75dce5ec727a","total_cost_usd":0.0181357,"usage":{"input_tokens":18,"cache_creation_input_tokens":5839,"cache_read_input_tokens":48647,"output_tokens":315}}';
const CONTROL_RESPONSE = '{"type":"control_response","response":{"subtype":"success","request_id":"int-1","response":{"still_queued":[]}}}';

describe("parseClaudeLine", () => {
  it("reads the session id from each turn's init line", () => {
    expect(parseClaudeLine(INIT)).toEqual({ kind: "init", sessionId: "f472038e-3221-4314-af40-2fbc326b6209" });
  });

  it("streams text deltas and ignores thinking", () => {
    expect(parseClaudeLine(TEXT_DELTA)).toEqual({ kind: "text_delta", text: "The" });
    expect(parseClaudeLine(THINKING_DELTA)).toEqual({ kind: "other" });
  });

  it("splits assistant lines into text blocks and tool uses", () => {
    expect(parseClaudeLine(ASSISTANT_TOOL)).toEqual({
      kind: "assistant",
      texts: [],
      tools: [{ id: "toolu_0157kDBqT6btcG5vksNWWWi7", name: "Bash", input: { command: "sleep 8 && echo first", description: 'Sleep for 8 seconds then print "first"' } }],
    });
    expect(parseClaudeLine(ASSISTANT_TEXT)).toMatchObject({ kind: "assistant", texts: ['The command printed "first" after the 8-second delay. BANANA'], tools: [] });
  });

  it("ignores lines from subagents", () => {
    expect(parseClaudeLine(ASSISTANT_TEXT.replace('"parent_tool_use_id":null', '"parent_tool_use_id":"toolu_x"'))).toEqual({ kind: "other" });
  });

  it("reports tool results, including denied ones", () => {
    expect(parseClaudeLine(TOOL_RESULT)).toEqual({ kind: "tool_results", results: [{ id: "toolu_0157kDBqT6btcG5vksNWWWi7", ok: true }] });
    expect(parseClaudeLine(TOOL_DENIED)).toEqual({ kind: "tool_results", results: [{ id: "toolu_01UKHn8SRaUHCyDtvGNiZCEX", ok: false }] });
  });

  it("turns a permission request into an approval with a reusable rule", () => {
    expect(parseClaudeLine(ASK_BASH)).toMatchObject({
      kind: "permission",
      requestId: "8e4e2307-ddb2-47b6-a3df-2c08568d70bc",
      toolName: "Bash",
      description: "Create directory named made-by-bash",
      rule: "Bash(mkdir made-by-bash *)",
    });
    expect(parseClaudeLine(ASK_WRITE)).toMatchObject({ kind: "permission", toolName: "Write", description: "hello.txt", rule: "Write", suggestion: null });
  });

  it("reads rate limits: state, percent of the current window and reset time", () => {
    expect(parseClaudeLine(RATE_LIMIT)).toMatchObject({ kind: "rate_limit", state: "ok", usedPercent: 2, resetsAt: new Date(1790341200 * 1000).toISOString() });
    expect(parseClaudeLine(RATE_LIMIT_REJECTED)).toMatchObject({ kind: "rate_limit", state: "reached", usedPercent: 100 });
  });

  it("reads a finished turn's per-turn usage and cumulative cost", () => {
    expect(parseClaudeLine(RESULT)).toEqual({
      kind: "result",
      ok: true,
      error: null,
      totalCostUsd: 0.0337045,
      model: "claude-haiku-4-5-20251001",
      usage: { input: 18 + 13868, cachedInput: 48655, output: 217, reasoning: 107 },
    });
  });

  it("reads a stopped turn as failed, with its terminal reason", () => {
    expect(parseClaudeLine(RESULT_STOPPED)).toMatchObject({ kind: "result", ok: false, error: "aborted_tools", totalCostUsd: 0.0181357 });
  });

  it("ignores control responses and non-JSON output", () => {
    expect(parseClaudeLine(CONTROL_RESPONSE)).toEqual({ kind: "other" });
    expect(parseClaudeLine("Warning: something")).toEqual({ kind: "other" });
  });

  it("ignores valid JSON that is not an object", () => {
    expect(parseClaudeLine("null")).toEqual({ kind: "other" });
    expect(parseClaudeLine("42")).toEqual({ kind: "other" });
  });
});

describe("toolSummary", () => {
  it("prefers the tool's own description, then its main argument, then its name", () => {
    expect(toolSummary("Bash", { command: "npm test", description: "Run the tests" })).toBe("Run the tests");
    expect(toolSummary("Read", { file_path: "C:\\bots\\notes.md" })).toBe("C:\\bots\\notes.md");
    expect(toolSummary("TodoWrite", { todos: [] })).toBe("TodoWrite");
  });

  it("omits Claude's tool loading steps while keeping other searches visible", () => {
    expect(toolSummary("ToolSearch", { query: "select:mcp__tjommie__add_routine" })).toBeNull();
    expect(toolSummary("ToolSearch", { query: "other tools" })).toBeNull();
    expect(toolSummary("WebSearch", { query: "select: best laptops" })).toBe("best laptops");
  });

  it("keeps double underscores in ordinary read paths", () => {
    expect(toolSummary("Read", { file_path: "src/__tests__/chat.test.ts" })).toBe("src/__tests__/chat.test.ts");
    expect(toolSummary("Read", { file_path: "C:\\proj\\pkg\\__init__.py" })).toBe("C:\\proj\\pkg\\__init__.py");
    expect(toolSummary("Read", { description: "Read src/__tests__/chat.test.ts" })).toBe("Read src/__tests__/chat.test.ts");
    expect(toolSummary("Read", { description: "Read C:\\proj\\pkg\\__init__.py" })).toBe("Read C:\\proj\\pkg\\__init__.py");
    expect(toolSummary("Read", { description: "Read mcp__ notes" })).toBe("Read notes");
  });

  it("gives each Tjommie tool a plain activity label from its input", () => {
    expect(toolSummary("mcp__tjommie__list_bots", {})).toBe("Looked at the team");
    expect(toolSummary("mcp__tjommie__message_bot", { bot: "Writer", message: "Hello" })).toBe("Messaged Writer");
    expect(toolSummary("mcp__tjommie__share_file", { path: "output\\notes.md" })).toBe("Shared notes.md");
    expect(toolSummary("mcp__tjommie__add_routine", { name: "Morning check" })).toBe("Proposed a routine: Morning check");
  });

  it("describes browser actions and other MCP tools without protocol names", () => {
    expect(toolSummary("mcp__browser__browser_navigate", { url: "https://pnet.co.za/jobs" })).toBe("Browser: opened pnet.co.za");
    expect(toolSummary("mcp__browser__browser_click", { element: "Apply" })).toBe("Browser: clicked Apply");
    expect(toolSummary("mcp__weather__check_forecast", {})).toBe("check forecast");
  });

  it("does not leak protocol markers through activity inputs", () => {
    expect(toolSummary("mcp__tjommie__message_bot", { bot: "mcp__my_server__Writer" })).toBe("Messaged Writer");
    for (const label of [
      toolSummary("mcp__tjommie__message_bot", { bot: "select:mcp__team__bot" }),
      toolSummary("mcp__tjommie__message_bot", { bot: "mcp__my_server__Writer" }),
      toolSummary("mcp__tjommie__add_routine", { name: "select:mcp__news__brief" }),
      toolSummary("Bash", { description: "select:mcp__browser__browser_navigate" }),
    ]) {
      expect(label).not.toMatch(/mcp__|select:/);
    }
  });
});


it("preserves both Claude quota windows and stable result identities", () => {
  const parsed = parseClaudeLine(RATE_LIMIT_REJECTED);
  expect(parsed).toMatchObject({ kind: "rate_limit", windows: [
    { windowKey: "five_hour", durationMinutes: 300, usedPercent: 100, state: "reached" },
    { windowKey: "seven_day", durationMinutes: 10080, usedPercent: 7, state: "ok" },
  ] });
  expect(parseClaudeLine(JSON.stringify({ type: "result", uuid: "result-one", subtype: "success" }))).toMatchObject({ kind: "result", eventId: "result-one" });
});
