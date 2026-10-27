import { afterEach, expect, it, vi } from "vitest";
import { createConnection, createServer, type AddressInfo } from "node:net";
import { parsePairingCode, type PairingCode } from "@tjommie/protocol";
import { connectTransport } from "../../client/src/client.ts";
import { authenticateRemote, RemoteRefusedError, type FrameTransport } from "../../client/src/remote-peer.ts";
import { FrameDecoder, lengthPrefixed } from "../../client/src/encrypted-channel.ts";
import { generateKeypair } from "../../transport/src/index.ts";
import { openDb } from "./db.ts";
import { addDevice, listDevices } from "./devices-store.ts";
import { RemoteHost } from "./remote-host.ts";
import type { PeerWire } from "./peer-wire.ts";

const cleanup: (() => Promise<void> | void)[] = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); vi.useRealTimers(); });

function relayWire(host: RemoteHost, address = "relay", kind: PeerWire["kind"] = "relay"): FrameTransport {
  const toHost = new Set<(frame: Uint8Array) => void>(), toClient = new Set<(frame: Uint8Array) => void>();
  const hostClosed = new Set<() => void>(), clientClosed = new Set<() => void>();
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    for (const listener of hostClosed) listener();
    for (const listener of clientClosed) listener();
  };
  const server: PeerWire = {
    kind, address, get buffered() { return 0; },
    write: (frame) => { if (closed) throw new Error("closed"); queueMicrotask(() => { if (!closed) for (const listener of toClient) listener(frame.slice()); }); },
    destroy: close, onFrame: (listener) => { toHost.add(listener); }, onClose: (listener) => { hostClosed.add(listener); },
  };
  host.acceptWire(server);
  return {
    send: (frame) => { if (closed) throw new Error("closed"); queueMicrotask(() => { if (!closed) for (const listener of toHost) listener(frame.slice()); }); },
    close, onFrame: (listener) => { toClient.add(listener); return () => { toClient.delete(listener); }; },
    onClose: (listener) => { clientClosed.add(listener); if (closed) queueMicrotask(listener); return () => { clientClosed.delete(listener); }; },
  };
}

async function wire(port: number): Promise<FrameTransport> {
  const socket = createConnection({ host: "127.0.0.1", port });
  await new Promise<void>((resolve, reject) => { socket.once("connect", resolve); socket.once("error", reject); });
  const frames = new Set<(frame: Uint8Array) => void>(), closed = new Set<() => void>();
  const decoder = new FrameDecoder();
  socket.on("data", (chunk) => { try { for (const frame of decoder.feed(chunk)) for (const listener of frames) listener(frame); } catch { socket.destroy(); } });
  socket.on("close", () => { for (const listener of closed) listener(); }); socket.on("error", () => socket.destroy());
  cleanup.push(() => { socket.destroy(); });
  return {
    send: (frame) => { socket.write(lengthPrefixed(frame)); }, close: () => { socket.destroy(); },
    onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
    onClose: (listener) => { closed.add(listener); return () => { closed.delete(listener); }; },
  };
}
async function fixture(relayUrl: () => string | null = () => null) {
  const db = openDb(":memory:"); cleanup.push(() => db.close());
  const vault = new Map<string, string>();
  let mutations = 0;
  const host = new RemoteHost({ db, root: "synthetic-remote-root", port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"], relayUrl,
    credentials: async (input) => { if (input.operation === "write" && input.key) vault.set(input.target, input.key); return input.operation === "read" ? vault.get(input.target) ?? null : null; },
    dispatch: async (method) => { if (method === "chat.stop") { mutations++; return {}; } if (method === "bots.list") return []; throw new Error(); },
  });
  cleanup.push(() => host.close());
  const invite = await host.createPairing();
  const code = parsePairingCode(invite.code);
  return { db, host, invite, code, port: code.lan[0].port, mutations: () => mutations };
}
async function pair(code: PairingCode, privateKey: Uint8Array, psk = Buffer.from(code.secret, "base64url"), transport?: FrameTransport) {
  return authenticateRemote({ wire: transport ?? await wire(code.lan[0].port), privateKey, hostPublicKey: Buffer.from(code.hostKey, "base64url"), psk, name: "Synthetic Android",
    preface: { v: 1, mode: "pair", hostId: code.hostId, pairingId: code.pairingId } });
}

