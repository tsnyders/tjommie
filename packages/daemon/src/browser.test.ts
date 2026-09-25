import type { ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, expect, it, vi } from "vitest";

const childProcessMocks = vi.hoisted(() => ({ spawn: vi.fn() }));

vi.mock("node:child_process", async (importOriginal) => ({
  ...(await importOriginal<typeof import("node:child_process")>()),
  spawn: childProcessMocks.spawn,
}));

import type { StoredBot } from "./bots.ts";
import { browserServer, findBrowser, showBrowser } from "./browser.ts";

const env = { ProgramFiles: "C:\\Program Files", "ProgramFiles(x86)": "C:\\Program Files (x86)", LOCALAPPDATA: "C:\\Users\\Thabo Mokoena\\AppData\\Local" };
const bot = { homeFolder: "C:\\Users\\Thabo Mokoena\\AppData\\Local\\Tjommie\\bots\\b1", autonomy: "folder" } as StoredBot;

describe("browser", () => {
  it("uses Chrome when it's installed, otherwise Edge", () => {
    expect(findBrowser((path) => path === "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", env)).toEqual({
      channel: "chrome",
      path: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    });
    expect(findBrowser(() => false, env)).toEqual({ channel: "msedge", path: "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe" });
  });

  it("runs the bundled Playwright MCP on the bot's own profile, asking first only in Always ask", () => {
    const server = browserServer(bot, {});
    expect(server).toMatchObject({ name: "browser", command: process.execPath, autoAllow: true });
    if (!("command" in server)) throw new Error("expected a program");
    expect(existsSync(server.args[0])).toBe(true);
    expect(server.args.slice(1, 5)).toEqual(["--user-data-dir", join(bot.homeFolder, "chrome-profile"), "--output-dir", join(bot.homeFolder, "browser-files")]);
    expect(server.args).not.toContain("--headless");
    expect(browserServer(bot, { TJOMMIE_BROWSER_HEADLESS: "1" })).toMatchObject({ args: expect.arrayContaining(["--headless"]) });
    expect(browserServer({ ...bot, autonomy: "ask" }, {}).autoAllow).toBe(false);
  });

  it("uses the injected environment when the browser server discovers Chrome", () => {
    const root = mkdtempSync(join(tmpdir(), "tjommie-browser-"));
    const chrome = join(root, "Google", "Chrome", "Application", "chrome.exe");
    try {
      mkdirSync(dirname(chrome), { recursive: true });
      writeFileSync(chrome, "");
      const server = browserServer(bot, { ProgramFiles: root, "ProgramFiles(x86)": join(root, "x86"), LOCALAPPDATA: join(root, "local") });
      if (!("command" in server)) throw new Error("expected a program");
      expect(server.args.slice(-2)).toEqual(["--browser", "chrome"]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("reports a browser spawn failure to its caller", async () => {
    childProcessMocks.spawn.mockReset();
    const child = Object.assign(new EventEmitter(), { unref: vi.fn() }) as unknown as ChildProcess;
    childProcessMocks.spawn.mockReturnValue(child);

    const shown = showBrowser(bot);
    child.emit("error", new Error("spawn failed"));

    await expect(shown).rejects.toThrow("spawn failed");
    expect(child.unref).not.toHaveBeenCalled();
  });
});
