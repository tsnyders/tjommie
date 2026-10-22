import type { ProviderEvent } from "./types.ts";
import { activityLabel, plainActivityText } from "./activity-labels.ts";

const TOOL_ITEMS = new Set(["commandExecution", "fileChange", "mcpToolCall", "webSearch"]);
const toIso = (seconds: unknown): string | null => {
  if (typeof seconds !== "number" || !Number.isFinite(seconds)) return null;
  const date = new Date(seconds * 1000);
  return Number.isFinite(date.getTime()) ? date.toISOString() : null;
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function firstCommand(item: Record<string, unknown>): unknown {
  const actions = Array.isArray(item.commandActions) ? item.commandActions : [];
  const first = actions.length > 0 && isRecord(actions[0]) ? actions[0] : null;
  return first?.command ?? item.command;
}

function toolStarted(value: unknown): ProviderEvent[] {
  if (!isRecord(value) || typeof value.type !== "string" || typeof value.id !== "string") return [];
  const item = value;
  const itemType = value.type;
  const itemId = value.id;

  switch (itemType) {
    case "commandExecution":
      return [{ type: "tool", id: itemId, name: "Shell", summary: plainActivityText(String(firstCommand(item))) }];
    case "fileChange": {
      const changes = Array.isArray(item.changes) ? item.changes : [];
      const summary = changes.map((change) => (isRecord(change) ? change.path : undefined)).join(", ").slice(0, 200);
      return [{ type: "tool", id: itemId, name: "Edit", summary: plainActivityText(summary) || "Change files" }];
    }
    case "mcpToolCall": {
      const argumentsValue = isRecord(item.arguments) ? item.arguments : {};
      if (typeof item.tool !== "string" || typeof item.server !== "string") return [];
      const label = activityLabel(item.tool, argumentsValue, item.server);
      return label ? [{ type: "tool", id: itemId, name: label, summary: label }] : [];
    }
    case "webSearch":
      return [{ type: "tool", id: itemId, name: "Web search", summary: plainActivityText(String(item.query ?? "Search the web")) }];
    default:
      return [];
  }
}

// Maps one app-server notification to Tjommie's provider events. `model` is the thread's model.
export function mapCodexNotification(method: string, params: unknown, model: string): ProviderEvent[] {
  if (!isRecord(params)) return [];

  switch (method) {
    case "item/agentMessage/delta":
      return typeof params.delta === "string" ? [{ type: "text", delta: params.delta }] : [];
    case "item/started":
      return toolStarted(params.item);
    case "item/completed": {
      if (!isRecord(params.item) || typeof params.item.type !== "string") return [];
      const item = params.item;
      const itemType = params.item.type;
      if (itemType === "agentMessage") return [{ type: "text_end" }];
      // Web searches carry no status; they only complete when they worked.
      if (TOOL_ITEMS.has(itemType) && typeof item.id === "string") {
        return [{ type: "tool_done", id: item.id, ok: item.status === undefined || item.status === "completed" }];
      }
      return [];
    }
    case "thread/tokenUsage/updated": {
      if (!isRecord(params.tokenUsage) || !isRecord(params.tokenUsage.last)) return [];
      const last = params.tokenUsage.last;
      if (
        typeof last.inputTokens !== "number"
        || typeof last.cachedInputTokens !== "number"
        || typeof last.outputTokens !== "number"
        || typeof last.reasoningOutputTokens !== "number"
      ) {
        return [];
      }
      return [
        {
          type: "usage",
          model,
          input: last.inputTokens - last.cachedInputTokens,
          cachedInput: last.cachedInputTokens,
          output: last.outputTokens,
          reasoning: last.reasoningOutputTokens,
          costUsd: null, // Codex reports no cost; Phase 2 prices tokens from a price list
        },
      ];
    }
    case "account/rateLimits/updated": {
      if (!isRecord(params.rateLimits)) return [];
      const limits = params.rateLimits;
      // Keep the single-window provider contract: percentages and resets always describe the same
      // window. If both are exhausted, work cannot resume until the later reset.
      const windows = [limits.primary, limits.secondary].filter(isRecord).map((window) => ({
        used: typeof window.usedPercent === "number" && Number.isFinite(window.usedPercent) ? window.usedPercent : null,
        reset: toIso(window.resetsAt),
      }));
      windows.sort((a, b) => {
        if (a.used !== null && a.used >= 100 && b.used !== null && b.used >= 100) {
          // An unknown reset for an exhausted window must not borrow another window's timestamp.
          if (a.reset === null || b.reset === null) return a.reset === b.reset ? 0 : a.reset === null ? -1 : 1;
          return Date.parse(b.reset) - Date.parse(a.reset);
        }
        return (b.used ?? -1) - (a.used ?? -1);
      });
      const selected = windows[0];
      const used = selected?.used ?? null;
      return [
        {
          type: "limit",
          state: limits.rateLimitReachedType || (used !== null && used >= 100) ? "reached" : used !== null && used >= 90 ? "near" : "ok",
          usedPercent: used,
          resetsAt: selected?.reset ?? null,
        },
      ];
    }
    case "turn/completed": {
      if (!isRecord(params.turn)) return [];
      const turn = params.turn;
      const error = isRecord(turn.error) ? turn.error : null;
      const errorInfo = error?.codexErrorInfo;
      const limited = errorInfo === "usageLimitExceeded" || errorInfo === "UsageLimitExceeded";
      const limit: ProviderEvent[] = limited ? [{ type: "limit", state: "reached", usedPercent: 100, resetsAt: null }] : [];
      const ok = turn.status === "completed";
      const message = error && typeof error.message === "string" ? error.message : `The turn ended: ${String(turn.status)}`;
      return [...limit, { type: "turn_done", ok, error: ok ? null : message }];
    }
    default:
      return [];
  }
}

// Maps an approval request from the app-server. The session refuses anything this returns null for.
export function mapCodexRequest(
  id: number | string,
  method: string,
  params: unknown,
): Extract<ProviderEvent, { type: "approval" }> | null {
  if (!isRecord(params)) return null;

  if (method === "item/commandExecution/requestApproval") {
    const command = String(firstCommand(params) ?? "a command");
    const proposal = params.proposedExecpolicyAmendment;
    return {
      type: "approval",
      id: String(id),
      tool: "Shell",
      summary: typeof params.reason === "string" ? params.reason : `Run ${command}`,
      detail: command,
      rule: Array.isArray(proposal) ? `codex:${JSON.stringify(proposal)}` : null,
    };
  }
  if (method === "item/fileChange/requestApproval") {
    return {
      type: "approval",
      id: String(id),
      tool: "Edit",
      summary: typeof params.reason === "string" ? params.reason : "Change files",
      detail: typeof params.grantRoot === "string" ? params.grantRoot : "",
      rule: null,
    };
  }
  return null;
}

type CodexMcpToolCall = {
  serverName: string;
  mode: "form";
  message: string;
  _meta: {
    codex_approval_kind: "mcp_tool_call";
    tool_description?: string;
    tool_params?: Record<string, unknown>;
  };
};

// Validate the tool-call fields independently of the wording used to build an approval card.
export function isCodexMcpToolCall(params: unknown): params is CodexMcpToolCall {
  if (!isRecord(params) || typeof params.serverName !== "string" || !params.serverName || params.mode !== "form" || typeof params.message !== "string") {
    return false;
  }
  if (!isRecord(params._meta) || params._meta.codex_approval_kind !== "mcp_tool_call") return false;
  const meta = params._meta;
  if (meta.tool_description !== undefined && typeof meta.tool_description !== "string") return false;
  if (meta.tool_params !== undefined && !isRecord(meta.tool_params)) return false;
  return true;
}

// Codex asks before every MCP tool call. For a server that isn't allowed outright this becomes an approval
// card. The tool's name only appears in the message text.
export function mapCodexElicitation(id: number | string, params: unknown): Extract<ProviderEvent, { type: "approval" }> | null {
  if (!isCodexMcpToolCall(params)) return null;
  const meta = params._meta;
  const tool = params.message.match(/tool "([^"]+)"/)?.[1];
  if (!tool) return null;
  return {
    type: "approval",
    id: String(id),
    tool: `${params.serverName}.${tool}`,
    summary: meta.tool_description ?? `Use ${tool}`,
    detail: JSON.stringify(meta.tool_params ?? {}, null, 2).slice(0, 4000),
    rule: `codex-mcp:${params.serverName}.${tool}`,
  };
}
