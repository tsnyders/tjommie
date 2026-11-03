// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DesktopApi } from "@/lib/desktop";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { AboutSection, DiagnosticsSection } from "./diagnostics-section";

const call = vi.hoisted(() => vi.fn());
vi.mock("@/lib/connection", () => ({ request: call }));
vi.mock("@/lib/local-speech", () => ({ localSpeech: () => null }));
vi.mock("@/lib/attachment-device", () => ({ attachmentDevice: () => null }));
vi.mock("@/lib/remote-hosts", () => ({ isAndroidApp: () => false, remoteHosts: () => [{ id: "host-a", name: "Private host name" }, { id: "host-b", name: "Other private name" }] }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const roots: Root[] = [];
const hostInfo = (version = "0.3.2") => ({ appVersion: version, daemonVersion: version, providers: [{ provider: "codex", version: "1.2.3" }], relay: "online", storage: "ok" });
const text = (): string => document.body.textContent ?? "";
const button = (label: string): HTMLButtonElement => {
  const found = [...document.querySelectorAll("button")].find((item) => item.textContent === label);
  if (!found) throw new Error(`Missing button ${label}`);
  return found;
};
const report = (): string => document.querySelector<HTMLTextAreaElement>('[aria-label="Diagnostic report preview"]')?.value ?? "";
const settle = (): Promise<void> => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
async function mount(node: ReactNode): Promise<void> {
  const root = createRoot(document.body.appendChild(document.createElement("div"))); roots.push(root);
  await act(async () => root.render(node)); await settle();
}
beforeEach(() => {
  update(() => ({ ...initialState, host: "host-a", connection: { status: "ready" } }));
  call.mockReset(); call.mockResolvedValue(hostInfo());
});
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  document.body.replaceChildren(); delete window.tjommie; vi.restoreAllMocks();
});

describe("diagnostic preview and About", () => {
  it("opens bundled notices inside the app without a blocked new window", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("Bundled license <script> is plain text"));
    await mount(<AboutSection />);
    await act(async () => button("Third-party notices").click()); await settle();
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain("Bundled license <script> is plain text");
    expect(document.querySelector('[role="dialog"] script')).toBeNull();
    expect(fetch).toHaveBeenCalledWith("/third-party-notices.txt", expect.objectContaining({ signal: expect.any(AbortSignal) }));
  });
  it("distinguishes the current device version from the selected host, and excludes its display name", async () => {
    window.tjommie = { diagnostics: { info: async () => ({ appVersion: "0.4.0", platform: "windows" }), saveReport: async () => ({ status: "cancelled" }) } } as unknown as DesktopApi;
    await mount(<DiagnosticsSection />);
    expect(text()).toContain("Private host name"); expect(report()).not.toContain("Private host name");
    expect(JSON.parse(report())).toMatchObject({ device: { appVersion: "0.4.0" }, host: { appVersion: "0.3.2" } });
    await mount(<AboutSection />);
    expect(text()).toContain("This device's app version0.4.0");
    expect(text()).toContain("Connected host's app version");
    expect(button("Third-party notices")).not.toBeNull();
  });
  it("never reuses a previous host's diagnostic result after switching PCs", async () => {
    let oldHost: ((value: ReturnType<typeof hostInfo>) => void) | undefined;
    call.mockImplementationOnce(() => new Promise((resolve) => { oldHost = resolve; }));
    await mount(<DiagnosticsSection />);
    await act(async () => update((state) => ({ ...state, host: "host-b" })));
    await settle();
    expect(JSON.parse(report())).toMatchObject({ host: { appVersion: "0.3.2" } });
    await act(async () => oldHost?.(hostInfo("9.9.9")));
    expect(report()).not.toContain("9.9.9");
    expect(text()).toContain("Other private name");
  });
  it("copies exactly the preview and reports save cancellation without claiming success", async () => {
    const writeText = vi.fn(async () => {});
    vi.spyOn(navigator.clipboard, "writeText").mockImplementation(writeText);
    const saveReport = vi.fn(async () => ({ status: "cancelled" as const }));
    window.tjommie = { diagnostics: { info: async () => ({ appVersion: "0.3.2", platform: "windows" }), saveReport } } as unknown as DesktopApi;
    await mount(<DiagnosticsSection />);
    const preview = report();
    await act(async () => button("Copy report").click());
    expect(writeText).toHaveBeenCalledWith(preview);
    await act(async () => button("Save report").click());
    expect(saveReport).toHaveBeenCalledWith(preview);
    expect(text()).toContain("Save cancelled."); expect(text()).not.toContain("Report saved");
  });
  it("shows unknown versions and safe errors when a bridge/check is unavailable", async () => {
    call.mockRejectedValue(new Error("SYNTHETIC_SECRET C:\\private\\file"));
    await mount(<DiagnosticsSection />);
    expect(report()).not.toContain("SYNTHETIC_SECRET"); expect(text()).not.toContain("SYNTHETIC_SECRET");
    expect(JSON.parse(report())).toMatchObject({ host: { appVersion: null, storage: "unknown" } });
    expect(button("Save report").disabled).toBe(true);
    await mount(<AboutSection />);
    expect(text()).toContain("Unknown in this app");
  });
});
