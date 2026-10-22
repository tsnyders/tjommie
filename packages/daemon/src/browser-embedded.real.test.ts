import { createServer } from "node:http";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { connect, type Client } from "@tjommie/client";
import type { BrowserFrame, BrowserInput, BrowserState } from "@tjommie/protocol";
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
    // Expanded pages exceed the screencast cap. Decode the actual JPEG and click a right-edge marker
    // in CSS coordinates to prove the two sizes are explicit rather than relabelled.
    await current.locator("#frame-marker").evaluate((element) => { element.setAttribute("style", `${element.getAttribute("style")};left:2480px`); element.removeAttribute("data-clicked"); });
    state = await app.call("browser.resize", { ...target(), width: 2560, height: 1600 });
    const expanded = await eventually(() => app.call("browser.frame", { botId: bot.id }), (value) => value.frame !== null);
    expect(expanded.frame).toMatchObject({ width: 1280, height: 800, viewportWidth: 2560, viewportHeight: 1600, viewportRevision: state.viewportRevision });
    const expandedPixels = await current.evaluate<{ width: number; height: number; marker: number[] }>(`(async () => {
      const image = new Image(); image.src = ${JSON.stringify(`data:image/jpeg;base64,${expanded.frame!.data}`)}; await image.decode();
      const canvas = document.createElement('canvas'); canvas.width=image.width; canvas.height=image.height;
      const ctx = canvas.getContext('2d'); ctx.drawImage(image,0,0);
      return {width:image.naturalWidth,height:image.naturalHeight,marker:Array.from(ctx.getImageData(1246,16,1,1).data)};
    })()`);
    expect(expandedPixels).toMatchObject({ width: 1280, height: 800 });
    expect(expandedPixels.marker[0]).toBeGreaterThan(200); expect(expandedPixels.marker[1]).toBeLessThan(30);
    for (const action of ["down", "up"] as const) await app.call("browser.input", { ...target(), viewportRevision: state.viewportRevision, input: { kind: "mouse", action, x: 2492, y: 32 } });
    expect(await current.locator("#frame-marker").getAttribute("data-clicked")).toBe("yes");
    await current.locator("#frame-marker").evaluate((element) => { element.setAttribute("style", `${element.getAttribute("style")};left:480px`); });
    state = await app.call("browser.resize", { ...target(), width: 528, height: 396 });
    await assertPanelSize(current);
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

