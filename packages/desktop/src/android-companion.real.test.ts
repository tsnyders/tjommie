import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Server } from "node:net";
import { chromium, type Browser, type Page } from "playwright-core";
import { expect, it } from "vitest";
import { methods, TEAM_ROOM_ID, type MethodName } from "@tjommie/protocol";
import { addMessage } from "../../daemon/src/chat.ts";
import { createApproval } from "../../daemon/src/approvals.ts";
import { newId, openDb, type Db } from "../../daemon/src/db.ts";
import { initializeHostNoise, RemoteHost } from "../../daemon/src/remote-host.ts";
import { loadRemoteIdentity } from "../../daemon/src/remote-identity.ts";
import { generateKeypair } from "../../transport/src/index.ts";
import { recordUsage } from "../../daemon/src/usage.ts";
import { windowsCredentialTransport } from "../../daemon/src/account-credentials.ts";
import { closeRealApp, eventually, launchRealApp, type RealApp } from "./real-app.ts";

const serial = process.env.TJOMMIE_ANDROID_SERIAL;
const cdp = process.env.TJOMMIE_ANDROID_CDP_PORT;
// Some handset IMEs suppress injected touches. Keep the strict automatic gate
// by default; an explicitly documented manual IME check can run the other gates.
const manualIme = process.env.TJOMMIE_ANDROID_MANUAL_IME === "1";
const shots = resolve("docs/screenshots/phone");
function adb(args: string[]): Buffer { return execFileSync("adb", ["-s", serial!, ...args], { windowsHide: true, maxBuffer: 16 * 1024 * 1024 }); }
async function shot(page: Page, name: string): Promise<void> {
  // Evidence captures exclude the system keyboard and its private suggestion strip.
  await page.evaluate("document.activeElement instanceof HTMLElement && document.activeElement.blur()");
  // Escape with no IME can become Android Back and race the next navigation.
  // Only dismiss an actual keyboard, then wait for the native inset to settle.
  if (await page.evaluate("window.tjommieInsets?.keyboardVisible===true")) {
    adb(["shell", "input", "keyevent", "111"]);
    await eventually(async () => await page.evaluate("window.tjommieInsets?.keyboardVisible!==true") || undefined, 5000);
  }
  expect(await page.evaluate("document.documentElement.scrollWidth <= innerWidth")).toBe(true);
  const activity = adb(["shell", "dumpsys", "activity", "activities"]).toString();
  const policy = adb(["shell", "dumpsys", "window", "policy"]).toString();
  if (!activity.split("\n").some((line) => line.includes("topResumedActivity") && line.includes("dev.tjommie.app/.MainActivity")) || !policy.includes("showing=false")) {
    throw new Error("The test will not capture another app or the phone's lock screen.");
  }
  writeFileSync(join(shots, name), adb(["exec-out", "screencap", "-p"]));
}

// Real Android touch input, rather than assigning scrollTop or Playwright's auto-scroll.
async function swipe(page: Page, selector: string, direction: "up" | "down") {
  const element = page.locator(selector);
  const box = await element.boundingBox();
  if (!box) throw new Error("The mobile scroll area is not visible.");
  let before = await element.evaluate((node) => node.scrollTop);
  const maximum = await element.evaluate((node) => node.scrollHeight - node.clientHeight);
  if (maximum <= 25) throw new Error("The touch-scroll fixture must overflow its viewport.");
  // Focusing a field or opening details may already put the scroll area at its edge.
  // Move away by touch first, so the requested gesture has somewhere to go.
  if (direction === "up" && before >= maximum - 20) { await swipe(page, selector, "down"); before = await element.evaluate((node) => node.scrollTop); }
  if (direction === "down" && before <= 20) { await swipe(page, selector, "up"); before = await element.evaluate((node) => node.scrollTop); }
  const width = await page.evaluate<number>("innerWidth");
  const dimensions = [...adb(["shell", "wm", "size"]).toString().matchAll(/(\d+)x(\d+)/g)].at(-1);
  if (!dimensions) throw new Error("Cannot measure the phone display.");
  const scale = Number(dimensions[1]) / width;
  const x = Math.round((box.x + box.width / 2) * scale);
  const top = Math.round((box.y + box.height * .25) * scale);
  const bottom = Math.round((box.y + box.height * .75) * scale);
  adb(["shell", "input", "swipe", String(x), String(direction === "up" ? bottom : top), String(x), String(direction === "up" ? top : bottom), "450"]);
  await eventually(async () => { const after = await element.evaluate((node) => node.scrollTop); return (direction === "up" ? after > before + 25 : after < before - 25) || undefined; }, 5000);
}

