import { randomUUID, timingSafeEqual } from "node:crypto";
import { z } from "zod";
import type { Route } from "./server.ts";
import type { WindowsWorkerBoundary } from "./windows-worker.ts";

const Request = z.object({ kind: z.enum(["status", "acquire", "renew", "release"]), id: z.string().uuid().optional() }).strict();
type Lease = { assert(): void; release(): void; timer: NodeJS.Timeout };
export class WorkerBoundaryHost {
  readonly #boundary: WindowsWorkerBoundary;
  readonly #token: string;
  readonly #lifetime = randomUUID();
  readonly #leases = new Map<string, Lease>();
  #closed = false;
  constructor(boundary: WindowsWorkerBoundary, privateToken: string) { this.#boundary = boundary; this.#token = privateToken; }
  #release(id: string): void { const lease = this.#leases.get(id); if (lease) { clearTimeout(lease.timer); lease.release(); this.#leases.delete(id); } }
  handle: Route = async (req, res) => {
    const candidate = Buffer.from(req.headers.authorization?.replace(/^Bearer /, "") ?? ""), actual = Buffer.from(this.#token);
    if (!actual.length || actual.length !== candidate.length || !timingSafeEqual(actual, candidate)) { res.writeHead(401).end(); return; }
    if (this.#closed) { res.writeHead(503).end(); return; }
    if (req.method !== "POST") { res.writeHead(405).end(); return; }
    let input = "";
    for await (const bytes of req) { input += bytes.toString(); if (input.length > 1024) { res.writeHead(413).end(); return; } }
    const answer = (value: unknown): void => { res.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" }).end(JSON.stringify(value)); };
    try {
      const command = Request.parse(JSON.parse(input));
      if (command.kind === "status") {
        const reason = await this.#boundary.unavailableReason(); answer({ lifetime: this.#lifetime, available: reason === null, reason }); return;
      }
      if (command.kind === "release") { if (command.id) this.#release(command.id); answer({ lifetime: this.#lifetime }); return; }
      if (command.kind === "renew") {
        if (!command.id) throw new Error();
        const lease = this.#leases.get(command.id); if (!lease || await this.#boundary.unavailableReason()) throw new Error(); lease.assert(); clearTimeout(lease.timer);
        lease.timer = setTimeout(() => this.#release(command.id!), 5000); lease.timer.unref();
        answer({ lifetime: this.#lifetime, id: command.id, expiresInMs: 1500 }); return;
      }
      if (this.#leases.size >= 8) throw new Error();
      const lease = await this.#boundary.acquire(), id = randomUUID();
      const timer = setTimeout(() => this.#release(id), 5000); timer.unref(); this.#leases.set(id, { ...lease, timer });
      answer({ lifetime: this.#lifetime, id, expiresInMs: 1500 });
    } catch { res.writeHead(503).end(); }
  };
  close(): void { this.#closed = true; this.#boundary.invalidate(); for (const id of this.#leases.keys()) this.#release(id); }
}
