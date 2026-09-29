import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { closeRealApp, daemonConnection, launchRealApp, quitApp, refused, type RealApp } from "./real-app.ts";

// The installed layout, from `npm run package`. Skipped when there's no build.
const exe = join(resolve(import.meta.dirname, "..", "..", ".."), "dist-desktop", "installer", "win-unpacked", "Tjommie.exe");

describe.skipIf(!existsSync(exe))("the packaged app", () => {
  it("the packaged app starts its own daemon and shows first-run setup", async () => {
    let app: RealApp | undefined;
    let quit = false;
    try {
      app = await launchRealApp(exe); // it waits for "Set up Tjommie", served from resources\ui
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
