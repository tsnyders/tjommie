import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import type { StoredBot } from "./bots.ts";
import { browserProfile, browsers, findBrowser } from "./browser-manager.ts";
import type { McpServer } from "./providers/types.ts";

export { browserProfile, browsers, findBrowser } from "./browser-manager.ts";

export function sharedBrowserServer(bot: StoredBot, url: string, token: string): McpServer {
  return { name: "browser", url, headers: { Authorization: `Bearer ${token}` }, autoAllow: bot.autonomy !== "ask" };
}

// Provider-only acceptance tests have no daemon. Their explicitly headless, disposable stdio helper
// remains available; the app uses sharedBrowserServer and never starts a second browser owner.
export function browserServer(bot: StoredBot, env: NodeJS.ProcessEnv = process.env): McpServer {
  if (env.TJOMMIE_BROWSER_HEADLESS !== "1") throw new Error("Browser automation must use the daemon's shared browser.");
  const require = createRequire(import.meta.url);
  return {
    name: "browser", command: process.execPath,
    args: [join(dirname(require.resolve("@playwright/mcp/package.json")), "cli.js"), "--user-data-dir", browserProfile(bot), "--output-dir", join(bot.homeFolder, "browser-files"), "--browser", findBrowser(undefined, env).channel, "--headless"],
    env: process.versions.electron ? { ELECTRON_RUN_AS_NODE: "1" } : {}, autoAllow: bot.autonomy !== "ask",
  };
}

// Compatibility for the existing RPC and runner deletion hook. Showing starts the shared headless context.
export async function showBrowser(bot: StoredBot): Promise<void> { await browsers.open(bot); }
export async function closeBrowser(botId: string): Promise<void> { await browsers.close(botId); }
