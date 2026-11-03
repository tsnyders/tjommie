import { z } from "zod";
import type { VaultIsolation, VaultIsolationLease } from "./login-vault.ts";

const Status = z.object({ lifetime: z.string().uuid(), available: z.boolean(), reason: z.string().nullable() }).strict();
const Lease = z.object({ lifetime: z.string().uuid(), id: z.string().uuid(), expiresInMs: z.number().int().min(500).max(5000) }).strict();
const unavailable = "Logins needs the verified Windows worker account and a protected connection to its provider host.";
export class RemoteWorkerIsolation implements VaultIsolation {
  readonly #url: string;
  readonly #token: string;
  readonly #leases = new Set<() => void>();
  #closed = false;
  constructor(port: number, privateToken: string) {
    if (!Number.isSafeInteger(port) || port < 1 || port > 65535 || !privateToken) throw new Error(unavailable);
    this.#url = `http://127.0.0.1:${port}/worker-host`; this.#token = privateToken;
  }
  async #request(kind: string, id?: string): Promise<unknown> {
    if (this.#closed) throw new Error(unavailable);
    const response = await fetch(this.#url, { method: "POST", headers: { Authorization: `Bearer ${this.#token}`, "content-type": "application/json" }, body: JSON.stringify({ kind, ...(id ? { id } : {}) }), signal: AbortSignal.timeout(kind === "renew" ? 3000 : 8000), redirect: "error" });
    if (!response.ok) throw new Error(unavailable); return response.json();
  }
  async unavailableReason(): Promise<string | null> {
    try { const status = Status.parse(await this.#request("status")); return status.available && !this.#closed ? null : unavailable; }
    catch { return unavailable; }
  }
  async acquire(): Promise<VaultIsolationLease> {
    const lease = Lease.parse(await this.#request("acquire"));
    if (this.#closed) throw new Error(unavailable);
    let expires = Date.now() + lease.expiresInMs, active = true, renewing = false;
    const finish = (): void => { active = false; clearInterval(timer); this.#leases.delete(finish); };
    const timer = setInterval(() => {
      if (renewing || !active) return;
      if (Date.now() >= expires) { finish(); return; }
      renewing = true;
      void this.#request("renew", lease.id).then((raw) => {
        const updated = Lease.parse(raw);
        if (!active || Date.now() >= expires || updated.lifetime !== lease.lifetime || updated.id !== lease.id) { finish(); return; }
        expires = Date.now() + updated.expiresInMs;
      }).catch(finish).finally(() => { renewing = false; });
    }, 1000); timer.unref(); this.#leases.add(finish);
    return { assert: () => { if (!active || this.#closed || Date.now() >= expires) { finish(); throw new Error(unavailable); } },
      release: () => { if (active) { finish(); void this.#request("release", lease.id).catch(() => {}); } } };
  }
  close(): void { this.#closed = true; for (const finish of [...this.#leases]) finish(); }
}
