import type { ProviderEvent } from "./types.ts";

const TOOL_ITEMS = new Set(["commandExecution", "fileChange", "mcpToolCall", "webSearch"]);
const toIso = (seconds: unknown): string | null => (typeof seconds === "number" ? new Date(seconds * 1000).toISOString() : null);

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
      return [{ type: "tool", id: itemId, name: "Shell", summary: String(firstCommand(item)).slice(0, 200) }];
    case "fileChange": {
      const changes = Array.isArray(item.changes) ? item.changes : [];
      const summary = changes.map((change) => (isRecord(change) ? change.path : undefined)).join(", ").slice(0, 200);
      return [{ type: "tool", id: itemId, name: "Edit", summary: summary || "Change files" }];
    }
    case "mcpToolCall": {
      const argumentsValue = isRecord(item.arguments) ? item.arguments : {};
      const tool = String(item.tool);
      return [{ type: "tool", id: itemId, name: `${String(item.server)}.${tool}`, summary: String(argumentsValue.title ?? tool).slice(0, 200) }];
    }
    case "webSearch":
      return [{ type: "tool", id: itemId, name: "Web search", summary: String(item.query ?? "Search the web").slice(0, 200) }];
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
      const primary = isRecord(limits.primary) ? limits.primary : null;
      const used: number | null = primary && typeof primary.usedPercent === "number" ? primary.usedPercent : null;
      return [
        {
          type: "limit",
          state: limits.rateLimitReachedType ? "reached" : used !== null && used >= 90 ? "near" : "ok",
          usedPercent: used,
          resetsAt: toIso(primary?.resetsAt),
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

// Codex asks before every MCP tool call. For a server that isn't allowed outright this becomes an approval
// card. The tool's name only appears in the message text.
export function mapCodexElicitation(id: number | string, params: unknown): Extract<ProviderEvent, { type: "approval" }> | null {
  if (!isRecord(params) || typeof params.serverName !== "string" || !params.serverName || params.mode !== "form" || typeof params.message !== "string") {
    return null;
  }
  if (!isRecord(params._meta) || params._meta.codex_approval_kind !== "mcp_tool_call") return null;
  const meta = params._meta;
  if (meta.tool_description !== undefined && typeof meta.tool_description !== "string") return null;
  if (meta.tool_params !== undefined && !isRecord(meta.tool_params)) return null;

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