it.each(["lan", "relay"] as const)("pairs and resumes before being refused after revoke over %s", async (kind) => {
  const { db, host, code, mutations } = await fixture();
  const open = () => kind === "relay" ? Promise.resolve(relayWire(host)) : wire(code.lan[0].port);
  const keys = generateKeypair();
  const paired = await pair(code, keys.privateKey, Buffer.from(code.secret, "base64url"), await open());
  expect(listDevices(db)).toHaveLength(1);
  // No RPC/event data is sent before the channel's encrypted hello/welcome completes.
  host.broadcast("devices.changed", {});
  const client = await connectTransport(paired.transport);
  expect(await client.call("bots.list", {})).toEqual([]);
  await client.call("chat.stop", { botId: "synthetic" });
  expect(mutations()).toBe(1);
  const event = new Promise<void>((resolve) => client.subscribe((value) => { if (value.event === "devices.changed") resolve(); }));
  host.broadcast("devices.changed", {}); await event;
  await client.close();
  const resumed = await authenticateRemote({ wire: await open(), privateKey: keys.privateKey, hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Ignored on reconnect",
    preface: { v: 1, mode: "resume", hostId: code.hostId } });
  expect(resumed.deviceId).toBe(paired.deviceId);
  const second = await connectTransport(resumed.transport);
  expect(await second.call("bots.list", {})).toEqual([]);
  host.revoke(paired.deviceId);
  await second.disconnected;
  expect(listDevices(db)[0].revokedAt).not.toBeNull();
  const refused = authenticateRemote({ wire: await open(), privateKey: keys.privateKey, hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Revoked",
    preface: { v: 1, mode: "resume", hostId: code.hostId } });
  await expect(refused).rejects.toBeInstanceOf(RemoteRefusedError);
  await expect(refused).rejects.toMatchObject({ reason: "revoked" });
}, 30_000);

it.each(["lan", "relay"] as const)("authentically refuses an unknown resume key over %s", async (kind) => {
  const { host, code } = await fixture();
  const refused = authenticateRemote({ wire: kind === "relay" ? relayWire(host) : await wire(code.lan[0].port), privateKey: generateKeypair().privateKey,
    hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Unknown",
    preface: { v: 1, mode: "resume", hostId: code.hostId } });
  await expect(refused).rejects.toBeInstanceOf(RemoteRefusedError);
  await expect(refused).rejects.toMatchObject({ reason: "unknown" });
});

it.each(["lan", "relay"] as const)("closes a refused %s peer even when the client leaves its wire open", async (kind) => {
  const { host, code } = await fixture();
  const raw = kind === "relay" ? relayWire(host) : await wire(code.lan[0].port);
  const closedByHost = new Promise<void>((resolve) => raw.onClose(() => resolve()));
  const refusal = authenticateRemote({ wire: { ...raw, close: () => {} }, privateKey: generateKeypair().privateKey,
    hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Unknown",
    preface: { v: 1, mode: "resume", hostId: code.hostId } });
  await expect(refusal).rejects.toMatchObject({ reason: "unknown" });
  await closedByHost;
}, 10_000);

it("sweeps relay peers and starts only one sweep when LAN listening fails", async () => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
  const blocker = createServer();
  await new Promise<void>((resolve) => blocker.listen(0, "127.0.0.1", resolve));
  cleanup.push(() => new Promise<void>((resolve) => blocker.close(() => resolve())));
  const db = openDb(":memory:"); cleanup.push(() => db.close());
  const vault = new Map<string, string>();
  const host = new RemoteHost({ db, root: "synthetic-remote-root", port: (blocker.address() as AddressInfo).port, bind: "127.0.0.1",
    credentials: async (input) => { if (input.operation === "write" && input.key) vault.set(input.target, input.key); return input.operation === "read" ? vault.get(input.target) ?? null : null; },
    dispatch: async () => ({}),
  });
  cleanup.push(() => host.close());
  await expect(host.ensureListening()).rejects.toMatchObject({ code: "EADDRINUSE" });
  const hostId = host.status().hostId;
  const hostKey = db.prepare("SELECT public_key FROM remote_host").get()?.public_key;
  if (!hostId || typeof hostKey !== "string") throw new Error("Host identity was not loaded.");
  const keys = generateKeypair();
  const device = addDevice(db, { publicKey: Buffer.from(keys.publicKey).toString("base64url"), name: "Relay phone" });
  const paired = await authenticateRemote({ wire: relayWire(host), privateKey: keys.privateKey,
    hostPublicKey: Buffer.from(hostKey, "base64url"), name: "Relay phone", preface: { v: 1, mode: "resume", hostId } });
  const client = await connectTransport(paired.transport);
  expect(host.status().connectedDeviceIds).toEqual([device.id]);
  await expect(host.ensureListening()).rejects.toMatchObject({ code: "EADDRINUSE" });
  expect(vi.getTimerCount()).toBe(1);
  await vi.advanceTimersByTimeAsync(150_000);
  await client.disconnected;
  expect(host.status().connectedDeviceIds).toEqual([]);
});