async function mobileFits(page: Page) {
  expect(await page.locator('.android-app').count()).toBe(1);
  expect(await page.evaluate("document.documentElement.scrollWidth<=innerWidth&&[...document.querySelectorAll('#bot-name,#bot-instructions,[data-testid=bot-form-scroll] [role=radiogroup],footer button')].filter(element=>element.getClientRects().length).every(element=>{const rect=element.getBoundingClientRect();return rect.left>=-1&&rect.right<=innerWidth+1})")).toBe(true);
}

async function selectText(page: Page, text: string, screenshot: string): Promise<void> {
  const target = page.getByText(text, { exact: true });
  await target.waitFor();
  const bounds = await target.boundingBox();
  if (!bounds) throw new Error("Selection fixture has no bounds.");
  const point = { x: bounds.x + Math.min(36, bounds.width / 2), y: bounds.y + Math.min(10, bounds.height / 2), width: await page.evaluate<number>("innerWidth") };
  const screen = [...adb(["shell", "wm", "size"]).toString().matchAll(/(\d+)x(\d+)/g)].at(-1);
  if (!screen) throw new Error("Cannot measure the phone display.");
  const scale = Number(screen[1]) / point.width;
  const x = String(Math.round(point.x * scale)), y = String(Math.round(point.y * scale));
  adb(["shell", "input", "swipe", x, y, x, y, "850"]);
  await eventually(async () => await page.evaluate("(getSelection()?.toString().length??0)>0") || undefined, 5000);
  // The accessibility dump waits for the native floating toolbar to settle.
  adb(["exec-out", "uiautomator", "dump", "/dev/tty"]);
  await shot(page, screenshot);
  const route = page.url();
  adb(["shell", "input", "keyevent", "4"]);
  await eventually(async () => await page.evaluate("(getSelection()?.toString().length??0)===0") || undefined, 5000);
  expect(page.url()).toBe(route);
}

