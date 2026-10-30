import { _electron, type ElectronApplication } from "playwright-core";
import { createServer, type Server } from "node:http";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, expect, it } from "vitest";
import type { NativeBrowserFixture } from "./native-browser-fixture.ts";
import type { NativeBrowserBot } from "./native-browser.ts";
import type { NativeBrowserToolResult } from "./native-browser-automation.ts";
import { nativeBrowserVideo } from "./native-browser-media-fixture.ts";

let root: string, origin: string, server: Server, electron: ElectronApplication | undefined;
let postBody = "";
type FixtureGlobal = { nativeBrowserFixture: NativeBrowserFixture };
// Functions sent to Electron must be self-contained; this declaration is type-only in their bodies.
async function launch(): Promise<ElectronApplication> {
  const env: Record<string, string> = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
  env.TJOMMIE_NATIVE_FIXTURE = root; delete env.ELECTRON_RUN_AS_NODE;
  const app = await _electron.launch({ args: [join(import.meta.dirname, "native-browser-fixture.ts")], env });
  // The engine owns dialog decisions. Playwright otherwise auto-dismisses dialogs on its
  // automatically attached guest pages, which would invalidate the native dialog assertion.
  app.context().on("page", (page) => page.on("dialog", () => {}));
  for (const page of app.context().pages()) page.on("dialog", () => {});
  try {
    for (let attempt = 0; attempt < 100; attempt++) {
      if (await app.evaluate(() => !!(globalThis as unknown as FixtureGlobal).nativeBrowserFixture)) return app;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error("The native browser fixture did not initialize.");
  } catch (error) { await app.close(); throw error; }
}
beforeAll(async () => {
  root = mkdtempSync(join(tmpdir(), "tjommie-native-"));
  server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://fixture.invalid");
    if (url.pathname === "/frames") {
      res.writeHead(200, { "content-type": "text/html" }).end(`<!doctype html><body><script>
        window.frameReady=[];addEventListener('message',e=>frameReady.push(e.data));
        const mark=label=>'<script>top.postMessage('+JSON.stringify(label)+',"*")</scr'+'ipt>';
        const nested='<iframe src="data:text/html,'+encodeURIComponent(mark('nested-data'))+'"></iframe>';
        const srcdoc=document.createElement('iframe');srcdoc.srcdoc=mark('srcdoc')+nested;document.body.append(srcdoc);
        const blob=document.createElement('iframe');blob.src=URL.createObjectURL(new Blob([mark('blob')],{type:'text/html'}));document.body.append(blob);
      </script></body>`); return;
    }
    if (url.pathname === "/video.webm") { res.writeHead(200, { "content-type": "video/webm", "content-length": nativeBrowserVideo.length }).end(nativeBrowserVideo); return; }
    if (url.pathname === "/download") { res.writeHead(200, { "content-type": "text/plain", "content-disposition": 'attachment; filename="native-fixture.txt"' }).end("synthetic native download"); return; }
    if (url.pathname === "/post") { req.on("data", (data: Buffer) => { postBody += data.toString(); }); }
    res.writeHead(200, { "content-type": "text/html", ...(url.pathname === "/cookie" ? { "set-cookie": "native_session=disposable; Max-Age=3600; Path=/; SameSite=Lax" } : {}) });
    res.end(`<!doctype html><html><head><title>Native fixture ${url.pathname}</title></head><body style="margin:0;background:rgb(200,30,40)">
      <input id="input" style="position:absolute;left:20px;top:20px;width:200px;height:30px">
      <input id="upload" type="file" aria-label="Fixture upload" style="position:absolute;left:300px;top:20px">
      <button id="popup" style="position:absolute;left:20px;top:80px" onclick="window.open('/popup','named-native','width=400,height=300')">Popup</button>
      <form method="POST" action="/post" target="post-native"><input name="proof" value="preserved"><button id="post">POST popup</button></form>
      <button id="media" style="position:absolute;left:20px;top:130px" onclick="startMedia().catch(e=>window.mediaError=String(e))">Media</button>
      <video id="video" muted playsinline style="position:absolute;left:20px;top:200px;width:200px;height:150px"></video>
      <script>window.messages=[];addEventListener('message',e=>messages.push(e.data));if(opener)opener.postMessage('popup-opener-ok','*');
      window.startMedia=async()=>{video.src='/video.webm';video.loop=true;await video.play();window.audio=new AudioContext();const o=audio.createOscillator(),g=audio.createGain();g.gain.value=.02;o.connect(g);g.connect(audio.destination);o.start();setTimeout(()=>o.stop(),1800);};</script></body></html>`);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address(); if (!address || typeof address === "string") throw new Error("Fixture server has no port.");
  origin = `http://127.0.0.1:${address.port}`;
});

it("shares native automation refs, uploads, dialogs and downloads while enforcing human control and immediate concealment", async () => {
  await electron?.close(); electron = await launch();
  const app = electron;
  const bot: NativeBrowserBot = { id: "native-tools", homeFolder: join(root, "bot-tools") };
  mkdirSync(bot.homeFolder); const upload = join(bot.homeFolder, "upload.txt"); writeFileSync(upload, "synthetic upload");
  const tool = (name: string, args: Record<string, unknown> = {}): Promise<NativeBrowserToolResult> => app.evaluate(async (_electron, { bot, name, args }) => {
    return await (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.execute(bot, { kind: "tool", name, args }) as NativeBrowserToolResult;
  }, { bot, name, args });
  const text = (result: NativeBrowserToolResult): string => result.content.flatMap((part) => part.type === "text" ? [part.text] : []).join("\n");
  expect((await tool("browser_navigate", { url: origin })).isError).not.toBe(true);
  const snapshot = await tool("browser_snapshot");
  const ref = text(snapshot).match(/- textbox .*?\[ref=([^\]]+)\]/)?.[1];
  expect(ref, text(snapshot)).toBeTruthy();
  expect((await tool("browser_type", { ref, text: "native automation" })).isError).not.toBe(true);
  expect(text(await tool("browser_evaluate", { function: "() => document.querySelector('#input').value" }))).toContain("native automation");
  const screenshot = await tool("browser_take_screenshot", { type: "jpeg" });
  expect(screenshot.isError, text(screenshot)).not.toBe(true);
  expect(screenshot.content.some((part) => part.type === "image" && part.data.length > 100)).toBe(true);
  expect((await tool("browser_click", { target: "#upload" })).isError).not.toBe(true);
  expect((await tool("browser_file_upload", { paths: [upload] })).isError).not.toBe(true);
  expect(text(await tool("browser_evaluate", { function: "() => document.querySelector('#upload').files[0]?.name" }))).toContain("upload.txt");
  const modal = await tool("browser_evaluate", { function: "() => confirm('Synthetic native confirmation')" });
  expect(await app.evaluate(() => (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.state("native-tools").dialog?.message), text(modal)).toBe("Synthetic native confirmation");
  expect((await tool("browser_handle_dialog", { accept: false })).isError).not.toBe(true);
  expect((await tool("browser_evaluate", { function: "() => { console.log('synthetic native log'); return true; }" })).isError).not.toBe(true);
  expect(text(await tool("browser_console_messages"))).toContain("synthetic native log");
  expect(text(await tool("browser_evaluate", { function: "() => new Promise(resolve => navigator.geolocation.getCurrentPosition(() => resolve('allowed'), error => resolve(error.code)))" }))).toContain("1");
  await tool("browser_evaluate", { function: "() => { const a=document.createElement('a'); a.href='/download'; a.download='native-fixture.txt'; a.click(); }" });
  const downloads = join(bot.homeFolder, "browser-files");
  await expect.poll(() => existsSync(downloads) ? readdirSync(downloads).filter((name) => name.endsWith("native-fixture.txt")).length : 0, { timeout: 5000 }).toBe(1);
  const download = readdirSync(downloads).find((name) => name.endsWith("native-fixture.txt"))!;
  expect(readFileSync(join(downloads, download), "utf8")).toBe("synthetic native download");
  await app.evaluate(async (_electron, bot) => {
    const { engine } = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    await engine.mount(bot, { visible: true, bounds: { x: 240, y: 80, width: 650, height: 600 } });
    await engine.control(bot, "human");
  }, bot);
  const rejected = await tool("browser_type", { target: "#input", text: "must not happen" });
  expect(rejected.isError).toBe(true); expect(text(rejected)).toMatch(/person|control/i);
  // Native held input is released on the original page before tab selection changes.
  const held = await app.evaluate(async ({ webContents }, bot) => {
    const { engine } = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    const state = engine.state(bot.id), first = state.activePageId!;
    const contents = webContents.getAllWebContents().find((contents) => contents.getURL().startsWith("http://127.0.0.1:") && contents.session.storagePath?.includes("native-browser"))!;
    await contents.executeJavaScript("window.releases=[];addEventListener('keyup',e=>releases.push(e.key));addEventListener('mouseup',()=>releases.push('mouse'));document.querySelector('#input').focus()");
    contents.sendInputEvent({ type: "keyDown", keyCode: "Shift" });
    contents.sendInputEvent({ type: "mouseDown", x: 80, y: 35, button: "left", clickCount: 1 });
    await engine.act(bot, { kind: "tab", generation: state.generation, action: "new" }, "human");
    const released: unknown = await contents.executeJavaScript("releases");
    await engine.act(bot, { kind: "tab", generation: state.generation, action: "select", pageId: first }, "human");
    contents.sendInputEvent({ type: "keyDown", keyCode: "Tab", modifiers: ["shift"] });
    return released;
  }, bot);
  expect(held).toEqual(expect.arrayContaining(["Shift", "mouse"]));
  await expect.poll(() => app.evaluate(() => (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.focusReturns)).toContain(bot.id);
  await app.evaluate(async (_electron, bot) => {
    const fixture = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    fixture.engine.hideBot(bot.id);
    if (fixture.owner.contentView.children.some((view) => view.getVisible())) throw new Error("hideBot left a native surface visible");
  }, bot);
  await expect.poll(() => app.evaluate(() => (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.state("native-tools").control)).toBe("bot");
  expect((await tool("browser_type", { target: "#input", text: "after handback" })).isError).not.toBe(true);
  await app.evaluate(async () => { await (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.closeAll(); });
});
afterAll(async () => {
  try { await electron?.close(); }
  finally {
    try { if (server) await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())); }
    finally { if (root) rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 }); }
  }
});

it("renders native views with isolated persistent sessions, direct input/media, in-panel POST popups and lifecycle guards", async () => {
  await electron?.close(); electron = await launch();
  const app = electron;
  const bot: NativeBrowserBot = { id: "native-a", homeFolder: join(root, "bot-a") };
  const other: NativeBrowserBot = { id: "native-b", homeFolder: join(root, "bot-b") };
  mkdirSync(bot.homeFolder); mkdirSync(other.homeFolder);
  const initial = await app.evaluate(async (_electron, bot) => (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.open(bot), bot);
  expect(initial.control).toBe("bot");
  await app.evaluate(async (_electron, { bot, origin }) => {
    const { engine } = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    await engine.withAutomation(bot, {}, async (contents) => { await contents.loadURL(origin + "/cookie"); });
    await engine.mount(bot, { visible: true, bounds: { x: 240, y: 80, width: 650, height: 600 } });
    await engine.control(bot, "human");
  }, { bot, origin });
  const security = await app.evaluate(async ({ webContents }, origin) => {
    const contents = webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!;
    const prefs = (contents as unknown as { getLastWebPreferences(): { nodeIntegration: boolean; contextIsolation: boolean; sandbox: boolean; preload?: string } }).getLastWebPreferences();
    return { node: prefs.nodeIntegration, isolated: prefs.contextIsolation, sandbox: prefs.sandbox, preload: prefs.preload,
      globals: await contents.executeJavaScript("[typeof require, typeof process, typeof window.tjommie]"), storage: contents.session.storagePath };
  }, origin);
  expect(security).toMatchObject({ node: false, isolated: true, sandbox: true, globals: ["undefined", "undefined", "undefined"] });
  expect(security.preload).toBeFalsy(); expect(security.storage).toContain(join(root, "data", "native-browser"));
  await app.evaluate(async ({ webContents }, origin) => {
    const contents = webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!;
    contents.sendInputEvent({ type: "mouseDown", x: 60, y: 35, button: "left", clickCount: 1 });
    contents.sendInputEvent({ type: "mouseUp", x: 60, y: 35, button: "left", clickCount: 1 });
    contents.sendInputEvent({ type: "keyDown", keyCode: "N" }); contents.sendInputEvent({ type: "char", keyCode: "n" }); contents.sendInputEvent({ type: "keyUp", keyCode: "N" });
  }, origin);
  await expect.poll(() => app.evaluate(async ({ webContents }, origin) => webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!.executeJavaScript("document.querySelector('#input').value"), origin)).toBe("n");
  await app.evaluate(async ({ webContents }, origin) => {
    const contents = webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!;
    contents.sendInputEvent({ type: "mouseDown", x: 45, y: 140, button: "left", clickCount: 1 }); contents.sendInputEvent({ type: "mouseUp", x: 45, y: 140, button: "left", clickCount: 1 });
  }, origin);
  let media: unknown;
  try {
    await expect.poll(async () => {
      media = await app.evaluate(async ({ webContents }, origin) => webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!.executeJavaScript("({audio:window.audio?.state,progress:video.currentTime > .1,ready:video.readyState,source:video.currentSrc.slice(0,20),bytes:window.recordedBytes,codec:video.canPlayType('video/webm;codecs=vp8'),error:window.mediaError ?? null})"), origin);
      return media;
    }, { timeout: 5000 }).toMatchObject({ audio: "running", progress: true, error: null });
  } catch (error) { console.info("Native synthetic media diagnostics", media); throw error; }
  await expect.poll(() => app.evaluate(({ webContents }, origin) => webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!.isCurrentlyAudible(), origin), { timeout: 5000 }).toBe(true);
  // The actual native contents produces pixels at its panel viewport, without any frame RPC.
  const image = await app.evaluate(async ({ webContents }, origin) => { const contents = webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!; return (await contents.capturePage()).getSize(); }, origin);
  expect(image).toEqual({ width: 650, height: 600 });
  await app.evaluate(async ({ webContents }, origin) => { await webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!.executeJavaScript("document.querySelector('#popup').click()", true); }, origin);
  await expect.poll(() => app.evaluate(() => (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.state("native-a").tabs.length)).toBe(2);
  await expect.poll(() => app.evaluate(async ({ webContents }, origin) => webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!.executeJavaScript("messages.includes('popup-opener-ok')"), origin)).toBe(true);
  expect(await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().length)).toBe(1);
  await app.evaluate(async ({ webContents }, origin) => { await webContents.getAllWebContents().find((item) => item.getURL() === origin + "/cookie")!.executeJavaScript("document.querySelector('form').requestSubmit()", true); }, origin);
  await expect.poll(() => postBody).toContain("proof=preserved");
  expect(await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().length)).toBe(1);
  // Permission, occlusion, handback and isolated cookie state.
  const isolation = await app.evaluate(async (_electron, { bot, other, origin }) => {
    const timed = <T>(promise: Promise<T>, label: string): Promise<T> => Promise.race([promise, new Promise<never>((_resolve, reject) => setTimeout(() => reject(new Error(`Timed out: ${label}`)), 5000))]);
    const { engine, owner } = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    engine.setOccluded(true);
    const hidden = owner.contentView.children.every((view) => !view.getVisible());
    engine.setOccluded(false);
    await timed(engine.mount(bot, { visible: false, bounds: { x: 240, y: 80, width: 650, height: 600 } }), "handback after popups");
    const control = engine.state(bot.id).control;
    const cookie = await timed(engine.withAutomation(other, {}, async (contents) => { await contents.loadURL(origin); return contents.executeJavaScript("document.cookie"); }), "second bot isolated page");
    const sessionCookie = await timed(engine.withAutomation(bot, { pageId: engine.state(bot.id).tabs[0]!.id }, (contents) => contents.executeJavaScript("document.cookie")), "first bot retained cookie");
    await timed(engine.closeBot(other.id), "close second bot");
    return { hidden, control, cookie, sessionCookie };
  }, { bot, other, origin });
  expect(isolation).toEqual({ hidden: true, control: "bot", cookie: "", sessionCookie: "native_session=disposable" });
  await app.evaluate(async () => { await (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.closeAll(); });
  await app.close(); electron = undefined;
  electron = await launch();
  const restored = await electron.evaluate(async (_electron, { bot, origin }) => (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.withAutomation(bot, {}, async (contents) => { await contents.loadURL(origin); return contents.executeJavaScript("document.cookie"); }), { bot, origin });
  expect(restored).toBe("native_session=disposable");
  await electron.evaluate(async () => { await (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.closeAll(); });
});

it("allows nested web frames and deletes authentication while pruning only orphan profiles", async () => {
  await electron?.close(); electron = await launch();
  const app = electron;
  const deleted: NativeBrowserBot = { id: "delete-profile", homeFolder: join(root, "delete-profile-home") };
  const retained: NativeBrowserBot = { id: "retained-profile", homeFolder: join(root, "retained-profile-home") };
  const live: NativeBrowserBot = { id: "live-profile", homeFolder: join(root, "live-profile-home") };
  const orphan: NativeBrowserBot = { id: "orphan-profile", homeFolder: join(root, "orphan-profile-home") };
  for (const bot of [deleted, retained, live, orphan]) mkdirSync(bot.homeFolder);
  const seeded = await app.evaluate(async (_electron, { deleted, retained, live, orphan, origin }) => {
    const { engine } = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    const paths: Record<string, string> = {};
    for (const bot of [deleted, retained, live, orphan]) {
      await engine.withAutomation(bot, {}, async (contents) => {
        await contents.loadURL(origin + "/cookie");
        await contents.executeJavaScript("localStorage.setItem('synthetic-secret','present')");
        paths[bot.id] = contents.session.storagePath!;
      });
    }
    await engine.closeBot(retained.id); await engine.closeBot(orphan.id);
    await engine.withAutomation(deleted, {}, async (contents) => { await contents.loadURL(origin + "/frames"); });
    return paths;
  }, { deleted, retained, live, orphan, origin });
  await expect.poll(() => app.evaluate(async ({ webContents }, origin) => webContents.getAllWebContents().find((item) => item.getURL() === origin + "/frames")!.executeJavaScript("frameReady"), origin)).toEqual(expect.arrayContaining(["srcdoc", "nested-data", "blob"]));
  await app.evaluate(async (_electron, { deleted, retained, origin }) => {
    const { engine } = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    const deletedSession = await engine.withAutomation(deleted, {}, async (contents) => contents.session);
    await engine.execute(deleted, { kind: "delete_profile" });
    if ((await deletedSession.cookies.get({})).length) throw new Error("Deleted profile still retained cookies before startup pruning");
    await engine.pruneProfiles([retained.id]);
    // deleted was closed; live is deliberately absent from valid IDs but must remain untouched.
    if (engine.state("live-profile").tabs.length !== 1) throw new Error("Prune closed a live browser");
    const retainedCookie = await engine.withAutomation(retained, {}, async (contents) => { await contents.loadURL(origin); return contents.executeJavaScript("document.cookie"); });
    if (retainedCookie !== "native_session=disposable") throw new Error("Normal close or prune erased a retained bot");
    await engine.closeAll();
  }, { deleted, retained, origin });
  await app.close(); electron = undefined;
  // The next process can remove files that Chromium held open in the previous process.
  electron = await launch();
  await electron.evaluate(async (_electron, retained) => (globalThis as unknown as FixtureGlobal).nativeBrowserFixture.engine.pruneProfiles([retained.id]), retained);
  expect(existsSync(seeded[deleted.id]!)).toBe(false);
  expect(existsSync(seeded[orphan.id]!)).toBe(false);
  expect(existsSync(seeded[live.id]!)).toBe(false);
  expect(existsSync(seeded[retained.id]!)).toBe(true);
  const storage = await electron.evaluate(async (_electron, { deleted, retained, origin }) => {
    const { engine } = (globalThis as unknown as FixtureGlobal).nativeBrowserFixture;
    const inspect = (bot: NativeBrowserBot) => engine.withAutomation(bot, {}, async (contents) => {
      await contents.loadURL(origin);
      return contents.executeJavaScript("({cookie:document.cookie,secret:localStorage.getItem('synthetic-secret')})") as Promise<{ cookie: string; secret: string | null }>;
    });
    const deletedState = await inspect(deleted), retainedState = await inspect(retained);
    await engine.closeAll();
    return { deletedState, retainedState };
  }, { deleted, retained, origin });
  expect(storage.deletedState).toEqual({ cookie: "", secret: null });
  expect(storage.retainedState).toEqual({ cookie: "native_session=disposable", secret: "present" });
});
