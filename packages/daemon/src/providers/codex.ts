import type { Autonomy } from "@tjommie/protocol";
import type { CliCommand } from "../cli.ts";
import { startCodexServer, type CodexServer, type JsonRpcLines } from "./codex-rpc.ts";
import { mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
import type { ApprovalAnswer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";

type Policy = { approvalPolicy: "untrusted" | "on-request" | "never"; sandbox: "read-only" | "workspace-write" | "danger-full-access" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function turnId(params: unknown): string | null {
  if (!isRecord(params) || !isRecord(params.turn) || typeof params.turn.id !== "string") return null;
  return params.turn.id;
}

export function codexPolicy(autonomy: Autonomy): Policy {
  if (autonomy === "ask") return { approvalPolicy: "untrusted", sandbox: "read-only" };
  if (autonomy === "folder") return { approvalPolicy: "on-request", sandbox: "workspace-write" };
  return { approvalPolicy: "never", sandbox: "danger-full-access" };
}

// Per-thread config overrides. The unelevated Windows sandbox needs no admin rights. Shell commands still
// can't reach the network inside it (TLS fails), so they ask; bots browse with the browser tool instead.
export function codexConfig(options: StartOptions, platform: NodeJS.Platform = process.platform): Record<string, unknown> {
  return {
    sandbox_workspace_write: { network_access: true, writable_roots: options.extraDirs },
    ...(platform === "win32" ? { windows: { sandbox: "unelevated" } } : {}),
  };
}

type ThreadResponse = { thread: { id: string }; model: string };

// Drives `codex app-server`: one process and one thread per bot session.
export class CodexSession implements ProviderSession {
  #cmd: CliCommand;
  #options: StartOptions;
  #server: CodexServer | null = null;
  #listeners: ((event: ProviderEvent) => void)[] = [];
  #approvals = new Map<string, number | string>();
  #threadId = "";
  #turnId: string | null = null;
  #model = "";

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

  async start(): Promise<void> {
    const server = await startCodexServer(this.#cmd, this.#options.env, this.#options.cwd);
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
      const approval = mapCodexRequest(id, method, params);
      if (!approval) {
        server.rpc.respondError(id, -32601, "Not supported by Tjommie");
        return;
      }
      this.#approvals.set(approval.id, id);
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
    this.#emit({ type: "session", cliSessionId: this.#threadId });
  }

  async send(text: string): Promise<void> {
    const response = await this.#rpc().request<{ turn: { id: string } }>("turn/start", { threadId: this.#threadId, input: [{ type: "text", text }] });
    this.#turnId = response.turn.id;
  }

  async steer(text: string): Promise<boolean> {
    if (!this.#turnId || !this.#server) return false;
    try {
      await this.#rpc().request("turn/steer", { threadId: this.#threadId, expectedTurnId: this.#turnId, input: [{ type: "text", text }] });
      return true;
    } catch {
      return false; // the turn ended in the meantime, so the caller queues the message
    }
  }

  async interrupt(): Promise<void> {
    if (!this.#turnId || !this.#server) return;
    await this.#rpc()
      .request("turn/interrupt", { threadId: this.#threadId, turnId: this.#turnId })
      .catch(() => {});
  }

  answer(approvalId: string, answer: ApprovalAnswer): void {
    const rpcId = this.#approvals.get(approvalId);
    if (rpcId === undefined || !this.#server) return;
    this.#approvals.delete(approvalId);
    // The daemon remembers "Always allow" per bot and answers matching requests itself, so Codex only
    // ever gets accept or decline.
    this.#server.rpc.respond(rpcId, { decision: answer === "deny" ? "decline" : "accept" });
  }

  close(): void {
    const server = this.#server;
    this.#server = null;
    server?.close();
  }

  #emit(event: ProviderEvent): void {
    for (const listener of this.#listeners) listener(event);
  }

  #rpc(): JsonRpcLines {
    if (!this.#server) throw new Error("The Codex session isn't running");
    return this.#server.rpc;
  }
}
