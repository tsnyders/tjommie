import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { closeRealApp, daemonConnection, launchRealApp, quitApp, refused, type RealApp } from "./real-app.ts";

// Required by `npm run test:packaged` after `npm run package`. The general real suite excludes this file.
const exe = join(resolve(import.meta.dirname, "..", "..", ".."), "dist-desktop", "installer", "win-unpacked", "Tjommie.exe");

describe("the packaged app", () => {
  it("the packaged app starts its own daemon and shows first-run setup", async () => {
    let app: RealApp | undefined;
    let quit = false;
    try {
      if (!existsSync(exe)) throw new Error("Packaged executable missing. Run npm run package before npm run test:packaged.");
      app = await launchRealApp(exe, { isolatedAccounts: true }); // startup smoke never uses the owner's provider profiles
      expect(await app.client.call("bots.list", {})).toEqual([]);
      // Playwright MCP starts only when a bot with the browser works, so check the files it runs from.
      expect(existsSync(join(dirname(exe), "resources", "daemon", "node_modules", "@playwright", "mcp", "cli.js"))).toBe(true);
      const { port } = await daemonConnection(app.page);
      await quitApp(app);
      quit = true;
      expect(await refused(port)).toBe(true);
    } finally {
      await closeRealApp(app, quit);
    }
  });
});