it("measures ordered input to DOM and decoded browser-frame presentation in a disposable fixture", async () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie browser latency-"));
  const bot = { id: "latency-fixture", homeFolder: join(root, "bot"), browser: true, autonomy: "folder" } as StoredBot;
  const manager = new BrowserManager();
  let client: Client | undefined;
  let server: Server | undefined;
  let closeView: (() => Promise<void>) | undefined;
  let closeSession: (() => Promise<void>) | undefined;
  type View = { state: BrowserState | null; frame: BrowserFrame | null; error: string | null };
  type Session = { open(): void; input(input: BrowserInput): void; dialog(accept: boolean): void; suspend(): Promise<void>; close(): Promise<void>; view: View };
  const runtime: unknown = await import(new URL("../../ui/lib/browser-session.ts", import.meta.url).href);
  const SessionClass = (runtime as { BrowserSession: new (botId: string, call: Client["call"], changed: (view: View) => void) => Session }).BrowserSession;
  try {
    server = await startServer({ port: 0, token: "latency-token", allowedOrigins: [], handlers: browserHandlers(() => bot, manager) });
    client = await connect(server.port, "latency-token");
    const state = await manager.control(bot, "human");
    const context = await manager.context(bot);
    const page = context.pages()[0];
    const starts = new Map<number, number>();
    const dom = new Map<number, number>();
    const shown = new Map<number, number>();
    await page.exposeBinding("domMark", (_source, value: number) => { if (starts.has(value)) dom.set(value, performance.now() - starts.get(value)!); });
    await page.setContent(`<style>html,body{margin:0;width:100%;height:100%;background:rgb(16,0,0)}</style><script>
      let step=0; addEventListener('keydown',e=>{if(e.key==='x'){step++;document.body.style.background='rgb('+(16+step*8)+',0,0)';window.domMark(step);}});
    </script>`);
    const renderer = context.browser();
    if (!renderer) throw new Error("Missing owned browser");
    const displayContext = await renderer.newContext({ viewport: { width: 1000, height: 720 } });
    closeView = () => displayContext.close();
    const display = await displayContext.newPage();
    await display.setContent("<canvas width='1000' height='720'></canvas>");
    let latest: BrowserFrame | null = null;
    let painting = false;
    let lastData = "";
    let paintError: unknown;
    const paint = async (): Promise<void> => {
      if (painting) return;
      painting = true;
      try {
        while (latest) {
          const frame = latest; latest = null;
          const step = await display.evaluate<number>(`(async()=>{
            const image=new Image();image.src=${JSON.stringify(`data:image/jpeg;base64,${frame.data}`)};await image.decode();
            const canvas=document.querySelector('canvas');const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0,1000,720);
            await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
            return Math.round((ctx.getImageData(500,360,1,1).data[0]-16)/8);
          })()`);
          if (starts.has(step) && !shown.has(step)) shown.set(step, performance.now() - starts.get(step)!);
        }
      } catch (error) { paintError = error; }
      finally { painting = false; }
    };
    const session = new SessionClass(bot.id, client.call, (view) => {
      if (view.frame && view.frame.data !== lastData) { lastData = view.frame.data; latest = view.frame; void paint(); }
    });
    closeSession = () => session.close();
    session.open();
    await eventually(async () => session.view, (view) => view.frame !== null);
    for (let step = 1; step <= 16; step++) {
      await new Promise((resolve) => setTimeout(resolve, 37 + step * 7 % 71));
      starts.set(step, performance.now());
      session.input({ kind: "key", action: "down", key: "x" });
      session.input({ kind: "key", action: "up", key: "x" });
      await eventually(async () => { if (paintError) throw paintError; return shown.has(step); }, Boolean);
    }
    const distribution = (values: number[]) => {
      const sorted = values.sort((a, b) => a - b);
      return { samples: sorted.length, p50: Math.round(sorted[Math.ceil(sorted.length * .5) - 1]), p95: Math.round(sorted[Math.ceil(sorted.length * .95) - 1]), max: Math.round(sorted.at(-1)!) };
    };
    process.stdout.write(`Browser latency milliseconds ${JSON.stringify({ dom: distribution([...dom.values()]), displayedJpeg: distribution([...shown.values()]) })}\n`);
    expect(dom.size).toBe(16);
    expect(shown.size).toBe(16);
    expect(state.control).toBe("human");
    const previous = session.view.frame?.data;
    await page.evaluate(`(()=>{const b=document.createElement('button');b.textContent='Confirm';b.style='position:fixed;left:20px;top:20px;width:120px;height:50px';b.onclick=()=>{document.body.dataset.confirmed=String(confirm('Latency fixture confirm'));};document.body.append(b);})()`);
    await eventually(async () => session.view.frame?.data, (value) => !!value && value !== previous);
    for (const action of ["down", "up"] as const) session.input({ kind: "mouse", action, button: "left", x: 50, y: 40 });
    await eventually(async () => session.view.state?.dialog, (dialog) => dialog?.message === "Latency fixture confirm");
    session.dialog(false);
    await eventually(async () => session.view.state?.dialog, (dialog) => dialog === null);
    expect(await page.locator("body").getAttribute("data-confirmed")).toBe("false");
    await session.suspend();
    expect((await manager.state(bot)).control).toBe("bot");
  } finally {
    await closeSession?.();
    await closeView?.();
    await client?.close();
    await manager.closeAll();
    await server?.close();
    rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  }
});