it("includes the current relay URL in pairing and ready, and handles ping locally", async () => {
  let url = "https://relay.example.workers.dev";
  const { host, code } = await fixture(() => url);
  expect(code).toMatchObject({ v: 2, relay: url });
  expect(host.status().relay.url).toBe(url);
  url = "https://next.example.workers.dev";
  const paired = await pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), relayWire(host));
  expect(paired.relay).toBe(url);
  const client = await connectTransport(paired.transport);
  expect(await client.call("remote.ping", {})).toEqual({});
  await client.close();
});

it.each(["lan", "relay"] as const)("registers and clears only the authenticated %s peer's push token", async (kind) => {
  const { db, host, code } = await fixture();
  const open = () => kind === "relay" ? Promise.resolve(relayWire(host)) : wire(code.lan[0].port);
  const paired = await pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), await open());
  const client = await connectTransport(paired.transport);
  const next = parsePairingCode((await host.createPairing()).code);
  const other = await pair(next, generateKeypair().privateKey, Buffer.from(next.secret, "base64url"), await open());
  const otherClient = await connectTransport(other.transport);
  expect(await otherClient.call("device.registerPush", { token: "other-device-token" })).toEqual({});
  expect(await client.call("device.registerPush", { token: "synthetic-fcm-token", kinds: ["done", "mention"] })).toEqual({});
  expect(db.prepare("SELECT push_token, push_kinds FROM devices WHERE id = ?").get(paired.deviceId)).toEqual({ push_token: "synthetic-fcm-token", push_kinds: '["done"]' });
  expect(db.prepare("SELECT push_token FROM devices WHERE id = ?").get(other.deviceId)).toEqual({ push_token: "other-device-token" });
  expect(host.pushTargets()).toHaveLength(2);
  expect(host.pushTargets()).toContainEqual({ deviceId: paired.deviceId, token: "synthetic-fcm-token", kinds: ["done"] });
  host.clearPushToken(paired.deviceId, "synthetic-fcm-token");
  expect(host.pushTargets()).toEqual([{ deviceId: other.deviceId, token: "other-device-token", kinds: null }]);
  expect(await client.call("device.registerPush", { token: "synthetic-fcm-token" })).toEqual({});
  expect(await client.call("device.registerPush", { token: null })).toEqual({});
  expect(host.pushTargets()).toEqual([{ deviceId: other.deviceId, token: "other-device-token", kinds: null }]);
  expect(await client.call("device.registerPush", { token: "synthetic-fcm-token" })).toEqual({});
  host.revoke(paired.deviceId);
  expect(host.pushTargets()).toEqual([{ deviceId: other.deviceId, token: "other-device-token", kinds: null }]);
  expect(db.prepare("SELECT push_token, push_kinds FROM devices WHERE id = ?").get(paired.deviceId)).toEqual({ push_token: null, push_kinds: null });
  await otherClient.close();
});

it.each(["lan", "relay"] as const)("keeps a %s peer alive on ping, then closes it after 150 seconds without inbound frames", async (kind) => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
  const { host, code } = await fixture();
  const paired = await pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), kind === "relay" ? relayWire(host) : await wire(code.lan[0].port));
  const client = await connectTransport(paired.transport);
  await vi.advanceTimersByTimeAsync(120_000);
  expect(await client.call("remote.ping", {})).toEqual({});
  await vi.advanceTimersByTimeAsync(120_000);
  expect(host.status().connectedDeviceIds).toEqual([paired.deviceId]);
  await vi.advanceTimersByTimeAsync(30_000);
  await client.disconnected;
  await vi.waitFor(() => expect(host.status().connectedDeviceIds).toEqual([]));
});

it("applies relay pending and active limits without a per-address relay cap", async () => {
  const { host, code } = await fixture();
  const peers = Array.from({ length: 8 }, () => relayWire(host));
  const rotated = new Promise<void>((resolve) => peers[0].onClose(() => resolve()));
  const paired = await pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), relayWire(host));
  await rotated;
  for (const peer of peers) peer.close();
  const active = [paired];
  for (let index = 1; index < 32; index++) {
    const next = parsePairingCode((await host.createPairing()).code);
    active.push(await pair(next, generateKeypair().privateKey, Buffer.from(next.secret, "base64url"), relayWire(host)));
  }
  expect(host.status().connectedDeviceIds).toHaveLength(32);
  const excess = relayWire(host);
  await new Promise<void>((resolve) => excess.onClose(() => resolve()));
  expect(host.status().connectedDeviceIds).toHaveLength(32);
  for (const peer of active) peer.transport.close();
}, 30_000);

