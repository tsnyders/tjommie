import { app, BrowserWindow } from "electron";

// No top-level await here: Electron emits "ready" only after this module finishes evaluating.
app.whenReady().then(() => {
  // ponytail: dev server only; the installer task decides how the static export is served
  new BrowserWindow({ width: 1280, height: 800 }).loadURL("http://localhost:3000");
});
