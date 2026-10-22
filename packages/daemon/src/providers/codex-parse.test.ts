import { describe, expect, it } from "vitest";
import { CODEX_FEATURES_OFF, codexConfig, codexPolicy } from "./codex.ts";
import { mapCodexElicitation, mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
import type { StartOptions } from "./types.ts";

type RpcFixture = Record<string, unknown> & { method: string; params: Record<string, unknown> };
type RpcRequestFixture = RpcFixture & { id: number | string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parse(line: string): RpcFixture {
  const value: unknown = JSON.parse(line);
  if (!isRecord(value) || typeof value.method !== "string" || !isRecord(value.params)) {
    throw new Error("Expected an RPC fixture");
  }
  return value as RpcFixture;
}

function parseRequest(line: string): RpcRequestFixture {
  const value = parse(line);
  if (typeof value.id !== "number" && typeof value.id !== "string") {
    throw new Error("Expected an RPC request fixture");
  }
  return value as RpcRequestFixture;
}

const MODEL = "gpt-6-sol";

const DELTA = parse( // built from the schema, real ids
  '{"method":"item/agentMessage/delta","params":{"threadId":"01a0d7cb-a738-7121-bc46-cb16f5896e3c","turnId":"01a0d7cb-b0ed-7a02-bc8c-042988202fa1","itemId":"msg_061da235ca62de87016ab63848eab087d0862ec7c6a83415fe","delta":"I will run"}}',
);
const COMMAND_STARTED = parse( // trimmed: the PowerShell wrapper command, cwd and process fields removed
  '{"method":"item/started","params":{"item":{"type":"commandExecution","id":"exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974","source":"unifiedExecStartup","status":"inProgress","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath \'inside.txt\' -Value \'hi\' -NoNewline; Get-Content -LiteralPath \'inside.txt\' -Raw"}],"aggregatedOutput":null,"exitCode":null,"durationMs":null},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","startedAtMs":1790327506815},"emittedAtMs":1790327506815}',
);
const COMMAND_COMPLETED = parse( // trimmed like COMMAND_STARTED
  '{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974","status":"completed","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath \'inside.txt\' -Value \'hi\' -NoNewline; Get-Content -LiteralPath \'inside.txt\' -Raw"}],"aggregatedOutput":"hi","exitCode":0,"durationMs":398},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","completedAtMs":1790327507214},"emittedAtMs":1790327507311}',
);
const AGENT_COMPLETED = parse(
  '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_04cbf98466447733016ab63af82afc87d092e78611119924a6","text":"1. `inside.txt`: wrote `hi` and read it back successfully.","phase":"final_answer","memoryCitation":null,"delivery":null,"questions":null},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5"}}',
);
const TOKEN_USAGE = parse(
  '{"method":"thread/tokenUsage/updated","params":{"threadId":"01a0d7cb-a738-7121-bc46-cb16f5896e3c","turnId":"01a0d7cb-b0ed-7a02-bc8c-042988202fa1","tokenUsage":{"total":{"totalTokens":52111,"inputTokens":51830,"cachedInputTokens":25600,"cacheWriteInputTokens":0,"outputTokens":281,"reasoningOutputTokens":35},"last":{"totalTokens":26163,"inputTokens":26025,"cachedInputTokens":25600,"cacheWriteInputTokens":0,"outputTokens":138,"reasoningOutputTokens":9},"modelContextWindow":258400}},"emittedAtMs":1790326878417}',
);
const RATE_LIMITS = parse(
  '{"method":"account/rateLimits/updated","params":{"rateLimits":{"limitId":"codex","limitName":null,"normalModelSlug":null,"primary":{"usedPercent":25,"windowDurationMins":10080,"resetsAt":1790438506},"secondary":null,"credits":{"hasCredits":false,"unlimited":false,"balance":"0"},"individualLimit":null,"spendControlReached":null,"planType":"pro","rateLimitReachedType":null}},"emittedAtMs":1790326867549}',
);
const TURN_COMPLETED = parse(
  '{"method":"turn/completed","params":{"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turn":{"id":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","items":[],"itemsView":"summary","status":"completed","error":null,"startedAt":1790327484,"completedAt":1790327547,"durationMs":62249}},"emittedAtMs":1790327547033}',
);
const TURN_LIMITED = { method: "turn/completed", params: { threadId: "t", turn: { id: "u", items: [], status: "failed", error: { message: "You've hit your usage limit.", codexErrorInfo: "usageLimitExceeded" } } } }; // built from the schema
const TURN_LIMITED_SPEC = { method: "turn/completed", params: { threadId: "t", turn: { id: "u", items: [], status: "failed", error: { message: "You've hit your usage limit.", codexErrorInfo: "UsageLimitExceeded" } } } }; // spec spelling
const APPROVAL = parseRequest( // trimmed: the PowerShell path in "command" shortened to powershell.exe
  '{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","itemId":"exec-0d7b2fc4-81aa-4a06-98bf-ef0ccc6c31bf","startedAtMs":1790327515964,"environmentId":"local","reason":"Allow writing and reading outside-netconfig.txt in the parent folder as requested?","command":"powershell.exe -Command \\\"Set-Content -LiteralPath \'C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\outside-netconfig.txt\' -Value \'hi\' -NoNewline\\\"","cwd":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\netconfig","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath \'C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\outside-netconfig.txt\' -Value \'hi\' -NoNewline"}],"proposedExecpolicyAmendment":["Set-Content","-LiteralPath","C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\outside-netconfig.txt","-Value","hi","-NoNewline"],"availableDecisions":["accept","cancel"]}}',
);
const FILE_APPROVAL = { reason: null, grantRoot: "C:\\work", itemId: "i", threadId: "t", turnId: "u", startedAtMs: 1 }; // built from the schema
const ELICITATION = parseRequest( // the log cut the line off after "requestedSchema"; its end is as in the run's other requests
  '{"method":"mcpServer/elicitation/request","id":0,"params":{"threadId":"01a0d957-abb2-79b3-a19e-4293c113bea1","turnId":"01a0d957-adfa-7f20-8558-a1eb40e165f2","serverName":"browser","mode":"form","_meta":{"codex_approval_kind":"mcp_tool_call","persist":["session","always"],"tool_description":"Navigate to a URL","tool_params":{"url":"https://example.com"},"tool_params_display":[{"name":"url","value":"https://example.com","display_name":"url"}]},"message":"Allow the browser MCP server to run tool \\"browser_navigate\\"?","requestedSchema":{"type":"object","properties":{}}}}',
);

describe("mapCodexNotification", () => {
  it("streams agent text and closes it when the message completes", () => {
    expect(mapCodexNotification(DELTA.method, DELTA.params, MODEL)).toEqual([{ type: "text", delta: "I will run" }]);
    expect(mapCodexNotification(AGENT_COMPLETED.method, AGENT_COMPLETED.params, MODEL)).toEqual([{ type: "text_end" }]);
  });

  it("reports a shell command as a tool, summarized by its inner command", () => {
    expect(mapCodexNotification(COMMAND_STARTED.method, COMMAND_STARTED.params, MODEL)).toEqual([
      { type: "tool", id: "exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974", name: "Shell", summary: "Set-Content -LiteralPath 'inside.txt' -Value 'hi' -NoNewline; Get-Content -LiteralPath 'inside.txt' -Raw" },
    ]);
    expect(mapCodexNotification(COMMAND_COMPLETED.method, COMMAND_COMPLETED.params, MODEL)).toEqual([{ type: "tool_done", id: "exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974", ok: true }]);
  });

  it("keeps double underscores in a shell command", () => {
    const command = "python -c 'print(__name__)'";
    expect(mapCodexNotification("item/started", { item: { type: "commandExecution", id: "shell-1", commandActions: [{ command }] } }, MODEL))
      .toEqual([{ type: "tool", id: "shell-1", name: "Shell", summary: command }]);
  });

  it.each([
    { server: "tjommie", tool: "list_bots", args: {}, label: "Looked at the team" },
    { server: "tjommie", tool: "message_bot", args: { bot: "Writer" }, label: "Messaged Writer" },
    { server: "tjommie", tool: "share_file", args: { path: "output/notes.md" }, label: "Shared notes.md" },
    { server: "tjommie", tool: "add_routine", args: { name: "Morning check" }, label: "Proposed a routine: Morning check" },
    { server: "browser", tool: "browser_navigate", args: { url: "https://pnet.co.za/jobs" }, label: "Browser: opened pnet.co.za" },
    { server: "browser", tool: "browser_click", args: { element: "Apply" }, label: "Browser: clicked Apply" },
    { server: "weather", tool: "check_forecast", args: {}, label: "check forecast" },
  ])("gives Codex MCP $server $tool a plain activity label", ({ server, tool, args, label }) => {
    const events = mapCodexNotification("item/started", { item: { type: "mcpToolCall", id: "mcp-1", server, tool, arguments: args } }, MODEL);
    expect(events).toEqual([{ type: "tool", id: "mcp-1", name: label, summary: label }]);
    expect(JSON.stringify(events)).not.toMatch(/mcp__|select:/);
  });

  it("normalizes a prefixed Codex MCP tool name and hides selector calls", () => {
    expect(mapCodexNotification("item/started", { item: { type: "mcpToolCall", id: "mcp-2", server: "tjommie", tool: "mcp__tjommie__add_routine", arguments: { name: "Morning check" } } }, MODEL))
      .toEqual([{ type: "tool", id: "mcp-2", name: "Proposed a routine: Morning check", summary: "Proposed a routine: Morning check" }]);
    expect(mapCodexNotification("item/started", { item: { type: "mcpToolCall", id: "mcp-3", server: "tjommie", tool: "ToolSearch", arguments: { query: "select:mcp__tjommie__add_routine" } } }, MODEL)).toEqual([]);
  });

  it("reports each model call's tokens, uncached input separately", () => {
    expect(mapCodexNotification(TOKEN_USAGE.method, TOKEN_USAGE.params, MODEL)).toEqual([
      { type: "usage", model: MODEL, input: 26025 - 25600, cachedInput: 25600, output: 138, reasoning: 9, costUsd: null },
    ]);
  });

  it("reports rate limits with percent and reset time", () => {
    expect(mapCodexNotification(RATE_LIMITS.method, RATE_LIMITS.params, MODEL)).toEqual([
      { type: "limit", state: "ok", usedPercent: 25, resetsAt: new Date(1790438506 * 1000).toISOString() },
    ]);
  });

  it.each([
    { primary: { usedPercent: 25, resetsAt: 100 }, secondary: { usedPercent: 100, resetsAt: 200 }, state: "reached", used: 100, reset: 200 },
    { primary: { usedPercent: 96, resetsAt: 100 }, secondary: { usedPercent: 20, resetsAt: 200 }, state: "near", used: 96, reset: 100 },
    { primary: { usedPercent: 100, resetsAt: 300 }, secondary: { usedPercent: 100, resetsAt: 200 }, state: "reached", used: 100, reset: 300 },
    { primary: null, secondary: { usedPercent: 94, resetsAt: 200 }, state: "near", used: 94, reset: 200 },
    { primary: { usedPercent: 25, resetsAt: 100 }, secondary: { usedPercent: 95, resetsAt: 200 }, state: "near", used: 95, reset: 200 },
  ])("reports the restrictive quota window ($state, $used%) with its own reset", ({ primary, secondary, state, used, reset }) => {
    expect(mapCodexNotification("account/rateLimits/updated", { rateLimits: { primary, secondary } }, MODEL))
      .toEqual([{ type: "limit", state, usedPercent: used, resetsAt: new Date(reset * 1000).toISOString() }]);
  });

  it("keeps account snapshots independent and handles invalid quota values", () => {
    expect(mapCodexNotification("account/rateLimits/updated", { rateLimits: { secondary: { usedPercent: 100, resetsAt: 200 } } }, MODEL))
      .toEqual([{ type: "limit", state: "reached", usedPercent: 100, resetsAt: new Date(200_000).toISOString() }]);
    expect(mapCodexNotification("account/rateLimits/updated", { rateLimits: { primary: { usedPercent: 10, resetsAt: 100 } } }, MODEL))
      .toEqual([{ type: "limit", state: "ok", usedPercent: 10, resetsAt: new Date(100_000).toISOString() }]);
    expect(mapCodexNotification("account/rateLimits/updated", { rateLimits: { primary: { usedPercent: NaN, resetsAt: Infinity }, secondary: { usedPercent: "100", resetsAt: 1e30 } } }, MODEL))
      .toEqual([{ type: "limit", state: "ok", usedPercent: null, resetsAt: null }]);
  });

  it("ends turns, and reports either usage-limit spelling as a reached limit first", () => {
    expect(mapCodexNotification(TURN_COMPLETED.method, TURN_COMPLETED.params, MODEL)).toEqual([{ type: "turn_done", ok: true, error: null }]);
    for (const limited of [TURN_LIMITED, TURN_LIMITED_SPEC]) {
      expect(mapCodexNotification(limited.method, limited.params, MODEL)).toEqual([
        { type: "limit", state: "reached", usedPercent: 100, resetsAt: null },
        { type: "turn_done", ok: false, error: "You've hit your usage limit." },
      ]);
    }
  });

  it("ignores notifications Tjommie doesn't use", () => {
    expect(mapCodexNotification("mcpServer/startupStatus/updated", { name: "codex_apps", status: "ready" }, MODEL)).toEqual([]);
  });
});

describe("mapCodexRequest", () => {
  it("turns a command approval into an approval card with a reusable rule", () => {
    expect(mapCodexRequest(APPROVAL.id, APPROVAL.method, APPROVAL.params)).toEqual({
      type: "approval",
      id: "0",
      tool: "Shell",
      summary: "Allow writing and reading outside-netconfig.txt in the parent folder as requested?",
      detail: "Set-Content -LiteralPath 'C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\codex\\outside-netconfig.txt' -Value 'hi' -NoNewline",
      rule: `codex:${JSON.stringify(APPROVAL.params.proposedExecpolicyAmendment)}`,
    });
  });

  it("turns a file-change approval into a card without a rule, and refuses unknown requests", () => {
    expect(mapCodexRequest(4, "item/fileChange/requestApproval", FILE_APPROVAL)).toEqual({ type: "approval", id: "4", tool: "Edit", summary: "Change files", detail: "C:\\work", rule: null });
    expect(mapCodexRequest(5, "item/tool/requestUserInput", {})).toBeNull();
  });

  it("turns the question before an MCP tool call into a card with a reusable rule", () => {
    expect(mapCodexElicitation(ELICITATION.id, ELICITATION.params)).toEqual({
      type: "approval",
      id: "0",
      tool: "browser.browser_navigate",
      summary: "Navigate to a URL",
      detail: JSON.stringify({ url: "https://example.com" }, null, 2),
      rule: "codex-mcp:browser.browser_navigate",
    });
  });

  it("refuses malformed MCP tool-call elicitations", () => {
    expect(mapCodexElicitation(1, { ...ELICITATION.params, serverName: 3 })).toBeNull();
    expect(mapCodexElicitation(2, { ...ELICITATION.params, _meta: { codex_approval_kind: "mcp_tool_call", tool_description: "Navigate", tool_params: "bad" } })).toBeNull();
    expect(mapCodexElicitation(3, { ...ELICITATION.params, message: "May I ask a question?" })).toBeNull();
  });
});

describe("codex policy and config", () => {
  const options: StartOptions = { cwd: "C:\\p", extraDirs: ["C:\\home"], model: "", instructions: "", autonomy: "folder", allowRules: [], resumeId: null, env: {}, mcpServers: [] };

  it("maps autonomy to approval policy and sandbox", () => {
    expect(codexPolicy("ask")).toEqual({ approvalPolicy: "untrusted", sandbox: "read-only" });
    expect(codexPolicy("folder")).toEqual({ approvalPolicy: "on-request", sandbox: "workspace-write" });
    expect(codexPolicy("full")).toEqual({ approvalPolicy: "never", sandbox: "danger-full-access" });
  });

  it("chooses elevated for Own folder, elevated for ready Always ask, and leaves Full auto as is", () => {
    const features = {
      apps: false,
      browser_use: false,
      browser_use_external: false,
      computer_use: false,
      in_app_browser: false,
      plugins: false,
      remote_plugin: false,
      memories: false,
      tool_suggest: false,
      skill_mcp_dependency_install: false,
    };
    const common = { sandbox_workspace_write: { network_access: false, writable_roots: ["C:\\home"] }, features, mcp_servers: {} };
    expect(codexConfig(options, "win32")).toEqual({ ...common, windows: { sandbox: "elevated" } });
    expect(codexConfig({ ...options, autonomy: "ask", codexSandboxReady: false }, "win32")).toEqual({ ...common, windows: { sandbox: "unelevated" } });
    expect(codexConfig({ ...options, autonomy: "ask", codexSandboxReady: true }, "win32")).toEqual({ ...common, windows: { sandbox: "elevated" } });
    expect(codexConfig({ ...options, autonomy: "full", codexSandboxReady: true }, "win32")).toEqual({ ...common, windows: { sandbox: "unelevated" } });
    expect(codexConfig(options, "darwin")).toEqual(common);
    expect(CODEX_FEATURES_OFF).toEqual({
      apps: false,
      browser_use: false,
      browser_use_external: false,
      computer_use: false,
      in_app_browser: false,
      plugins: false,
      remote_plugin: false,
      memories: false,
      tool_suggest: false,
      skill_mcp_dependency_install: false,
    });
  });

  it("attaches Tjommie's MCP servers to the thread", () => {
    const config = codexConfig({
      ...options,
      mcpServers: [
        { name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: { Authorization: "Bearer t0k" }, autoAllow: true },
        { name: "browser", command: "C:\\node.exe", args: ["cli.js"], env: { ELECTRON_RUN_AS_NODE: "1" }, autoAllow: true },
      ],
    });
    expect(config.mcp_servers).toEqual({
      tjommie: { url: "http://127.0.0.1:47100/mcp", http_headers: { Authorization: "Bearer t0k" } },
      browser: { command: "C:\\node.exe", args: ["cli.js"], env: { ELECTRON_RUN_AS_NODE: "1" } },
    });
  });
});
