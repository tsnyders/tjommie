import { afterEach, expect, it, vi } from "vitest";
import type { VaultHostCommand } from "../../protocol/src/vault.ts";
import { VaultHostBroker } from "./vault-host-broker-fixture.ts";
import { startHostServer, type HostServer } from "./host-server-fixture.ts";
import { startVaultHost } from "./vault-host.ts";
import type { ServerResponse } from "node:http";

const token = "v".repeat(43), locked = { state: "locked", reason: null, pendingDeletes: 0 };
let server: HostServer | undefined, broker: VaultHostBroker | undefined, host: ReturnType<typeof startVaultHost> | undefined;
afterEach(async () => { await host?.close(); host = undefined; broker?.close(); await server?.close(); vi.restoreAllMocks(); });
async function fixture(execute: (command: VaultHostCommand) => Promise<unknown>) {
  broker = new VaultHostBroker(token, 1000); server = await startHostServer({ "/vault-host": broker.handle }); host = startVaultHost({ port: server.port, token, execute });
}
it("executes global actions and returns only validated metadata or a fixed failure", async () => {
  await fixture(async ({ action }) => { if (action.kind === "entry") throw new Error("SYNTHETIC_SECRET_ERROR"); return locked; });
  await expect(broker!.request({ controllerId: "local", action: { kind: "status" } })).resolves.toEqual(locked);
  await expect(broker!.request({ controllerId: "local", action: { kind: "entry", botId: "one" } })).rejects.toThrow("could not complete");
});
it.each(["lock", "disconnect", "cancel"] as const)("delivers urgent %s while an entry window is pending and discards its late result", async (kind) => {
  let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; }), commands: string[] = [];
  await fixture(async ({ action }) => { commands.push(action.kind); if (action.kind === "entry") { await gate; return { status: "cancelled", entry: null }; } if (action.kind === "disconnect" || action.kind === "cancel") { release(); return {}; } if (action.kind === "lock") { release(); return locked; } return locked; });
  const pending = broker!.request({ controllerId: "rpc:one", action: { kind: "entry", botId: "one" } }), rejected = expect(pending).rejects.toThrow("cancelled");
  await vi.waitFor(() => expect(commands).toContain("entry"));
  await expect(broker!.request({ controllerId: "rpc:one", action: { kind } })).resolves.toEqual(kind === "lock" ? locked : {}); await rejected;
  expect(commands.slice(0, 2)).toEqual(["entry", kind]);
});
it("cancels an in-flight operation when its authenticated request is aborted", async () => {
  const abort = new AbortController(), commands: string[] = []; let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; });
  await fixture(async ({ action }) => { commands.push(action.kind); if (action.kind === "entry") { await gate; return { status: "cancelled", entry: null }; } if (action.kind === "cancel") { release(); return {}; } release(); return locked; });
  const rejected = expect(broker!.request({ controllerId: "rpc:one", action: { kind: "entry", botId: "one" } }, { signal: abort.signal })).rejects.toThrow("cancelled");
  await vi.waitFor(() => expect(commands).toContain("entry")); abort.abort(); await rejected; await vi.waitFor(() => expect(commands).toContain("cancel"));
});
it("locks on host close before waiting for a pending native operation", async () => {
  let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; }), commands: string[] = [];
  await fixture(async ({ action }) => { commands.push(action.kind); if (action.kind === "entry") { await gate; return { status: "cancelled", entry: null }; } release(); return locked; });
  const pending = broker!.request({ controllerId: "rpc:one", action: { kind: "entry", botId: "one" } }).catch(() => {});
  await vi.waitFor(() => expect(commands).toContain("entry")); await host!.close(); expect(commands).toContain("lock"); broker!.close(); await pending;
});

it("rejects an entry job whose body arrives after its controller's newer cancel barrier", async () => {
  const oldId = "41f8bac7-358c-48cf-9f48-a3a358804da6", cancelId = "83c70656-e163-478e-a934-6a5801f5ef18";
  let normal: ServerResponse | undefined, sawCancel = false, sawStale = false;
  const commands: string[] = [];
  server = await startHostServer({ "/vault-host": async (req, res) => {
    if (req.method === "GET") {
      if (req.url?.includes("control=1")) {
        if (sawCancel) return;
        sawCancel = true; res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ id: cancelId, sequence: 2, command: { controllerId: "rpc:one", action: { kind: "cancel" } } }));
      } else if (!normal) normal = res;
      return;
    }
    let body = ""; for await (const chunk of req) body += chunk.toString(); const reply = JSON.parse(body) as { id: string; ok: boolean }; res.writeHead(204).end();
    if (reply.id === cancelId) {
      await vi.waitFor(() => expect(normal).toBeDefined()); normal!.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ id: oldId, sequence: 1, command: { controllerId: "rpc:one", action: { kind: "entry", botId: "one" } } }));
    } else if (reply.id === oldId) { expect(reply.ok).toBe(false); sawStale = true; }
  } });
  host = startVaultHost({ port: server.port, token, execute: async ({ action }) => { commands.push(action.kind); return action.kind === "cancel" ? {} : locked; } });
  await vi.waitFor(() => expect(sawStale).toBe(true)); expect(commands).toEqual(["cancel"]);
});
