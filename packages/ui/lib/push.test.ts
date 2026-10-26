import { ClientConnectionError, type Client } from "@tjommie/client";
import type { PushData } from "@tjommie/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PushApi } from "./push";

const saved = vi.hoisted(() => ({
  hosts: [] as { id: string; name: string }[], opened: [] as string[], events: [] as string[], nativeAvailable: false, clearFails: false, clearHangs: false, forgetFails: false,
  connections: 0, sends: [] as string[], // each connection's number, and what each device.registerPush carried to which connection ("<host>#<n> <params>")
  opening: null as Promise<void> | null, lists: null as Promise<void> | null, disconnected: new Map<number, () => void>(),
}));
// This JS boundary model deletes on unregister completion; Android's unawaited deleteToken task is a separate native limitation.
const native = vi.hoisted(() => {
  const live = new Set<string>(); // the tokens Firebase still delivers to
  const listeners = new Map<string, Set<(value: never) => void>>();
  const retained = new Map<string, unknown[]>();
  return {
    live,
    issued: 0,
    permission: "denied",
    registering: null as Promise<void> | null,
    answers: [] as Promise<{ value: boolean }>[], // answers the push service gives late, to the next calls that ask whether push is in this build
    listeners, retained,
    emit(event: string, value: unknown) {
      const listening = listeners.get(event);
      if (!listening?.size) { retained.set(event, [...(retained.get(event) ?? []), value]); return; }
      for (const listener of listening) listener(value as never);
    },
    unregister: vi.fn(async () => { live.clear(); }),
  };
});
vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => true, getPlatform: () => "android" },
  registerPlugin: () => ({
    start: async () => {}, stop: async () => {}, available: () => native.answers.shift() ?? Promise.resolve({ value: saved.nativeAvailable }), addListener: async () => ({ remove: async () => {} }),
  }),
}));
vi.mock("@capacitor/push-notifications", () => ({ PushNotifications: {
  checkPermissions: async () => ({ receive: native.permission }), requestPermissions: async () => ({ receive: native.permission }),
  register: async () => {
    await native.registering;
    const value = `token-${(native.issued += 1)}`;
    native.live.add(value);
    void Promise.resolve().then(() => native.emit("registration", { value }));
  },
  unregister: native.unregister,
  addListener: async (event: string, listener: (value: never) => void) => {
    const listening = native.listeners.get(event) ?? new Set();
    native.listeners.set(event, listening.add(listener));
    const replay = native.retained.get(event) ?? [];
    native.retained.delete(event);
    for (const value of replay) native.emit(event, value);
    return { remove: async () => { listening.delete(listener); } };
  },
} }));
vi.mock("./remote-hosts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./remote-hosts")>()),
  remoteHosts: () => saved.hosts,
  // Each saved PC answers the startup lists at once.
  openRemoteClient: async (id: string): Promise<Client> => {
    await saved.opening;
    saved.opened.push(id);
    const connection = (saved.connections += 1);
    const answers: Record<string, unknown> = { "settings.get": { chainLimit: 5, relayUrl: null } };
    const call = async (method: string, params: unknown) => {
      if (method === "bots.list") await saved.lists;
      if (method === "device.registerPush") {
        saved.events.push(`push:${id}:${JSON.stringify(params)}`);
        saved.sends.push(`${id}#${connection} ${JSON.stringify(params)}`);
        if (saved.clearFails) throw new Error("PC went offline");
        if (saved.clearHangs) return new Promise(() => {}); // a half-dead link: no answer, and no close either
      }
      return answers[method] ?? [];
    };
    const disconnected = new Promise<ClientConnectionError>((resolve) => { saved.disconnected.set(connection, () => resolve(new ClientConnectionError(1006, "connection lost", true))); });
    return { call: call as unknown as Client["call"], events: [], subscribe: () => () => {}, disconnected, close: async () => {} };
  },
  forgetRemoteHost: async (id: string) => {
    if (saved.forgetFails) throw new Error("keys unavailable");
    saved.events.push(`forget:${id}`); saved.hosts = saved.hosts.filter((host) => host.id !== id);
  },
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
  saved.hosts = []; saved.opened = []; saved.events = []; saved.nativeAvailable = false; saved.clearFails = false; saved.clearHangs = false;
  saved.forgetFails = false;
  saved.connections = 0; saved.sends = [];
  saved.opening = null; saved.lists = null; saved.disconnected.clear(); native.registering = null;
  native.unregister.mockReset();
  native.live.clear(); native.issued = 0; native.permission = "denied"; native.answers.length = 0; native.listeners.clear();
  native.retained.clear();
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: () => {} });
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

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

