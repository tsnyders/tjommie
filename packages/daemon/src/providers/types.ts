import type { Autonomy } from "@tjommie/protocol";

export type ApprovalAnswer = "once" | "always" | "deny";

// What every CLI adapter reports, whatever its CLI prints.
export type ProviderEvent =
  | { type: "session"; cliSessionId: string }
  | { type: "text"; delta: string }
  | { type: "text_end" }
  | { type: "tool"; id: string; name: string; summary: string }
  | { type: "tool_done"; id: string; ok: boolean }
  | { type: "approval"; id: string; tool: string; summary: string; detail: string; rule: string | null }
  | { type: "usage"; model: string; input: number; cachedInput: number; output: number; reasoning: number; costUsd: number | null; totalCostUsd?: number | null }
  | { type: "limit"; state: "ok" | "near" | "reached"; usedPercent: number | null; resetsAt: string | null }
  | { type: "turn_done"; ok: boolean; error: string | null }
  | { type: "exit"; error: string | null; completedTurn?: { ok: boolean; error: string | null } };

// An MCP server attached to one CLI session: Tjommie's own over HTTP, or a program the CLI starts (the browser).
// autoAllow: its tools run without asking the person.
export type McpServer =
  | { name: string; url: string; headers: Record<string, string>; autoAllow: boolean }
  | { name: string; command: string; args: string[]; env: Record<string, string>; autoAllow: boolean };

export type StartOptions = {
  cwd: string;
  extraDirs: string[];
  model: string;
  instructions: string;
  autonomy: Autonomy;
  codexSandboxReady?: boolean; // Always ask uses elevated only after the account setup is ready.
  allowRules: string[];
  resumeId: string | null;
  resumeCostUsd?: number | null; // last reported cumulative cost for this saved CLI session
  env: Record<string, string>;
  mcpServers: McpServer[];
};

// One running CLI conversation. Claude and Codex each have an adapter that implements this.
export interface ProviderSession {
  start(): Promise<void>;
  send(text: string): Promise<void>;
  steer(text: string): Promise<boolean>; // false: couldn't add it to the current turn, so the caller queues it
  interrupt(): Promise<boolean | void>; // true: the provider handled this Stop, including a pending send
  answer(approvalId: string, answer: ApprovalAnswer): void;
  close(): void;
  waitForExit?(): Promise<void>; // resolves after the underlying process has exited, including a pending start
  pid(): number | undefined;
  onEvent(listener: (event: ProviderEvent) => void): void;
}
