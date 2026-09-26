import type { Autonomy } from "@tjommie/protocol";
import type { CliCommand } from "../cli.ts";
import { RpcRemoteError, startCodexServer, type CodexServer, type JsonRpcLines } from "./codex-rpc.ts";
import { isCodexMcpToolCall, mapCodexElicitation, mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
import type { ApprovalAnswer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";

type Policy = { approvalPolicy: "untrusted" | "on-request" | "never"; sandbox: "read-only" | "workspace-write" | "danger-full-access" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function turnId(params: unknown): string | null {
  if (!isRecord(params) || !isRecord(params.turn) || typeof params.turn.id !== "string") return null;
  return params.turn.id;
}

function isTurnAlreadyEnded(error: unknown): boolean {
  return error instanceof RpcRemoteError && /(?:^|: )no active turn to interrupt$/.test(error.message);
}

function isTurnNotSteerable(error: unknown): boolean {
  if (!(error instanceof RpcRemoteError) || error.code !== -32600) return false;
  // Codex's turn_steer_inner uses invalid-request errors for these turn-state races.
  return error.message === "no active turn to steer"
    || error.message === "cannot steer a review turn"
    || error.message === "cannot steer a compact turn"
    || /^expected active turn id `[^`\r\n]+` but found `[^`\r\n]+`$/.test(error.message);
}

export function codexPolicy(autonomy: Autonomy): Policy {
  if (autonomy === "ask") return { approvalPolicy: "untrusted", sandbox: "read-only" };
  // Codex 0.155.1's unelevated Windows sandbox reaches loopback with networkAccess false,
  // leaves reads unrestricted, and cannot enforce deny-read rules. See follow-up rounds 1 and 2:
  // .superpowers/sdd/2026-09-26-tjommie-phase1ab-final-review/fix-A-report.md
  if (autonomy === "folder") return { approvalPolicy: "on-request", sandbox: "workspace-write" };
  return { approvalPolicy: "never", sandbox: "danger-full-access" };
}

// Codex features that are on by default and would reach past Tjommie: ChatGPT apps, driving the person's own
// browser and desktop, plugins, and memories shared by every bot on the account. Bots get Tjommie's MCP servers only.
export const CODEX_FEATURES_OFF = {
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

// Per-thread config overrides. Windows read and loopback limits are documented beside codexPolicy above.
// The unelevated Windows sandbox needs no admin rights.
export function codexConfig(options: StartOptions, platform: NodeJS.Platform = process.platform): Record<string, unknown> {
  return {
    sandbox_workspace_write: { network_access: false, writable_roots: options.extraDirs },
    ...(platform === "win32" ? { windows: { sandbox: "unelevated" } } : {}),
    features: CODEX_FEATURES_OFF,
    mcp_servers: Object.fromEntries(
      options.mcpServers.map((server) => [
        server.name,
        "url" in server
          ? { url: server.url, http_headers: server.headers }
          : { command: server.command, args: server.args, env: server.env },
      ]),
    ),
  };
}

type ThreadResponse = { thread: { id: string }; model: string };
type Pending = { rpcId: number | string; answers: "decision" | "action" }; // command and file approvals, or MCP tool calls

// Drives `codex app-server`: one process and one thread per bot session.
export class CodexSession implements ProviderSession {
  #cmd: CliCommand;
  #options: StartOptions;
  #server: CodexServer | null = null;
  #listeners: ((event: ProviderEvent) => void)[] = [];
  #approvals = new Map<string, Pending>();
  #threadId = "";
  #turnId: string | null = null;
  #model = "";
  #closed = false;
  #starting: Promise<void> | null = null;
  #sending: Promise<void> | null = null;
  #exited: Promise<void> = Promise.resolve();

  constructor(cmd: CliCommand, options: StartOptions) {
    this.#cmd = cmd;
    this.#options = options;
  }

  onEvent(listener: (event: ProviderEvent) => void): void {
    this.#listeners.push(listener);
  }

  pid(): number | undefined {
    return this.#server?.child.pid;
  }

  start(): Promise<void> {
    this.#starting = this.#start();
    return this.#starting;
  }

  async #start(): Promise<void> {
    if (this.#closed) throw new Error("The Codex session is closed");
    const server = await startCodexServer(this.#cmd, this.#options.env, this.#options.cwd);
    this.#exited = new Promise<void>((resolve) => {
      if (server.child.exitCode !== null && server.child.exitCode !== undefined || server.child.signalCode) resolve();
      else {
        server.child.once("exit", () => resolve());
        server.child.once("error", () => resolve());
      }
    });
    if (this.#closed) {
      server.close();
      throw new Error("The Codex session is closed");
    }
    this.#server = server;
    server.child.on("exit", (code) => {
      if (this.#server !== server) return; // closed on purpose
      this.#server = null;
      this.#emit({ type: "exit", error: code === 0 || code === null ? null : `Codex exited with code ${code}` });
    });
    server.rpc.onNotification = (method, params) => {
      if (method === "turn/started") this.#turnId = turnId(params);
      if (method === "turn/completed") this.#turnId = null;
      for (const event of mapCodexNotification(method, params, this.#model)) this.#emit(event);
    };
    server.rpc.onRequest = (id, method, params) => {
      if (method === "mcpServer/elicitation/request") return this.#onElicitation(server.rpc, id, params);
      const approval = mapCodexRequest(id, method, params);
      if (!approval) {
        server.rpc.respondError(id, -32601, "Not supported by Tjommie");
        return;
      }
      this.#approvals.set(approval.id, { rpcId: id, answers: "decision" });
      this.#emit(approval);
    };
    const params = {
      cwd: this.#options.cwd,
      model: this.#options.model || null,
      ...codexPolicy(this.#options.autonomy),
      developerInstructions: this.#options.instructions,
      config: codexConfig(this.#options),
    };
    try {
      const response = this.#options.resumeId
        ? await server.rpc.request<ThreadResponse>("thread/resume", { threadId: this.#options.resumeId, ...params })
        : await server.rpc.request<ThreadResponse>("thread/start", params);
      this.#threadId = response.thread.id;
      this.#model = response.model;
    } catch (error) {
      this.close();
      throw error;
    }
    if (this.#closed) throw new Error("The Codex session is closed");
    this.#emit({ type: "session", cliSessionId: this.#threadId });
  }

  async send(text: string): Promise<void> {
    const pending = this.#rpc().request<{ turn: { id: string } }>("turn/start", { threadId: this.#threadId, input: [{ type: "text", text }] })
      .then((response) => { this.#turnId = response.turn.id; });
    this.#sending = pending;
    try {
      await pending;
    } finally {
      if (this.#sending === pending) this.#sending = null;
    }
  }

  async steer(text: string): Promise<boolean> {
    if (!this.#turnId || !this.#server) return false;
    try {
      await this.#rpc().request("turn/steer", { threadId: this.#threadId, expectedTurnId: this.#turnId, input: [{ type: "text", text }] });
      return true;
    } catch (error) {
      if (isTurnNotSteerable(error)) return false; // the caller queues work that cannot join this turn
      throw error; // the runner logs unexpected RPC and transport failures
    }
  }

  async interrupt(): Promise<void> {
    await this.#sending;
    if (!this.#turnId || !this.#server) return;
    try {
      await this.#rpc().request("turn/interrupt", { threadId: this.#threadId, turnId: this.#turnId });
    } catch (error) {
      if (isTurnAlreadyEnded(error)) return;
      console.error("Codex turn interrupt failed", error);
      throw error;
    }
  }

  answer(approvalId: string, answer: ApprovalAnswer): void {
    const pending = this.#approvals.get(approvalId);
    if (pending === undefined || !this.#server) return;
    this.#approvals.delete(approvalId);
    // The daemon remembers "Always allow" per bot and answers matching requests itself, so Codex only
    // ever gets yes or no.
    const yes = answer !== "deny";
    this.#server.rpc.respond(pending.rpcId, pending.answers === "action" ? { action: yes ? "accept" : "decline" } : { decision: yes ? "accept" : "decline" });
  }

  close(): void {
    this.#closed = true;
    const server = this.#server;
    this.#server = null;
    server?.close();
  }

  async waitForExit(): Promise<void> {
    try { await this.#starting; } catch { /* A failed start still closes its process. */ }
    await this.#exited;
  }

  // Codex asks before every MCP tool call. Tjommie's own servers marked autoAllow are answered at once; other
  // tool calls become approval cards. Any other question from an MCP server is declined.
  #onElicitation(rpc: JsonRpcLines, id: number | string, params: unknown): void {
    if (!isCodexMcpToolCall(params)) {
      rpc.respond(id, { action: "decline" });
      return;
    }
    const server = this.#options.mcpServers.find((candidate) => candidate.name === params.serverName);
    if (!server) {
      rpc.respond(id, { action: "decline" });
      return;
    }
    if (server.autoAllow) {
      rpc.respond(id, { action: "accept" });
      return;
    }
    const approval = mapCodexElicitation(id, params);
    if (!approval) {
      rpc.respond(id, { action: "decline" });
      return;
    }
    this.#approvals.set(approval.id, { rpcId: id, answers: "action" });
    this.#emit(approval);
  }

  #emit(event: ProviderEvent): void {
    for (const listener of this.#listeners) listener(event);
  }

  #rpc(): JsonRpcLines {
    if (!this.#server) throw new Error("The Codex session isn't running");
    return this.#server.rpc;
  }
}
