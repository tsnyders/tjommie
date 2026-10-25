import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import type { WebContents } from "electron";
import { afterEach, expect, it, vi } from "vitest";
import { FrameDecoder, lengthPrefixed } from "../../client/src/encrypted-channel.ts";
import type { FrameTransport } from "../../client/src/frame-socket.ts";
import { RemoteWires } from "./remote-wire.ts";

const servers: Server[] = [];
const sockets = new Set<Socket>();
const bridges: RemoteWires[] = [];

function contents() {
  return Object.assign(new EventEmitter(), { send: vi.fn(), isDestroyed: () => false });
}

async function localServer(onSocket: (socket: Socket) => void): Promise<number> {
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
    onSocket(socket);
  });
  servers.push(server);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Local server has no port.");
  return address.port;
}

afterEach(async () => {
  for (const bridge of bridges.splice(0)) bridge.closeAll();
  for (const socket of sockets) socket.destroy();
  for (const server of servers.splice(0)) {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

it("carries one length-prefixed LAN frame each way", async () => {
  const port = await localServer((socket) => {
    const decoder = new FrameDecoder();
    socket.on("data", (chunk: Buffer) => {
      for (const frame of decoder.feed(chunk)) socket.write(lengthPrefixed(new Uint8Array([...frame, 9])));
    });
  });
  const page = contents();
  const bridge = new RemoteWires(page as unknown as WebContents);
  bridges.push(bridge);
  const { id } = await bridge.open({ kind: "lan", host: "127.0.0.1", port }, 1000);
  await bridge.send(id, new Uint8Array([1, 2]));
  await vi.waitFor(() => expect(page.send).toHaveBeenCalledWith("remote:frame", id, new Uint8Array([1, 2, 9])));
  bridge.close(id);
  await vi.waitFor(() => expect(page.send).toHaveBeenCalledWith("remote:closed", id, undefined));
});

it("closes a LAN wire when the peer sends an oversized frame", async () => {
  const port = await localServer((socket) => {
    const header = Buffer.alloc(4);
    header.writeUInt32BE(65_536);
    socket.write(header);
  });
  const page = contents();
  const bridge = new RemoteWires(page as unknown as WebContents);
  bridges.push(bridge);
  const { id } = await bridge.open({ kind: "lan", host: "127.0.0.1", port }, 1000);
  await vi.waitFor(() => expect(page.send).toHaveBeenCalledWith("remote:closed", id, undefined));
  await expect(bridge.send(id, new Uint8Array([1]))).rejects.toThrow();
});

it("rejects a ninth simultaneous wire", async () => {
  const port = await localServer(() => {});
  const bridge = new RemoteWires(contents() as unknown as WebContents);
  bridges.push(bridge);
  await Promise.all(Array.from({ length: 8 }, () => bridge.open({ kind: "lan", host: "127.0.0.1", port }, 1000)));
  await expect(bridge.open({ kind: "lan", host: "127.0.0.1", port }, 1000)).rejects.toThrow();
});

it("opens a relay while eight LAN dials are pending, and cancels those dials on cleanup", async () => {
  const signals: AbortSignal[] = [];
  const lan = (_endpoint: { host: string; port: number }, _timeoutMs: number, signal: AbortSignal) => {
    signals.push(signal);
    return new Promise<FrameTransport>((_resolve, reject) => signal.addEventListener("abort", () => reject(new Error("canceled"))));
  };
  const relay = async (): Promise<FrameTransport> => ({
    send: () => {}, close: () => {}, onFrame: () => () => {}, onClose: () => () => {},
  });
  const bridge = new RemoteWires(contents() as unknown as WebContents, relay, lan);
  bridges.push(bridge);
  const endpoint = { kind: "lan", host: "127.0.0.1", port: 1 };
  const openings = Array.from({ length: 8 }, () => bridge.open(endpoint, 1000));
  const settled = openings.map((opening) => opening.catch(() => {}));
  expect(signals).toHaveLength(8);
  await expect(bridge.open(endpoint, 1000)).rejects.toThrow();
  await expect(bridge.open({ kind: "relay", url: "https://relay.example", hostId: "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11" }, 1000)).resolves.toHaveProperty("id");
  bridge.closeAll();
  await Promise.all(settled);
  expect(signals.every((signal) => signal.aborted)).toBe(true);
});

it("keeps canceled relay dials counted until they settle", async () => {
  const resolveDials: Array<(transport: FrameTransport) => void> = [];
  const relay = () => new Promise<FrameTransport>((resolve) => { resolveDials.push(resolve); });
  const bridge = new RemoteWires(contents() as unknown as WebContents, relay);
  bridges.push(bridge);
  const endpoint = { kind: "relay", url: "https://relay.example", hostId: "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11" };
  const openings = Array.from({ length: 8 }, () => bridge.open(endpoint, 1000));
  const settled = openings.map((opening) => opening.catch(() => {}));
  bridge.closeAll();
  await expect(bridge.open(endpoint, 1000)).rejects.toThrow();
  const close = vi.fn();
  resolveDials[0]({ send: () => {}, close, onFrame: () => () => {}, onClose: () => () => {} });
  await settled[0];
  expect(close).toHaveBeenCalledOnce();
  const next = bridge.open(endpoint, 1000);
  const nextSettled = next.catch(() => {});
  expect(resolveDials).toHaveLength(9);
  bridge.closeAll();
  for (const resolve of resolveDials.slice(1)) resolve({ send: () => {}, close: () => {}, onFrame: () => () => {}, onClose: () => () => {} });
  await Promise.all([...settled, nextSettled]);
});

it("closes a LAN wire before its queued writes exceed 32 MiB", async () => {
  const port = await localServer((socket) => socket.pause());
  const page = contents();
  const bridge = new RemoteWires(page as unknown as WebContents);
  bridges.push(bridge);
  const { id } = await bridge.open({ kind: "lan", host: "127.0.0.1", port }, 1000);
  const frame = new Uint8Array(65_535);
  let closed = false;
  for (let sent = 0; sent < 1024; sent++) {
    try { await bridge.send(id, frame); }
    catch { closed = true; break; }
  }
  expect(closed).toBe(true);
  expect(page.send).toHaveBeenCalledWith("remote:closed", id, undefined);
});

it("closes wires when a main-frame reload commits", async () => {
  let peerClosed = false;
  const port = await localServer((socket) => { socket.resume(); socket.once("close", () => { peerClosed = true; }); });
  const page = contents();
  const bridge = new RemoteWires(page as unknown as WebContents);
  bridges.push(bridge);
  const { id } = await bridge.open({ kind: "lan", host: "127.0.0.1", port }, 1000);
  page.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false });
  await bridge.send(id, new Uint8Array([1]));
  page.emit("did-navigate-in-page");
  await bridge.send(id, new Uint8Array([1]));
  page.emit("did-navigate");
  await expect(bridge.send(id, new Uint8Array([2]))).rejects.toThrow();
  await vi.waitFor(() => expect(peerClosed).toBe(true));
});

it("closes a wire that finishes opening after its renderer reloads", async () => {
  let resolveTransport: (transport: FrameTransport) => void = () => {};
  const relay = () => new Promise<FrameTransport>((resolve) => { resolveTransport = resolve; });
  const page = contents();
  const bridge = new RemoteWires(page as unknown as WebContents, relay);
  bridges.push(bridge);
  const opening = bridge.open({ kind: "relay", url: "https://relay.example", hostId: "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11" }, 1000);
  page.emit("did-navigate");
  const close = vi.fn();
  resolveTransport({ send: () => {}, close, onFrame: () => () => {}, onClose: () => () => {} });
  await expect(opening).rejects.toThrow();
  expect(close).toHaveBeenCalledOnce();
});

it.each(["render-process-gone", "destroyed"])("closes wires on %s", async (event) => {
  const port = await localServer(() => {});
  const page = contents();
  const bridge = new RemoteWires(page as unknown as WebContents);
  bridges.push(bridge);
  const { id } = await bridge.open({ kind: "lan", host: "127.0.0.1", port }, 1000);
  page.emit(event);
  await expect(bridge.send(id, new Uint8Array([1]))).rejects.toThrow();
});

it("forwards relay frames and rejects malformed endpoints and timeouts", async () => {
  let inbound = (_frame: Uint8Array): void => {};
  const relay = async (): Promise<FrameTransport> => ({
    send: () => {}, close: () => {},
    onFrame: (listener) => { inbound = listener; return () => {}; },
    onClose: () => () => {},
  });
  const page = contents();
  const bridge = new RemoteWires(page as unknown as WebContents, relay);
  bridges.push(bridge);
  const endpoint = { kind: "relay", url: "https://relay.example", hostId: "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11" };
  const { id } = await bridge.open(endpoint, 1000);
  inbound(new Uint8Array([4, 5]));
  expect(page.send).toHaveBeenCalledWith("remote:frame", id, new Uint8Array([4, 5]));
  await expect(bridge.open({ ...endpoint, hostId: "not-a-uuid" }, 1000)).rejects.toThrow(/^The PC connection is unavailable\.$/);
  await expect(bridge.open({ kind: "lan", host: "localhost", port: 0 }, 1000)).rejects.toThrow(/^The PC connection is unavailable\.$/);
  await expect(bridge.open(endpoint, 0)).rejects.toThrow(/^The PC connection is unavailable\.$/);
  await expect(bridge.open(endpoint, 60_001)).rejects.toThrow(/^The PC connection is unavailable\.$/);
});

it("reports a relay close code even when the relay closes before open returns to the renderer", async () => {
  const page = contents();
  const relay = vi.fn(async (): Promise<FrameTransport> => ({
    send: () => {}, close: () => {}, onFrame: () => () => {},
    onClose: (listener) => { queueMicrotask(() => listener(4404)); return () => {}; },
  }));
  const bridge = new RemoteWires(page as unknown as WebContents, relay);
  bridges.push(bridge);
  const { id } = await bridge.open({ kind: "relay", url: "https://relay.example/path?ignored=1", hostId: "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11" }, 1000);
  expect(relay).toHaveBeenCalledWith("wss://relay.example/v1/hosts/5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11/connect", 1000);
  expect(page.send).toHaveBeenCalledWith("remote:closed", id, 4404);
});

it("allows Noise WASM without widening the renderer's network policy", () => {
  const main = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
  const csp = /const CSP =\s*"([^"]+)";/.exec(main)?.[1];
  expect(csp).toContain("script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'");
  expect(csp).toContain("connect-src 'self' ws://127.0.0.1:*");
  expect(csp).not.toMatch(/connect-src[^;]*wss:/);
});
