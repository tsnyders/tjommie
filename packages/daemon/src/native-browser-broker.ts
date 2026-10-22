import { timingSafeEqual, randomUUID } from "node:crypto";
import type { ServerResponse } from "node:http";
import type { Route } from "./server.ts";

type BotIdentity = { id: string; homeFolder: string; name: string; projectFolder?: string | null };
type Job<C> = { id: string; bot: BotIdentity; command: C };
type Pending<C> = { job: Job<C>; sent: boolean; authorized: () => boolean; resolve(value: unknown): void; reject(error: Error): void; timer: NodeJS.Timeout };
const unavailable = (): Error => new Error("The native browser needs the Tjommie desktop app. Open or restart Tjommie and try again.");

// Only the desktop main process receives this independent capability through its private startup pipe.
// Browser jobs and page results never pass through the renderer's event stream or persistent chat logs.
export class NativeBrowserBroker<C> {
  #secret: string;
  #pending = new Map<string, Pending<C>>();
  #poll: ServerResponse | null = null;
  #controlPoll: ServerResponse | null = null;
  #closed = false;
  #timeoutMs: number;
  constructor(secret: string, timeoutMs = 45_000) { this.#secret = secret; this.#timeoutMs = timeoutMs; }
  request(bot: BotIdentity, command: C, authorized: () => boolean = () => true): Promise<unknown> {
    if (this.#closed || !this.#secret || this.#pending.size >= 64) return Promise.reject(unavailable());
    const id = randomUUID();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const sent = this.#pending.get(id)?.sent;
        this.#pending.delete(id);
        reject(new Error(sent ? "The native browser command timed out; its outcome is unknown. Check the page before retrying." : "The native browser command was not started because the desktop host did not become available in time."));
        this.#dispatch();
      }, this.#timeoutMs);
      this.#pending.set(id, { job: { id, bot: { id: bot.id, name: bot.name, homeFolder: bot.homeFolder, ...(bot.projectFolder === undefined ? {} : { projectFolder: bot.projectFolder }) }, command }, sent: false, authorized, resolve, reject, timer });
      this.#dispatch();
    });
  }
  #dispatch(): void {
    if (!this.#poll && !this.#controlPoll) return;
    for (const [id, item] of this.#pending) if (!item.sent && !item.authorized()) {
      clearTimeout(item.timer); this.#pending.delete(id); item.reject(new Error("Browser session is no longer authorized."));
    }
    const jobs = [...this.#pending.values()];
    const priority = (item: Pending<C>): boolean => typeof item.job.command === "object" && item.job.command !== null && "kind" in item.job.command && item.job.command.kind === "cancel_tools";
    const control = this.#controlPoll && jobs.find((item) => !item.sent && priority(item));
    if (control) { const response = this.#controlPoll!; this.#controlPoll = null; this.#send(control, response); }
    const pending = this.#poll && jobs.find((item) => !item.sent && !priority(item) && !jobs.some((other) => other !== item && other.job.bot.id === item.job.bot.id && (other.sent || priority(other))));
    if (!pending || !this.#poll) return;
    const response = this.#poll;
    this.#poll = null;
    this.#send(pending, response);
  }
  #send(pending: Pending<C>, response: ServerResponse): void {
    pending.sent = true;
    response.once("close", () => {
      if (!response.writableFinished && this.#pending.has(pending.job.id)) { pending.sent = false; this.#dispatch(); }
    });
    response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" }).end(JSON.stringify(pending.job));
  }
  handle: Route = async (req, res) => {
    const header = req.headers.authorization ?? "";
    const given = header.startsWith("Bearer ") ? header.slice(7) : "";
    const actual = Buffer.from(this.#secret), candidate = Buffer.from(given);
    if (!actual.length || candidate.length !== actual.length || !timingSafeEqual(actual, candidate)) { res.writeHead(401).end(); return; }
    if (this.#closed) { res.writeHead(503).end(); return; }
    if (req.method === "GET") {
      // One outstanding poll per desktop host. A new poll replaces a disconnected or stale one.
      const control = req.url?.endsWith("?control=1") === true;
      if (control) { this.#controlPoll?.writeHead(204).end(); this.#controlPoll = res; }
      else { this.#poll?.writeHead(204).end(); this.#poll = res; }
      const clear = (): void => { if (this.#poll === res) this.#poll = null; if (this.#controlPoll === res) this.#controlPoll = null; };
      const timer = setTimeout(() => { clear(); if (!res.writableEnded) res.writeHead(204).end(); }, 15_000);
      res.once("close", () => { clearTimeout(timer); clear(); });
      this.#dispatch();
      return;
    }
    if (req.method !== "POST") { res.writeHead(405).end(); return; }
    const chunks: Buffer[] = [];
    let bytes = 0;
    for await (const chunk of req) {
      if (!Buffer.isBuffer(chunk) || (bytes += chunk.length) > 8 * 1024 * 1024) { res.writeHead(413).end(); return; }
      chunks.push(chunk);
    }
    let value: unknown;
    try { value = JSON.parse(Buffer.concat(chunks).toString("utf8")); } catch { res.writeHead(400).end(); return; }
    if (typeof value !== "object" || value === null || !("id" in value) || typeof value.id !== "string" || !("ok" in value) || typeof value.ok !== "boolean") { res.writeHead(400).end(); return; }
    const pending = this.#pending.get(value.id);
    if (!pending?.sent) { res.writeHead(404).end(); return; }
    this.#pending.delete(value.id);
    clearTimeout(pending.timer);
    this.#dispatch();
    if (value.ok) pending.resolve("result" in value ? value.result : null);
    else pending.reject(new Error("error" in value && typeof value.error === "string" ? value.error.slice(0, 2000) : "The native browser command failed."));
    res.writeHead(204).end();
  };
  closeBot(botId: string): void {
    for (const [id, pending] of this.#pending) if (pending.job.bot.id === botId) {
      clearTimeout(pending.timer); this.#pending.delete(id); pending.reject(new Error("This bot's native browser closed."));
    }
  }
  cancelTools(botId: string): void {
    for (const [id, pending] of this.#pending) if (pending.job.bot.id === botId && typeof pending.job.command === "object" && pending.job.command !== null && "kind" in pending.job.command && pending.job.command.kind === "tool") {
      clearTimeout(pending.timer); this.#pending.delete(id); pending.reject(new Error("The browser tool session closed."));
    }
  }
  close(): void {
    this.#closed = true;
    this.#poll?.writeHead(503).end(); this.#poll = null;
    this.#controlPoll?.writeHead(503).end(); this.#controlPoll = null;
    for (const pending of this.#pending.values()) { clearTimeout(pending.timer); pending.reject(unavailable()); }
    this.#pending.clear();
  }
}
