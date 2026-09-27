// Renders assets/icon.svg into the PNGs the window, taskbar, tray and notifications use. Windows can't load
// SVG icons. Run it after changing the SVG: npx electron packages/desktop/scripts/render-icons.ts
import { app, BrowserWindow } from "electron";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const assets = join(import.meta.dirname, "..", "assets");
const outputs = [
  { size: 256, file: "icon-256.png" },
  { size: 32, file: "tray-32.png" },
];

// Destroying the first offscreen window must not quit Electron before the tray icon is rendered.
app.on("window-all-closed", () => {});

app.whenReady().then(async () => {
  const svg = readFileSync(join(assets, "icon.svg"), "utf8");
  for (const { size, file } of outputs) {
    const window = new BrowserWindow({ width: size, height: size, show: false, frame: false, transparent: true, webPreferences: { offscreen: true } });
    const html = `<!doctype html><html><body style="margin:0;background:transparent;overflow:hidden">${svg.replace("<svg ", `<svg width="${size}" height="${size}" style="display:block" `)}</body></html>`;
    await window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(html)}`);
    await new Promise((resolve) => setTimeout(resolve, 200)); // let the offscreen frame paint
    const image = await window.webContents.capturePage({ x: 0, y: 0, width: size, height: size });
    writeFileSync(join(assets, file), image.toPNG());
    window.destroy();
  }
  app.quit();
});
