import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { syncKeepAlive } from "@/lib/keep-alive";
import type { PushApi } from "@/lib/push";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { clearPresses, press } from "../press-fixture";
import { PhoneSettings, allowNotifications, notificationState } from "./this-phone";

// The phone's TjommieKeepAlive service, and the PC behind request.
const plugin = vi.hoisted(() => ({
  start: vi.fn<(options: { hostName: string }) => Promise<void>>(async () => {}), stop: vi.fn(async () => {}), status: vi.fn(async () => ({ running: false })),
  batteryUnrestricted: vi.fn(async () => ({ value: false })), openBatterySettings: vi.fn(async () => {}),
}));
const requested = vi.hoisted(() => vi.fn<(...call: unknown[]) => Promise<object>>(async () => ({})));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => true, getPlatform: () => "android" }, registerPlugin: () => plugin }));
vi.mock("@/lib/connection", () => ({ request: requested }));
vi.mock("@/components/pill", async (original) => (await import("@/components/press-fixture")).recordingPill(await original()));
vi.mock("@/components/controls", async (original) => (await import("@/components/press-fixture")).recordingSwitch(await original()));

const saved = new Map<string, string>();
vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key) ?? null, setItem: (key: string, value: string) => void saved.set(key, value) });
const SETTING = "tjommie.mobile.keepAlive.v1";

const html = (props: Partial<ComponentProps<typeof PhoneSettings>> = {}) =>
  renderToStaticMarkup(<PhoneSettings restricted={false} notifications="on" onNotifications={() => {}} {...props} />);

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
    addListener: (async (event: string, listener: (value: never) => void) => {
      listeners.set(event, listener);
      return { remove: async () => { listeners.delete(event); } };
    }) as PushApi["addListener"],
  };
}

beforeEach(() => {
  syncKeepAlive("outside"); // no service left running from the test before
  clearPresses();
  vi.clearAllMocks();
  saved.clear();
  update(() => ({ ...initialState, connection: { status: "ready" } }));
});

describe("PhoneSettings", () => {
  it("has Always connected, on unless turned off, saying what it costs", () => {
    const out = html();
    expect(out).toContain("Always connected");
    expect(out).toContain("Keeps a notification showing so Tjommie stays connected in the background. Uses a little more battery.");
    expect(out).toMatch(/aria-checked="true"[^>]*aria-label="Always connected"|aria-label="Always connected"[^>]*aria-checked="true"/);
    saved.set(SETTING, "false");
    expect(html()).toMatch(/aria-checked="false"[^>]*aria-label="Always connected"|aria-label="Always connected"[^>]*aria-checked="false"/);
  });

  it("starts and stops the keep-alive service with the switch, and remembers the choice", () => {
    saved.set(SETTING, "false");
    syncKeepAlive("ready", "Home PC"); // the connection says which PC is up; with the switch off nothing starts
    expect(plugin.start).not.toHaveBeenCalled();
    html();
    press("Always connected", true);
    expect(plugin.start).toHaveBeenCalledWith({ hostName: "Home PC" });
    expect(saved.get(SETTING)).toBe("true");
    press("Always connected", false);
    expect(plugin.stop).toHaveBeenCalledTimes(1);
    expect(saved.get(SETTING)).toBe("false");
  });

  it("asks to let Tjommie run in the background while the battery restricts it, with the Samsung steps, and opens the battery settings", () => {
    const out = html({ restricted: true });
    expect(out).toContain("Let Tjommie run in the background");
    expect(out).toContain("On Samsung: Settings → Battery → Background usage limits → Never sleeping apps → add Tjommie.");
    press("Battery settings");
    expect(plugin.openBatterySettings).toHaveBeenCalledTimes(1);
    expect(html({ restricted: false })).not.toContain("Let Tjommie run in the background");
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
    await vi.waitFor(() => expect(shown).toHaveBeenCalledWith("unavailable")); // this build has no push yet (T7 plugs it in)
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
    expect(await allowNotifications(fakePush({ permission: "prompt", answer: "granted" }))).toBe("on");
    expect(requested).toHaveBeenCalledWith("device.registerPush", { token: "synthetic-push-token" });
  });

  it("gives the PC nothing on no", async () => {
    expect(await allowNotifications(fakePush({ permission: "prompt", answer: "denied" }))).toBe("off");
    expect(requested).not.toHaveBeenCalled();
  });

  it("is Not in this build without push", async () => {
    expect(await allowNotifications(null)).toBe("unavailable");
  });
});
