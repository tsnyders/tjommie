import { afterEach, expect, it } from "vitest";
import { createConnection } from "node:net";
import { parsePairingCode, type PairingCode } from "@tjommie/protocol";
import { connectTransport } from "../../client/src/client.ts";
import { authenticateRemote, type FrameTransport } from "../../client/src/remote-peer.ts";
import { FrameDecoder, lengthPrefixed } from "../../client/src/encrypted-channel.ts";
import { generateKeypair } from "../../transport/src/index.ts";
import { openDb } from "./db.ts";
import { listDevices } from "./devices-store.ts";
import { RemoteHost } from "./remote-host.ts";

const cleanup: (() => Promise<void> | void)[] = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); });

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
async function fixture() {
  const db = openDb(":memory:"); cleanup.push(() => db.close());
  const vault = new Map<string, string>();
  let mutations = 0;
  const host = new RemoteHost({ db, root: "synthetic-remote-root", port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"],
    credentials: async (input) => { if (input.operation === "write" && input.key) vault.set(input.target, input.key); return input.operation === "read" ? vault.get(input.target) ?? null : null; },
    dispatch: async (method) => { if (method === "chat.stop") { mutations++; return {}; } if (method === "bots.list") return []; throw new Error(); },
  });
  cleanup.push(() => host.close());
  const invite = await host.createPairing();
  const code = parsePairingCode(invite.code);
  return { db, host, invite, code, port: code.lan[0].port, mutations: () => mutations };
}
async function pair(code: PairingCode, privateKey: Uint8Array, psk = Buffer.from(code.secret, "base64url")) {
  return authenticateRemote({ wire: await wire(code.lan[0].port), privateKey, hostPublicKey: Buffer.from(code.hostKey, "base64url"), psk, name: "Synthetic Android",
    preface: { v: 1, mode: "pair", hostId: code.hostId, pairingId: code.pairingId } });
}

it("pairs once, shares typed RPC/events, reconnects by device key and enforces revocation", async () => {
  const { db, host, code, mutations } = await fixture();
  const keys = generateKeypair();
  const paired = await pair(code, keys.privateKey);
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
  const resumed = await authenticateRemote({ wire: await wire(code.lan[0].port), privateKey: keys.privateKey, hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Ignored on reconnect",
    preface: { v: 1, mode: "resume", hostId: code.hostId } });
  expect(resumed.deviceId).toBe(paired.deviceId);
  const second = await connectTransport(resumed.transport);
  expect(await second.call("bots.list", {})).toEqual([]);
  host.revoke(paired.deviceId);
  await second.disconnected;
  expect(listDevices(db)[0].revokedAt).not.toBeNull();
  await expect(authenticateRemote({ wire: await wire(code.lan[0].port), privateKey: keys.privateKey, hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Revoked",
    preface: { v: 1, mode: "resume", hostId: code.hostId } })).rejects.toThrow("authenticated");
}, 30_000);

it("never consumes or registers on a wrong PSK, and rejects reuse after valid confirmation", async () => {
  const { db, code } = await fixture();
  await expect(pair(code, generateKeypair().privateKey, Buffer.alloc(32, 99))).rejects.toThrow("authenticated");
  expect(listDevices(db)).toEqual([]);
  expect(db.prepare("SELECT used_at FROM pairing_codes").get()?.used_at).toBeNull();
  const valid = await pair(code, generateKeypair().privateKey);
  valid.transport.close();
  expect(db.prepare("SELECT used_at FROM pairing_codes").get()?.used_at).not.toBeNull();
  await expect(pair(code, generateKeypair().privateKey)).rejects.toThrow();
  expect(listDevices(db)).toHaveLength(1);
}, 30_000);

it("rejects cancelled invites and a mismatched PC key without registering a device", async () => {
  const { db, host, code } = await fixture();
  const bad = { ...code, hostKey: Buffer.from(generateKeypair().publicKey).toString("base64url") };
  await expect(pair(bad, generateKeypair().privateKey)).rejects.toThrow();
  host.cancelPairing(code.pairingId);
  await expect(pair(code, generateKeypair().privateKey)).rejects.toThrow();
  expect(listDevices(db)).toEqual([]);
}, 30_000);

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

it("does not register after a persisted invitation expires", async () => {
  const { db, code } = await fixture();
  db.prepare("UPDATE pairing_codes SET expires_at = ? WHERE id = ?").run("2020-01-01T00:00:00.000Z", code.pairingId);
  await expect(pair(code, generateKeypair().privateKey)).rejects.toThrow();
  expect(listDevices(db)).toEqual([]);
});
