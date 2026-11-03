import { randomUUID } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RemoteWorkerIsolation } from "./worker-isolation.ts";

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });
const response = (value: unknown): Response => new Response(JSON.stringify(value), { status: 200, headers: { "content-type": "application/json" } });
const reply = () => ({ lifetime: randomUUID(), id: randomUUID(), expiresInMs: 5000 });

describe("desktop worker lease expiry", () => {
  it("does not revive an expired lease from a delayed heartbeat", async () => {
    vi.useFakeTimers();
    const acquired = reply(); let finish!: (value: Response) => void;
    const request = vi.fn().mockResolvedValueOnce(response(acquired)).mockImplementationOnce(() => new Promise<Response>((done) => { finish = done; }));
    vi.stubGlobal("fetch", request);
    const isolation = new RemoteWorkerIsolation(12345, "synthetic-private-capability"), lease = await isolation.acquire();
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(5000);
    finish(response(acquired)); await vi.advanceTimersByTimeAsync(0);
    expect(() => lease.assert()).toThrow("protected connection"); isolation.close();
  });

  it("rejects a heartbeat from a replacement daemon lifetime", async () => {
    vi.useFakeTimers(); const acquired = reply();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(response(acquired)).mockResolvedValueOnce(response({ ...acquired, lifetime: randomUUID() })));
    const isolation = new RemoteWorkerIsolation(12345, "synthetic-private-capability"), lease = await isolation.acquire();
    await vi.advanceTimersByTimeAsync(1000);
    expect(() => lease.assert()).toThrow(); isolation.close();
  });

  it("refuses a late acquire or status response after desktop lifecycle close", async () => {
    let finish!: (value: Response) => void;
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>((done) => { finish = done; })));
    const isolation = new RemoteWorkerIsolation(12345, "synthetic-private-capability");
    const pending = isolation.acquire(); isolation.close(); finish(response(reply()));
    await expect(pending).rejects.toThrow("protected connection");
    expect(await isolation.unavailableReason()).not.toBeNull();
  });
});
