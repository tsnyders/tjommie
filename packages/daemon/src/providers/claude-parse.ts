import { activityLabel } from "./activity-labels.ts";

// One line of `claude -p --output-format stream-json` output, reduced to what Tjommie uses.
export type ClaudeLine =
  | { kind: "init"; sessionId: string }
  | { kind: "text_delta"; text: string }
  | { kind: "assistant"; texts: string[]; tools: { id: string; name: string; input: Record<string, unknown> }[] }
  | { kind: "tool_results"; results: { id: string; ok: boolean }[] }
  | {
      kind: "permission";
      requestId: string;
      toolName: string;
      input: Record<string, unknown>;
      description: string;
      rule: string;
      suggestion: Record<string, unknown> | null;
    }
  | { kind: "rate_limit"; state: "ok" | "near" | "reached"; usedPercent: number | null; resetsAt: string | null }
  | {
      kind: "result";
      ok: boolean;
      error: string | null;
      totalCostUsd: number | null;
      model: string | null;
      usage: { input: number; cachedInput: number; output: number; reasoning: number };
    }
  | { kind: "other" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function numberOrZero(value: unknown): number {
  return typeof value === "number" ? value : 0;
}

export function parseClaudeLine(line: string): ClaudeLine {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line) as unknown;
  } catch {
    return { kind: "other" };
  }
  if (!isRecord(parsed)) return { kind: "other" };

  // Subagent traffic stays inside its tool call and never reaches the chat.
  const parentToolUseId = parsed.parent_tool_use_id;
  const fromSubagent = parentToolUseId !== null && parentToolUseId !== undefined;

  switch (parsed.type) {
    case "system":
      return parsed.subtype === "init" && typeof parsed.session_id === "string"
        ? { kind: "init", sessionId: parsed.session_id }
        : { kind: "other" };
    case "stream_event": {
      if (fromSubagent || !isRecord(parsed.event) || parsed.event.type !== "content_block_delta" || !isRecord(parsed.event.delta)) {
        return { kind: "other" };
      }
      return parsed.event.delta.type === "text_delta" && typeof parsed.event.delta.text === "string"
        ? { kind: "text_delta", text: parsed.event.delta.text }
        : { kind: "other" };
    }
    case "assistant": {
      if (fromSubagent) return { kind: "other" };
      const message = isRecord(parsed.message) ? parsed.message : {};
      const content = Array.isArray(message.content) ? message.content : [];
      const texts: string[] = [];
      const tools: { id: string; name: string; input: Record<string, unknown> }[] = [];

      for (const block of content) {
        if (!isRecord(block)) continue;
        if (block.type === "text" && typeof block.text === "string") {
          texts.push(block.text);
        } else if (block.type === "tool_use" && typeof block.id === "string" && typeof block.name === "string") {
          tools.push({ id: block.id, name: block.name, input: isRecord(block.input) ? block.input : {} });
        }
      }

      return { kind: "assistant", texts, tools };
    }
    case "user": {
      if (fromSubagent || !isRecord(parsed.message) || !Array.isArray(parsed.message.content)) return { kind: "other" };
      const results: { id: string; ok: boolean }[] = [];

      for (const block of parsed.message.content) {
        if (isRecord(block) && block.type === "tool_result" && typeof block.tool_use_id === "string") {
          results.push({ id: block.tool_use_id, ok: block.is_error !== true });
        }
      }

      return results.length ? { kind: "tool_results", results } : { kind: "other" };
    }
    case "control_request": {
      if (!isRecord(parsed.request) || parsed.request.subtype !== "can_use_tool") return { kind: "other" };
      const request = parsed.request;
      if (typeof parsed.request_id !== "string" || typeof request.tool_name !== "string") return { kind: "other" };

      const suggestions = Array.isArray(request.permission_suggestions) ? request.permission_suggestions : [];
      const suggestion = suggestions.find(
        (candidate): candidate is Record<string, unknown> =>
          isRecord(candidate) && candidate.type === "addRules" && candidate.behavior === "allow",
      ) ?? null;
      const rules = suggestion && Array.isArray(suggestion.rules) ? suggestion.rules : [];
      const first = rules.length > 0 && isRecord(rules[0]) ? rules[0] : null;
      const firstToolName = first && typeof first.toolName === "string" ? first.toolName : null;
      const firstRuleContent = first && typeof first.ruleContent === "string" ? first.ruleContent : null;

      return {
        kind: "permission",
        requestId: parsed.request_id,
        toolName: request.tool_name,
        input: isRecord(request.input) ? request.input : {},
        description: typeof request.description === "string" ? request.description : request.tool_name,
        rule: firstToolName ? (firstRuleContent ? `${firstToolName}(${firstRuleContent})` : firstToolName) : request.tool_name,
        suggestion,
      };
    }
    case "rate_limit_event": {
      const info = isRecord(parsed.rate_limit_info) ? parsed.rate_limit_info : {};
      const windows = isRecord(info.unifiedWindows) ? info.unifiedWindows : {};
      const selectedWindow = typeof info.rateLimitType === "string" ? windows[info.rateLimitType] : undefined;
      const window = isRecord(selectedWindow) ? selectedWindow : null;
      return {
        kind: "rate_limit",
        state: info.status === "rejected" ? "reached" : info.status === "allowed_warning" ? "near" : "ok",
        usedPercent: window && typeof window.utilization === "number" ? Math.round(window.utilization * 100) : null,
        resetsAt: typeof info.resetsAt === "number" ? new Date(info.resetsAt * 1000).toISOString() : null,
      };
    }
    case "result": {
      const usage = isRecord(parsed.usage) ? parsed.usage : {};
      const outputDetails = isRecord(usage.output_tokens_details) ? usage.output_tokens_details : {};
      const modelUsage = isRecord(parsed.modelUsage) ? parsed.modelUsage : {};
      const failed = parsed.is_error === true || parsed.subtype !== "success";
      return {
        kind: "result",
        ok: !failed,
        error: failed ? String(parsed.result || parsed.terminal_reason || parsed.subtype || "The turn failed") : null,
        totalCostUsd: typeof parsed.total_cost_usd === "number" ? parsed.total_cost_usd : null,
        model: Object.keys(modelUsage)[0] ?? null,
        usage: {
          input: numberOrZero(usage.input_tokens) + numberOrZero(usage.cache_creation_input_tokens),
          cachedInput: numberOrZero(usage.cache_read_input_tokens),
          output: numberOrZero(usage.output_tokens),
          reasoning: numberOrZero(outputDetails.thinking_tokens),
        },
      };
    }
    default:
      return { kind: "other" };
  }
}

// A one-line description of a tool call for the chat's activity row.
export function toolSummary(name: string, input: Record<string, unknown>): string | null {
  return activityLabel(name, input);
}
