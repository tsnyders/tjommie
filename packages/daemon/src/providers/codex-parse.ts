import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { existsSync, realpathSync } from "node:fs";
import type { ProviderEvent } from "./types.ts";
import { activityLabel, plainActivityText } from "./activity-labels.ts";

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

// The approval request contains an item id, not the changed paths. Codex sends the paths on
// item/fileChange/patchUpdated (and sometimes item/started) before asking for approval.
export function fileChangePaths(changes: unknown): string[] | null {
  if (!Array.isArray(changes) || changes.length === 0) return null;
  const paths: string[] = [];
  for (const change of changes) {
    if (!isRecord(change) || typeof change.path !== "string" || !change.path) return null;
    paths.push(change.path);
    if (isRecord(change.kind)) {
      const moved = change.kind.move_path ?? change.kind.movePath;
      if (moved !== undefined && moved !== null) {
        if (typeof moved !== "string" || !moved) return null;
        paths.push(moved);
      }
    }
  }
  return paths;
}

function canonicalPath(path: string): string {
  let existing = path;
  const missing: string[] = [];
  while (!existsSync(existing)) {
    const parent = dirname(existing);
    if (parent === existing) return path;
    missing.unshift(existing.slice(parent.length).replace(/^[/\\]/, ""));
    existing = parent;
  }
  return join(realpathSync.native(existing), ...missing);
}

function atOrInside(folder: string, path: string): boolean {
  const suffix = relative(canonicalPath(folder), canonicalPath(path));
  return suffix === "" || (suffix !== ".." && !suffix.startsWith(`..${sep}`) && !isAbsolute(suffix));
}

function inWritableFolders(path: string, cwd: string, folders: string[]): boolean {
  if (!path || /^[A-Za-z]:[^\\/]/.test(path)) return false;
  const target = resolve(cwd, path);
  return folders.some((folder) => atOrInside(folder, target));
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
  writableFolders: string[] = [],
  changedPaths: string[] | null = null,
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
    const folders = writableFolders.map((folder) => resolve(folder));
    const grantRoot = params.grantRoot;
    const safeGrant = grantRoot === null || grantRoot === undefined
      || (typeof grantRoot === "string" && inWritableFolders(grantRoot, folders[0] ?? "", folders));
    const autoAllow = safeGrant && changedPaths !== null && changedPaths.length > 0
      && changedPaths.every((path) => inWritableFolders(path, folders[0] ?? "", folders));
    return {
      type: "approval",
      id: String(id),
      tool: "Edit",
      summary: typeof params.reason === "string" ? params.reason : "Change files",
      detail: typeof params.grantRoot === "string" ? params.grantRoot : "",
      rule: folders.length > 0 ? `codex-edit:${JSON.stringify(folders)}` : null,
      autoAllow,
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
