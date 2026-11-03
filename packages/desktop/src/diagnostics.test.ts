import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { tempRoot } from "../../daemon/src/testing.ts";
import { createDesktopDiagnostics } from "./diagnostics.ts";

const report = () => JSON.stringify({
  reportVersion: 1, device: { platform: "windows", appVersion: "0.3.2", fileBridge: "present" },
  host: { selection: "this-device", appVersion: null, daemonVersion: null, providers: [{ provider: "claude", version: null }, { provider: "codex", version: null }], relay: "unknown", storage: "unknown" },
  connection: { status: "ready", issue: null, route: "unknown" }, speech: { engine: "unknown", availability: "unknown", permission: "unknown", model: "unknown" },
}, null, 2);
function fixture(destination: string | null = null) {
  const frame = { url: "app://tjommie/settings/" };
  const contents = { mainFrame: frame, getURL: () => frame.url, isDestroyed: () => false };
  const window = { webContents: contents, isDestroyed: () => false };
  const event = { sender: contents, senderFrame: frame };
  let invalidate: (() => void) | undefined;
  const choose = vi.fn(async () => destination), stop = vi.fn();
  const actions = createDesktopDiagnostics({ window: () => window, production: true, appVersion: () => "0.3.2", platform: "win32", choose, watch: (callback) => { invalidate = callback; return stop; } });
  return { actions, event, frame, contents, choose, stop, invalidate: () => invalidate?.() };
}

describe("trusted diagnostic IPC actions", () => {
  it("returns real app metadata only to the trusted main frame", () => {
    const { actions, event, contents } = fixture();
    expect(actions.info(event)).toEqual({ appVersion: "0.3.2", platform: "windows" });
    expect(() => actions.info({ ...event, sender: { ...contents } })).toThrow("Untrusted diagnostic request.");
    expect(() => actions.info({ ...event, senderFrame: { url: "app://tjommie/settings/" } })).toThrow("Untrusted diagnostic request.");
    event.senderFrame.url = "https://attacker.invalid/";
    expect(() => actions.info(event)).toThrow("Untrusted diagnostic request.");
  });
  it("atomically saves exactly the preview while preserving no partial file", async () => {
    const root = tempRoot(), destination = join(root, "report.json"), test = fixture(destination);
    writeFileSync(destination, "previous complete report");
    expect(await test.actions.saveReport(test.event, report())).toEqual({ status: "saved" });
    expect(readFileSync(destination, "utf8")).toBe(report());
    expect(readdirSync(root)).toEqual(["report.json"]);
    expect(test.stop).toHaveBeenCalledOnce();
  });
  it("does not write after picker cancellation or app navigation", async () => {
    const cancelled = fixture();
    expect(await cancelled.actions.saveReport(cancelled.event, report())).toEqual({ status: "cancelled" });
    const destination = join(tempRoot(), "report.json"), navigated = fixture(destination);
    navigated.choose.mockImplementationOnce(async () => { navigated.invalidate(); return destination; });
    expect(await navigated.actions.saveReport(navigated.event, report())).toEqual({ status: "cancelled" });
    expect(existsSync(destination)).toBe(false);
  });
  it("validates the allowlist before showing a destination picker", async () => {
    const { actions, event, choose } = fixture();
    await expect(actions.saveReport(event, JSON.stringify({ ...JSON.parse(report()), logs: ["SYNTHETIC_SECRET"] }))).rejects.toThrow("Invalid diagnostic report.");
    await expect(actions.saveReport({ ...event, senderFrame: null }, report())).rejects.toThrow("Untrusted diagnostic request.");
    expect(choose).not.toHaveBeenCalled();
  });
  it("keeps the existing destination and removes staging on a failed final save", async () => {
    const root = tempRoot(), destination = join(root, "existing-folder"), test = fixture(destination); mkdirSync(destination);
    await expect(test.actions.saveReport(test.event, report())).rejects.toThrow("The diagnostic report could not be saved.");
    expect(readdirSync(root)).toEqual(["existing-folder"]);
    expect(test.stop).toHaveBeenCalledOnce();
  });
  it("does not open overlapping save dialogs", async () => {
    const test = fixture();
    let finish: ((path: string | null) => void) | undefined;
    test.choose.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const first = test.actions.saveReport(test.event, report());
    await expect(test.actions.saveReport(test.event, report())).rejects.toThrow("already open");
    finish?.(null);
    expect(await first).toEqual({ status: "cancelled" });
    expect(test.choose).toHaveBeenCalledOnce();
  });
});
