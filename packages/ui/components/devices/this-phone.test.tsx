import type { ComponentProps } from "react";
import type { Client } from "@tjommie/client";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { registerPushIfAllowed, stopSendingPush, unregisterPush, type PushApi } from "@/lib/push";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { clearPresses, press } from "../press-fixture";
import { PhoneSettings, allowNotifications, notificationState } from "./this-phone";

const plugin = vi.hoisted(() => ({ available: vi.fn(async () => ({ value: false })) }));
const requested = vi.hoisted(() => vi.fn<(...call: unknown[]) => Promise<object>>(async () => ({})));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => true, getPlatform: () => "android" }, registerPlugin: () => plugin }));
vi.mock("@/lib/connection", () => ({ request: requested }));
vi.mock("@/components/pill", async (original) => (await import("@/components/press-fixture")).recordingPill(await original()));
vi.mock("@/components/controls", async (original) => (await import("@/components/press-fixture")).recordingSwitch(await original()));

const saved = new Map<string, string>();
vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key) ?? null, setItem: (key: string, value: string) => void saved.set(key, value) });

const html = (props: Partial<ComponentProps<typeof PhoneSettings>> = {}) =>
  renderToStaticMarkup(<PhoneSettings restricted={false} notifications="on" onNotifications={() => {}} {...props} />);
// Each switch on the page, as its label and whether it's on.
const switches = (out: string): [string | undefined, boolean][] =>
  [...out.matchAll(/<span[^>]*role="switch"[^>]*>/g)].map(([tag]) => [/aria-label="([^"]+)"/.exec(tag)?.[1], tag.includes('aria-checked="true"')]);
const ALL = ["approval", "limit", "routine_failed", "done", "failed"];

// The phone's push as the official plugin has it: whether this build has it, the permission now and after asking, and a token on register.
function fakePush(options: { available?: boolean; permission: string; answer?: string }): PushApi {
  const listeners = new Map<string, (value: never) => void>();
  return {
    available: () => options.available ?? true,
    checkPermissions: async () => ({ receive: options.permission }),
    requestPermissions: async () => {
      options.permission = options.answer ?? options.permission;
      return { receive: options.permission };
    },
    register: async () => { queueMicrotask(() => listeners.get("registration")?.({ value: "synthetic-push-token" } as never)); },
    unregister: async () => {},
    addListener: (async (event: string, listener: (value: never) => void) => {
      listeners.set(event, listener);
      return { remove: async () => { listeners.delete(event); } };
    }) as PushApi["addListener"],
  };
}

beforeEach(() => {
  stopSendingPush();
  clearPresses();
  vi.clearAllMocks();
  saved.clear();
  update(() => ({ ...initialState, connection: { status: "ready" } }));
});

