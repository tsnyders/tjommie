import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import { killTree, spawnCli, type CliCommand } from "../cli.ts";

export class RpcRemoteError extends Error {
  code: number;

  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
}

type Pending = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isRpcId(value: unknown): value is number | string {
  return typeof value === "number" || typeof value === "string";
}

// JSON-RPC over newline-delimited JSON, as spoken by `codex app-server` (it omits the "jsonrpc" field).
export class JsonRpcLines {
  onNotification: (method: string, params: unknown) => void = () => {};
  onRequest: (id: number | string, method: string, params: unknown) => void = () => {};
  #nextId = 1;
  #pending = new Map<number, Pending>();
  #write: (line: string) => void;

  constructor(write: (line: string) => void) {
    this.#write = write;
  }

  request<T = unknown>(method: string, params: unknown = {}, timeoutMs = 120_000): Promise<T> {
    const id = this.#nextId++;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeoutMs);
      this.#pending.set(id, { resolve: (value) => resolve(value as T), reject, timer });
      try {
        this.#write(JSON.stringify({ id, method, params }));
      } catch (error) {
        this.#pending.delete(id);
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  notify(method: string, params?: unknown): void {
    this.#write(JSON.stringify(params === undefined ? { method } : { method, params }));
  }

  respond(id: number | string, result: unknown): void {
    this.#write(JSON.stringify({ id, result }));
  }

  respondError(id: number | string, code: number, message: string): void {
    this.#write(JSON.stringify({ id, error: { code, message } }));
  }

  handleLine(line: string): void {
    let message: unknown;
    try {
      message = JSON.parse(line) as unknown;
    } catch {
      return; // stray log output
    }
    if (!isRecord(message)) return;

    const { id, method, params } = message;
    if (typeof method === "string" && isRpcId(id)) {
      this.onRequest(id, method, params);
      return;
    }
    if (typeof method === "string") {
      this.onNotification(method, params);
      return;
    }
    if (typeof id !== "number") return;

    const pending = this.#pending.get(id);
    if (!pending) return;
    this.#pending.delete(id);
    clearTimeout(pending.timer);
    const remoteError = message.error;
    if (remoteError !== undefined && remoteError !== null) {
      const code = isRecord(remoteError) && typeof remoteError.code === "number" ? remoteError.code : 0;
      const text =
        isRecord(remoteError) && typeof remoteError.message === "string"
          ? remoteError.message
          : (JSON.stringify(remoteError) ?? String(remoteError));
      pending.reject(new RpcRemoteError(code, text));
    } else {
      pending.resolve(message.result);
    }
  }

  failAll(error: Error): void {
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.#pending.clear();
  }
}

export type CodexServer = {
  rpc: JsonRpcLines;
  child: ChildProcessWithoutNullStreams;
  close: () => void;
};

// Starts `codex app-server` with the given environment (CODEX_HOME = the account folder) and completes
// the initialize handshake.
export async function startCodexServer(
  cmd: CliCommand,
  env: Record<string, string>,
  cwd: string,
): Promise<CodexServer> {
  const child = spawnCli(cmd, ["app-server"], { cwd, env });
  const rpc = new JsonRpcLines((line) => child.stdin.write(`${line}\n`));
  createInterface({ input: child.stdout, crlfDelay: Infinity }).on("line", (line) => rpc.handleLine(line));
  child.stderr.resume(); // Codex logs to stderr; drain it so a full pipe can never block the process
  let exited = false;
  let killTimer: NodeJS.Timeout | undefined;
  child.on("exit", () => {
    exited = true;
    if (killTimer !== undefined) {
      clearTimeout(killTimer);
      killTimer = undefined;
    }
    rpc.failAll(new Error("codex app-server exited"));
  });
  child.on("error", (error) => rpc.failAll(error));
  child.stdin.on("error", (error) => rpc.failAll(error));
  const close = () => {
    if (exited || killTimer !== undefined) return;
    child.stdin.end();
    if (exited) return;
    killTimer = setTimeout(() => {
      killTimer = undefined;
      if (!exited && child.exitCode === null && child.signalCode === null) killTree(child.pid);
    }, 3000);
    killTimer.unref();
  };
  try {
    await rpc.request(
      "initialize",
      { clientInfo: { name: "tjommie", title: "Tjommie", version: "0.1.0" } },
      30_000,
    );
    rpc.notify("initialized");
  } catch (error) {
    close();
    throw error;
  }
  return { rpc, child, close };
}
