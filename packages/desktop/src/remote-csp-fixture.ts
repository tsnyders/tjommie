import { app, BrowserWindow } from "electron";
import { join } from "node:path";

const root = process.env.TJOMMIE_CSP_TEST_ROOT;
const url = process.env.TJOMMIE_CSP_TEST_URL;
if (!root || !url) throw new Error("The isolated CSP fixture needs its test paths.");
app.setName("Tjommie Update Test");
app.setAppUserModelId("com.tsnyders.tjommie.updatetest");
app.setPath("userData", join(root, "electron"));
app.whenReady().then(async () => {
  // This fixture checks CSP; sandboxed full-app acceptance runs separately.
  const window = new BrowserWindow({ show: false, webPreferences: { sandbox: false, contextIsolation: true, nodeIntegration: false } });
  await window.loadURL(url);
}).catch(() => app.exit(1));
app.on("window-all-closed", () => app.quit());
