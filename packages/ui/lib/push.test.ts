import type { Client } from "@tjommie/client";
import type { PushData } from "@tjommie/protocol";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { PushApi } from "./push";

const saved = vi.hoisted(() => ({ hosts: [] as { id: string; name: string }[], opened: [] as string[], events: [] as string[], nativeAvailable: false, clearFails: false }));
const native = vi.hoisted(() => ({ unregister: vi.fn(async () => {}) }));
vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => true, getPlatform: () => "android" },
  registerPlugin: () => ({ start: async () => {}, stop: async () => {}, available: async () => ({ value: saved.nativeAvailable }), addListener: async () => ({ remove: async () => {} }) }),
}));
vi.mock("@capacitor/push-notifications", () => ({ PushNotifications: {
  checkPermissions: async () => ({ receive: "denied" }), requestPermissions: async () => ({ receive: "denied" }),
  register: async () => {}, unregister: native.unregister, addListener: async () => ({ remove: async () => {} }),
} }));
vi.mock("./remote-hosts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./remote-hosts")>()),
  remoteHosts: () => saved.hosts,
  // Each saved PC answers the startup lists at once.
  openRemoteClient: async (id: string): Promise<Client> => {
    saved.opened.push(id);
    const answers: Record<string, unknown> = { "settings.get": { chainLimit: 5, relayUrl: null } };
    const call = async (method: string, params: unknown) => {
      if (method === "device.registerPush") {
        saved.events.push(`push:${id}:${JSON.stringify(params)}`);
        if (saved.clearFails) throw new Error("PC went offline");
      }
      return answers[method] ?? [];
    };
    return { call: call as unknown as Client["call"], events: [], subscribe: () => () => {}, disconnected: new Promise(() => {}), close: async () => {} };
  },
  forgetRemoteHost: async (id: string) => { saved.events.push(`forget:${id}`); saved.hosts = saved.hosts.filter((host) => host.id !== id); },
}));

const home = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11", office = "9c4f6b1d-2a7e-4e3b-8d55-0f6a1c2b3d4e";
const homeBot = "7a1ccbc4-d89e-4e14-80c7-f64b5edcc1b0", officeBot = "6b1f37ea-cf49-4ca4-885c-f30acc11b415";

// The phone's push as the connection sees it: whether this build has it, the permission, the token, and taps.
function fakePush(options: { available: boolean; permission: string }) {
  const listeners = new Map<string, (value: never) => void>();
  const push: PushApi = {
    available: () => options.available,
    checkPermissions: async () => ({ receive: options.permission }),
    requestPermissions: async () => ({ receive: options.permission }),
    register: async () => { queueMicrotask(() => listeners.get("registration")?.({ value: "synthetic-push-token" } as never)); },
    unregister: native.unregister,
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
  saved.hosts = []; saved.opened = []; saved.events = []; saved.nativeAvailable = false; saved.clearFails = false;
  native.unregister.mockClear();
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: () => {} });
});
afterEach(() => vi.unstubAllGlobals());

it("clears a connected PC's push token before forgetting its keys, then unregisters when it was the last saved PC", async () => {
  saved.hosts = [{ id: home, name: "Home PC" }];
  saved.nativeAvailable = true;
  const connection = await import("./connection");
  const { getState } = await import("./store");
  connection.startConnection();
  await vi.waitFor(() => expect(getState().connection.status).toBe("ready"));
  saved.events = [];
  await connection.forgetRemote(home);
  expect(saved.events).toEqual([`push:${home}:{"token":null}`, `forget:${home}`]);
  expect(native.unregister).toHaveBeenCalledOnce();
});

it("does not send a push-clear RPC to a different PC while forgetting it", async () => {
  saved.hosts = [{ id: home, name: "Home PC" }, { id: office, name: "Office PC" }];
  const connection = await import("./connection");
  const { getState } = await import("./store");
  connection.startConnection();
  await vi.waitFor(() => expect(getState().connection.status).toBe("ready"));
  saved.events = [];
  await connection.forgetRemote(office);
  expect(saved.events).toEqual([`forget:${office}`]);
  expect(native.unregister).not.toHaveBeenCalled();
});

it("still deletes the PC keys if clearing its push token fails", async () => {
  saved.hosts = [{ id: home, name: "Home PC" }];
  const connection = await import("./connection");
  const { getState } = await import("./store");
  connection.startConnection();
  await vi.waitFor(() => expect(getState().connection.status).toBe("ready"));
  saved.events = [];
  saved.clearFails = true;
  await connection.forgetRemote(home);
  expect(saved.events).toEqual([`push:${home}:{"token":null}`, `forget:${home}`]);
});

it("resets the cached token after unregistering so another pairing registers afresh", async () => {
  const { registerPushIfAllowed, unregisterPush } = await import("./push");
  const { push } = fakePush({ available: true, permission: "granted" });
  const register = vi.spyOn(push, "register");
  const { client } = recorder();
  await registerPushIfAllowed(client, push);
  await unregisterPush(push);
  await registerPushIfAllowed(client, push);
  expect(native.unregister).toHaveBeenCalledOnce();
  expect(register).toHaveBeenCalledTimes(2);
});

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
  onPushTap((data: PushData) => { const route = connection.routePush(data); if (route) routes.push(route); }, push);
  await Promise.resolve();
  tap({ v: "1", hostId: office, kind: "approval", botId: officeBot });
  expect(routes).toEqual([`/chat/?bot=${officeBot}`]);
  await vi.waitFor(() => expect(saved.opened).toEqual([home, office]));
  await vi.waitFor(() => expect(getState().connection).toEqual({ status: "ready" }));

  tap({ v: "1", hostId: office, kind: "limit", botId: officeBot }); // already on that PC: no reconnect
  tap({ v: "1", hostId: office, kind: "limit", botId: officeBot, from: "fcm-transport" });
  tap({ v: "1", hostId: office, kind: "approval", title: "A bot needs you", botId: homeBot }); // not a push this app sends
  expect(routes).toEqual([`/chat/?bot=${officeBot}`, `/chat/?bot=${officeBot}`, `/chat/?bot=${officeBot}`]);
  expect(saved.opened).toEqual([home, office]);

  tap({ v: "1", hostId: "a34620cb-9f84-47f1-a428-51259cc1f3b6", kind: "approval", botId: homeBot });
  tap({ v: "1", hostId: home, kind: "approval", botId: "javascript:alert(1)" });
  expect(routes).toEqual([`/chat/?bot=${officeBot}`, `/chat/?bot=${officeBot}`, `/chat/?bot=${officeBot}`]);
  expect(saved.opened).toEqual([home, office]);
});
