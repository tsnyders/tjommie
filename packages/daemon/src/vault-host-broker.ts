import { randomUUID, timingSafeEqual } from "node:crypto";
import type { ServerResponse } from "node:http";
import { VAULT_HOST_BODY_BYTES, VaultHostReply, vaultHostResult, type VaultHostJob } from "../../protocol/src/vault-host.ts";
import { VaultHostCommand, type VaultHostCommand as Command } from "../../protocol/src/vault.ts";
import type { Route } from "./server.ts";

export type VaultRequestOptions = { signal?: AbortSignal; authorized?: () => boolean };
export type VaultHostRequest = (command: Command, options?: VaultRequestOptions) => Promise<unknown>;
type Pending = { job: VaultHostJob; sent: boolean; promise: Promise<unknown>; resolve(value: unknown): void; reject(error: Error): void; timer: NodeJS.Timeout; release(): void; authorized(): boolean };
const priority = (command: Command): boolean => ["lock", "disconnect", "cancel"].includes(command.action.kind);
const unavailable = () => new Error("Logins needs its protected desktop host. Open or restart Tjommie and try again.");
const cancelled = () => new Error("This Logins operation was cancelled. Any private browser remains private until its fields are cleared or it is closed.");

/** Independent native-host capability; this channel never carries a secret or a fabricated bot. */
export class VaultHostBroker {
  readonly #secret: string;
  readonly #timeoutMs: number;
  readonly #pending = new Map<string, Pending>();
  #poll: ServerResponse | null = null;
  #controlPoll: ServerResponse | null = null;
  #closed = false;
  #sequence = 0;
  constructor(secret: string, timeoutMs = 150_000) { this.#secret = secret; this.#timeoutMs = timeoutMs; }
  request: VaultHostRequest = (raw, options = {}) => {
    let command: Command;
    try { command = VaultHostCommand.parse(raw); } catch { return Promise.reject(new Error("Invalid Logins command.")); }
    if (this.#closed || !this.#secret || this.#sequence === Number.MAX_SAFE_INTEGER) return Promise.reject(unavailable());
    if (!priority(command) && (options.signal?.aborted || options.authorized?.() === false)) return Promise.reject(cancelled());
    if (priority(command)) {
      for (const item of [...this.#pending.values()]) if (!priority(item.job.command) && (command.action.kind === "lock" || item.job.command.controllerId === command.controllerId)) this.#reject(item, cancelled());
      const existing = [...this.#pending.values()].find((item) => item.job.command.action.kind === command.action.kind && (command.action.kind === "lock" || item.job.command.controllerId === command.controllerId));
      if (existing) return existing.promise;
    }
    if (this.#pending.size >= 64) return Promise.reject(unavailable());
    const id = randomUUID(); let resolve!: (value: unknown) => void, reject!: (error: Error) => void;
    const promise = new Promise<unknown>((accept, fail) => { resolve = accept; reject = fail; });
    const abort = () => { const item = this.#pending.get(id); if (!item) return; this.#reject(item, cancelled()); if (item.sent) this.#cancel(command.controllerId); this.#dispatch(); };
    const timer = setTimeout(() => { const item = this.#pending.get(id); if (!item) return; this.#reject(item, new Error("The Logins request expired. Check the host before retrying; any private browser remains private.")); if (item.sent && !priority(command)) this.#cancel(command.controllerId); this.#dispatch(); }, this.#timeoutMs);
    timer.unref();
    const item: Pending = { job: { id, sequence: ++this.#sequence, command }, sent: false, promise, resolve, reject, timer, authorized: options.authorized ?? (() => true), release: () => options.signal?.removeEventListener("abort", abort) };
    this.#pending.set(id, item);
    if (!priority(command)) options.signal?.addEventListener("abort", abort, { once: true });
    this.#dispatch(); return promise;
  };
  #disconnect(controllerId: string): void { void this.request({ controllerId, action: { kind: "disconnect" } }).catch(() => {}); }
  #cancel(controllerId: string): void { void this.request({ controllerId, action: { kind: "cancel" } }).catch(() => {}); }
  #remove(item: Pending): void { clearTimeout(item.timer); item.release(); this.#pending.delete(item.job.id); }
  #reject(item: Pending, error: Error): void { this.#remove(item); item.reject(error); }
  #send(item: Pending, response: ServerResponse): void {
    item.sent = true;
    response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" }).end(JSON.stringify(item.job));
  }
  #dispatch(): void {
    if (this.#closed) return;
    for (const item of [...this.#pending.values()]) if (!priority(item.job.command) && !item.authorized()) { this.#reject(item, cancelled()); if (item.sent) this.#disconnect(item.job.command.controllerId); }
    const items = [...this.#pending.values()], control = items.find((item) => !item.sent && priority(item.job.command));
    if (control && this.#controlPoll) { const response = this.#controlPoll; this.#controlPoll = null; this.#send(control, response); }
    if (items.some((item) => priority(item.job.command) || item.sent)) return;
    const normal = items.find((item) => !item.sent);
    if (normal && this.#poll) { const response = this.#poll; this.#poll = null; this.#send(normal, response); }
  }
  handle: Route = async (req, res) => {
    if (req.headers.origin !== undefined) { res.writeHead(403).end(); return; }
    const given = req.headers.authorization?.startsWith("Bearer ") ? req.headers.authorization.slice(7) : "", actual = Buffer.from(this.#secret), candidate = Buffer.from(given);
    if (!actual.length || actual.length !== candidate.length || !timingSafeEqual(actual, candidate)) { res.writeHead(401).end(); return; }
    if (this.#closed) { res.writeHead(503).end(); return; }
    if (req.method === "GET") {
      const control = req.url?.endsWith("?control=1") === true;
      if (control) { this.#controlPoll?.writeHead(204).end(); this.#controlPoll = res; } else { this.#poll?.writeHead(204).end(); this.#poll = res; }
      const clear = () => { if (this.#poll === res) this.#poll = null; if (this.#controlPoll === res) this.#controlPoll = null; };
      const timer = setTimeout(() => { clear(); if (!res.writableEnded) res.writeHead(204).end(); }, 15_000); timer.unref();
      res.once("close", () => { clearTimeout(timer); clear(); }); this.#dispatch(); return;
    }
    if (req.method !== "POST") { res.writeHead(405).end(); return; }
    const chunks: Buffer[] = []; let size = 0;
    try { for await (const chunk of req) { if (!Buffer.isBuffer(chunk) || (size += chunk.length) > VAULT_HOST_BODY_BYTES) { res.writeHead(413).end(); return; } chunks.push(chunk); } }
    catch { if (!res.writableEnded) res.writeHead(400).end(); return; }
    let raw: unknown; try { raw = JSON.parse(Buffer.concat(chunks).toString("utf8")); } catch { res.writeHead(400).end(); return; }
    const reply = VaultHostReply.safeParse(raw); if (!reply.success) { res.writeHead(400).end(); return; }
    const item = this.#pending.get(reply.data.id); if (!item?.sent) { res.writeHead(404).end(); return; }
    this.#remove(item);
    if (!priority(item.job.command) && !item.authorized()) item.reject(cancelled());
    else if (!reply.data.ok) item.reject(new Error("The protected desktop host could not complete this Logins operation."));
    else {
      try { item.resolve(vaultHostResult(item.job.command.action, reply.data.result)); }
      catch { item.reject(new Error("The desktop returned an invalid Logins response.")); }
    }
    res.writeHead(204).end(); this.#dispatch();
  };
  close(): void {
    this.#closed = true; this.#poll?.writeHead(503).end(); this.#poll = null; this.#controlPoll?.writeHead(503).end(); this.#controlPoll = null;
    for (const item of [...this.#pending.values()]) this.#reject(item, unavailable());
  }
}
