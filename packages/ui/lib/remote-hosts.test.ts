import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { encodePairingCode } from "@tjommie/protocol";
import { decodeMobileIdentity, encodeMobileIdentity } from "./mobile-identity";

const fake = vi.hoisted(() => ({
  vault: new Map<string, string>(), publicStore: new Map<string, string>(), afterConfirm: false, acceptKeys: new Set<number>(), refuseKeys: new Set<number>(), relay: null as string | null,
  Refused: class extends Error {}, // the PC's authenticated refusal (RemoteRefusedError)
}));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => true, getPlatform: () => "android" } }));
vi.mock("@tjommie/transport", () => ({ initNoise: () => {}, generateKeypair: () => ({ privateKey: new Uint8Array(32).fill(20), publicKey: new Uint8Array(32).fill(21) }) }));
vi.mock("../../mobile/src/plugins.ts", () => ({
  secureStore: {
    seal: async ({ name, valueBase64 }: { name: string; valueBase64: string }) => { fake.vault.set(name, valueBase64); },
    read: async ({ name }: { name: string }) => ({ valueBase64: fake.vault.get(name) ?? null }),
    delete: async ({ name }: { name: string }) => { fake.vault.delete(name); },
  },
  scanner: { scan: async () => ({ text: null }) },
  socket: { addListener: async () => ({ remove: async () => {} }), open: async () => ({ connectionId: "synthetic" }), close: async () => {}, send: async () => {} },
}));
vi.mock("../../client/src/remote-peer.ts", () => ({ authenticateRemote: async (options: { preface: { mode: string }; privateKey: Uint8Array; onConfirmSent?: () => void }) => {
  if (options.preface.mode === "pair") { if (fake.afterConfirm) { options.onConfirmSent?.(); fake.acceptKeys.add(options.privateKey[0]); } throw new Error("Synthetic lost connection"); }
  if (fake.refuseKeys.has(options.privateKey[0])) throw new fake.Refused("This device is not paired with this PC.");
  if (!fake.acceptKeys.has(options.privateKey[0])) throw new Error("Synthetic unknown key");
  let message: ((text: string) => void) | undefined;
  return { deviceId: "0d6ff97e-a44e-42f4-86a0-27ace431918d", hostId: "ee87d56c-f1ba-453f-8210-d0f3536e5f53", hostName: "Synthetic PC", relay: fake.relay, transport: {
    send: () => queueMicrotask(() => message?.('{"type":"welcome","protocol":1}')), close: () => {},
    onMessage: (listener: (text: string) => void) => { message = listener; return () => { message = undefined; }; }, onClose: () => () => {},
  } };
}, RemoteRefusedError: fake.Refused }));
import { HostOfflineError, openRemoteClient, pairRemote, remoteHosts } from "./remote-hosts";

const id = "ee87d56c-f1ba-453f-8210-d0f3536e5f53", hostKey = "AQ".padEnd(43, "A");
const host = { id, name: "Synthetic PC", hostKey, lan: [{ host: "192.168.1.2", port: 47101 }], deviceId: "322bccbc-6ba0-409e-8d7f-b736bb7ce3e3", relay: null };
const invite = () => encodePairingCode({ v: 2, relay: null, pairingId: "8074dc6f-2eb2-4e53-84d5-6263dfe4c0a2", hostId: id, hostName: host.name, hostKey, lan: host.lan, secret: "Ag".padEnd(43, "A"), expires: new Date(Date.now() + 600_000).toISOString() });
const base64 = (bytes: Uint8Array) => Buffer.from(bytes).toString("base64");
beforeEach(() => {
  fake.vault.clear(); fake.publicStore.clear(); fake.acceptKeys.clear(); fake.refuseKeys.clear(); fake.afterConfirm = false; fake.relay = null;
  vi.stubGlobal("localStorage", { getItem: (key: string) => fake.publicStore.get(key) ?? null, setItem: (key: string, value: string) => fake.publicStore.set(key, value) });
  vi.stubGlobal("fetch", async () => new Response(new Uint8Array([0])));
});
afterEach(() => vi.unstubAllGlobals());

it("preserves a working paired identity when a new pairing fails before confirmation", async () => {
  const original = base64(encodeMobileIdentity(id, hostKey, new Uint8Array(32).fill(9)));
  fake.vault.set(`identity-${id}`, original);
  fake.publicStore.set("tjommie.mobile.hosts.v1", JSON.stringify([host]));
  await expect(pairRemote(invite(), "Phone")).rejects.toThrow();
  expect(fake.vault.get(`identity-${id}`)).toBe(original);
  expect(fake.vault.has(`identity-${id}.pending`)).toBe(false);
  expect(remoteHosts()).toEqual([host]);
});

