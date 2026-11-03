import { afterEach, expect, it } from "vitest";
import { VaultHostBroker } from "./vault-host-broker.ts";
import { startServer, type Server } from "./server.ts";

const token = "v".repeat(43), status = { state: "locked", reason: null, pendingDeletes: 0 };
let server: Server | undefined, broker: VaultHostBroker | undefined;
afterEach(async () => { broker?.close(); await server?.close(); });
async function fixture(timeout = 1000) { broker = new VaultHostBroker(token, timeout); server = await startServer({ port: 0, token: "ordinary-rpc-token", handlers: {}, allowedOrigins: [], routes: { "/vault-host": broker.handle } }); return `http://127.0.0.1:${server.port}/vault-host`; }
const headers = { Authorization: `Bearer ${token}` };
it("requires the independent host capability and rejects browser callers, raw secrets and fake bots", async () => {
  const url = await fixture();
  expect((await fetch(url, { headers: { Authorization: "Bearer ordinary-rpc-token" } })).status).toBe(401);
  expect((await fetch(url, { headers: { ...headers, Origin: "app://tjommie" } })).status).toBe(403);
  await expect(broker!.request({ controllerId: "local", action: { kind: "entry", botId: "one", password: "SYNTHETIC_NEVER_TRANSPORT" } } as unknown as Parameters<VaultHostBroker["request"]>[0])).rejects.toThrow("Invalid Logins");
  await expect(broker!.request({ controllerId: "local", action: { kind: "status" }, bot: { homeFolder: "C:/private" } } as unknown as Parameters<VaultHostBroker["request"]>[0])).rejects.toThrow("Invalid Logins");
});
it("delivers strict global metadata commands once without a StoredBot payload", async () => {
  const url = await fixture(), result = broker!.request({ controllerId: "local", action: { kind: "status" } });
  const job = await (await fetch(url, { headers })).json() as { id: string; command: unknown };
  expect(Object.keys(job).sort()).toEqual(["command", "id", "sequence"]); expect(job.command).toEqual({ controllerId: "local", action: { kind: "status" } });
  const reply = { method: "POST", headers, body: JSON.stringify({ id: job.id, ok: true, result: status }) };
  expect((await fetch(url, reply)).status).toBe(204); await expect(result).resolves.toEqual(status); expect((await fetch(url, reply)).status).toBe(404);
});
it("rejects secret-bearing host responses with a fixed error and bounds the body", async () => {
  const url = await fixture(), pending = broker!.request({ controllerId: "local", action: { kind: "status" } }), failed = expect(pending).rejects.toThrow("invalid Logins response");
  const job = await (await fetch(url, { headers })).json() as { id: string };
  await fetch(url, { method: "POST", headers, body: JSON.stringify({ id: job.id, ok: true, result: { ...status, password: "SYNTHETIC_NEVER_RETURN" } }) }); await failed;
  expect((await fetch(url, { method: "POST", headers, body: "x".repeat(4 * 1024 * 1024 + 1) })).status).toBe(413);
});
it("expires absent-host work and rejects unsent work when its request aborts", async () => {
  await fixture(20); await expect(broker!.request({ controllerId: "local", action: { kind: "status" } })).rejects.toThrow("expired");
  const abort = new AbortController(), failed = expect(broker!.request({ controllerId: "local", action: { kind: "list" } }, { signal: abort.signal })).rejects.toThrow("cancelled"); abort.abort(); await failed;
});

it("repeated cancel also removes newly queued work while the earlier cancel is awaiting the host", async () => {
  const url = await fixture(), first = broker!.request({ controllerId: "rpc:one", action: { kind: "cancel" } });
  const queued = broker!.request({ controllerId: "rpc:one", action: { kind: "entry", botId: "one" } }), rejected = expect(queued).rejects.toThrow("cancelled");
  const repeated = broker!.request({ controllerId: "rpc:one", action: { kind: "cancel" } }); expect(repeated).toBe(first); await rejected;
  const job = await (await fetch(`${url}?control=1`, { headers })).json() as { id: string };
  await fetch(url, { method: "POST", headers, body: JSON.stringify({ id: job.id, ok: true, result: {} }) }); await first;
});
