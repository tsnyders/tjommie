import { randomUUID } from "node:crypto";
import { createServer, type Server } from "node:http";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RemoteWorkerIsolation } from "../../desktop/src/worker-isolation.ts";
import { WorkerBoundaryHost } from "./worker-boundary-host.ts";

const servers: Server[] = [];
afterEach(async () => { for (const server of servers.splice(0)) { server.closeAllConnections(); await new Promise<void>((done) => server.close(() => done())); } });
async function fixture() {
  let generation = 0, available = true;
  const releases = vi.fn();
  const boundary: ConstructorParameters<typeof WorkerBoundaryHost>[0] = {
    unavailableReason: vi.fn(async () => available ? null : "synthetic boundary lost"),
    acquire: vi.fn(async () => {
      const initial = generation;
      return { assert: () => { if (generation !== initial || !available) throw new Error("synthetic lease invalidated"); }, release: releases };
    }),
    invalidate: () => { generation++; },
  };
  const token = `synthetic-${randomUUID()}`, host = new WorkerBoundaryHost(boundary, token);
  const server = createServer((req, res) => { void host.handle(req, res); }); servers.push(server);
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const address = server.address(); if (!address || typeof address === "string") throw new Error("Missing synthetic server port.");
  const url = `http://127.0.0.1:${address.port}/worker-host`;
  const request = (body: unknown, bearer = token) => fetch(url, { method: "POST", headers: { Authorization: `Bearer ${bearer}`, "content-type": "application/json" }, body: JSON.stringify(body) });
  return { boundary, host, request, token, port: address.port, releases, lose: () => { available = false; } };
}

describe("worker isolation lifetime channel", () => {
  it("authenticates host status and leases independently from ordinary RPC", async () => {
    const f = await fixture();
    expect((await f.request({ kind: "acquire" }, "ordinary-rpc-token")).status).toBe(401);
    expect(f.boundary.acquire).not.toHaveBeenCalled();
    const isolation = new RemoteWorkerIsolation(f.port, f.token);
    try {
      expect(await isolation.unavailableReason()).toBeNull();
      const lease = await isolation.acquire(); lease.assert();
      f.host.close();
      await expect(f.request({ kind: "status" })).resolves.toHaveProperty("status", 503);
      expect(f.releases).toHaveBeenCalledTimes(1);
      isolation.close(); expect(() => lease.assert()).toThrow("protected connection");
    } finally { isolation.close(); f.host.close(); }
  });

  it("releases a late acquisition after host close instead of returning a usable lease", async () => {
    const f = await fixture();
    let started!: () => void, finish!: () => void;
    const ready = new Promise<void>((done) => { started = done; });
    vi.mocked(f.boundary.acquire).mockImplementationOnce(async () => {
      started(); await new Promise<void>((done) => { finish = done; });
      return { assert: () => {}, release: f.releases };
    });
    const response = f.request({ kind: "acquire" }); await ready; f.host.close(); finish();
    expect((await response).status).toBe(503); expect(f.releases).toHaveBeenCalledTimes(1);
  });

  it("rechecks isolation on renew and retires the failed lease immediately", async () => {
    const f = await fixture();
    const acquired = await (await f.request({ kind: "acquire" })).json() as { id: string; lifetime: string };
    f.lose(); expect((await f.request({ kind: "renew", id: acquired.id })).status).toBe(503);
    expect(f.releases).toHaveBeenCalledTimes(1);
    expect((await f.request({ kind: "renew", id: acquired.id })).status).toBe(503);
    f.host.close(); expect(f.releases).toHaveBeenCalledTimes(1);
  });

  it("bounds active leases and rejects unknown lifetime lease IDs", async () => {
    const f = await fixture();
    for (let count = 0; count < 8; count++) expect((await f.request({ kind: "acquire" })).status).toBe(200);
    expect((await f.request({ kind: "acquire" })).status).toBe(503);
    expect((await f.request({ kind: "renew", id: randomUUID() })).status).toBe(503);
    f.host.close(); expect(f.releases).toHaveBeenCalledTimes(8);
  });
});
