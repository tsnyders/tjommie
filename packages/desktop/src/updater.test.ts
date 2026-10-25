import { EventEmitter } from "node:events";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { autoUpdater } from "electron-updater";
import { createUpdater, trayTemplate } from "./updater.ts";

function fakeUpdater() {
  const events = new EventEmitter();
  return Object.assign(events, {
    autoDownload: false,
    autoInstallOnAppQuit: false,
    logger: console,
    checkForUpdates: vi.fn(async (): Promise<{ downloadPromise?: Promise<string[]> } | null> => null),
    quitAndInstall: vi.fn(),
    setFeedURL: vi.fn(),
  });
}

afterEach(() => vi.useRealTimers());

describe("desktop updater", () => {
  it("reports unsupported without configuring or checking an unpackaged app", async () => {
    const fake = fakeUpdater();
    const updates = createUpdater(false, fake as unknown as typeof autoUpdater);
    expect(updates.state()).toEqual({ status: "unsupported" });
    await updates.check();
    expect(fake.checkForUpdates).not.toHaveBeenCalled();
    updates.stop();
  });

  it("tracks download and ready states, and restarts silently", async () => {
    vi.useFakeTimers();
    const fake = fakeUpdater();
    const quit = vi.fn();
    const updates = createUpdater(true, fake as unknown as typeof autoUpdater, { TJOMMIE_UPDATE_FEED: "http://127.0.0.1:8765" }, quit);
    const seen: string[] = [];
    updates.onState((state) => seen.push(state.status));
    expect(fake.autoDownload).toBe(true);
    expect(fake.autoInstallOnAppQuit).toBe(true);
    expect(fake.logger).toBeNull();
    expect(fake.setFeedURL).toHaveBeenCalledWith({ provider: "generic", url: "http://127.0.0.1:8765" });
    fake.emit("checking-for-update");
    fake.emit("download-progress", { percent: 42.5 });
    fake.emit("update-downloaded", { version: "0.3.1-test" });
    expect(updates.state()).toEqual({ status: "ready", version: "0.3.1-test" });
    expect(seen).toEqual(["checking", "downloading", "ready"]);
    updates.restart();
    expect(quit).toHaveBeenCalledOnce();
    expect(fake.quitAndInstall).not.toHaveBeenCalled();
    updates.installOnQuit();
    expect(fake.quitAndInstall).toHaveBeenCalledExactlyOnceWith(true, true);
    updates.installOnQuit();
    expect(fake.quitAndInstall).toHaveBeenCalledTimes(1);
    fake.emit("checking-for-update");
    fake.emit("error", new Error("offline"));
    expect(updates.state()).toEqual({ status: "ready", version: "0.3.1-test" });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fake.checkForUpdates).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1000 - 60_000);
    expect(fake.checkForUpdates).toHaveBeenCalledTimes(2);
    updates.stop();
  });

  it("shows restart in the tray only after an update is ready", () => {
    const open = vi.fn();
    const restart = vi.fn();
    const quit = vi.fn();
    expect(trayTemplate({ status: "idle" }, open, restart, quit).map((item) => item.label ?? item.type))
      .toEqual(["Open Tjommie", "separator", "Quit Tjommie"]);
    const ready = trayTemplate({ status: "ready", version: "0.3.1" }, open, restart, quit);
    expect(ready.map((item) => item.label ?? item.type))
      .toEqual(["Open Tjommie", "separator", "Restart to update to v0.3.1", "Quit Tjommie"]);
    ready[2]?.click?.({} as Electron.MenuItem, {} as Electron.BrowserWindow, {});
    expect(restart).toHaveBeenCalledOnce();
  });

  it("maps no-update and errors to state without throwing", async () => {
    const fake = fakeUpdater();
    const updates = createUpdater(true, fake as unknown as typeof autoUpdater);
    fake.emit("update-not-available");
    expect(updates.state()).toEqual({ status: "none" });
    fake.emit("error", new Error("offline"));
    expect(updates.state()).toEqual({ status: "error", message: "offline" });
    updates.stop();
  });

  it("disables checks by environment and ignores restart before an update is ready", async () => {
    const fake = fakeUpdater();
    const quit = vi.fn();
    const disabled = createUpdater(true, fake as unknown as typeof autoUpdater, { TJOMMIE_UPDATES: "off" }, quit);
    expect(disabled.state()).toEqual({ status: "unsupported" });
    await disabled.check();
    disabled.restart();
    expect(fake.checkForUpdates).not.toHaveBeenCalled();
    expect(quit).not.toHaveBeenCalled();
    const active = createUpdater(true, fake as unknown as typeof autoUpdater, {}, quit);
    active.restart();
    expect(quit).not.toHaveBeenCalled();
    active.stop();
  });

  it("handles a rejected automatic download and a rejected check", async () => {
    const fake = fakeUpdater();
    fake.checkForUpdates.mockResolvedValueOnce({ downloadPromise: Promise.reject(new Error("download failed")) });
    const updates = createUpdater(true, fake as unknown as typeof autoUpdater);
    await updates.check();
    await Promise.resolve();
    fake.checkForUpdates.mockRejectedValueOnce(new Error("offline"));
    await updates.check();
    expect(updates.state()).toEqual({ status: "error", message: "offline" });
    updates.stop();
  });
});