it("waits at most 3 s on the PC and 3 s on the phone's push service, then forgets anyway", async () => {
  saved.hosts = [{ id: home, name: "Home PC" }];
  saved.nativeAvailable = true;
  vi.useFakeTimers();
  const connection = await import("./connection");
  const { getState } = await import("./store");
  connection.startConnection();
  await vi.waitFor(() => expect(getState().connection.status).toBe("ready"));
  saved.events = [];
  saved.clearHangs = true;
  native.unregister.mockImplementationOnce(() => new Promise(() => {}));
  let done = false;
  void connection.forgetRemote(home).then(() => { done = true; });
  await vi.advanceTimersByTimeAsync(2_999);
  expect(saved.events).toEqual([`push:${home}:{"token":null}`]);
  await vi.advanceTimersByTimeAsync(1);
  expect(saved.events).toEqual([`push:${home}:{"token":null}`, `forget:${home}`]);
  expect(native.unregister).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(2_999);
  expect(done).toBe(false);
  await vi.advanceTimersByTimeAsync(1);
  expect(done).toBe(true);
  expect(getState().connection.status).toBe("outside"); // the last PC is gone, so the phone is at its connect screen
});

describe("push reconciled through Forget and pairing, with the push service slow or ending out of order", () => {
  const lab = "3e8f2a6b-1c4d-4f7e-9a2b-6d5c4b3a2f1e";
  const deferred = <T,>() => {
    let resolve: (value: T) => void = () => {};
    const promise = new Promise<T>((done) => { resolve = done; });
    return { promise, resolve };
  };
  // The next unregister the push service is asked for ends when `ends` does, deleting the token there is then. Queued in the order they're asked for.
  const unregisterEnding = (ends: Promise<unknown>): void => {
    native.unregister.mockImplementationOnce(async () => { await ends; native.live.clear(); });
  };
  // A phone paired with Home PC, which holds the phone's token (token-1). Forget gives up on the push service after 3 s.
  async function phoneOnHomePc() {
    saved.hosts = [{ id: home, name: "Home PC" }];
    saved.nativeAvailable = true;
    native.permission = "granted";
    vi.useFakeTimers();
    const connection = await import("./connection");
    const { getState } = await import("./store");
    connection.startConnection();
    await vi.waitFor(() => expect(saved.events).toContain(`push:${home}:{"token":"token-1"}`));
    const connected = (): Promise<void> => vi.waitFor(() => expect(getState().connection.status).toBe("ready"));
    return {
      forget: async (id: string): Promise<void> => {
        void connection.forgetRemote(id);
        await vi.advanceTimersByTimeAsync(3_000);
        expect(getState().connection.status).toBe("outside");
      },
      pair: async (id: string): Promise<void> => {
        saved.hosts = [{ id, name: "Another PC" }];
        connection.connectRemote(id);
        await connected();
      },
      reconnect: async (id: string): Promise<void> => {
        connection.connectRemote(id);
        await connected();
      },
    };
  }
  // What a PC was given, from the `from`th event on.
  const sentTo = (id: string, from = 0): string[] => saved.events.slice(from).filter((event) => event.startsWith(`push:${id}:`)).map((event) => event.slice(`push:${id}:`.length));
  const given = (token: string): string => JSON.stringify({ token });

  it("pairs with the live token after auto-init was retained and startup unregistered it", async () => {
    saved.nativeAvailable = true; native.permission = "granted";
    native.live.add("auto-init-token");
    native.emit("registration", { value: "auto-init-token" });
    vi.useFakeTimers();
    const connection = await import("./connection");
    connection.startConnection();
    await vi.advanceTimersByTimeAsync(0);
    expect(native.live.size).toBe(0);
    saved.hosts = [{ id: home, name: "Home PC" }];
    connection.connectRemote(home);
    await vi.advanceTimersByTimeAsync(0);
    expect(sentTo(home).at(-1)).toBe(given("token-1"));
    expect(native.live.has("token-1")).toBe(true);
  });

  it("pairs with the latest live token after a duplicate onNewToken event and Forget", async () => {
    const phone = await phoneOnHomePc();
    native.emit("registration", { value: "token-1" });
    await phone.forget(home);
    await phone.pair(office);
    await vi.advanceTimersByTimeAsync(0);
    expect(sentTo(office).at(-1)).toBe(given("token-2"));
    expect(native.live.has("token-2")).toBe(true);
    expect(native.listeners.get("registration")?.size).toBe(1);
    expect(native.listeners.get("registrationError")?.size).toBe(1);
  });

  it.each([false, true])("coalesces rapid Forget/pair/Forget/pair, with U2's answer released before U1: %s", async (reverse) => {
    const phone = await phoneOnHomePc();
    const first = deferred<void>(), second = deferred<void>();
    unregisterEnding(first.promise); // U1, for Forget Home PC
    unregisterEnding(second.promise); // U2, for Forget Office PC
    await phone.forget(home); // U1 starts, and hangs
    const afterHome = saved.events.length;
    await phone.pair(office);
    await vi.advanceTimersByTimeAsync(5_000); // the queue stops waiting for U1
    await phone.forget(office); // U2 must coalesce into the latest desired state
    const afterOffice = saved.events.length;
    await phone.pair(lab);
    for (const end of reverse ? [second, first] : [first, second]) {
      end.resolve();
      await vi.advanceTimersByTimeAsync(0);
    }
    await vi.advanceTimersByTimeAsync(0);
    expect(native.unregister).toHaveBeenCalledOnce();
    expect(sentTo(lab)).toEqual([given("token-2")]);
    expect(native.issued).toBe(2);
    expect(native.live.has("token-2")).toBe(true);
    expect(sentTo(home, afterHome)).toEqual([]);
    expect(sentTo(office, afterOffice)).toEqual([]);
  });

  it("returns from Forget and pairs without overlapping an unregister that never ends", async () => {
    const phone = await phoneOnHomePc();
    unregisterEnding(new Promise(() => {}));
    await phone.forget(home);
    await phone.pair(office);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(sentTo(office)).toEqual([]);
    expect(native.issued).toBe(1);
    expect(native.unregister).toHaveBeenCalledOnce();
  });

  it("answers a late unregister with one registration, for the PC's newest connection only, however often it reconnected", async () => {
    const phone = await phoneOnHomePc();
    const ended = deferred<void>();
    unregisterEnding(ended.promise);
    await phone.forget(home);
    await phone.pair(office);
    await vi.advanceTimersByTimeAsync(5_000);
    for (let each = 0; each < 3; each += 1) await phone.reconnect(office);
    const issued = native.issued;
    ended.resolve(); // it deletes the phone's token
    await vi.advanceTimersByTimeAsync(1_000);
    expect(native.issued).toBe(issued + 1); // one registration, not one per connection
    const fresh = given(`token-${native.issued}`);
    expect(saved.sends.filter((each) => each.endsWith(fresh))).toEqual([`${office}#${saved.connections} ${fresh}`]);
    expect(native.live.has(`token-${native.issued}`)).toBe(true);
  });

  it("gives the next PC the phone's live token when the push service answers Forget only after that pairing (timeout, new pairing, late answer)", async () => {
    const phone = await phoneOnHomePc();
    const answer = deferred<{ value: boolean }>();
    native.answers.push(answer.promise);
    await phone.forget(home);
    await phone.pair(office);
    answer.resolve({ value: true }); // Forget's question is answered with a PC paired again: nothing is unregistered
    await vi.advanceTimersByTimeAsync(0);
    expect(sentTo(office)).toEqual([given("token-1")]);
    expect(native.unregister).not.toHaveBeenCalled();
    expect(native.live.has("token-1")).toBe(true);
  });

  it("registers afresh for the next PC once an unregister the queue waited on ends (timeout, late answer, new pairing)", async () => {
    const phone = await phoneOnHomePc();
    const answer = deferred<{ value: boolean }>(), ended = deferred<void>();
    native.answers.push(answer.promise);
    unregisterEnding(ended.promise);
    await phone.forget(home);
    answer.resolve({ value: true }); // answered after Forget gave up: the unregister starts
    await vi.advanceTimersByTimeAsync(0);
    await phone.pair(office);
    ended.resolve(); // within the 5 s the queue waits for it
    await vi.advanceTimersByTimeAsync(0);
    expect(sentTo(office)).toEqual([given("token-2")]);
    expect(native.live.has("token-2")).toBe(true);
  });

  it("does not restore a forgotten client when its welcome availability check finishes late", async () => {
    saved.hosts = [{ id: home, name: "Home PC" }];
    saved.nativeAvailable = true; native.permission = "granted";
    vi.useFakeTimers();
    const answer = deferred<{ value: boolean }>();
    native.answers.push(answer.promise);
    const connection = await import("./connection");
    connection.startConnection();
    await vi.waitFor(() => expect(saved.connections).toBe(1));
    void connection.forgetRemote(home);
    await vi.advanceTimersByTimeAsync(5_000);
    const afterForget = saved.sends.length;
    answer.resolve({ value: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.sends.slice(afterForget)).toEqual([]);
    expect(native.live.size).toBe(0);
  });

  it("keeps the newest reconnect when welcome checks answer out of order", async () => {
    const phone = await phoneOnHomePc();
    const older = deferred<{ value: boolean }>(), newer = deferred<{ value: boolean }>();
    native.answers.push(older.promise);
    await phone.reconnect(home);
    native.answers.push(newer.promise);
    await phone.reconnect(home);
    newer.resolve({ value: true });
    await vi.advanceTimersByTimeAsync(0);
    older.resolve({ value: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.sends.filter((send) => send.includes("#2 "))).toEqual([]);
    expect(saved.sends.at(-1)).toBe(`${home}#3 ${given("token-1")}`);
  });

  it.each([false, true])("restores push after Forget cannot delete keys only if still current; switched PC: %s", async (switched) => {
    const phone = await phoneOnHomePc();
    const connection = await import("./connection");
    saved.forgetFails = true;
    saved.clearHangs = switched;
    const rejected = expect(connection.forgetRemote(home)).rejects.toThrow("keys unavailable");
    if (switched) {
      saved.hosts.push({ id: office, name: "Office PC" });
      await phone.reconnect(office);
      await vi.advanceTimersByTimeAsync(3_000);
    }
    await rejected;
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.sends.at(-1)).toBe(`${switched ? `${office}#2` : `${home}#1`} ${given("token-1")}`);
    expect(saved.hosts.map((host) => host.id)).toEqual(switched ? [home, office] : [home]);
    expect(native.unregister).not.toHaveBeenCalled();
  });

  it.each(["reconnect", "loss", "hidden loss"])("clears the push target on %s before the replacement welcomes", async (event) => {
    saved.hosts = [{ id: home, name: "Home PC" }];
    saved.nativeAvailable = true; native.permission = "granted";
    vi.useFakeTimers();
    const registered = deferred<void>(), opened = deferred<void>();
    native.registering = registered.promise;
    const connection = await import("./connection");
    const { getState } = await import("./store");
    connection.startConnection();
    await vi.waitFor(() => expect(getState().connection.status).toBe("ready"));
    await vi.advanceTimersByTimeAsync(0);
    saved.opening = opened.promise;
    if (event === "hidden loss") vi.stubGlobal("document", { visibilityState: "hidden" });
    if (event === "reconnect") connection.connectRemote(home);
    else saved.disconnected.get(1)?.();
    await vi.advanceTimersByTimeAsync(0);
    registered.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.sends).toEqual([]);
    if (event === "hidden loss") connection.connectRemote(home);
    opened.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.sends).toEqual([`${home}#2 ${given("token-1")}`]);
  });

  it("sends registration and Forget's clear immediately even when all screen RPC slots are busy", async () => {
    saved.hosts = [{ id: home, name: "Home PC" }];
    saved.nativeAvailable = true; native.permission = "granted";
    vi.useFakeTimers();
    const registered = deferred<void>(), listed = deferred<void>();
    native.registering = registered.promise;
    const connection = await import("./connection");
    const { getState } = await import("./store");
    connection.startConnection();
    await vi.waitFor(() => expect(getState().connection.status).toBe("ready"));
    saved.lists = listed.promise;
    const screens = Array.from({ length: 16 }, () => connection.request("bots.list", {}));
    registered.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.sends).toEqual([`${home}#1 ${given("token-1")}`]);
    const forgotten = connection.forgetRemote(home);
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.sends.at(-1)).toBe(`${home}#1 {"token":null}`);
    await forgotten;
    listed.resolve();
    await Promise.all(screens);
    expect(saved.sends).toHaveLength(2);
  });
});

it("resets the cached token after unregistering so another pairing registers afresh", async () => {
  const { registerPushIfAllowed, unregisterPush } = await import("./push");
  const { push } = fakePush({ available: true, permission: "granted" });
  const register = vi.spyOn(push, "register");
  const { calls, client } = recorder();
  await registerPushIfAllowed(client, push);
  await unregisterPush(push);
  await registerPushIfAllowed(client, push);
  expect(native.unregister).toHaveBeenCalledOnce();
  expect(register).toHaveBeenCalledTimes(2);
  expect(calls).toHaveLength(2); // Firebase may reissue the same token; Forget still invalidates the PC's copy
});

it("unregisters on startup when no PC is saved", async () => {
  saved.nativeAvailable = true;
  native.live.add("previous-session-token");
  native.unregister.mockImplementation(async () => { native.live.clear(); });
  vi.useFakeTimers();
  const connection = await import("./connection");
  connection.startConnection();
  await vi.advanceTimersByTimeAsync(0);
  expect(native.unregister).toHaveBeenCalledOnce();
  expect(native.live.size).toBe(0);
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
