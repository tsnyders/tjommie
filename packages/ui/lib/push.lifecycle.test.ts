import type { Client } from "@tjommie/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { PushApi } from "./push";

vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => false }, registerPlugin: () => ({}) }));

const flush = async (): Promise<void> => { await vi.advanceTimersByTimeAsync(0); };
beforeEach(() => { vi.resetModules(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

// Bridge calls and token events settle independently. This JS model still completes deletion with unregister;
// Android's fire-and-forget deleteToken completion needs a native contract change to model faithfully.
async function phone() {
  const lifecycle = await import("./push");
  type Target = Pick<Client, "call">;
  type Call = { kind: string; settled: boolean; finish(): void; fail(): void };
  const calls: Call[] = [];
  const listeners = new Map<string, Set<(value: never) => void>>();
  const retained = new Map<string, unknown[]>();
  const events: (() => void)[] = [];
  const stored = new Map<Target, string>();
  const wrong: Target[] = [];
  const sends: Target[] = [];
  let current: Target | null = null;
  let saved = false;
  let active = 0;
  let peak = 0;
  let live: string | null = null;
  let issued = 0;
  let automatic = true;
  let automaticEvents = true;
  let replayed = 0;
  let permission = "granted";
  let rpc: () => Promise<object> = async () => ({});
  function native<T>(kind: string, complete: () => T): Promise<T> {
    active += 1;
    peak = Math.max(peak, active);
    return new Promise<T>((resolve, reject) => {
      const call: Call = {
        kind, settled: false,
        finish() {
          if (call.settled) return;
          call.settled = true;
          active -= 1;
          resolve(complete());
        },
        fail() {
          if (call.settled) return;
          call.settled = true;
          active -= 1;
          reject(new Error("native failed"));
        },
      };
      calls.push(call);
      if (automatic) void Promise.resolve().then(call.finish);
    });
  }
  const emit = (event: string, value: unknown): void => {
    const group = listeners.get(event);
    if (!group?.size) { retained.set(event, [...(retained.get(event) ?? []), value]); return; }
    for (const listener of group) listener(value as never);
  };
  const rotate = (): void => { live = `token-${++issued}`; emit("registration", { value: live }); };
  const deliver = (): void => { events.shift()?.(); };
  const api: PushApi = {
    available: () => native("available", () => true),
    checkPermissions: () => native("permission", () => ({ receive: permission })),
    requestPermissions: () => native("request", () => ({ receive: permission })),
    register: () => native("register", () => {
      events.push(() => { if (live === null) live = `token-${++issued}`; emit("registration", { value: live }); });
      if (automaticEvents) void Promise.resolve().then(deliver);
    }),
    unregister: () => native("unregister", () => { live = null; }),
    addListener: ((event: string, listener: (value: never) => void) => native("listen", () => {
      const group = listeners.get(event) ?? new Set();
      listeners.set(event, group);
      group.add(listener);
      const replay = retained.get(event) ?? [];
      retained.delete(event);
      replayed += replay.length;
      for (const value of replay) emit(event, value);
      return { remove: () => native("remove", () => { group.delete(listener); }) };
    })) as PushApi["addListener"],
  };
  function client(): Target {
    const target: Target = {
      call: (async (_method: string, params: { token: string }) => {
        if (target !== current) wrong.push(target);
        stored.set(target, params.token);
        sends.push(target);
        return rpc();
      }) as Client["call"],
    };
    return target;
  }
  function invariant(): void {
    expect(wrong, "I1: send must target the current client").toEqual([]);
    expect(peak, "I4: at most one native call can be pending").toBeLessThanOrEqual(1);
    expect(active).toBeLessThanOrEqual(1);
  }
  const pending = (): Call[] => calls.filter((call) => !call.settled);
  return {
    lifecycle, api, calls, pending, client, stored, sends, invariant, emit, rotate, deliver, events, retained,
    live: () => live,
    issued: () => issued,
    listeners: () => [...listeners.values()].reduce((count, group) => count + group.size, 0),
    replayed: () => replayed,
    permission: (value: string) => { permission = value; },
    automatic: (value: boolean) => { automatic = value; },
    automaticEvents: (value: boolean) => { automaticEvents = value; },
    rpc: (work: () => Promise<object>) => { rpc = work; },
    pair() { saved = true; current = null; lifecycle.stopSendingPush(); },
    welcome(target = client()) { saved = true; current = target; return lifecycle.registerPushIfAllowed(target, api); },
    loss() { current = null; lifecycle.stopSendingPush(); },
    forget() { saved = false; current = null; return lifecycle.unregisterPush(api); },
    async quiesce() {
      automatic = true;
      automaticEvents = true;
      for (let step = 0; step < 20; step += 1) {
        for (const call of pending()) call.finish();
        while (events.length) deliver();
        await flush();
        invariant();
        if (!pending().length && !events.length) break;
      }
      expect(pending()).toEqual([]);
      expect([...listeners.values()].every((group) => group.size === 1), "one permanent listener per event").toBe(true);
      expect(calls.filter((call) => call.kind === "remove"), "token listeners stay attached for the app's lifetime").toEqual([]);
      if (!saved) expect(live, "I2: without a saved PC the phone is unregistered").toBeNull();
      else if (current && permission === "granted") {
        expect(live, "I2: allowed current client has a live token").not.toBeNull();
        expect(stored.get(current), "I2: current client has the latest live token").toBe(live);
      }
    },
  };
}

it.each(["available", "permission"])("discards a late welcome %s check after Forget", async (kind) => {
  const p = await phone();
  p.automatic(false);
  let welcomeDone = false;
  void p.welcome().then(() => { welcomeDone = true; });
  await flush();
  if (kind === "permission") { p.pending()[0].finish(); await flush(); }
  expect(p.pending()[0].kind).toBe(kind);
  void p.forget();
  await vi.advanceTimersByTimeAsync(5_000);
  expect(welcomeDone, "I3: a stalled welcome check returns after 5 s").toBe(true);
  await p.quiesce();
  expect(p.sends).toEqual([]);
});

it("keeps the latest of two reconnects when their welcome answers are released in reverse order", async () => {
  const p = await phone();
  await p.welcome();
  p.automatic(false);
  void p.welcome();
  await flush();
  void p.welcome();
  await flush();
  for (const call of p.pending().toReversed()) call.finish();
  await p.quiesce();
  expect(p.sends).toHaveLength(2); // first welcome, then only the newest reconnect
});

it.each([false, true])("coalesces r2 Forget/pair/Forget/pair with the second unregister released first: %s", async (reverse) => {
  const p = await phone();
  await p.welcome();
  p.automatic(false);
  void p.forget();
  await flush();
  p.pending()[0].finish(); // availability
  await flush();
  expect(p.pending()[0].kind).toBe("unregister");
  await vi.advanceTimersByTimeAsync(5_000);
  p.pair(); void p.welcome();
  void p.forget();
  p.pair(); void p.welcome();
  await vi.advanceTimersByTimeAsync(5_000);
  const unregisters = p.pending().filter((call) => call.kind === "unregister");
  for (const call of reverse ? unregisters.toReversed() : unregisters) call.finish();
  await p.quiesce();
  expect(p.calls.filter((call) => call.kind === "unregister")).toHaveLength(1); // U2 must never start beside U1
  expect(p.issued()).toBe(2);
});

it("bounds a stalled token RPC and then registers only the newest client", async () => {
  const p = await phone();
  p.rpc(() => new Promise(() => {}));
  let done = false;
  void p.welcome().then(() => { done = true; });
  await flush();
  expect(p.sends).toHaveLength(1);
  p.pair();
  p.rpc(async () => ({}));
  void p.welcome();
  await vi.advanceTimersByTimeAsync(4_999);
  expect(done).toBe(false);
  await vi.advanceTimersByTimeAsync(1);
  expect(done).toBe(true);
  await p.quiesce();
  expect(p.sends).toHaveLength(2);
});

it("retains one native slot across repeated Forget/pair cycles while unregister never settles", async () => {
  const p = await phone();
  await p.welcome();
  p.automatic(false);
  void p.forget(); await flush();
  p.pending()[0].finish(); await flush();
  const stuck = p.pending()[0];
  expect(stuck.kind).toBe("unregister");
  await vi.advanceTimersByTimeAsync(5_000);
  const count = p.calls.length;
  for (let cycle = 0; cycle < 25; cycle += 1) {
    p.pair(); void p.welcome();
    await flush();
    void p.forget();
    await vi.advanceTimersByTimeAsync(5_000);
    p.invariant();
    expect(p.calls).toHaveLength(count);
    expect(p.pending()).toEqual([stuck]);
    expect(vi.getTimerCount()).toBe(0);
  }
  p.pair(); void p.welcome();
  await p.quiesce();
  expect(p.issued()).toBe(2);
});

it.each(["listen", "register"])("keeps a hung %s in the slot and reconciles its late completion after Forget", async (kind) => {
  const p = await phone();
  p.automatic(false);
  let done = false;
  void p.welcome().then(() => { done = true; });
  await flush();
  for (let step = 0; p.pending()[0]?.kind !== kind && step < 10; step += 1) {
    expect(p.pending()).toHaveLength(1);
    p.pending()[0].finish(); await flush();
  }
  expect(p.pending()[0].kind).toBe(kind);
  void p.forget();
  await vi.advanceTimersByTimeAsync(5_000);
  expect(done).toBe(true);
  p.invariant();
  await p.quiesce();
  expect(p.sends).toEqual([]);
});

it("does not release the native slot when the registration event arrives before the bridge call settles", async () => {
  const p = await phone();
  p.automatic(false);
  void p.welcome(); await flush();
  while (p.pending()[0]?.kind !== "register") { p.pending()[0].finish(); await flush(); }
  p.emit("registration", { value: "token-1" });
  await flush();
  void p.forget();
  await vi.advanceTimersByTimeAsync(5_000);
  p.invariant();
  expect(p.pending().map((call) => call.kind)).toEqual(["register"]);
  await p.quiesce();
  expect(p.sends).toEqual([]);
});

it.each(["available", "permission", "listen", "register"])("handles a rejected %s without spinning and retries at the next welcome", async (kind) => {
  const p = await phone();
  p.automatic(false);
  void p.welcome(); await flush();
  while (p.pending()[0]?.kind !== kind) { p.pending()[0].finish(); await flush(); }
  p.automatic(true);
  p.pending()[0].fail();
  await flush();
  const count = p.calls.length;
  await vi.advanceTimersByTimeAsync(10_000);
  expect(p.calls).toHaveLength(count);
  expect(p.listeners()).toBeLessThanOrEqual(2);
  await p.welcome();
  await p.quiesce();
});

it("uses the latest live token on first pairing after retained auto-init and startup unregister", async () => {
  const p = await phone();
  p.rotate(); // Firebase auto-init before JS has listeners
  expect(p.retained.get("registration")).toHaveLength(1);
  await p.forget(); // startup with no saved PC deletes the auto-init token
  expect(p.live()).toBeNull();
  await p.welcome();
  await p.quiesce();
  expect(p.replayed()).toBe(1);
  expect(p.live()).toBe("token-2");
  expect(p.calls.filter((call) => call.kind === "listen")).toHaveLength(2);
});

it("keeps the listeners through Forget/pair and consumes a late onNewToken copy without replaying a deleted token", async () => {
  const p = await phone();
  await p.welcome();
  p.emit("registration", { value: p.live() }); // onNewToken's delayed copy after getToken's event
  await p.forget();
  expect(p.live()).toBeNull();
  p.pair(); await p.welcome();
  await p.quiesce();
  expect(p.live()).toBe("token-2");
  expect(p.replayed()).toBe(0);
  expect(p.calls.filter((call) => call.kind === "listen")).toHaveLength(2);
});

it("updates the current PC for every token event without a register pending", async () => {
  const p = await phone();
  await p.welcome();
  const count = p.calls.length;
  p.rotate(); await flush();
  p.rotate(); await flush();
  await p.quiesce();
  expect(p.calls).toHaveLength(count);
  expect(p.sends).toHaveLength(3);
  expect(p.live()).toBe("token-3");
  p.loss();
  p.rotate(); await flush();
  expect(p.sends).toHaveLength(3);
  await p.welcome();
  await p.quiesce();
});

it("does not repeat register while awaiting events, and observes every later token", async () => {
  const p = await phone();
  p.automaticEvents(false);
  let done = false;
  void p.welcome().then(() => { done = true; }); // the bridge resolves with no token event yet
  await flush();
  expect(done).toBe(true);
  await vi.advanceTimersByTimeAsync(10_000);
  expect(p.calls.filter((call) => call.kind === "register")).toHaveLength(1);
  expect(p.events).toHaveLength(1);
  expect(p.sends).toHaveLength(0);
  p.deliver(); await flush();
  p.rotate(); await flush();
  await p.quiesce();
  expect(p.sends).toHaveLength(2);
});

it("replays old errors before mutations and accepts tokens after an error without removing listeners", async () => {
  const p = await phone();
  p.rotate();
  p.emit("registrationError", { error: "old retained error" });
  await p.forget();
  await p.quiesce();
  p.automaticEvents(false);
  await p.welcome();
  p.emit("registrationError", { error: "getToken failed" }); await flush();
  expect(p.calls.filter((call) => call.kind === "register")).toHaveLength(1);
  p.deliver(); await flush();
  await p.quiesce();
  expect(p.replayed()).toBe(2);
  expect(p.sends).toHaveLength(1);
});

it("preserves I1/I4 at every seeded random step and I2 after activity stops", async () => {
  const seed = 0x37a4f19;
  let randomState = seed;
  const random = (): number => { randomState = (Math.imul(randomState, 1664525) + 1013904223) >>> 0; return randomState; };
  const p = await phone();
  const history: number[] = [];
  let duringRandom = 0;
  try {
    p.rotate(); // retained before the first listener exists, then replayed by addListener
    // Quiesce repeatedly so the run explores late completions as well as long stalls.
    for (let episode = 0; episode < 30; episode += 1) {
      p.automatic(false);
      p.automaticEvents(false);
      for (let step = 0; step < 30; step += 1) {
        const event = (random() >>> 16) % 12;
        const before = p.sends.length;
        history.push(event);
        switch (event) {
          case 0: void p.forget(); break;
          case 1: p.pair(); void p.welcome(); break;
          case 2: void p.welcome(); break;
          case 3: p.loss(); void p.welcome(); break;
          case 4: p.loss(); break;
          case 5: case 6: case 7: case 8: {
            // Short completion bursts let sends race subsequent changes during the episode, not just at quiescence.
            const burst = 1 + (random() >>> 16) % 8;
            for (let each = 0; each < burst; each += 1) {
              if (p.events.length && (random() & 1)) p.deliver();
              else p.pending()[0]?.finish();
              await flush();
              p.invariant();
            }
            break;
          }
          case 9: await vi.advanceTimersByTimeAsync(5_000); break;
          case 10: p.rotate(); break; // unsolicited onNewToken, including while no register is pending
          case 11: p.deliver(); break; // the event can follow its bridge completion much later
        }
        await flush();
        p.invariant();
        duringRandom += p.sends.length - before;
      }
      await p.quiesce();
    }
    expect(p.replayed(), "retained token events were replayed").toBeGreaterThan(0);
    expect(duringRandom, "sends during random steps, before quiescence").toBeGreaterThanOrEqual(40);
  } catch (error) {
    throw new Error(`push interleaving seed=${seed} (0x${seed.toString(16)}), events=${history.join(",")}`, { cause: error });
  }
});
