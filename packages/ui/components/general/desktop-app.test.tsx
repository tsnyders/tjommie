// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DesktopApi, UpdateState } from "@/lib/desktop";
import { DesktopApp } from "./desktop-app";

Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const LOGIN = "Start Tjommie when you sign in to Windows";
const roots: Root[] = [];

// The desktop app's login item and updater, as its preload exposes them; each call answers as the test says, and emit() is the updater's own news.
function stubBridge(answers: { get?: () => Promise<boolean>; set?: (on: boolean) => Promise<void>; state?: () => Promise<UpdateState>; check?: () => Promise<void> } = {}) {
  const listeners: ((state: UpdateState) => void)[] = [];
  const bridge = {
    loginItem: { get: vi.fn(answers.get ?? (async () => true)), set: vi.fn(answers.set ?? (async () => {})) },
    updates: {
      state: vi.fn(answers.state ?? (async (): Promise<UpdateState> => ({ status: "none" }))),
      onState: vi.fn((listener: (state: UpdateState) => void) => {
        listeners.push(listener);
        return () => {};
      }),
      check: vi.fn(answers.check ?? (async () => {})),
      restart: vi.fn(),
    },
  };
  window.tjommie = bridge as unknown as DesktopApi;
  return { bridge, emit: (state: UpdateState) => act(async () => listeners.forEach((listener) => listener(state))) };
}

const settle = (): Promise<void> => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
async function mount(): Promise<HTMLElement> {
  const host = document.body.appendChild(document.createElement("div"));
  const root = createRoot(host);
  roots.push(root);
  await act(async () => root.render(<DesktopApp />));
  await settle();
  return host;
}
const button = (host: HTMLElement, text: string): HTMLButtonElement | undefined => [...host.querySelectorAll("button")].find((each) => each.textContent === text);
const loginSwitch = (host: HTMLElement): HTMLElement => {
  const found = host.querySelector<HTMLElement>(`[role="switch"][aria-label="${LOGIN}"]`);
  if (!found) throw new Error("No sign-in switch");
  return found;
};

afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  document.body.replaceChildren();
  delete window.tjommie;
});

describe("DesktopApp", () => {
  it("isn't there outside the desktop app, or in one from before start-at-sign-in and updates", async () => {
    expect((await mount()).textContent).toBe("");
    window.tjommie = {} as DesktopApi;
    expect((await mount()).textContent).toBe("");
  });

  it("shows starting at sign-in as the desktop app says, and turns it off through it", async () => {
    const { bridge } = stubBridge();
    const host = await mount();
    expect(host.textContent).toContain(LOGIN);
    expect(loginSwitch(host).getAttribute("aria-checked")).toBe("true");
    await act(async () => loginSwitch(host).click());
    await settle();
    expect(bridge.loginItem.set).toHaveBeenCalledWith(false);
    expect(loginSwitch(host).getAttribute("aria-checked")).toBe("false");
  });

  it("keeps the switch at what the desktop app last confirmed, and says why, when a change fails", async () => {
    stubBridge({ set: async () => { throw new Error("Access is denied."); } });
    const host = await mount();
    await act(async () => loginSwitch(host).click());
    await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toBe("Couldn't change this setting: Access is denied.");
    expect(loginSwitch(host).getAttribute("aria-checked")).toBe("true");
  });

  it("says why when it can't read the setting, and doesn't let the switch guess", async () => {
    const { bridge } = stubBridge({ get: async () => { throw new Error("The desktop app didn't answer."); } });
    const host = await mount();
    expect(host.querySelector('[role="alert"]')?.textContent).toBe("Couldn't read this setting: The desktop app didn't answer.");
    await act(async () => loginSwitch(host).click());
    await settle();
    expect(bridge.loginItem.set).not.toHaveBeenCalled();
  });

  it("says where updates stand, as the updater reports each change", async () => {
    const { emit } = stubBridge();
    const host = await mount();
    expect(host.textContent).toContain("Up to date");
    await emit({ status: "checking" });
    expect(host.textContent).toContain("Checking");
    await emit({ status: "downloading", percent: 42.4 });
    expect(host.textContent).toContain("Downloading 42%");
    await emit({ status: "error", message: "Network is down." });
    expect(host.textContent).toContain("Couldn't check: Network is down.");
    const detail = `HTTP 404: ${"x".repeat(140)}`;
    await emit({ status: "error", message: `${detail}\r\nHeaders: { authorization: secret }` });
    expect(Array.from(host.querySelectorAll("p")).find((row) => row.textContent?.startsWith("Couldn't check: HTTP 404:"))?.textContent)
      .toBe(`Couldn't check: ${detail.slice(0, 120)}`);
  });

  it("says it couldn't check when the updater's state can't be read, and Check now tries again", async () => {
    const { bridge } = stubBridge({ state: async () => { throw new Error("The updater didn't answer."); } });
    const host = await mount();
    expect(host.textContent).toContain("Couldn't check: The updater didn't answer.");
    await act(async () => button(host, "Check now")?.click());
    expect(bridge.updates.check).toHaveBeenCalledTimes(1);
  });

  it("says it couldn't check when Check now fails, instead of staying Up to date", async () => {
    stubBridge({ check: async () => { throw new Error("Network is down."); } });
    const host = await mount();
    await act(async () => button(host, "Check now")?.click());
    await settle();
    expect(host.textContent).toContain("Couldn't check: Network is down.");
    expect(host.textContent).not.toContain("Up to date");
  });

  it("doesn't offer Check now while a check or a download runs", async () => {
    const { emit } = stubBridge();
    const host = await mount();
    for (const busy of [{ status: "checking" }, { status: "downloading", percent: 10 }] as const) {
      await emit(busy);
      expect(button(host, "Check now")?.disabled).toBe(true);
    }
  });

  it("offers Restart once a version is ready, in the brief's words, and restarts through the desktop app", async () => {
    const { bridge } = stubBridge({ state: async () => ({ status: "ready", version: "0.3.1" }) });
    const host = await mount();
    expect(host.textContent).toContain("Ready: Restart to update to v0.3.1");
    expect(button(host, "Check now")).toBeUndefined();
    await act(async () => button(host, "Restart")?.click());
    expect(bridge.updates.restart).toHaveBeenCalledTimes(1);
  });

  it("offers no check where the app can't update itself, such as a build run from source", async () => {
    stubBridge({ state: async () => ({ status: "unsupported" }) });
    const host = await mount();
    expect(host.textContent).toContain("This build doesn't update itself.");
    expect(button(host, "Check now")).toBeUndefined();
  });
});
