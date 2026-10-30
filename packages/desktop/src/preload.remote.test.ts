import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { expect, it, vi } from "vitest";

it("exposes the remote IPC methods and forwards relay close codes", async () => {
  const listeners = new Map<string, (_event: unknown, ...values: unknown[]) => void>();
  const invoke = vi.fn(async (_channel: string, ..._args: unknown[]) => ({ id: "wire-1" }));
  const send = vi.fn();
  const removeListener = vi.fn((channel: string) => listeners.delete(channel));
  let exposed: unknown;
  runInNewContext(readFileSync(new URL("./preload.cjs", import.meta.url), "utf8"), {
    require: () => ({
      contextBridge: { exposeInMainWorld: (_name: string, api: unknown) => { exposed = api; } },
      ipcRenderer: { invoke, send, on: (channel: string, handler: (_event: unknown, ...values: unknown[]) => void) => { listeners.set(channel, handler); }, removeListener },
      webUtils: { getPathForFile: () => "" },
    }),
  });
  const remote = (exposed as { remote: {
    open(endpoint: unknown, timeoutMs: number): Promise<{ id: string }>;
    send(id: string, frame: Uint8Array): Promise<void>;
    close(id: string): void;
    onFrame(listener: (id: string, frame: Uint8Array) => void): () => void;
    onClosed(listener: (id: string, code?: number) => void): () => void;
    seal(name: string, value: Uint8Array): Promise<void>;
    read(name: string): Promise<Uint8Array | null>;
    delete(name: string): Promise<void>;
  } }).remote;
  const endpoint = { kind: "lan", host: "127.0.0.1", port: 47101 };
  const frame = new Uint8Array([1, 2]);
  expect(await remote.open(endpoint, 1000)).toEqual({ id: "wire-1" });
  expect(invoke).toHaveBeenCalledWith("remote:open", endpoint, 1000);
  await remote.send("wire-1", frame);
  await remote.seal("device", frame);
  await remote.read("device");
  await remote.delete("device");
  expect(invoke.mock.calls.map(([channel]) => channel)).toEqual(["remote:open", "remote:send", "remote:seal", "remote:read", "remote:delete"]);
  remote.close("wire-1");
  expect(send).toHaveBeenCalledWith("remote:close", "wire-1");
  const onFrame = vi.fn(), onClosed = vi.fn();
  const offFrame = remote.onFrame(onFrame), offClosed = remote.onClosed(onClosed);
  listeners.get("remote:frame")?.({}, "wire-1", frame);
  listeners.get("remote:closed")?.({}, "wire-1", 4404);
  expect(onFrame).toHaveBeenCalledWith("wire-1", frame);
  expect(onClosed).toHaveBeenCalledWith("wire-1", 4404);
  offFrame(); offClosed();
  expect(removeListener).toHaveBeenCalledTimes(2);
});
