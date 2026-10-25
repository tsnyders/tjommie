import { readFileSync } from "node:fs";
import type { Bot, Message } from "@tjommie/protocol";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { generateKeypair, initNoise } from "../../transport/src/index.ts";
import { encodeMobileIdentity } from "./mobile-identity";
import { endpointKey, fakePc, type FakePc } from "./remote-pc-fixture";

const device = vi.hoisted(() => ({
  pc: null as FakePc | null,
  vault: new Map<string, string>(),
  saved: new Map<string, string>(),
  network: [] as ((event: { type: string }) => void)[],
  statuses: [] as string[], // the connection status after every state change
  keepAlive: { start: vi.fn(async () => {}), stop: vi.fn(async () => {}), status: vi.fn(async () => ({ running: false })), batteryUnrestricted: vi.fn(async () => ({ value: true })), openBatterySettings: vi.fn(async () => {}) },
}));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => true, getPlatform: () => "android" }, registerPlugin: () => device.keepAlive }));
vi.mock("../../mobile/src/plugins.ts", () => ({
  secureStore: {
    seal: async ({ name, valueBase64 }: { name: string; valueBase64: string }) => { device.vault.set(name, valueBase64); },
    read: async ({ name }: { name: string }) => ({ valueBase64: device.vault.get(name) ?? null }),
    delete: async ({ name }: { name: string }) => { device.vault.delete(name); },
  },
  scanner: { scan: async () => ({ text: null }) },
  appearance: { setCanvas: async () => {}, getInsets: async () => ({ imeBottom: 0, keyboardVisible: false }) },
  socket: {
    addListener: async (event: string, listener: (event: { type: string }) => void) => {
      if (event === "network") device.network.push(listener);
      return { remove: async () => {} };
    },
  },
}));
vi.mock("./relay-wire", async (importOriginal) => ({ ...(await importOriginal<typeof import("./relay-wire")>()), frameSockets: () => device.pc!.sockets }));
vi.mock("./store", async (importOriginal) => {
  const store = await importOriginal<typeof import("./store")>();
  return { ...store, update: (change: Parameters<typeof store.update>[0]) => { store.update(change); device.statuses.push(store.getState().connection.status); } };
});

const wasm = new Uint8Array(readFileSync(new URL("../../transport/wasm/tjommie_noise_bg.wasm", import.meta.url)));
const hostId = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11", deviceId = "0d6ff97e-a44e-42f4-86a0-27ace431918d";
const lan = { host: "192.168.1.2", port: 47101 };
const relay = "https://relay.example.workers.dev";
let page = Object.assign(new EventTarget(), { visibilityState: "visible" });

const message = (id: string): Message => ({ id, conversationId: "b1", author: "b1", kind: "text", body: id, data: null, status: "done", createdAt: "2026-10-25T09:00:00.000Z" });
const bot = (id: string): Bot => ({
  id, name: id, color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet", autonomy: "folder", homeFolder: `C:\\bots\\${id}`,
  projectFolder: null, browser: false, paused: false, canMessage: [], state: "idle", setupReason: null, limitedUntil: null, createdAt: "2026-10-25T08:00:00.000Z",
});
const settle = async (): Promise<void> => { for (let turn = 0; turn < 10; turn += 1) await vi.advanceTimersByTimeAsync(0); };
const show = (visible: boolean): void => { page.visibilityState = visible ? "visible" : "hidden"; page.dispatchEvent(new Event("visibilitychange")); };
const changes = (): string[] => device.statuses.filter((status, at) => status !== device.statuses[at - 1]);

