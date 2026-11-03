import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { join } from "node:path";
import { app, BrowserWindow, nativeImage } from "electron";
import { BrowserState } from "../../../packages/protocol/src/browser.ts";
import type { NativeBrowserRemoteAction } from "../../../packages/protocol/src/native-browser.ts";
import { NativeBrowserEngine } from "../../../packages/desktop/src/native-browser.ts";
import { nativeBrowserAutomation } from "../../../packages/desktop/src/native-browser-automation.ts";

const configuredRoot = process.env.TJOMMIE_PHASE4_COMPUTER_ROOT;
if (!configuredRoot) throw new Error("The probe needs a disposable root.");
const root: string = configuredRoot;
app.setPath("userData", join(root, "electron"));
const server = createServer((_request, response) => response.writeHead(200, { "content-type": "text/html" }).end(
  '<!doctype html><title>Synthetic browser</title><body style="margin:0;background:rgb(200,30,40)"><input aria-label="Synthetic input" style="position:absolute;left:20px;top:20px;width:220px;height:30px"><p style="margin-top:80px">Disposable browser fixture</p></body>',
));
let owner: BrowserWindow | undefined;
let engine: NativeBrowserEngine | undefined;
async function run(): Promise<void> {
  try {
    await app.whenReady();
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const address = server.address(); assert(address && typeof address !== "string");
    owner = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false } });
    const states: unknown[] = [], notices: unknown[] = [];
    const instance = new NativeBrowserEngine({ window: owner, profileRoot: join(root, "profiles"), automation: nativeBrowserAutomation, onState: (state) => states.push(state), onNotice: (notice) => notices.push(notice) });
    engine = instance;
    const bot = { id: "p4-synthetic", name: "Synthetic", homeFolder: join(root, "bot") };
    await instance.open(bot); await instance.resize(bot, 640, 480);
    const page = await instance.withAutomation(bot, {}, async (contents) => {
      await contents.loadURL("http://127.0.0.1:" + address.port);
      assert.equal(contents.session.storagePath?.startsWith(root), true);
      return contents;
    });
    const started = performance.now(), picture = await instance.frame(bot), captureMs = Math.round(performance.now() - started);
    assert(picture.frame);
    const bytes = Buffer.from(picture.frame.data, "base64"), pixels = nativeImage.createFromBuffer(bytes), size = pixels.getSize();
    const actualViewport: unknown = await page.executeJavaScript("({width:innerWidth,height:innerHeight})");
    assert(bytes.length > 100 && bytes.length <= 2 * 1024 * 1024);
    const bitmap = pixels.toBitmap(), offset = (200 * size.width + 500) * 4;
    assert(bitmap[offset + 2]! > 150 && bitmap[offset + 1]! < 80, "The source frame must contain the red fixture.");
    assert.equal(picture.state.busy, false, "A viewer frame must not make the remote UI busy.");
    const thumbnail = await instance.frame(bot, "local", undefined, true);
    assert(thumbnail.frame && thumbnail.frame.width <= 320 && thumbnail.frame.height <= 240 && Buffer.byteLength(thumbnail.frame.data, "base64") <= 64 * 1024);
    assert.deepEqual(actualViewport, { width: picture.frame.viewportWidth, height: picture.frame.viewportHeight });
    assert(!owner.isVisible() && owner.contentView.children.every((view) => !view.getVisible()));

    const remote = (controllerId: string, action: NativeBrowserRemoteAction): Promise<unknown> => instance.execute(bot, { kind: "remote", controllerId, action });
    const first = "rpc:synthetic-first", second = "rpc:synthetic-second", third = "rpc:synthetic-third";
    let lease = BrowserState.parse(await remote(first, { kind: "control", control: "human", generation: picture.state.generation }));
    assert(lease.canControl && lease.controlEpoch !== undefined && lease.viewportRevision !== undefined && lease.activePageId);
    const target = { generation: lease.generation, pageId: lease.activePageId, controlEpoch: lease.controlEpoch, viewportRevision: lease.viewportRevision };
    await assert.rejects(remote(second, { kind: "input", ...target, input: { kind: "text", text: "wrong controller" } }), /control changed/);
    await assert.rejects(remote(first, { kind: "input", ...target, controlEpoch: target.controlEpoch - 1, input: { kind: "text", text: "stale input" } }), /control changed/);
    await assert.rejects(instance.withAutomation(bot, {}, async () => { throw new Error("Must not enter a bot callback during takeover."); }), /person|control/i);
    await page.executeJavaScript("window.releases=[];addEventListener('keyup',e=>releases.push(e.key));addEventListener('mouseup',()=>releases.push('mouse'))");
    await remote(first, { kind: "input", ...target, input: { kind: "mouse", action: "down", x: 60, y: 35, button: "left" } });
    await remote(first, { kind: "input", ...target, input: { kind: "mouse", action: "up", x: 60, y: 35, button: "left" } });
    for (const key of ["1", "2"]) {
      await remote(first, { kind: "input", ...target, input: { kind: "key", action: "down", key } });
      await remote(first, { kind: "input", ...target, input: { kind: "key", action: "up", key } });
    }
    assert.equal(await page.executeJavaScript("document.querySelector('input').value"), "12", "Remote key events must type characters.");
    for (const [key, action] of [["Control", "down"], ["a", "down"], ["a", "up"], ["Control", "up"]] as const) await remote(first, { kind: "input", ...target, input: { kind: "key", key, action } });
    await remote(first, { kind: "input", ...target, input: { kind: "text", text: "246810" } });
    assert.equal(await page.executeJavaScript("document.querySelector('input').value"), "246810", "Hidden native remote input must reach the focused field.");
    await remote(first, { kind: "input", ...target, input: { kind: "key", action: "down", key: "Shift" } });
    await remote(first, { kind: "input", ...target, input: { kind: "mouse", action: "down", x: 60, y: 35, button: "left" } });
    await remote(first, { kind: "disconnect" });
    await assert.rejects(remote(first, { kind: "control", control: "human" }), /disconnected/);
    const releases: unknown = await page.executeJavaScript("window.releases");
    assert(Array.isArray(releases) && releases.includes("Shift") && releases.includes("mouse"), "Disconnect must release held keys and buttons.");
    await instance.withAutomation(bot, {}, async () => {});

    lease = BrowserState.parse(await remote(second, { kind: "control", control: "human" }));
    assert(lease.controlEpoch !== undefined);
    const privateEpoch = await instance.beginPrivate(bot, second, lease.controlEpoch);
    await instance.mount(bot, { visible: false, bounds: { x: 0, y: 44, width: 640, height: 480 } });
    const syntheticSecret = "P4-synthetic-" + randomUUID();
    await page.executeJavaScript("document.querySelector('input').value=" + JSON.stringify(syntheticSecret) + ";document.title=" + JSON.stringify(syntheticSecret) + ";console.log(" + JSON.stringify(syntheticSecret) + ")");
    const privateFrame = await instance.frame(bot);
    assert.equal(privateFrame.frame, null); assert(privateFrame.state.privateActive);
    const privateTool = await nativeBrowserAutomation(instance, bot, { kind: "tool", name: "browser_evaluate", args: { function: "() => document.querySelector('input').value" } });
    assert(privateTool.isError);
    await remote(second, { kind: "disconnect" });
    assert(instance.state(bot.id).privateActive && instance.state(bot.id).control === "human", "Disconnect/hiding must not resume a private browser.");
    await remote(third, { kind: "control", control: "human" });
    await page.executeJavaScript("document.querySelector('input').value='';document.title='Synthetic browser'");
    const ended = await instance.endPrivate(bot, third, privateEpoch, async () => { assert.equal(await page.executeJavaScript("document.querySelector('input').value"), ""); });
    await remote(third, { kind: "control", control: "bot", generation: ended.generation, controlEpoch: ended.controlEpoch });
    let resumed = false; await instance.withAutomation(bot, {}, async () => { resumed = true; });
    assert(resumed);
    assert(!JSON.stringify({ states, notices, privateFrame, privateTool }).includes(syntheticSecret), "Synthetic secret must be absent from the checked app output sinks.");
    assert(!owner.isVisible() && owner.contentView.children.every((view) => !view.getVisible()));
    process.stdout.write("PHASE4_COMPUTER_RESULT " + JSON.stringify({ electron: process.versions.electron, captureBytes: bytes.length, width: size.width, height: size.height, viewportWidth: picture.frame.viewportWidth, viewportHeight: picture.frame.viewportHeight, actualViewport, captureMs, hiddenOwner: true, fixtureForcesChildVisible: false, controllerAndStaleInput: true, syntheticPin: true, disconnectHeldRelease: true, privateHidingDisconnect: true, checkedOutputSinks: ["state", "notice", "private-tool", "private-frame"], resumed, remoteTransportTested: false, credentialStoreUsed: false, vaultFillTested: false }) + "\n");
  } catch (error) {
    process.exitCode = 1;
    process.stderr.write((error instanceof Error ? error.stack : String(error)) + "\n");
  } finally {
    await engine?.closeAll(); owner?.destroy();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    app.exit(Number(process.exitCode ?? 0));
  }
}
// Electron must finish evaluating the entry module before app.whenReady can resolve.
void run();