// Opt in explicitly: this installs/opens no app itself. The owner authorizes a specific Android device,
// and root installs this build and forwards only its own WebView before running the test.
it.skipIf(!serial || !cdp)("keeps the original phone UI usable with real swipe scrolling, bot creation, chat, approvals and reconnect", async () => {
  let app: RealApp | undefined, browser: Browser | undefined, phone: Page | undefined;
  let remote: RemoteHost | undefined, remoteDb: Db | undefined, off: (() => void) | undefined;
  let code = "", deviceId: string | undefined, hostName: string | undefined;
  const errors: string[] = [];
  let acceptedConnections = 0;
  mkdirSync(shots, { recursive: true });
  try {
    app = await launchRealApp(undefined, { isolatedAccounts: true });
    writeFileSync(resolve(".cache/android-test-root.txt"), app.root);
    const bot = await app.client.call("bots.create", { name: "S25 test agent", provider: "claude", model: "sonnet", color: "blue", shape: "circle", instructions: "Disposable Android UI fixture. Never run a provider turn.", autonomy: "ask", browser: false });
    await app.client.call("bots.pause", { id: bot.id, paused: true });
    for (let index = 1; index <= 12; index++) {
      const agent = await app.client.call("bots.create", { name: `Mention fixture ${index}`, provider: "claude", model: "sonnet", color: "gray", shape: "circle", instructions: "Synthetic paused mention-list fixture.", autonomy: "ask", browser: false });
      await app.client.call("bots.pause", { id: agent.id, paused: true });
    }
    const accountId = bot.accountId;
    if (!accountId) throw new Error("The disposable bot has no pinned account.");
    const db = openDb(join(app.root, "data", "tjommie.db"));
    const approvalId = newId();
    try {
      addMessage(db, { conversationId: TEAM_ROOM_ID, author: bot.id, kind: "text", body: "Selection proof in the Team room. The toolbar must keep its own surface without blanking the conversation.", status: "done" });
      for (let index = 0; index < 35; index++) addMessage(db, { conversationId: bot.id, author: bot.id, kind: "text", body: `Scroll proof ${index + 1}. This is a deliberately long mobile conversation, tested with real touch input.`, status: "done" });
      addMessage(db, { conversationId: bot.id, author: bot.id, kind: "text", body: "Synthetic phone connection test. No provider was called.", status: "done" });
      const message = addMessage(db, { conversationId: bot.id, author: "system", kind: "approval", body: "Allow this synthetic read-only request?", data: { approvalId, tool: "Read", detail: "fixture-only.txt", rule: null, status: "pending" }, status: "done" });
      createApproval(db, { id: approvalId, botId: bot.id, tool: "Read", summary: message.body, detail: "fixture-only.txt", rule: null, messageId: message.id });
      recordUsage(db, { botId: bot.id, accountId, provider: "claude", model: "sonnet", input: 100, cachedInput: 10, output: 20, reasoning: 5, costUsd: 2.5, costSource: "reported" });
    } finally { db.close(); }
    // The owner's installed host may already hold 47101. Use an isolated ephemeral
    // listener and the installed fixture's real typed RPC; never stop the owner's app.
    remoteDb = openDb(join(app.root, "data", "tjommie.db"));
    const vault = new Map<string, string>();
    const client = app.client;
    const credentials = async (input: Parameters<typeof windowsCredentialTransport>[0]) => { if (input.operation === "write" && input.key) vault.set(input.target, input.key); return input.operation === "read" ? vault.get(input.target) ?? null : null; };
    initializeHostNoise();
    await loadRemoteIdentity({ db: remoteDb, root: app.root, name: `Mobile UI test PC ${newId().slice(0, 8)}`, credentials, generate: generateKeypair });
    remote = new RemoteHost({ db: remoteDb, root: app.root, port: 0,
      credentials,
      dispatch: async (method, params) => { if (!(method in methods)) throw new Error("Unknown fixture method"); const name = method as MethodName; return client.call(name, methods[name].params.parse(params)); },
    });
    off = client.subscribe((event) => remote?.broadcast(event.event, event.data));
    code = (await remote.createPairing()).code;
    const handles = (process as unknown as { _getActiveHandles(): unknown[] })._getActiveHandles();
    const listener = handles.find((handle): handle is Server => handle instanceof Server && typeof handle.address() === "object" && handle.address() !== null && (handle.address() as { port: number }).port === remote!.status().port);
    listener?.on("connection", () => { acceptedConnections++; });

    browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdp}`);
    phone = browser.contexts().flatMap((context) => context.pages()).find((page) => page.url().startsWith("https://localhost"));
    if (!phone) throw new Error("The Tjommie Android WebView is not open.");
    await phone.addInitScript("window.addEventListener('DOMContentLoaded',()=>{const c=window.Capacitor;if(!c?.nativePromise)return;const original=c.nativePromise;c.nativePromise=function(plugin,method,options){return original.call(c,plugin,method,options).catch(error=>{if(plugin==='TjommieSocket'){window.__socketTestErrors??=[];if(window.__socketTestErrors.length<32)window.__socketTestErrors.push({method,code:error?.code??'unknown'});}throw error;});};});");
    phone.on("pageerror", (error) => errors.push(error.message));
    // Preserve the owner's saved pairing, if present. Switch connections without forgetting it.
    await phone.goto("https://localhost/settings/?section=devices");
    const switchPc = phone.getByRole("button", { name: "Switch PC", exact: true });
    // The pairing form also appears during automatic reconnect. Wait for the saved
    // PC to finish opening before deciding whether to switch away from it.
    const connected = await switchPc.waitFor({ timeout: 20_000 }).then(() => true, () => false);
    if (connected) await switchPc.click();
    else await phone.getByRole("heading", { name: "Connecting to your PC", exact: true }).waitFor({ state: "hidden", timeout: 45_000 });
    await phone.getByLabel("Pairing code", { exact: true }).fill(code);
    await phone.getByLabel("Name for this phone", { exact: true }).fill("S25 Ultra test device");
    await phone.getByRole("button", { name: "Connect", exact: true }).click();
    await phone.getByRole("heading", { name: "Devices", exact: true }).waitFor({ timeout: 45_000 });
    await phone.goto("https://localhost/agents/");
    await phone.getByRole("heading", { name: "Agents", exact: true }).waitFor();
    const devices = await app.client.call("devices.list", {});
    deviceId = devices.find((device) => device.name === "S25 Ultra test device")?.id;
    hostName = remote.status().hostName;
    expect(deviceId).toBeTruthy();
    await shot(phone, "android-agents.png");

    // Real settings controls, persisted style/theme, and native system-bar
    // appearance on the physical phone, using only this disposable connection.
    for (const style of ["Hush", "Folio", "Aurora"]) {
      await phone.goto("https://localhost/settings/?section=general");
      await phone.getByRole("radiogroup", { name: "Mobile style", exact: true }).getByRole("radio", { name: style, exact: true }).click();
      for (const theme of ["Dark", "Light"]) {
        await phone.getByRole("radiogroup", { name: "Appearance", exact: true }).getByRole("radio", { name: theme, exact: true }).click();
        expect(await phone.locator("html").getAttribute("data-mobile-style")).toBe(style.toLowerCase());
        await phone.reload();
        await phone.getByRole("radiogroup", { name: "Mobile style", exact: true }).waitFor();
        expect(await phone.getByRole("radio", { name: style, exact: true }).getAttribute("aria-checked")).toBe("true");
        await shot(phone, `android-style-${style.toLowerCase()}-${theme.toLowerCase()}.png`);
      }
    }
    await phone.getByRole("radio", { name: "Hush", exact: true }).click();
    await phone.getByRole("radiogroup", { name: "Appearance", exact: true }).getByRole("radio", { name: "Dark", exact: true }).click();
    await phone.goto("https://localhost/agents/");
    await phone.getByRole("heading", { name: "Agents", exact: true }).waitFor();

    // The original form and visual design stay. Actual touch must reach its lower fields.
    await phone.getByRole("link", { name: "New bot", exact: true }).click();
    await phone.getByLabel("Name", { exact: true }).fill("Mobile form proof");
    await phone.getByLabel("What should this bot do?", { exact: true }).fill("Synthetic mobile creation proof. No provider calls.");
    await swipe(phone, '[data-testid="bot-form-scroll"]', "down");
    await mobileFits(phone);
    await shot(phone, "android-new-bot-top.png");
    await swipe(phone, '[data-testid="bot-form-scroll"]', "up");
    await mobileFits(phone);
    await shot(phone, "android-new-bot-bottom.png");
    const createBounds = await phone.getByRole("button", { name: "Create bot", exact: true }).boundingBox();
    expect(createBounds).not.toBeNull();
    expect(createBounds!.y + createBounds!.height).toBeLessThanOrEqual(await phone.evaluate<number>("visualViewport.height") + 1);
    await phone.getByRole("button", { name: "Create bot", exact: true }).click();
    await eventually(async () => (await app!.client.call("bots.list", {})).some((item) => item.name === "Mobile form proof") || undefined);
    await phone.getByRole("link", { name: "Back to Agents", exact: true }).click();
    // The Rooms tab opens the rooms list; the seeded post is in Everyone.
    await phone.getByRole("navigation", { name: "Tjommie", exact: true }).getByRole("link", { name: "Rooms", exact: true }).click();
    await phone.getByRole("heading", { name: "Rooms", exact: true }).waitFor();
    await mobileFits(phone);
    await shot(phone, "android-rooms.png");
    await phone.getByRole("link", { name: "Everyone", exact: true }).click();
    await selectText(phone, "Selection proof in the Team room. The toolbar must keep its own surface without blanking the conversation.", "android-selection-room.png");
    const roomInput = phone.getByRole("combobox", { name: "Message Everyone", exact: true });
    // A CDP click may focus without asking Android to show its IME. Use the
    // handset's input path for the keyboard acceptance check.
    const inputBounds = await roomInput.boundingBox();
    if (!inputBounds) throw new Error("Everyone's input is not visible.");
    const screen = [...adb(["shell", "wm", "size"]).toString().matchAll(/(\d+)x(\d+)/g)].at(-1);
    if (!screen) throw new Error("Cannot measure the phone display.");
    const inputScale = Number(screen[1]) / await phone.evaluate<number>("innerWidth");
    adb(["shell", "input", "tap", String(Math.round((inputBounds.x + inputBounds.width / 2) * inputScale)), String(Math.round((inputBounds.y + inputBounds.height / 2) * inputScale))]);
    if (!manualIme) {
      await eventually(async () => await phone!.evaluate("window.tjommieInsets?.keyboardVisible===true") || undefined, 8000);
      await eventually(async () => await phone!.getByTestId("phone-nav").getAttribute("aria-hidden") === "true" || undefined, 8000);
    }
    await roomInput.fill(`${Array.from({ length: 15 }, (_, index) => `Long draft line ${index + 1}`).join("\n")} @`);
    await phone.getByRole("listbox", { name: "Mention a bot", exact: true }).waitFor();
    const mentionBox = await phone.getByRole("listbox", { name: "Mention a bot", exact: true }).boundingBox();
    const roomSend = await phone.getByRole("button", { name: "Send", exact: true }).boundingBox();
    expect(mentionBox).not.toBeNull();
    expect(roomSend).not.toBeNull();
    expect(roomSend!.y + roomSend!.height).toBeLessThanOrEqual(await phone.evaluate<number>("visualViewport.height") + 1);
    await swipe(phone, '#mention-list', "up");
    adb(["shell", "input", "keyevent", "4"]);
    try { await phone.getByRole("listbox", { name: "Mention a bot", exact: true }).waitFor({ state: "hidden", timeout: 1500 }); }
    catch { adb(["shell", "input", "keyevent", "4"]); }
    await phone.getByRole("listbox", { name: "Mention a bot", exact: true }).waitFor({ state: "hidden" });
    expect(phone.url()).toContain("/room/?id=team");
    await roomInput.fill("");
    await phone.getByRole("navigation", { name: "Tjommie", exact: true }).getByRole("link", { name: "Agents", exact: true }).click();

    await phone.getByRole("link", { name: bot.name, exact: true }).click();
    await phone.getByText("Synthetic phone connection test. No provider was called.", { exact: true }).waitFor();
    await selectText(phone, "Synthetic phone connection test. No provider was called.", "android-selection-chat.png");
    await swipe(phone, '[data-testid="message-scroll"]', "down");
    await mobileFits(phone);
    // Back to the latest request by real touch; no programmatic scroll assertion.
    await swipe(phone, '[data-testid="message-scroll"]', "up");
    await phone.getByRole("button", { name: "Allow once", exact: true }).click();
    await phone.getByText(/^Allowed once:/).waitFor();
    const composer = phone.getByRole("textbox", { name: `Message ${bot.name}`, exact: true });
    await composer.click();
    await composer.fill("Synthetic message from the S25. Keep this bot paused.");
    const bounds = await phone.getByRole("button", { name: "Send", exact: true }).boundingBox();
    const viewportHeight = await phone.evaluate("visualViewport.height") as number;
    expect(bounds).not.toBeNull();
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewportHeight + 1);
    await phone.getByRole("button", { name: "Send", exact: true }).click();
    await eventually(async () => (await app!.client.call("chat.history", { botId: bot.id })).messages.some((message) => message.body === "Synthetic message from the S25. Keep this bot paused.") || undefined);
    await phone.getByRole("button", { name: /Show details/ }).click();
    await phone.getByRole("button", { name: "Close details", exact: true }).waitFor();
    expect(await phone.evaluate("getComputedStyle(document.querySelector('aside[aria-label^=About]')).animationName")).toBe(await phone.evaluate("matchMedia('(prefers-reduced-motion: reduce)').matches") ? "none" : "phone-sheet-in");
    adb(["shell", "input", "keyevent", "4"]);
    // Android may consume the first Back to hide an IME that was still closing after Send.
    try { await phone.getByRole("button", { name: "Close details", exact: true }).waitFor({ state: "hidden", timeout: 1500 }); }
    catch { adb(["shell", "input", "keyevent", "4"]); }
    await phone.getByRole("button", { name: "Close details", exact: true }).waitFor({ state: "hidden" });
    expect(phone.url()).toContain(`/chat/?bot=${bot.id}`);
    await phone.getByRole("link", { name: "Back to Agents", exact: true }).click();
    await phone.getByRole("navigation", { name: "Tjommie", exact: true }).getByRole("link", { name: "Settings", exact: true }).click();
    await phone.getByRole("link", { name: /^Usage\b/ }).click();
    await phone.getByText("Combined cost estimate", { exact: true }).waitFor();
    await phone.getByRole("group", { name: "Range", exact: true }).getByRole("button", { name: "This year", exact: true }).click();
    await phone.getByText("$2.50", { exact: true }).first().waitFor();
    await shot(phone, "android-usage.png");
    await swipe(phone, '[data-testid="settings-scroll"]', "up");

    await phone.reload();
    await phone.getByText("Combined cost estimate", { exact: true }).waitFor({ timeout: 30_000 });
    expect((await app.client.call("devices.list", {})).filter((device) => !device.revokedAt)).toHaveLength(1);
    await app.client.call("devices.revoke", { id: deviceId! });
    await phone.getByRole("button", { name: `Forget ${hostName}`, exact: true }).waitFor({ timeout: 30_000 });
    await shot(phone, "android-revoked.png");
    await phone.getByRole("button", { name: `Forget ${hostName}`, exact: true }).click();
    expect(errors).toEqual([]);
  } catch (error) {
    if (phone) {
      writeFileSync(resolve(".cache/android-test-error.txt"), JSON.stringify({ acceptedConnections, page: await phone.evaluate("({path:location.pathname,search:location.search,width:innerWidth,height:innerHeight,socketErrors:window.__socketTestErrors??[],mobile:!!document.querySelector('.android-app'),scrolls:[...document.querySelectorAll('[data-testid$=scroll]')].map(el=>({kind:el.getAttribute('data-testid'),height:el.clientHeight,content:el.scrollHeight,top:el.scrollTop}))})").catch(() => ({ unavailable: true })) }));
    }
    throw new Error((error instanceof Error ? error.message : "Android test failed.").replaceAll(code || "no-code", "[synthetic pairing code]"));
  } finally {
    if (deviceId) await app?.client.call("devices.revoke", { id: deviceId }).catch(() => {});
    if (deviceId && phone && hostName) {
      await phone.getByRole("button", { name: `Forget ${hostName}`, exact: true }).waitFor({ timeout: 20_000 }).catch(() => {});
      await phone.getByRole("button", { name: `Forget ${hostName}`, exact: true }).click({ timeout: 5000 }).catch(() => {});
    }
    await browser?.close();
    off?.();
    await remote?.close();
    remoteDb?.close();
    if (app) {
      const namespace = createHash("sha256").update(resolve(app.root).toLowerCase()).digest("hex").slice(0, 32);
      await closeRealApp(app, false);
      await windowsCredentialTransport({ operation: "delete", target: `Tjommie/${namespace}/remote-host` });
    }
  }
}, 240_000);
