import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import type { StoredBot } from "./bots.ts";
import type { McpServer } from "./providers/types.ts";

const require = createRequire(import.meta.url);

const chromePaths = (env: NodeJS.ProcessEnv) => [
  join(env.ProgramFiles ?? "C:\\Program Files", "Google", "Chrome", "Application", "chrome.exe"),
  join(env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Google", "Chrome", "Application", "chrome.exe"),
  join(env.LOCALAPPDATA ?? "", "Google", "Chrome", "Application", "chrome.exe"),
];
const edgePath = (env: NodeJS.ProcessEnv) => join(env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft", "Edge", "Application", "msedge.exe");

// Chrome if it's installed, otherwise Edge, which every Windows PC has.
export function findBrowser(exists: (path: string) => boolean = existsSync, env: NodeJS.ProcessEnv = process.env): { channel: "chrome" | "msedge"; path: string } {
  const chrome = chromePaths(env).find(exists);
  return chrome ? { channel: "chrome", path: chrome } : { channel: "msedge", path: edgePath(env) };
}

export const browserProfile = (bot: StoredBot): string => join(bot.homeFolder, "chrome-profile");

// Playwright MCP (bundled with Tjommie), started by the bot's CLI. It opens a real browser window on the bot's
// own profile, so sites the person signs in to there stay signed in.
export function browserServer(bot: StoredBot, env: NodeJS.ProcessEnv = process.env): McpServer {
  const cli = join(dirname(require.resolve("@playwright/mcp/package.json")), "cli.js");
  const args = [
    cli,
    "--user-data-dir",
    browserProfile(bot),
    "--output-dir",
    join(bot.homeFolder, "browser-files"),
    "--browser",
    findBrowser(existsSync, env).channel,
  ];
  if (env.TJOMMIE_BROWSER_HEADLESS === "1") args.push("--headless"); // tests only
  return {
    name: "browser",
    command: process.execPath,
    args,
    env: process.versions.electron ? { ELECTRON_RUN_AS_NODE: "1" } : {}, // inside Electron, run the script as plain Node
    autoAllow: bot.autonomy !== "ask",
  };
}

// "Show window": opens the bot's browser profile. If the bot's browser is already open, the browser opens another
// window of it. ponytail: while a window opened here stays open, the bot's own browser can't start ("already in
// use") and the bot asks the person to close it; a daemon-owned browser (Phase 4, for the live view) removes that.
export function showBrowser(bot: StoredBot, env: NodeJS.ProcessEnv = process.env): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(findBrowser(existsSync, env).path, [`--user-data-dir=${browserProfile(bot)}`, "--no-first-run", "--no-default-browser-check"], {
      detached: true,
      stdio: "ignore",
    });
    child.once("error", reject);
    child.once("spawn", () => {
      child.unref();
      resolve();
    });
  });
}
