import { spawnSync, type ChildProcessWithoutNullStreams } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import type { Autonomy } from "@tjommie/protocol";
import { killTree, spawnCli, type CliCommand } from "../cli.ts";
import { parseClaudeLine, toolSummary } from "./claude-parse.ts";
import type { ApprovalAnswer, McpServer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";

const INTERRUPT_GRACE_MS = 10_000;

export function claudeMode(autonomy: Autonomy): "manual" | "acceptEdits" | "bypassPermissions" {
  return autonomy === "ask" ? "manual" : autonomy === "folder" ? "acceptEdits" : "bypassPermissions";
}

type SessionFiles = { instructions: string; mcp: string };

export function claudeArgs(options: StartOptions, files: SessionFiles): string[] {
  const args = [
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--permission-prompt-tool",
    "stdio",
    "--permission-mode",
    claudeMode(options.autonomy),
    "--model",
    options.model,
    "--append-system-prompt-file",
    files.instructions,
    "--strict-mcp-config",
  ];
  if (options.mcpServers.length) args.push("--mcp-config", files.mcp);
  for (const dir of options.extraDirs) args.push("--add-dir", dir);
  if (options.resumeId) args.push("--resume", options.resumeId);
  const allowed = [...options.mcpServers.filter((server) => server.autoAllow).map((server) => `mcp__${server.name}`), ...options.allowRules];
  if (allowed.length) args.push("--allowedTools", ...allowed); // takes several values, so it goes last
  return args;
}

export function claudeMcpConfig(servers: McpServer[]): { mcpServers: Record<string, unknown> } {
  return {
    mcpServers: Object.fromEntries(
      servers.map((server) => [
        server.name,
        "url" in server
          ? { type: "http", url: server.url, headers: server.headers }
          : { type: "stdio", command: server.command, args: server.args, env: server.env },
      ]),
    ),
  };
}

type PendingApproval = { input: Record<string, unknown>; toolName: string; suggestion: Record<string, unknown> | null };

// Drives `claude -p` over stream-json: one process per bot session; turns go to stdin as JSON lines.
export class ClaudeSession implements ProviderSession {
  #cmd: CliCommand;
  #options: StartOptions;
  #child: ChildProcessWithoutNullStreams | null = null;
  #listeners: ((event: ProviderEvent) => void)[] = [];
  #pending = new Map<string, PendingApproval>();
  #lastTotalCost: number | null;
  #cliSessionId: string | null;
  #streamed = "";
  #inTurn = false;
  #closing = false;
  #interruptTimer: NodeJS.Timeout | undefined;
  #closeTimer: NodeJS.Timeout | undefined;
  #controlId = 0;
  #filesDir: string | null = null;
  #exitPromise: Promise<void> = Promise.resolve();
  #resolveExit: (() => void) | null = null;

  constructor(cmd: CliCommand, options: StartOptions) {
    this.#cmd = cmd;
    this.#options = options;
    this.#lastTotalCost = options.resumeId === null ? 0 : options.resumeCostUsd ?? null;
    this.#cliSessionId = options.resumeId;
  }

  onEvent(listener: (event: ProviderEvent) => void): void {
    this.#listeners.push(listener);
  }

  pid(): number | undefined {
    return this.#child?.pid;
  }

  async start(): Promise<void> {
    const dir = mkdtempSync(join(this.#options.env.CLAUDE_CONFIG_DIR ?? this.#options.cwd, ".tjommie-session-"));
    this.#filesDir = dir;
    let child: ChildProcessWithoutNullStreams;
    try {
      restrictSessionFolder(dir);
      const files = { instructions: join(dir, "instructions.txt"), mcp: join(dir, "mcp.json") };
      writeFileSync(files.instructions, this.#options.instructions, { mode: 0o600 });
      if (this.#options.mcpServers.length) writeFileSync(files.mcp, JSON.stringify(claudeMcpConfig(this.#options.mcpServers)), { mode: 0o600 });
      child = spawnCli(this.#cmd, claudeArgs(this.#options, files), { cwd: this.#options.cwd, env: this.#options.env });
    } catch (error) {
      this.#removeFiles();
      throw error;
    }
    this.#child = child;
    this.#exitPromise = new Promise((resolve) => { this.#resolveExit = resolve; });
    let stderr = "";
    child.stdin.on("error", () => {});
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => (stderr = (stderr + chunk).slice(-2000)));
    createInterface({ input: child.stdout, crlfDelay: Infinity }).on("line", (line) => this.#onLine(line));
    child.on("error", (error) => this.#exited(child, this.#closing ? null : error.message));
    child.on("exit", (code, signal) => {
      const status = code === null ? `signal ${signal ?? "unknown"}` : `code ${code}`;
      this.#exited(child, this.#closing || code === 0 ? null : stderr.trim() || `Claude exited with ${status}`);
    });
  }

  async send(text: string): Promise<void> {
    if (!this.#child || this.#closing) return;
    this.#inTurn = true;
    this.#write({ type: "user", message: { role: "user", content: text } });
  }

  // Claude reads a message written mid-turn at its next step, inside the same turn.
  async steer(text: string): Promise<boolean> {
    if (!this.#child || this.#closing) return false;
    this.#write({ type: "user", message: { role: "user", content: text } });
    return true;
  }

  async interrupt(): Promise<void> {
    const child = this.#child;
    if (!child || !this.#inTurn || this.#closing) return;
    this.#write({ type: "control_request", request_id: `tjommie-${++this.#controlId}`, request: { subtype: "interrupt" } });
    clearTimeout(this.#interruptTimer);
    this.#interruptTimer = setTimeout(() => {
      this.#interruptTimer = undefined;
      if (this.#child === child && this.#inTurn && child.exitCode === null && child.signalCode === null) {
        this.#closing = true;
        killTree(child.pid);
      }
    }, INTERRUPT_GRACE_MS);
  }

  answer(approvalId: string, answer: ApprovalAnswer): void {
    if (!this.#child || this.#closing) return;
    const pending = this.#pending.get(approvalId);
    if (!pending) return;
    this.#pending.delete(approvalId);
    const response =
      answer === "deny"
        ? { behavior: "deny", message: "The person declined this in Tjommie." }
        : { behavior: "allow", updatedInput: pending.input, ...(answer === "always" ? { updatedPermissions: [sessionRule(pending)] } : {}) };
    this.#write({ type: "control_response", response: { subtype: "success", request_id: approvalId, response } });
  }

  close(): void {
    this.#removeFiles();
    const child = this.#child;
    if (!child || this.#closing) return;
    this.#closing = true;
    child.stdin.end();
    clearTimeout(this.#closeTimer);
    this.#closeTimer = setTimeout(() => {
      this.#closeTimer = undefined;
      if (this.#child === child && child.exitCode === null && child.signalCode === null) {
        this.#closing = true;
        killTree(child.pid);
      }
    }, 5000);
    this.#closeTimer.unref();
  }

  waitForExit(): Promise<void> {
    return this.#exitPromise;
  }

  #removeFiles(): void {
    if (!this.#filesDir) return;
    try {
      rmSync(this.#filesDir, { recursive: true, force: true });
      this.#filesDir = null;
    } catch (error) {
      // A locked file must not prevent the process from closing. Exit retries this cleanup.
      console.error("Could not remove Claude's session files", error);
    }
  }

  #emit(event: ProviderEvent): void {
    for (const listener of this.#listeners) listener(event);
  }

  #write(message: unknown): void {
    if (!this.#child || this.#closing) return;
    this.#child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  #exited(child: ChildProcessWithoutNullStreams, error: string | null): void {
    if (this.#child !== child) return; // already reported or from a superseded child
    this.#child = null;
    this.#removeFiles();
    this.#resolveExit?.();
    this.#resolveExit = null;
    this.#inTurn = false;
    clearTimeout(this.#interruptTimer);
    clearTimeout(this.#closeTimer);
    this.#interruptTimer = undefined;
    this.#closeTimer = undefined;
    this.#emit({ type: "exit", error });
  }

  #onLine(line: string): void {
    const out = parseClaudeLine(line);
    switch (out.kind) {
      case "init":
        if (this.#cliSessionId !== out.sessionId) this.#lastTotalCost = 0;
        this.#cliSessionId = out.sessionId;
        this.#inTurn = true;
        this.#emit({ type: "session", cliSessionId: out.sessionId });
        return;
      case "text_delta":
        this.#streamed += out.text;
        this.#emit({ type: "text", delta: out.text });
        return;
      case "assistant":
        if (out.texts.length) {
          // Text normally arrives as deltas first. If it didn't, send the whole block now.
          if (!this.#streamed) this.#emit({ type: "text", delta: out.texts.join("") });
          this.#streamed = "";
          this.#emit({ type: "text_end" });
        }
        for (const tool of out.tools) this.#emit({ type: "tool", id: tool.id, name: tool.name, summary: toolSummary(tool.name, tool.input) });
        return;
      case "tool_results":
        for (const result of out.results) this.#emit({ type: "tool_done", id: result.id, ok: result.ok });
        return;
      case "permission":
        this.#pending.set(out.requestId, { input: out.input, toolName: out.toolName, suggestion: out.suggestion });
        this.#emit({
          type: "approval",
          id: out.requestId,
          tool: out.toolName,
          summary: out.description,
          detail: JSON.stringify(out.input, null, 2).slice(0, 4000),
          rule: out.rule,
        });
        return;
      case "rate_limit":
        this.#emit({ type: "limit", state: out.state, usedPercent: out.usedPercent, resetsAt: out.resetsAt });
        return;
      case "result": {
        clearTimeout(this.#interruptTimer);
        this.#interruptTimer = undefined;
        this.#inTurn = false;
        this.#streamed = "";
        // Claude restores cumulative session cost on --resume, including earlier processes.
        // A legacy resumed session has no baseline, so its first turn's cost is unknown.
        const cost = out.totalCostUsd === null || this.#lastTotalCost === null ? null : Math.max(0, out.totalCostUsd - this.#lastTotalCost);
        if (out.totalCostUsd !== null) this.#lastTotalCost = out.totalCostUsd;
        this.#emit({ type: "usage", model: out.model ?? this.#options.model, ...out.usage, costUsd: cost, totalCostUsd: out.totalCostUsd });
        this.#emit({ type: "turn_done", ok: out.ok, error: out.error });
        return;
      }
      case "other":
        return;
    }
  }
}

// Windows ignores POSIX file modes. Remove inherited ACL entries before placing secrets in the folder.
function restrictSessionFolder(folder: string): void {
  if (process.platform !== "win32") {
    chmodSync(folder, 0o700);
    return;
  }
  const identity = spawnSync("whoami", ["/user", "/fo", "csv", "/nh"], { encoding: "utf8", windowsHide: true });
  const sid = identity.stdout?.match(/S-1-[0-9-]+/)?.[0];
  if (!sid) throw new Error("Could not identify the user for Claude's session files.");
  const result = spawnSync("icacls", [folder, "/inheritance:r", "/grant:r", `*${sid}:(OI)(CI)F`], { windowsHide: true });
  if (result.status !== 0) throw new Error("Could not make Claude's session files private.");
}

// "Always allow" also tells this session to stop asking; the daemon saves the rule for later sessions.
function sessionRule(pending: PendingApproval): Record<string, unknown> {
  if (pending.suggestion) return { ...pending.suggestion, destination: "session" };
  return { type: "addRules", rules: [{ toolName: pending.toolName }], behavior: "allow", destination: "session" };
}
