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
  | { type: "usage"; model: string; input: number; cachedInput: number; output: number; reasoning: number; costUsd: number | null }
  | { type: "limit"; state: "ok" | "near" | "reached"; usedPercent: number | null; resetsAt: string | null }
  | { type: "turn_done"; ok: boolean; error: string | null }
  | { type: "exit"; error: string | null };

export type StartOptions = {
  cwd: string;
  extraDirs: string[];
  model: string;
  instructions: string;
  autonomy: Autonomy;
  allowRules: string[];
  resumeId: string | null;
  env: Record<string, string>;
};

// One running CLI conversation. Claude and Codex each have an adapter that implements this.
export interface ProviderSession {
  start(): Promise<void>;
  send(text: string): Promise<void>;
  steer(text: string): Promise<boolean>; // false: couldn't add it to the current turn, so the caller queues it
  interrupt(): Promise<void>;
  answer(approvalId: string, answer: ApprovalAnswer): void;
  close(): void;
  pid(): number | undefined;
  onEvent(listener: (event: ProviderEvent) => void): void;
}