beforeAll(() => initNoise(wasm));
beforeEach(() => {
  vi.resetModules(); // a fresh connection module and store for each test
  vi.useFakeTimers();
  device.vault.clear(); device.saved.clear(); device.network.length = 0; device.statuses.length = 0;
  device.keepAlive.start.mockClear(); device.keepAlive.stop.mockClear();
  page = Object.assign(new EventTarget(), { visibilityState: "visible" });
  vi.stubGlobal("document", page);
  vi.stubGlobal("localStorage", { getItem: (key: string) => device.saved.get(key) ?? null, setItem: (key: string, value: string) => device.saved.set(key, value) });
  vi.stubGlobal("fetch", async () => ({ ok: true, arrayBuffer: async () => wasm.slice().buffer }));
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

// A phone paired with the synthetic PC, which is saved with its endpoints and this phone's sealed identity, and connects on start.
async function boot(options: { relay?: string | null } = {}) {
  const pc = (device.pc = fakePc({ hostId, deviceId, hostName: "Synthetic PC", relay: options.relay ?? null }));
  device.saved.set("tjommie.mobile.hosts.v1", JSON.stringify([{ id: hostId, name: "Synthetic PC", deviceId, hostKey: pc.hostKey, lan: [lan], relay: options.relay ?? null }]));
  device.vault.set(`identity-${hostId}`, Buffer.from(encodeMobileIdentity(hostId, pc.hostKey, generateKeypair().privateKey)).toString("base64"));
  const connection = await import("./connection");
  const { getState } = await import("./store");
  return { pc, connection, getState, start: async () => { connection.startConnection(); await settle(); } };
}

it("locked 30 min then visible: the dead link is replaced at once, without a failed screen, and the open chat catches up", async () => {
  device.saved.set("tjommie.mobile.keepAlive.v1", "false"); // nothing ran while the phone slept
  const { pc, connection, getState, start } = await boot();
  pc.messages.b1 = Array.from({ length: 60 }, (_, at) => message(`m${String(at + 1).padStart(2, "0")}`));
  await start();
  expect(getState().connection).toEqual({ status: "ready" });
  await connection.loadConversation("b1", connection.botHistory("b1"));
  await connection.loadOlder("b1", connection.botHistory("b1")); // all 60 loaded: the newest page and the one before it
  expect(getState().conversations.b1).toMatchObject({ status: "ready", hasMore: false });

  show(false);
  pc.wires[0].die(); // Doze froze the app: the socket looks open but carries nothing
  pc.add(message("m61")); // and the PC moved on meanwhile
  await vi.advanceTimersByTimeAsync(30 * 60_000);
  expect(pc.opens).toHaveLength(1);

  show(true);
  expect(getState().connection).toEqual({ status: "reconnecting" }); // at once, not after a ping times out
  await settle();
  expect(pc.opens).toHaveLength(2);
  expect(getState().connection).toEqual({ status: "ready" });
  const messages = getState().conversations.b1.messages.map((each) => each.id);
  expect(messages).toHaveLength(61); // the older page stayed, and the new message is there
  expect(messages.slice(0, 2)).toEqual(["m01", "m02"]);
  expect(messages.at(-1)).toBe("m61");
  expect(getState().conversations.b1.hasMore).toBe(false);
  expect(changes()).toEqual(["connecting", "ready", "reconnecting", "ready"]);
});

it("keeps the link through a quick trip away, checking it with a ping instead of reconnecting", async () => {
  device.saved.set("tjommie.mobile.keepAlive.v1", "false");
  const { pc, getState, start } = await boot();
  await start();
  show(false);
  await vi.advanceTimersByTimeAsync(5_000);
  show(true);
  await settle();
  expect(pc.opens).toHaveLength(1);
  expect(pc.calls.filter((method) => method === "remote.ping")).toHaveLength(1);
  expect(changes()).toEqual(["connecting", "ready"]);
  expect(getState().connection).toEqual({ status: "ready" });
});

it("host offline over relay: the relay's 4404 says the PC is offline, over the bots already listed", async () => {
  const { pc, getState, start } = await boot({ relay });
  pc.bots = [bot("b1")];
  await start();
  expect(getState().bots.map((each) => each.id)).toEqual(["b1"]);

  pc.legs.set(endpointKey({ kind: "lan", ...lan }), "unreachable");
  pc.legs.set("relay", "offline");
  pc.wires[0].drop(); // the PC went to sleep
  await settle();
  await vi.advanceTimersByTimeAsync(1_000); // the LAN window passes, then the relay answers 4404
  await settle();
  expect(getState().connection).toEqual({ status: "offline", reason: "host_offline" });
  expect(getState().bots.map((each) => each.id)).toEqual(["b1"]);
  expect(pc.opens.map((endpoint) => endpoint.kind)).toEqual(["lan", "lan", "relay"]);
  expect(changes()).not.toContain("failed");
});

it("can't reach the PC: a saved PC that stops answering shows unreachable over the cached screens, never the failed screen", async () => {
  const { pc, getState, start } = await boot();
  await start();
  pc.legs.set(endpointKey({ kind: "lan", ...lan }), "refused");
  pc.wires[0].drop();
  await settle();
  expect(getState().connection).toEqual({ status: "offline", reason: "unreachable" });
  await vi.advanceTimersByTimeAsync(60_000);
  expect(changes()).not.toContain("failed");
});

it("revoked: the PC's authenticated refusal is terminal: removed, and nothing reopens", async () => {
  const { pc, getState, start } = await boot({ relay });
  await start();
  expect(device.keepAlive.start).toHaveBeenCalledWith({ hostName: "Synthetic PC" });

  pc.refuse = "revoked";
  pc.wires[0].drop();
  await settle();
  expect(getState().connection).toEqual({ status: "removed" });
  expect(device.keepAlive.stop).toHaveBeenCalled();
  const opens = pc.opens.length;
  await vi.advanceTimersByTimeAsync(60_000);
  show(false);
  show(true);
  for (const listener of device.network) listener({ type: "cellular" });
  await vi.advanceTimersByTimeAsync(60_000);
  expect(pc.opens).toHaveLength(opens);
  expect(getState().connection).toEqual({ status: "removed" });
});

it("backs off 0, 1, 2, 5 and 10 s, then keeps trying every 10 s while the app is in front", async () => {
  const { pc, start } = await boot();
  await start();
  pc.legs.set(endpointKey({ kind: "lan", ...lan }), "refused");
  pc.wires[0].drop();
  await settle();
  expect(pc.opens).toHaveLength(2); // straight away
  for (const wait of [1_000, 2_000, 5_000, 10_000, 10_000]) {
    const opens = pc.opens.length;
    await vi.advanceTimersByTimeAsync(wait - 1);
    expect(pc.opens).toHaveLength(opens);
    await vi.advanceTimersByTimeAsync(1);
    expect(pc.opens).toHaveLength(opens + 1);
  }
});

it("says why a first connect failed, and tries again on return without taking the connect screen away", async () => {
  const { pc, connection, getState } = await boot();
  const lanKey = endpointKey({ kind: "lan", ...lan });
  pc.legs.set(lanKey, "refused");
  connection.startConnection();
  await settle();
  await vi.advanceTimersByTimeAsync(1_000 + 2_000);
  expect(getState().connection).toEqual({ status: "failed", reason: "Can't reach your PC. Keep both devices on the same Wi-Fi, and keep Tjommie open on the PC." });

  show(false);
  show(true); // back from copying a pairing code, say
  await settle();
  expect(pc.opens).toHaveLength(4);
  expect(getState().connection.status).toBe("failed");
  pc.legs.delete(lanKey);
  connection.connectRemote(hostId); // Connect pressed
  expect(getState().connection).toEqual({ status: "connecting" });
  await settle();
  expect(getState().connection).toEqual({ status: "ready" });
});

it("stops retrying while hidden without keep-alive, and tries again the moment the app is back", async () => {
  device.saved.set("tjommie.mobile.keepAlive.v1", "false");
  const { pc, getState, start } = await boot();
  await start();
  expect(device.keepAlive.start).not.toHaveBeenCalled();

  const lanKey = endpointKey({ kind: "lan", ...lan });
  pc.legs.set(lanKey, "refused");
  show(false);
  pc.wires[0].drop();
  await vi.advanceTimersByTimeAsync(10 * 60_000);
  expect(pc.opens).toHaveLength(1);
  expect(getState().connection).toEqual({ status: "reconnecting" });

  pc.legs.delete(lanKey);
  show(true);
  await settle();
  expect(pc.opens).toHaveLength(2);
  expect(getState().connection).toEqual({ status: "ready" });
});