it("keeps LAN pending handshakes when relay handshakes fill and rotate their own budget", async () => {
  const { host } = await fixture();
  const lan = Array.from({ length: 8 }, (_, index) => relayWire(host, `192.0.2.${index + 1}`, "lan"));
  const closed = lan.map((peer) => vi.fn(() => {}));
  lan.forEach((peer, index) => peer.onClose(closed[index]));
  const relay = Array.from({ length: 8 }, () => relayWire(host));
  const rotated = new Promise<void>((resolve) => relay[0].onClose(() => resolve()));
  const next = relayWire(host);
  await rotated;
  expect(closed.every((listener) => listener.mock.calls.length === 0)).toBe(true);
  [...lan, ...relay, next].forEach((peer) => peer.close());
});

it("keeps relay pending handshakes when LAN handshakes fill and rotate their own budget", async () => {
  const { host } = await fixture();
  const relay = Array.from({ length: 8 }, () => relayWire(host));
  const relayClosed = vi.fn();
  relay.forEach((peer) => peer.onClose(relayClosed));
  const lan = Array.from({ length: 8 }, (_, index) => relayWire(host, `192.0.2.${index + 1}`, "lan"));
  const rotated = vi.fn();
  lan[0].onClose(rotated);
  const next = relayWire(host, "192.0.2.9", "lan");
  expect(rotated).toHaveBeenCalledOnce();
  expect(relayClosed).not.toHaveBeenCalled();
  [...relay, ...lan, next].forEach((peer) => peer.close());
});

it.each(["lan", "relay"] as const)("never consumes or registers on a wrong PSK over %s, and rejects reuse", async (kind) => {
  const { db, host, code } = await fixture();
  const open = () => kind === "relay" ? relayWire(host) : undefined;
  await expect(pair(code, generateKeypair().privateKey, Buffer.alloc(32, 99), open())).rejects.toThrow("authenticated");
  expect(listDevices(db)).toEqual([]);
  expect(db.prepare("SELECT used_at FROM pairing_codes").get()?.used_at).toBeNull();
  const valid = await pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), open());
  valid.transport.close();
  expect(db.prepare("SELECT used_at FROM pairing_codes").get()?.used_at).not.toBeNull();
  await expect(pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), open())).rejects.toThrow();
  expect(listDevices(db)).toHaveLength(1);
}, 30_000);

it.each(["lan", "relay"] as const)("rejects cancelled invites and a mismatched PC key over %s", async (kind) => {
  const { db, host, code } = await fixture();
  const open = () => kind === "relay" ? relayWire(host) : undefined;
  const bad = { ...code, hostKey: Buffer.from(generateKeypair().publicKey).toString("base64url") };
  await expect(pair(bad, generateKeypair().privateKey, Buffer.from(bad.secret, "base64url"), open())).rejects.toThrow();
  host.cancelPairing(code.pairingId);
  await expect(pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), open())).rejects.toThrow();
  expect(listDevices(db)).toEqual([]);
}, 30_000);

it("keeps a pairing code single-use across relay then LAN", async () => {
  const { db, host, code } = await fixture();
  const first = await pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), relayWire(host));
  first.transport.close();
  await expect(pair(code, generateKeypair().privateKey)).rejects.toThrow();
  expect(listDevices(db)).toHaveLength(1);
});

it("keeps authenticated RPC available while refusing excess pending connections", async () => {
  const { host, code } = await fixture();
  const peer = await pair(code, generateKeypair().privateKey);
  const client = await connectTransport(peer.transport);
  cleanup.push(() => client.close());
  for (let index = 0; index < 4; index++) await wire(code.lan[0].port);
  const excess = createConnection({ host: "127.0.0.1", port: code.lan[0].port });
  cleanup.push(() => { excess.destroy(); });
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Excess unauthenticated connection stayed open.")), 2000);
    excess.once("close", () => { clearTimeout(timer); resolve(); }); excess.on("error", () => {});
  });
  expect(await client.call("bots.list", {})).toEqual([]);
  expect(host.status().connectedDeviceIds).toEqual([peer.deviceId]);
}, 10_000);

it.each(["lan", "relay"] as const)("does not register after a persisted invitation expires over %s", async (kind) => {
  const { db, host, code } = await fixture();
  db.prepare("UPDATE pairing_codes SET expires_at = ? WHERE id = ?").run("2020-01-01T00:00:00.000Z", code.pairingId);
  await expect(pair(code, generateKeypair().privateKey, Buffer.from(code.secret, "base64url"), kind === "relay" ? relayWire(host) : undefined)).rejects.toThrow();
  expect(listDevices(db)).toEqual([]);
});
