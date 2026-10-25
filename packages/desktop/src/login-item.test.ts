import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getLoginItem, setLoginItem, startsHidden, syncLoginItem } from "./login-item.ts";

const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });

function fakeApp(packaged = true) {
  const root = mkdtempSync(join(tmpdir(), "tjommie-login-test-"));
  roots.push(root);
  let enabled = false;
  let approved = true;
  const setLoginItemSettings = vi.fn((options: { openAtLogin: boolean; args: string[] }) => { enabled = options.openAtLogin; });
  return {
    isPackaged: packaged,
    getPath: (_name: "userData") => root,
    getLoginItemSettings: vi.fn((_options: { args: string[] }) => ({ openAtLogin: enabled, launchItems: [{ args: ["--hidden"], enabled: approved }] })),
    setLoginItemSettings,
    setStartupApproval: (value: boolean) => { approved = value; },
  };
}

describe("Windows login item", () => {
  it("starts hidden only when requested", () => {
    expect(startsHidden(["Tjommie.exe"])).toBe(false);
    expect(startsHidden(["Tjommie.exe", "--hidden"])).toBe(true);
    expect(startsHidden(["Tjommie.exe", "--updated"])).toBe(true);
  });

  it("defaults on once for a packaged app and preserves a later user choice", () => {
    const app = fakeApp();
    syncLoginItem(app);
    expect(app.setLoginItemSettings).toHaveBeenCalledExactlyOnceWith({ openAtLogin: true, args: ["--hidden"] });
    setLoginItem(app, false);
    syncLoginItem(app);
    expect(getLoginItem(app)).toBe(false);
    expect(app.getLoginItemSettings).toHaveBeenCalledWith({ args: ["--hidden"] });
    expect(app.setLoginItemSettings).toHaveBeenCalledTimes(2);
    expect(app.setLoginItemSettings).toHaveBeenLastCalledWith({ openAtLogin: false, args: ["--hidden"] });
  });

  it("does not register a login item in an unpackaged app", () => {
    const app = fakeApp(false);
    syncLoginItem(app);
    expect(app.setLoginItemSettings).not.toHaveBeenCalled();
  });

  it("reports Windows startup approval, not just the Run key", () => {
    const app = fakeApp();
    syncLoginItem(app);
    app.setStartupApproval(false);
    expect(getLoginItem(app)).toBe(false);
  });
});
