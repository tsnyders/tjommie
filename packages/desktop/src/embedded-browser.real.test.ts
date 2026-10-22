import { createServer, type Server } from "node:http";
import { mkdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join, resolve } from "node:path";
import { afterAll, beforeAll, expect, it } from "vitest";
import { closeRealApp, eventually, launchRealApp, openChat, type RealApp } from "./real-app.ts";

let app: RealApp | undefined;
let site: Server;
let origin: string;
beforeAll(async () => {
  site = createServer((req, res) => {
    if (req.url === "/login") { res.writeHead(302, { "set-cookie": "native_fixture=signed-in; Path=/; SameSite=Lax", location: "/account" }).end(); return; }
    res.writeHead(200, { "content-type": "text/html" }).end(`<!doctype html><title>Native fixture</title><body style="background:#dbecf5;font:20px sans-serif"><input id="name" style="position:absolute;left:10px;top:10px;width:200px;height:30px"><button id="login" onclick="location.href='/login'" style="position:absolute;left:10px;top:60px">Sign in</button><p style="margin-top:120px">Native page ${req.url}</p></body>`);
  });
  await new Promise<void>((done) => site.listen(0, "127.0.0.1", done));
  const address = site.address();
  if (!address || typeof address === "string") throw new Error("Fixture did not bind.");
  origin = `http://127.0.0.1:${address.port}`;
  app = await launchRealApp(undefined, { isolatedAccounts: true });
});
afterAll(async () => {
  try { await closeRealApp(app, false); }
  finally { if (site) await new Promise<void>((done, reject) => site.close((error) => error ? reject(error) : done())); }
});
function running(): RealApp { if (!app) throw new Error("App unavailable."); return app; }
async function guest(script: string): Promise<unknown> {
  return running().electronApp.evaluate(async ({ webContents }, data) => {
    const target = webContents.getAllWebContents().find((contents) => contents.getURL().startsWith(data.origin));
    if (!target) throw new Error("Native fixture guest missing.");
    return target.executeJavaScript(data.script, true) as Promise<unknown>;
  }, { origin, script });
}
async function surfaces() {
  return running().electronApp.evaluate(({ BrowserWindow, WebContentsView }, fixtureOrigin) => {
    return BrowserWindow.getAllWindows()[0].contentView.children
      .filter((view) => view instanceof WebContentsView && view.webContents.getURL().startsWith(fixtureOrigin))
      .map((view) => ({ bounds: view.getBounds(), visible: view.getVisible() }));
  }, origin);
}
async function screenshot(name: string, cropWidth = 0): Promise<void> {
  // PrintWindow captures a minimized HWND as a tiny title bar, not its child views.
  // Restore only this fixture window before checking the composed native surface.
  await running().electronApp.evaluate(({ BrowserWindow }) => {
    const window = BrowserWindow.getAllWindows()[0];
    if (window.isMinimized()) window.restore();
    if (!window.isVisible()) window.show();
  });
  await running().page.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  const identity = await running().electronApp.evaluate(({ BrowserWindow }) => {
    const handle = BrowserWindow.getAllWindows()[0].getNativeWindowHandle();
    return { handle: (handle.length === 8 ? handle.readBigUInt64LE() : BigInt(handle.readUInt32LE())).toString(), pid: process.pid };
  });
  const directory = resolve(import.meta.dirname, "../../../docs/screenshots");
  mkdirSync(directory, { recursive: true });
  const powershell = join(process.env.SystemRoot ?? "C:/Windows", "System32/WindowsPowerShell/v1.0/powershell.exe");
  const output = execFileSync(powershell, ["-NoProfile", "-NonInteractive", "-File", join(import.meta.dirname, "native-window-capture.ps1"), "-WindowHandle", identity.handle, "-OwnerPid", String(identity.pid), "-OutputPath", join(directory, name), "-CropWidth", String(cropWidth)], { windowsHide: true, encoding: "utf8" });
  const value: unknown = JSON.parse(output);
  expect(value).toMatchObject({ nativePixelSamples: expect.any(Number) });
  if (typeof value !== "object" || value === null || !("nativePixelSamples" in value) || typeof value.nativePixelSamples !== "number") throw new Error("Native composited capture unavailable.");
  expect(value.nativePixelSamples).toBeGreaterThan(1000);
}

