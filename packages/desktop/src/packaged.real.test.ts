import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { packagedDaemonPath } from "./daemon-process.ts";
import { closeRealApp, daemonConnection, launchRealApp, quitApp, refused, type RealApp } from "./real-app.ts";
import type { SpeechApi } from "../../protocol/src/speech.ts";
import { verifySpeechResources } from "./speech-resources.ts";

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
      // launchRealApp already proved the process listening on the daemon's port is the bundled exe. The Rust daemon
      // ships as one exe; nothing of the Node daemon, Playwright MCP or the daemon's Noise wasm does.
      const resources = join(dirname(exe), "resources");
      const daemonFolder = join(resources, "daemon");
      expect(existsSync(packagedDaemonPath(resources))).toBe(true);
      for (const gone of ["main.mjs", "node_modules", "tjommie_noise_bg.wasm"]) expect(existsSync(join(daemonFolder, gone)), gone).toBe(false);
      verifySpeechResources(join(dirname(exe), "resources", "speech"));
      const speech = await app.page.evaluate(async () => (globalThis as unknown as { tjommie: { speech: SpeechApi } }).tjommie.speech.status());
      expect(speech.available).toBe(false);
      expect(speech.problem?.code).toBe("model_missing");
      const { port } = await daemonConnection(app.page);
      await quitApp(app);
      quit = true;
      expect(await refused(port)).toBe(true);
    } finally {
      await closeRealApp(app, quit);
    }
  });
});
