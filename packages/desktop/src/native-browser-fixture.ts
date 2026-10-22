// Disposable Electron entry used only by native-browser.real.test.ts; never imported by the product.
import { app, BrowserWindow } from "electron";
import { join } from "node:path";
import { NativeBrowserEngine } from "./native-browser.ts";
import { nativeBrowserAutomation } from "./native-browser-automation.ts";

// A failing synthetic fixture must never leave an Electron crash dialog on the person's desktop.
process.on("uncaughtException", (error) => { process.stderr.write(`Native fixture uncaught exception: ${error.stack ?? error.message}\n`); app.exit(1); });
process.on("unhandledRejection", (error: unknown) => { process.stderr.write(`Native fixture unhandled rejection: ${String(error)}\n`); app.exit(1); });

export type NativeBrowserFixture = { engine: NativeBrowserEngine; owner: BrowserWindow; focusReturns: string[] };
const root = process.env.TJOMMIE_NATIVE_FIXTURE;
if (!root) throw new Error("The native browser fixture requires an isolated root.");
app.setPath("userData", join(root, "electron"));
app.whenReady().then(async () => {
  const owner = new BrowserWindow({ width: 1000, height: 800, show: true, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false } });
  await owner.loadURL("data:text/html,<html><body style='background:rgb(12,24,36)'><input id='host' aria-label='Host field'></body></html>");
  const focusReturns: string[] = [];
  const engine = new NativeBrowserEngine({ window: owner, profileRoot: join(root, "data", "native-browser"), onFocusReturn: (id) => focusReturns.push(id), automation: nativeBrowserAutomation });
  (globalThis as unknown as { nativeBrowserFixture: NativeBrowserFixture }).nativeBrowserFixture = { engine, owner, focusReturns };
}).catch((error: unknown) => { process.stderr.write(String(error)); app.exit(1); });
app.on("window-all-closed", () => app.quit());