describe("PhoneSettings", () => {
  it("hides Always connected and battery controls after the background spike", () => {
    const out = html({ restricted: true });
    expect(out).not.toContain("Always connected");
    expect(out).not.toContain("Let Tjommie run in the background");
  });

  it("says where notifications stand: Allow, On, Off, or Not in this build", () => {
    expect(html({ notifications: "allow" })).toContain(">Allow<");
    expect(html({ notifications: "on" })).toMatch(/Notifications[\s\S]*>On</);
    expect(html({ notifications: "off" })).toMatch(/Notifications[\s\S]*>Off</);
    expect(html({ notifications: "unavailable" })).toContain("Not in this build");
  });

  it("asks for notifications on Allow and shows the answer", async () => {
    const shown = vi.fn();
    html({ notifications: "allow", onNotifications: shown });
    press("Allow");
    await vi.waitFor(() => expect(shown).toHaveBeenCalledWith("unavailable"));
  });

  it("offers the three kinds of notification, all on, only while notifications are On", () => {
    expect(switches(html({ notifications: "on" }))).toEqual([["A bot needs you", true], ["A bot is done", true], ["Something failed", true]]);
    for (const notifications of ["allow", "off", "unavailable", null] as const) expect(switches(html({ notifications }))).toEqual([]);
  });

  it.each([
    ["A bot needs you", ["routine_failed", "done", "failed"]],
    ["A bot is done", ["approval", "limit", "routine_failed", "failed"]],
    ["Something failed", ["approval", "limit", "done"]],
  ])("turning off %s registers again without its kinds, and stays off", async (label, kinds) => {
    await registerPushIfAllowed({ call: requested as Client["call"] }, fakePush({ permission: "granted" }));
    expect(requested).toHaveBeenLastCalledWith("device.registerPush", { token: "synthetic-push-token", kinds: ALL });
    html();
    press(label, false);
    await vi.waitFor(() => expect(requested).toHaveBeenLastCalledWith("device.registerPush", { token: "synthetic-push-token", kinds }));
    expect(switches(html()).filter(([, on]) => !on).map(([name]) => name)).toEqual([label]);
  });

  it("turning a switch back on asks for its kinds again", async () => {
    await registerPushIfAllowed({ call: requested as Client["call"] }, fakePush({ permission: "granted" }));
    html();
    press("A bot is done", false);
    await vi.waitFor(() => expect(requested).toHaveBeenLastCalledWith("device.registerPush", { token: "synthetic-push-token", kinds: ["approval", "limit", "routine_failed", "failed"] }));
    html();
    press("A bot is done", true);
    await vi.waitFor(() => expect(requested).toHaveBeenLastCalledWith("device.registerPush", { token: "synthetic-push-token", kinds: ALL }));
    expect(switches(html()).every(([, on]) => on)).toBe(true);
  });
});

describe("notificationState", () => {
  it("is Not in this build without push, Allow before asking, then On or Off as answered", async () => {
    expect(await notificationState(null)).toBe("unavailable");
    expect(await notificationState(fakePush({ available: false, permission: "granted" }))).toBe("unavailable");
    expect(await notificationState(fakePush({ permission: "prompt" }))).toBe("allow");
    expect(await notificationState(fakePush({ permission: "prompt-with-rationale" }))).toBe("allow");
    expect(await notificationState(fakePush({ permission: "granted" }))).toBe("on");
    expect(await notificationState(fakePush({ permission: "denied" }))).toBe("off");
  });
});

describe("allowNotifications", () => {
  it("asks once and, on yes, gives the PC this phone's push token", async () => {
    const push = fakePush({ permission: "prompt", answer: "granted" });
    await registerPushIfAllowed({ call: requested as Client["call"] }, push); // welcome before Allow: no permission yet
    expect(requested).not.toHaveBeenCalled();
    expect(await allowNotifications(push)).toBe("on");
    expect(requested).toHaveBeenCalledWith("device.registerPush", { token: "synthetic-push-token", kinds: ALL });
  });

  it("gives the PC nothing on no", async () => {
    expect(await allowNotifications(fakePush({ permission: "prompt", answer: "denied" }))).toBe("off");
    expect(requested).not.toHaveBeenCalled();
  });

  it("is Not in this build without push", async () => {
    expect(await allowNotifications(null)).toBe("unavailable");
  });

  it("does not revive a forgotten PC when the permission dialog answers late", async () => {
    const options = { permission: "prompt", answer: "granted" };
    const push = fakePush(options);
    const register = vi.spyOn(push, "register");
    let answer: (value: { receive: string }) => void = () => {};
    const asking = vi.spyOn(push, "requestPermissions").mockImplementation(() => new Promise((resolve) => { answer = resolve; }));
    await registerPushIfAllowed({ call: requested as Client["call"] }, push);
    const allowing = allowNotifications(push);
    await vi.waitFor(() => expect(asking).toHaveBeenCalledOnce());
    stopSendingPush();
    await unregisterPush(push);
    options.permission = "granted";
    answer({ receive: "granted" });
    expect(await allowing).toBe("on");
    expect(register).not.toHaveBeenCalled();
    expect(requested).not.toHaveBeenCalled();
  });
});
