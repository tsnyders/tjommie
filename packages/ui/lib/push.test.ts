import type { Client } from "@tjommie/client";
import type { PushData } from "@tjommie/protocol";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { PushApi } from "./push";

const saved = vi.hoisted(() => ({ hosts: [] as { id: string; name: string }[], opened: [] as string[] }));
vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => true, getPlatform: () => "android" },
  registerPlugin: () => ({ start: async () => {}, stop: async () => {}, addListener: async () => ({ remove: async () => {} }) }),
}));
vi.mock("./remote-hosts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./remote-hosts")>()),
  remoteHosts: () => saved.hosts,
  // Each saved PC answers the startup lists at once.
  openRemoteClient: async (id: string): Promise<Client> => {
    saved.opened.push(id);
    const answers: Record<string, unknown> = { "settings.get": { chainLimit: 5, relayUrl: null } };
    const call = async (method: string) => answers[method] ?? [];
    return { call: call as unknown as Client["call"], events: [], subscribe: () => () => {}, disconnected: new Promise(() => {}), close: async () => {} };
  },
}));

const home = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11", office = "9c4f6b1d-2a7e-4e3b-8d55-0f6a1c2b3d4e";

// The phone's push as the connection sees it: whether this build has it, the permission, the token, and taps.
function fakePush(options: { available: boolean; permission: string }) {
  const listeners = new Map<string, (value: never) => void>();
  const push: PushApi = {
    available: () => options.available,
    checkPermissions: async () => ({ receive: options.permission }),
    requestPermissions: async () => ({ receive: options.permission }),
    register: async () => { queueMicrotask(() => listeners.get("registration")?.({ value: "synthetic-push-token" } as never)); },
    addListener: (async (event: string, listener: (value: never) => void) => {
      listeners.set(event, listener);
      return { remove: async () => { listeners.delete(event); } };
    }) as PushApi["addListener"],
  };
  const tap = (data: unknown): void => listeners.get("pushNotificationActionPerformed")?.({ actionId: "tap", notification: { data } } as never);
  return { push, tap };
}

const recorder = () => {
  const calls: [string, unknown][] = [];
  const call = async (method: string, params: unknown) => { calls.push([method, params]); return {}; };
  return { calls, client: { call: call as unknown as Client["call"] } };
};

beforeEach(() => {
  vi.resetModules();
  saved.hosts = []; saved.opened = [];
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: () => {} });
});
afterEach(() => vi.unstubAllGlobals());

it("registers this phone's push token with the PC only when this build has push and the person allowed notifications", async () => {
  const { registerPushIfAllowed } = await import("./push");
  const { calls, client } = recorder();
  await registerPushIfAllowed(client, fakePush({ available: false, permission: "granted" }).push);
  await registerPushIfAllowed(client, fakePush({ available: true, permission: "prompt" }).push);
  await registerPushIfAllowed(client, fakePush({ available: true, permission: "denied" }).push);
  expect(calls).toEqual([]);
  await registerPushIfAllowed(client, fakePush({ available: true, permission: "granted" }).push);
  expect(calls).toEqual([["device.registerPush", { token: "synthetic-push-token" }]]);
});

it("does nothing at all in a build without push", async () => {
  const { onPushTap, registerPushIfAllowed } = await import("./push");
  const { calls, client } = recorder();
  await registerPushIfAllowed(client);
  expect(calls).toEqual([]);
  expect(() => onPushTap(() => {})()).not.toThrow();
});

it("routes a tap to the bot's chat, switching to that bot's PC first, and ignores a tap it can't read", async () => {
  saved.hosts = [{ id: home, name: "Home PC" }, { id: office, name: "Office PC" }];
  const connection = await import("./connection");
  const { onPushTap } = await import("./push");
  const { getState } = await import("./store");
  connection.startConnection();
  await vi.waitFor(() => expect(getState().connection).toEqual({ status: "ready" }));
  expect(saved.opened).toEqual([home]);

  const { push, tap } = fakePush({ available: true, permission: "granted" });
  const routes: string[] = [];
  onPushTap((data: PushData) => routes.push(connection.routePush(data)), push);
  await Promise.resolve();
  tap({ v: "1", hostId: office, kind: "approval", botId: "b7" });
  expect(routes).toEqual(["/chat/?bot=b7"]);
  await vi.waitFor(() => expect(saved.opened).toEqual([home, office]));
  await vi.waitFor(() => expect(getState().connection).toEqual({ status: "ready" }));

  tap({ v: "1", hostId: office, kind: "limit", botId: "b2" }); // already on that PC: no reconnect
  tap({ v: "1", hostId: office, kind: "approval", title: "A bot needs you", botId: "b3" }); // not a push this app sends
  expect(routes).toEqual(["/chat/?bot=b7", "/chat/?bot=b2"]);
  expect(saved.opened).toEqual([home, office]);
});