it("leaves no unusable saved PC when a first pairing fails before confirmation", async () => {
  await expect(pairRemote(invite(), "Phone")).rejects.toThrow();
  expect(remoteHosts()).toEqual([]);
  expect(fake.vault.size).toBe(0);
});

it("recovers a lost acknowledgement with the protected pending identity and promotes only after success", async () => {
  fake.afterConfirm = true;
  fake.vault.set(`identity-${id}`, base64(encodeMobileIdentity(id, hostKey, new Uint8Array(32).fill(9))));
  fake.publicStore.set("tjommie.mobile.hosts.v1", JSON.stringify([host]));
  await expect(pairRemote(invite(), "Phone")).rejects.toThrow();
  expect(remoteHosts()).toEqual([host]);
  expect(fake.vault.has(`identity-${id}.pending`)).toBe(true);
  const client = await openRemoteClient(id);
  await client.close();
  expect(fake.vault.has(`identity-${id}.pending`)).toBe(false);
  const envelope = new Uint8Array(Buffer.from(fake.vault.get(`identity-${id}`)!, "base64"));
  expect(decodeMobileIdentity(envelope, id, hostKey)[0]).toBe(20);
  expect(remoteHosts()[0].deviceId).toBe("0d6ff97e-a44e-42f4-86a0-27ace431918d");
});

it("keeps the PC's refusal of the saved identity when the pending one can't connect, and still recovers with a pending one that works", async () => {
  fake.refuseKeys.add(9); // the saved identity was revoked
  fake.vault.set(`identity-${id}`, base64(encodeMobileIdentity(id, hostKey, new Uint8Array(32).fill(9))));
  fake.vault.set(`identity-${id}.pending`, base64(encodeMobileIdentity(id, hostKey, new Uint8Array(32).fill(20))));
  fake.publicStore.set("tjommie.mobile.hosts.v1", JSON.stringify([host]));
  fake.publicStore.set("tjommie.mobile.pending.v1", JSON.stringify([host]));
  await expect(openRemoteClient(id)).rejects.toBeInstanceOf(fake.Refused); // the pending identity failed in the ordinary way

  fake.acceptKeys.add(20); // the pending pairing did land on the PC
  await (await openRemoteClient(id)).close();
  expect(fake.vault.has(`identity-${id}.pending`)).toBe(false);
  expect(decodeMobileIdentity(new Uint8Array(Buffer.from(fake.vault.get(`identity-${id}`)!, "base64")), id, hostKey)[0]).toBe(20);
});

it("says the PC is offline when the desktop bridge reports the relay's 4404 before the relay leg's open returns", async () => {
  const identity = encodeMobileIdentity(id, hostKey, new Uint8Array(32).fill(9));
  const closes = new Set<(wire: string, code?: number) => void>();
  vi.stubGlobal("window", { tjommie: { remote: {
    open: async (endpoint: { kind: string }) => {
      if (endpoint.kind === "lan") throw new Error("Synthetic LAN timeout");
      for (const listener of closes) listener("relay-1", 4404); // the closed event overtakes the open's reply
      return { id: "relay-1" };
    },
    send: async () => {}, close: () => {}, onFrame: () => () => {},
    onClosed: (listener: (wire: string, code?: number) => void) => { closes.add(listener); return () => { closes.delete(listener); }; },
    seal: async () => {}, read: async (name: string) => (name === `identity-${id}` ? identity.slice() : null), delete: async () => {},
  } } });
  fake.publicStore.set("tjommie.mobile.hosts.v1", JSON.stringify([{ ...host, relay: "https://relay.example.workers.dev" }]));
  await expect(openRemoteClient(id)).rejects.toBeInstanceOf(HostOfflineError);
});

it("reads a PC saved before the relay existed as having no relay", () => {
  const before = { id, name: host.name, hostKey, lan: host.lan, deviceId: host.deviceId };
  fake.publicStore.set("tjommie.mobile.hosts.v1", JSON.stringify([before]));
  expect(remoteHosts()).toEqual([{ ...before, relay: null }]);
});

it("keeps the relay the PC announces in its ready message, so the next connection can use it", async () => {
  fake.relay = "https://relay.example.workers.dev";
  fake.acceptKeys.add(9);
  fake.vault.set(`identity-${id}`, base64(encodeMobileIdentity(id, hostKey, new Uint8Array(32).fill(9))));
  fake.publicStore.set("tjommie.mobile.hosts.v1", JSON.stringify([host]));
  const client = await openRemoteClient(id);
  await client.close();
  expect(remoteHosts()[0].relay).toBe("https://relay.example.workers.dev");
  fake.relay = null; // the PC turned its relay off
  await (await openRemoteClient(id)).close();
  expect(remoteHosts()[0].relay).toBeNull();
});