it("keeps SPA typing uninterrupted and delivers held key and pointer releases across a real modal", async () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie browser SPA-"));
  const bot = { id: "spa-fixture", homeFolder: join(root, "bot"), browser: true, autonomy: "folder" } as StoredBot;
  const manager = new BrowserManager();
  const fixture = createServer((_req, res) => res.writeHead(200, { "Content-Type": "text/html" }).end(`<!doctype html><title>SPA input fixture</title>
    <style>input,button{position:fixed;left:20px;width:260px;height:50px}input{top:20px}button{top:100px}</style>
    <input id="spa" autofocus oninput="history.replaceState(null,'','?q='+encodeURIComponent(this.value))">
    <button id="modal" onmousedown="confirm('Held input confirm')">Open modal while held</button>
    <script>addEventListener('keydown',e=>document.body.dataset.lastShift=String(e.shiftKey));
    addEventListener('mousemove',e=>document.body.dataset.buttons=String(e.buttons));
    addEventListener('mouseup',()=>document.body.dataset.released='yes');</script>`));
  let server: Server | undefined, client: Client | undefined;
  let closeSession: (() => Promise<void>) | undefined;
  type View = { state: BrowserState | null; frame: BrowserFrame | null; error: string | null; busy?: boolean };
  type Session = { open(): void; input(input: BrowserInput): void; dialog(accept: boolean): void; close(): Promise<void>; view: View };
  const runtime: unknown = await import(new URL("../../ui/lib/browser-session.ts", import.meta.url).href);
  const SessionClass = (runtime as { BrowserSession: new (botId: string, call: Client["call"], changed: (view: View) => void) => Session }).BrowserSession;
  try {
    await new Promise<void>((resolve) => fixture.listen(0, "127.0.0.1", resolve));
    const address = fixture.address();
    if (!address || typeof address === "string") throw new Error("Missing SPA fixture address");
    const url = `http://127.0.0.1:${address.port}/`;
    server = await startServer({ port: 0, token: "spa-token", allowedOrigins: [], handlers: browserHandlers(() => bot, manager) });
    client = await connect(server.port, "spa-token");
    const initial = await client.call("browser.control", { botId: bot.id, control: "human" });
    await client.call("browser.navigate", { botId: bot.id, generation: initial.generation, pageId: initial.activePageId!, url });
    const page = (await manager.context(bot)).pages()[0];
    const streams = new Set<string>();
    const errors: string[] = [];
    let watching = false, blanks = 0;
    const session = new SessionClass(bot.id, client.call, (view) => {
      if (!watching) return;
      if (!view.frame) blanks++;
      if (view.frame?.streamId) streams.add(view.frame.streamId);
      if (view.error) errors.push(view.error);
    });
    closeSession = () => session.close();
    session.open();
    await eventually(async () => session.view, (view) => !!view.frame && !view.busy);
    const layout = session.view.state!.viewportRevision;
    streams.add(session.view.frame!.streamId!); watching = true;
    const text = "abcdefghijklmnop";
    for (const key of text) {
      session.input({ kind: "key", action: "down", key });
      session.input({ kind: "key", action: "up", key });
      await new Promise((resolve) => setTimeout(resolve, 12));
    }
    await eventually(() => page.locator("#spa").inputValue(), (value) => value === text);
    await eventually(async () => session.view.state?.tabs[0].url, (value) => value === `${url}?q=${text}`);
    expect(session.view.state?.viewportRevision).toBe(layout);
    expect(blanks).toBe(0); expect(streams.size).toBe(1); expect(errors).toEqual([]);
    // pushState and hash changes are also same-document navigation and retain the stream.
    await page.evaluate("history.pushState(null,'','?route=next');location.hash='section'");
    await eventually(async () => session.view.state?.tabs[0].url, (value) => value === `${url}?route=next#section`);
    expect(blanks).toBe(0); expect(streams.size).toBe(1);
    watching = false;

    session.input({ kind: "key", action: "down", key: "Shift" });
    session.input({ kind: "mouse", action: "down", button: "left", x: 50, y: 120 });
    await eventually(async () => session.view.state?.dialog, (dialog) => dialog?.message === "Held input confirm");
    session.input({ kind: "key", action: "up", key: "Shift" });
    session.input({ kind: "mouse", action: "up", button: "left", x: 50, y: 120 });
    session.dialog(false);
    await eventually(async () => session.view.state?.dialog, (dialog) => dialog === null);
    session.input({ kind: "mouse", action: "move", x: 300, y: 200 });
    await eventually(() => page.locator("body").getAttribute("data-buttons"), (value) => value === "0");
    expect(await page.locator("body").getAttribute("data-released")).toBe("yes");
    for (const action of ["down", "up"] as const) session.input({ kind: "mouse", action, button: "left", x: 200, y: 40 });
    session.input({ kind: "key", action: "down", key: "a" });
    session.input({ kind: "key", action: "up", key: "a" });
    await eventually(() => page.locator("body").getAttribute("data-last-shift"), (value) => value === "false");
    expect((await page.locator("#spa").inputValue()).length).toBe(17);
    expect(session.view.error).toBeNull();
  } finally {
    await closeSession?.();
    await client?.close();
    await manager.closeAll();
    await server?.close();
    await new Promise<void>((resolve) => fixture.close(() => resolve()));
    rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  }
});
