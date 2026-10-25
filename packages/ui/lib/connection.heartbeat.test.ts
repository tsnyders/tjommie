import { readFileSync } from "node:fs";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { generateKeypair, initNoise } from "../../transport/src/index.ts";
import { encodeMobileIdentity } from "./mobile-identity";
import { endpointKey, fakePc, type FakePc } from "./remote-pc-fixture";

const device = vi.hoisted(() => ({
  pc: null as FakePc | null,
  vault: new Map<string, string>(),
  saved: new Map<string, string>(),
  network: [] as ((event: { type: string }) => void)[],
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

const wasm = new Uint8Array(readFileSync(new URL("../../transport/wasm/tjommie_noise_bg.wasm", import.meta.url)));
const hostId = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11", deviceId = "0d6ff97e-a44e-42f4-86a0-27ace431918d";
const first = { host: "192.168.1.2", port: 47101 }, second = { host: "10.0.0.7", port: 47101 };
const relay = "https://relay.example.workers.dev";
let page = Object.assign(new EventTarget(), { visibilityState: "visible" });

const settle = async (): Promise<void> => { for (let turn = 0; turn < 10; turn += 1) await vi.advanceTimersByTimeAsync(0); };
const show = (visible: boolean): void => { page.visibilityState = visible ? "visible" : "hidden"; page.dispatchEvent(new Event("visibilitychange")); };
const networkChanged = (): void => { for (const listener of device.network) listener({ type: "cellular" }); };
const lanKey = (endpoint: { host: string; port: number }): string => endpointKey({ kind: "lan", ...endpoint });

beforeAll(() => initNoise(wasm));
beforeEach(() => {
  vi.resetModules(); // a fresh connection module and store for each test
  vi.useFakeTimers();
  device.vault.clear(); device.saved.clear(); device.network.length = 0;
  device.keepAlive.start.mockClear(); device.keepAlive.stop.mockClear();
  page = Object.assign(new EventTarget(), { visibilityState: "visible" });
  vi.stubGlobal("document", page);
  vi.stubGlobal("localStorage", { getItem: (key: string) => device.saved.get(key) ?? null, setItem: (key: string, value: string) => device.saved.set(key, value) });
  vi.stubGlobal("fetch", async () => ({ ok: true, arrayBuffer: async () => wasm.slice().buffer }));
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

// A phone paired with the synthetic PC, which is saved with its endpoints and this phone's sealed identity.
async function boot(options: { lan?: { host: string; port: number }[]; relay?: string | null } = {}) {
  const pc = (device.pc = fakePc({ hostId, deviceId, hostName: "Synthetic PC", relay: options.relay ?? null }));
  device.saved.set("tjommie.mobile.hosts.v1", JSON.stringify([{ id: hostId, name: "Synthetic PC", deviceId, hostKey: pc.hostKey, lan: options.lan ?? [first], relay: options.relay ?? null }]));
  device.vault.set(`identity-${hostId}`, Buffer.from(encodeMobileIdentity(hostId, pc.hostKey, generateKeypair().privateKey)).toString("base64"));
  const connection = await import("./connection");
  const { getState } = await import("./store");
  return { pc, connection, status: () => getState().connection };
}

it("pings every 20 s while in front, and replaces a link whose ping gets no reply within 10 s", async () => {
  const { pc, connection, status } = await boot();
  connection.startConnection();
  await settle();
  expect(status()).toEqual({ status: "ready" });
  await vi.advanceTimersByTimeAsync(20_000);
  expect(pc.calls.filter((method) => method === "remote.ping")).toHaveLength(1); // answered: the link stays
  expect(pc.opens).toHaveLength(1);

  pc.wires[0].die();
  await vi.advanceTimersByTimeAsync(20_000); // the next ping goes out into the dead socket
  await vi.advanceTimersByTimeAsync(9_999);
  expect(pc.opens).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(1);
  await settle();
  expect(pc.opens).toHaveLength(2);
  expect(status()).toEqual({ status: "ready" });
});

it("opens a new wire the moment the network changes", async () => {
  const { pc, connection, status } = await boot();
  connection.startConnection();
  await settle();
  networkChanged();
  await settle();
  expect(pc.opens).toHaveLength(2);
  expect(pc.wires[0].closed).toBe(true);
  expect(status()).toEqual({ status: "ready" });
});

it("rejects a chat.send that was in flight when the link was lost, and never sends it again", async () => {
  const { pc, connection, status } = await boot();
  connection.startConnection();
  await settle();
  pc.hold.add("chat.send");
  const outcome = connection.request("chat.send", { botId: "b1", text: "Ship it" }).then(() => "sent", () => "rejected");
  await settle();
  expect(pc.calls.filter((method) => method === "chat.send")).toHaveLength(1);

  pc.wires[0].die(); // Wi-Fi to mobile data: the old socket is a zombie
  networkChanged();
  expect(await outcome).toBe("rejected");
  await settle();
  expect(status()).toEqual({ status: "ready" });
  expect(pc.calls.filter((method) => method === "chat.send")).toHaveLength(1);
});

it("races the LAN endpoints: the first to finish the handshake wins and the slower one is closed", async () => {
  const { pc, connection, status } = await boot({ lan: [first, second] });
  pc.legs.set(lanKey(first), { delayMs: 500 });
  connection.startConnection();
  await settle();
  expect(status()).toEqual({ status: "ready" });
  expect(pc.wires.map((wire) => wire.endpoint)).toEqual([{ kind: "lan", ...first }, { kind: "lan", ...second }]);
  expect(pc.wires[0].closed).toBe(true);
  expect(pc.wires[1].closed).toBe(false);
});

it("tries the relay after the 1 s LAN window when every LAN endpoint fails", async () => {
  const { pc, connection, status } = await boot({ lan: [first, second], relay });
  pc.legs.set(lanKey(first), "unreachable");
  pc.legs.set(lanKey(second), "unreachable");
  connection.startConnection();
  await settle();
  expect(pc.opens.map((endpoint) => endpoint.kind)).toEqual(["lan", "lan"]);
  await vi.advanceTimersByTimeAsync(999);
  expect(pc.opens.map((endpoint) => endpoint.kind)).toEqual(["lan", "lan"]);
  await vi.advanceTimersByTimeAsync(1);
  await settle();
  expect(pc.opens.map((endpoint) => endpoint.kind)).toEqual(["lan", "lan", "relay"]);
  expect(pc.opens[2]).toEqual({ kind: "relay", url: relay, hostId });
  expect(status()).toEqual({ status: "ready" });
});

it("doesn't let a LAN endpoint that connects but stalls its handshake hold the relay back past the 1 s window", async () => {
  const { pc, connection, status } = await boot({ relay });
  pc.legs.set(lanKey(first), { delayMs: 60_000 }); // something answers on that address, but not the PC
  connection.startConnection();
  await settle();
  await vi.advanceTimersByTimeAsync(999);
  expect(pc.opens.map((endpoint) => endpoint.kind)).toEqual(["lan"]);
  await vi.advanceTimersByTimeAsync(1);
  await settle();
  expect(pc.opens.map((endpoint) => endpoint.kind)).toEqual(["lan", "relay"]);
  expect(status()).toEqual({ status: "ready" });
  expect(pc.wires[0].closed).toBe(true);
});

it("still pings in the background when the phone was put away before Always connected finished starting", async () => {
  let started = (): void => {};
  device.keepAlive.start.mockImplementationOnce(() => new Promise<void>((resolve) => { started = resolve; }));
  const { pc, connection } = await boot();
  connection.startConnection();
  await settle();
  expect(device.keepAlive.start).toHaveBeenCalledWith({ hostName: "Synthetic PC" }); // asked, not answered yet
  show(false);
  started();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(pc.calls.filter((method) => method === "remote.ping")).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(pc.calls.filter((method) => method === "remote.ping")).toHaveLength(2);
  expect(pc.opens).toHaveLength(1);
});

it("keeps the link alive in the background while Always connected runs: a ping every 60 s while hidden", async () => {
  const { pc, connection } = await boot();
  connection.startConnection();
  await settle();
  expect(device.keepAlive.start).toHaveBeenCalledWith({ hostName: "Synthetic PC" });
  show(false);
  await vi.advanceTimersByTimeAsync(59_999);
  expect(pc.calls.filter((method) => method === "remote.ping")).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(1);
  expect(pc.calls.filter((method) => method === "remote.ping")).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(pc.calls.filter((method) => method === "remote.ping")).toHaveLength(2);
  expect(pc.opens).toHaveLength(1);
});
