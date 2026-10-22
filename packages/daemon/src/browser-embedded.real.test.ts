import { createServer } from "node:http";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { connect, type Client } from "@tjommie/client";
import type { BrowserState } from "@tjommie/protocol";
import type { StoredBot } from "./bots.ts";
import { browserHandlers } from "./browser-handlers.ts";
import { BrowserManager, browserProfile } from "./browser-manager.ts";
import { BrowserMcp } from "./browser-mcp.ts";
import { McpTokens } from "./mcp.ts";
import { startServer, type Server } from "./server.ts";

const page = `<!doctype html><title>Shared browser fixture</title>
<style>body{margin:20px;font:18px sans-serif}input,button{display:block;margin:20px 0;padding:10px}</style>
<input id="name" aria-label="Name"><button id="save" onclick="document.cookie='signed_in=fixture;path=/;max-age=3600';localStorage.setItem('name',document.querySelector('input').value);document.title='Signed in fixture'">Sign in fixture</button>
<button id="popup" onclick="window.open('/popup','fixture-popup')">Open login popup</button>
<button id="dialog" onclick="alert('Fixture dialog')">Show dialog</button><a id="download" href="/download">Download fixture</a><input id="upload" type="file">
<div id="frame-marker" onclick="this.dataset.clicked='yes'" style="position:fixed;left:480px;top:20px;width:24px;height:24px;background:red"></div>`;
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
async function eventually<T>(read: () => Promise<T>, check: (value: T) => boolean): Promise<T> {
  const deadline = Date.now() + 20_000;
  while (true) {
    const value = await read();
    if (check(value)) return value;
    if (Date.now() > deadline) throw new Error("Browser fixture timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

it("shares real panel input, MCP tabs/cookies, persistent sign-in, and isolated bot profiles without a CDP port", async () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie embedded browser-"));
  const bot = { id: "fixture-one", homeFolder: join(root, "one"), browser: true, autonomy: "folder" } as StoredBot;
  const other = { ...bot, id: "fixture-two", homeFolder: join(root, "two") };
  const getBot = (id: string): StoredBot => { if (id === bot.id) return bot; if (id === other.id) return other; throw new Error("Unknown bot"); };
  const manager = new BrowserManager();
  const tokens = new McpTokens();
  const mcp = new BrowserMcp({ tokens, getBot, manager });
  const fixture = createServer((req, res) => {
    if (req.url === "/download") { res.writeHead(200, { "Content-Type": "text/plain", "Content-Disposition": "attachment; filename=fixture-download.txt" }).end("browser download fixture"); return; }
    if (req.url === "/frame/red" || req.url === "/frame/blue") { res.writeHead(200, { "Content-Type": "text/html" }).end(`<!doctype html><style>html{background:${req.url.endsWith("red") ? "red" : "blue"}}</style>`); return; }
    res.writeHead(200, { "Content-Type": "text/html" }).end(req.url === "/popup" ? "<!doctype html><title>Login popup</title><button onclick='window.close()'>Done</button>" : page);
  });
  let server: Server | undefined;
  let client: Client | undefined;
  try {
    await new Promise<void>((resolve) => fixture.listen(0, "127.0.0.1", resolve));
    const address = fixture.address();
    if (!address || typeof address === "string") throw new Error("Missing fixture address");
    const url = `http://127.0.0.1:${address.port}/`;
    server = await startServer({ port: 0, token: "test-app-token", allowedOrigins: ["app://tjommie"], handlers: browserHandlers(getBot, manager), routes: { "/mcp/browser": mcp.handle } });
    client = await connect(server.port, "test-app-token");
    const app = client;
    let state: BrowserState = await app.call("browser.open", { botId: bot.id });
    expect(state.control).toBe("bot");
    state = await app.call("browser.control", { botId: bot.id, control: "human" });
    const target = () => ({ botId: bot.id, generation: state.generation, pageId: state.activePageId! });
    state = await app.call("browser.navigate", { ...target(), url });
    const frame = await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.frame !== null && value.state.tabs[0]?.title === "Shared browser fixture");
    expect(frame.frame?.data.length).toBeGreaterThan(100);
    expect(frame.frame?.width).toBe(1000);
    const context = await manager.context(bot);
    const current = context.pages()[0];
    // Inspect only the synthetic profile's browser parent; never print command lines or inherited secrets.
    const flags: unknown = JSON.parse(execFileSync("powershell.exe", ["-NoProfile", "-Command", "$owned = @(Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'chrome.exe' -or $_.Name -eq 'msedge.exe') -and $_.CommandLine -and $_.CommandLine.Contains($env:TJOMMIE_BROWSER_TEST_PROFILE) -and -not $_.CommandLine.Contains('--type=') }); [pscustomobject]@{ count=$owned.Count; pipe=($owned.Count -eq 1 -and $owned[0].CommandLine.Contains('--remote-debugging-pipe')); tcp=($owned.Count -eq 1 -and $owned[0].CommandLine.Contains('--remote-debugging-port')); headless=($owned.Count -eq 1 -and $owned[0].CommandLine.Contains('--headless')) } | ConvertTo-Json -Compress"], { encoding: "utf8", windowsHide: true, env: { ...process.env, TJOMMIE_BROWSER_TEST_PROFILE: browserProfile(bot) } }));
    expect(flags).toEqual({ count: 1, pipe: true, tcp: false, headless: true });
    const readFrameColor = async () => {
      const view = await app.call("browser.frame", { botId: bot.id });
      if (!view.frame) return null;
      const rgb = await current.evaluate<number[]>(`(async () => {
        const image = new Image(); image.src = ${JSON.stringify(`data:image/jpeg;base64,${view.frame.data}`)}; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
        const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
        return Array.from(context.getImageData(20, 20, 1, 1).data);
      })()`);
      return { color: rgb[0] > 200 && rgb[2] < 20 ? "red" : rgb[2] > 200 && rgb[0] < 20 ? "blue" : "other", url: view.state.tabs[0].url };
    };
    state = await app.call("browser.navigate", { ...target(), url: `${url}frame/red` });
    await eventually(readFrameColor, (view) => view?.color === "red");
    state = await app.call("browser.navigate", { ...target(), url: `${url}frame/blue` });
    expect(await eventually(readFrameColor, (view) => view !== null)).toEqual({ color: "blue", url: `${url}frame/blue` });
    for (const [action, color] of [["back", "red"], ["forward", "blue"], ["reload", "blue"]] as const) {
      state = await app.call("browser.history", { ...target(), action });
      expect(await eventually(readFrameColor, (view) => view !== null)).toEqual({ color, url: `${url}frame/${color}` });
    }
    state = await app.call("browser.navigate", { ...target(), url });
    // Use UI RPC coordinate and keyboard input; no direct fill/click in the real page.
    const box = await current.locator("#name").boundingBox();
    if (!box) throw new Error("Missing input");
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), input: { kind: "mouse", action, x: box.x + 8, y: box.y + 8 } });
    await app.call("browser.input", { ...target(), input: { kind: "text", text: "Manual sign-in" } });
    await app.call("browser.input", { ...target(), input: { kind: "key", action: "down", key: "Tab" } });
    await app.call("browser.input", { ...target(), input: { kind: "key", action: "up", key: "Tab" } });
    await app.call("browser.input", { ...target(), input: { kind: "key", action: "down", key: "Enter" } });
    await app.call("browser.input", { ...target(), input: { kind: "key", action: "up", key: "Enter" } });
    expect(await current.title()).toBe("Signed in fixture");
    expect(client.events).toEqual([]); // frames and typed input never enter the event history
    // A real dialog must not prevent the input RPC completing or frame polling from surfacing it.
    const dialogBox = await current.locator("#dialog").boundingBox();
    if (!dialogBox) throw new Error("Missing dialog button");
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), input: { kind: "mouse", action, x: dialogBox.x + 8, y: dialogBox.y + 8 } });
    const dialogView = await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.state.dialog !== null);
    expect(dialogView.state.dialog?.message).toBe("Fixture dialog");
    await app.call("browser.dialog", { ...target(), accept: true });
    expect((await app.call("browser.frame", { botId: bot.id })).state.dialog).toBeNull();
    const assertPanelSize = async (selectedPage: typeof current): Promise<void> => {
      expect(selectedPage.viewportSize()).toEqual({ width: 528, height: 396 });
      expect(await selectedPage.evaluate("({width: innerWidth, height: innerHeight})")).toEqual({ width: 528, height: 396 });
      const view = await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.frame !== null);
      const pixels = await selectedPage.evaluate<{ width: number; height: number; marker: number[] }>(`(async () => {
        const image = new Image(); image.src = ${JSON.stringify(`data:image/jpeg;base64,${view.frame!.data}`)}; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
        const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
        return {width: image.naturalWidth, height: image.naturalHeight, marker: Array.from(context.getImageData(492, 32, 1, 1).data)};
      })()`);
      expect(view.frame).toMatchObject({ width: 528, height: 396 });
      expect(pixels).toMatchObject({ width: 528, height: 396 });
      if (selectedPage === current) {
        expect(pixels.marker[0]).toBeGreaterThan(200);
        expect(pixels.marker[1]).toBeLessThan(20);
        expect(pixels.marker[2]).toBeLessThan(20);
      }
    };
    state = await app.call("browser.resize", { ...target(), width: 528, height: 396 });
    await assertPanelSize(current);
    const popupBox = await current.locator("#popup").boundingBox();
    if (!popupBox) throw new Error("Missing manual popup button");
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), input: { kind: "mouse", action, x: popupBox.x + 8, y: popupBox.y + 8 } });
    state = (await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.state.tabs.length === 2)).state;
    const manualPopup = context.pages().find((value) => value !== current);
    if (!manualPopup) throw new Error("Missing manual popup page");
    state = await app.call("browser.tab", { ...target(), action: "select" });
    state = await app.call("browser.resize", { ...target(), width: 528, height: 396 });
    await assertPanelSize(manualPopup);
    state = await app.call("browser.tab", { ...target(), action: "close" });
    state = await app.call("browser.resize", { ...target(), width: 528, height: 396 });
    await assertPanelSize(current);
    // Closing quickly can leave the popup at its default 1000px width; the original must still resize correctly.
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), input: { kind: "mouse", action, x: popupBox.x + 8, y: popupBox.y + 8 } });
    state = (await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.state.tabs.length === 2)).state;
    state = await app.call("browser.tab", { ...target(), action: "close" });
    state = await app.call("browser.resize", { ...target(), width: 528, height: 396 });
    await assertPanelSize(current);
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), input: { kind: "mouse", action, x: 492, y: 32 } });
    expect(await current.locator("#frame-marker").getAttribute("data-clicked")).toBe("yes");
    const mcpToken = tokens.issue(bot.id);
    const rpc = async (name: string, args: Record<string, unknown>): Promise<unknown> => {
      const response = await fetch(`http://127.0.0.1:${server!.port}/mcp/browser`, { method: "POST", headers: { Authorization: `Bearer ${mcpToken}`, "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } }) });
      expect(response.status).toBe(200);
      const result: unknown = await response.json();
      if (!isRecord(result)) throw new Error("Missing MCP response");
      return result.result;
    };
    expect(await rpc("browser_snapshot", {})).toMatchObject({ isError: true });
    state = await app.call("browser.control", { botId: bot.id, control: "bot" });
    await expect(app.call("browser.resize", { ...target(), width: 500, height: 400 })).rejects.toThrow(/Take over/);
    const snapshot = await rpc("browser_evaluate", { function: "() => ({cookie:document.cookie, name:localStorage.getItem('name')})" });
    expect(snapshot).not.toHaveProperty("isError", true);
    expect(JSON.stringify(snapshot)).toContain("signed_in=fixture");
    expect(JSON.stringify(snapshot)).toContain("Manual sign-in");
    const download = await rpc("browser_evaluate", { function: "() => { document.querySelector('#download').click(); return true; }" });
    expect(download).not.toHaveProperty("isError", true);
    const savedDownloads = (): string[] => readdirSync(join(bot.homeFolder, "browser-files")).filter((file) => file.endsWith(".txt") && readFileSync(join(bot.homeFolder, "browser-files", file), "utf8") === "browser download fixture");
    await eventually(async () => savedDownloads(), (files) => files.length >= 1);
    expect(savedDownloads()).toHaveLength(1);
    const raised = await rpc("browser_evaluate", { function: "() => { setTimeout(() => alert('Bot dialog'), 50); return true; }" });
    expect(raised).not.toHaveProperty("isError", true);
    await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.state.dialog?.message === "Bot dialog");
    const answered = await rpc("browser_handle_dialog", { accept: true });
    expect(answered).not.toHaveProperty("isError", true);
    await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.state.dialog === null);
    const popupResult = await rpc("browser_evaluate", { function: "() => { window.open('/popup', 'fixture-popup'); return true; }" });
    expect(popupResult).not.toHaveProperty("isError", true);
    state = (await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.state.tabs.length === 2)).state;
    expect(state.tabs.some((tab) => tab.url.endsWith("/popup"))).toBe(true);
    await rpc("browser_tabs", { action: "select", index: 1 });
    state = (await app.call("browser.frame", { botId: bot.id })).state;
    expect(state.activePageId).toBe(state.tabs[1].id);
    state = await app.call("browser.control", { botId: bot.id, control: "human" });
    // A CLI stopping or going idle closes its MCP connection, not the person's browser/sign-in session.
    const connectedContext = await manager.context(bot);
    const generationBeforeDisconnect = state.generation;
    await mcp.close(bot.id);
    expect(await manager.context(bot)).toBe(connectedContext);
    state = await app.call("browser.navigate", { ...target(), url: `${url}?after=mcp-disconnect` });
    expect(state.generation).toBe(generationBeforeDisconnect);
    expect(state.tabs).toHaveLength(2);
    expect((await connectedContext.cookies(url)).some((cookie) => cookie.name === "signed_in")).toBe(true);
    const activePage = connectedContext.pages()[1];
    const downloadBox = await activePage.locator("#download").boundingBox();
    if (!downloadBox) throw new Error("Missing manual download link");
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), input: { kind: "mouse", action, x: downloadBox.x + 4, y: downloadBox.y + 4 } });
    await eventually(async () => savedDownloads(), (files) => files.length >= 2);
    expect(savedDownloads()).toHaveLength(2); // one MCP-owned save and one manual save after MCP disconnect
    state = await app.call("browser.control", { botId: bot.id, control: "bot" });
    expect(await rpc("browser_snapshot", {})).not.toHaveProperty("isError", true);
    state = await app.call("browser.control", { botId: bot.id, control: "human" });
    // There is no native file picker in the panel. The bot can cancel a manual chooser after resuming.
    const uploadBox = await activePage.locator("#upload").boundingBox();
    if (!uploadBox) throw new Error("Missing file input");
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), input: { kind: "mouse", action, x: uploadBox.x + 4, y: uploadBox.y + 4 } });
    state = await app.call("browser.control", { botId: bot.id, control: "bot" });
    expect(await rpc("browser_file_upload", { paths: [] })).not.toHaveProperty("isError", true);
    expect(await rpc("browser_snapshot", {})).not.toHaveProperty("isError", true);
    state = await app.call("browser.control", { botId: bot.id, control: "human" });
    await expect(app.call("browser.navigate", { ...target(), url: "file:///C:/Windows/win.ini" })).rejects.toThrow(/web addresses/);
    const isolated = await app.call("browser.open", { botId: other.id });
    await app.call("browser.control", { botId: other.id, control: "human" });
    await app.call("browser.navigate", { botId: other.id, generation: isolated.generation, pageId: isolated.activePageId!, url });
    expect(await (await manager.context(other)).cookies(url)).toEqual([]);
    const oldGeneration = state.generation;
    await mcp.close(bot.id);
    await manager.close(bot.id);
    state = await app.call("browser.open", { botId: bot.id });
    expect(state.generation).not.toBe(oldGeneration);
    expect(state.control).toBe("bot");
    state = await app.call("browser.control", { botId: bot.id, control: "human" });
    state = await app.call("browser.navigate", { ...target(), url });
    expect((await (await manager.context(bot)).cookies(url)).some((cookie) => cookie.name === "signed_in" && cookie.value === "fixture")).toBe(true);
    expect(await (await manager.context(bot)).pages()[0].evaluate(() => localStorage.getItem("name"))).toBe("Manual sign-in");
    await expect(app.call("browser.input", { ...target(), generation: oldGeneration, input: { kind: "text", text: "stale" } })).rejects.toThrow(/restarted/);
  } finally {
    await mcp.closeAll();
    await manager.closeAll();
    await client?.close();
    await server?.close();
    await new Promise<void>((resolve) => fixture.close(() => resolve()));
    rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  }
});
