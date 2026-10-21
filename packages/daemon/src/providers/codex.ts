import type { Autonomy } from "@tjommie/protocol";
import { killTreeAndWait, type CliCommand } from "../cli.ts";
import { RpcRemoteError, startCodexServer, type CodexServer, type JsonRpcLines } from "./codex-rpc.ts";
import { isCodexMcpToolCall, mapCodexElicitation, mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
import type { ApprovalAnswer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";

type Policy = { approvalPolicy: "untrusted" | "on-request" | "never"; sandbox: "read-only" | "workspace-write" | "danger-full-access" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function turnId(params: unknown): string | null {
  if (!isRecord(params)) return null;
  if (typeof params.turnId === "string") return params.turnId;
  return isRecord(params.turn) && typeof params.turn.id === "string" ? params.turn.id : null;
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

// Per-thread config overrides. The daemon gates Own folder before this session starts.
export function codexConfig(options: StartOptions, platform: NodeJS.Platform = process.platform): Record<string, unknown> {
  const windowsSandbox = options.autonomy === "folder" || (options.autonomy === "ask" && options.codexSandboxReady)
    ? "elevated" : "unelevated";
  return {
    sandbox_workspace_write: { network_access: false, writable_roots: options.extraDirs },
    ...(platform === "win32" ? { windows: { sandbox: windowsSandbox } } : {}),
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
  #completedTurns = new Set<string>();
  #model = "";
  #closed = false;
  #starting: Promise<void> | null = null;
  #sending: Promise<void> | null = null;
  #interrupting: Promise<boolean> | null = null;
  #stopping = false;
  #stoppedTurnId: string | null = null;
  #stopCompletion: { ok: boolean; error: string | null } | null = null;
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
      if (this.#stopping) return; // interrupt() publishes exit after the whole tree has stopped
      this.#emit({ type: "exit", error: code === 0 || code === null ? null : `Codex exited with code ${code}` });
    });
    server.rpc.onNotification = (method, params) => {
      const id = turnId(params);
      // An approved command can emit item/started and item/completed AFTER its interrupted
      // turn/completed. Do not let that old work look like a new background turn to the runner.
      // Usage notifications may still arrive late and must remain billable.
      if (id && this.#completedTurns.has(id) && (method.startsWith("item/") || method.startsWith("turn/"))) return;
      if (method === "turn/started" && id) this.#turnId = id;
      if (method === "turn/completed") {
        const completedId = id ?? this.#turnId;
        if (completedId) this.#completedTurns.add(completedId);
        if (this.#turnId !== null && this.#turnId !== completedId) return;
        this.#turnId = null;
        if (this.#stopping) {
          this.#stoppedTurnId = completedId;
          const completion = mapCodexNotification(method, params, this.#model).find((event) => event.type === "turn_done");
          if (completion) this.#stopCompletion = { ok: completion.ok, error: completion.error };
          return; // the interrupted command can still start; wait for process-tree termination
        }
      }
      for (const event of mapCodexNotification(method, params, this.#model)) this.#emit(event);
    };
    server.rpc.onRequest = (id, method, params) => {
      const requestTurn = turnId(params);
      const completed = requestTurn !== null && this.#completedTurns.has(requestTurn);
      if (method === "mcpServer/elicitation/request") {
        if (completed) server.rpc.respond(id, { action: "decline" });
        else this.#onElicitation(server.rpc, id, params);
        return;
      }
      const approval = mapCodexRequest(id, method, params);
      if (!approval) {
        server.rpc.respondError(id, -32601, "Not supported by Tjommie");
        return;
      }
      if (completed) {
        server.rpc.respond(id, { decision: "decline" });
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
      .then((response) => {
        // Notifications can finish this turn (and start another) before its start response.
        const id = response.turn.id;
        if (!this.#completedTurns.has(id) && (this.#turnId === null || this.#turnId === id)) this.#turnId = id;
      });
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

  async interrupt(): Promise<boolean> {
    if (this.#interrupting) return this.#interrupting;
    const pending = this.#interrupt();
    this.#interrupting = pending;
    try {
      return await pending;
    } finally {
      if (this.#interrupting === pending) this.#interrupting = null;
    }
  }

  async #interrupt(): Promise<boolean> {
    const ownedServer = this.#server;
    this.#stopping = true;
    try {
      await this.#sending;
      const server = this.#server;
      const id = this.#turnId ?? this.#stoppedTurnId;
      if (!server) {
        if (!ownedServer || this.#closed) return false;
        await this.#exited;
        throw new Error("Codex exited before its process tree could be stopped.");
      }
      if (!id && !this.#stopCompletion) return false;
      this.#stoppedTurnId = id;
      if (id) {
        try {
          await server.rpc.request("turn/interrupt", { threadId: this.#threadId, turnId: id });
        } catch (error) {
          if (!isTurnAlreadyEnded(error) && !this.#completedTurns.has(id) && this.#server === server) throw error;
        }
      }
      // Codex can launch an approved command after acknowledging interruption. Retire this
      // owned process tree; the runner resumes the saved thread for its next queued item.
      if (this.#server !== server) throw new Error("Codex exited before its process tree could be stopped.");
      await killTreeAndWait(server.child.pid);
      await this.#exited;
      this.#finishStop();
      return true;
    } catch (error) {
      // An exit can reject send/interrupt, or precede a failed taskkill. Its callback was
      // held above, so publish the failure instead of leaving the runner busy forever.
      if (ownedServer && this.#server !== ownedServer && !this.#closed) {
        this.#closed = true;
        this.#emit({ type: "exit", error: error instanceof Error ? error.message : String(error) });
      }
      throw error;
    } finally {
      this.#stopping = false;
    }
  }

  #finishStop(): void {
    this.#closed = true;
    this.#stoppedTurnId = null;
    const completedTurn = this.#stopCompletion?.ok ? this.#stopCompletion : null;
    this.#stopCompletion = null;
    this.#emit({ type: "exit", error: null, ...(completedTurn ? { completedTurn } : {}) });
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