it("uses a live native guest while retaining details, resize, expansion, cookies and bot isolation", async () => {
  const { client, page, electronApp } = running();
  const create = (name: string) => client.call("bots.create", { name, color: "blue", shape: "circle", provider: "claude", model: "haiku", instructions: "Synthetic native browser fixture", browser: true, autonomy: "ask" });
  const bot = await create("Native browser fixture");
  await openChat(page, bot.name);
  await page.getByRole("button", { name: "Open browser", exact: true }).click();
  await page.getByRole("button", { name: "Take control", exact: true }).click();
  await page.getByRole("textbox", { name: "Browser address" }).fill(`${origin}/start`);
  await page.getByRole("button", { name: "Go", exact: true }).click();
  await eventually(async () => (await surfaces()).some((view) => view.visible && view.bounds.width > 200) || undefined);
  expect(await guest("({node:typeof require,bridge:typeof window.tjommie,cookie:document.cookie})")).toEqual({ node: "undefined", bridge: "undefined", cookie: "" });
  expect(await page.getByRole("application", { name: "Browser viewport" }).locator("img").count()).toBe(0);
  // Playwright also exposes native guest targets as pages; count actual top-level BrowserWindows.
  expect(await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().length)).toBe(1);
  await electronApp.evaluate(({ webContents }, fixtureOrigin) => {
    const contents = webContents.getAllWebContents().find((value) => value.getURL().startsWith(fixtureOrigin));
    if (!contents) throw new Error("Guest missing.");
    contents.sendInputEvent({ type: "mouseDown", x: 50, y: 20, button: "left", clickCount: 1 });
    contents.sendInputEvent({ type: "mouseUp", x: 50, y: 20, button: "left", clickCount: 1 });
    contents.sendInputEvent({ type: "char", keyCode: "N" });
  }, origin);
  await eventually(async () => await guest("document.getElementById('name').value") === "N" || undefined);
  await guest("document.getElementById('login').click()");
  await eventually(async () => String(await guest("document.cookie")).includes("native_fixture=signed-in") || undefined);
  await screenshot("browser-usability-panel.png");
  await page.getByRole("tab", { name: "Details", exact: true }).click();
  await page.getByText("Routines", { exact: true }).waitFor();
  await eventually(async () => (await surfaces()).every((view) => !view.visible) || undefined);
  await page.getByRole("tab", { name: "Browser", exact: true }).click();
  await page.getByRole("button", { name: "Take control", exact: true }).click();
  expect(String(await guest("document.cookie"))).toContain("native_fixture=signed-in");
  const splitter = page.getByRole("separator", { name: "Resize panel" });
  const width = Number(await splitter.getAttribute("aria-valuenow"));
  await splitter.focus(); await page.keyboard.press("ArrowLeft");
  await eventually(async () => Number(await splitter.getAttribute("aria-valuenow")) !== width || undefined);
  await page.getByRole("button", { name: "Expand browser", exact: true }).click();
  await eventually(async () => (await surfaces()).some((view) => view.visible && view.bounds.width > 900) || undefined);
  await screenshot("browser-usability-expanded.png");
  await page.getByRole("button", { name: "Restore browser", exact: true }).click();
  await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].setSize(960, 800));
  await eventually(async () => (await surfaces()).some((view) => view.visible && view.bounds.width > 400 && view.bounds.width < 950) || undefined);
  expect(await page.getByRole("textbox", { name: `Message ${bot.name}` }).evaluate((element) => element.closest("[inert]") !== null)).toBe(true);
  await screenshot("browser-usability-narrow.png");
  await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].setSize(1440, 900));
  await page.getByRole("button", { name: "Hide panel", exact: true }).click();
  await page.getByRole("button", { name: "Bot panel", exact: true }).click();
  await page.getByRole("application", { name: "Browser viewport" }).waitFor();
  expect(String(await guest("document.cookie"))).toContain("native_fixture=signed-in");
  const other = await create("Other native fixture");
  await screenshot("browser-usability-rail.png", 96);
  const state = await client.call("browser.open", { botId: other.id });
  await client.call("browser.control", { botId: other.id, control: "human" });
  if (!state.activePageId) throw new Error("Second guest has no page.");
  await client.call("browser.navigate", { botId: other.id, generation: state.generation, pageId: state.activePageId, url: `${origin}/other` });
  await eventually(async () => await electronApp.evaluate(({ webContents }, fixtureOrigin) => webContents.getAllWebContents().some((contents) => contents.getURL() === `${fixtureOrigin}/other`), origin) || undefined);
  const cookies = await electronApp.evaluate(async ({ webContents }, fixtureOrigin) => {
    const target = webContents.getAllWebContents().find((contents) => contents.getURL() === `${fixtureOrigin}/other`);
    if (!target) throw new Error("The isolated second guest did not load.");
    return target.session.cookies.get({ url: fixtureOrigin });
  }, origin);
  expect(cookies).toEqual([]);
  await client.call("bots.update", { id: bot.id, patch: { browser: false } });
  await page.getByRole("tab", { name: "Browser", exact: true }).waitFor({ state: "hidden" });
  expect(await page.getByRole("textbox", { name: `Message ${bot.name}` }).evaluate((element) => element.closest("[inert]") !== null)).toBe(false);
  await client.call("bots.delete", { id: other.id });
}, 120_000);
