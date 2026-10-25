import { createServer, type Server } from "node:http";
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { expect, it } from "vitest";
import { addMessage } from "../../daemon/src/chat.ts";
import { newId, nowIso, openDb } from "../../daemon/src/db.ts";
import { recordUsage } from "../../daemon/src/usage.ts";
import { replaceAccountLimits } from "../../daemon/src/accounts-store.ts";
import { closeRealApp, eventually, launchRealApp, type RealApp } from "./real-app.ts";

const screenshots = resolve(import.meta.dirname, "../../../docs/screenshots/phone");

async function visit(app: RealApp, path: string): Promise<void> {
  await app.page.goto(new URL(path, app.page.url()).href);
}

async function resize(app: RealApp, width: number, height: number): Promise<void> {
  await app.electronApp.evaluate(({ BrowserWindow }, size) => {
    const window = BrowserWindow.getAllWindows()[0];
    if (window.isMinimized()) window.restore();
    window.show();
    window.setContentSize(size.width, size.height);
  }, { width, height });
  await app.page.bringToFront();
  await eventually(async () => await app.page.evaluate("innerWidth") === width || undefined);
}

async function fits(app: RealApp): Promise<void> {
  const okay = await app.page.evaluate("document.documentElement.scrollWidth <= innerWidth");
  if (!okay) mkdirSync(resolve(".cache"), { recursive: true });
  if (!okay) writeFileSync(resolve(".cache/phone-overflow.json"), JSON.stringify(await app.page.evaluate("({width:innerWidth,scroll:document.documentElement.scrollWidth,offenders:[...document.querySelectorAll('body *')].filter(el=>{const r=el.getBoundingClientRect();return r.width&&r.right>innerWidth+1}).slice(-25).map(el=>({tag:el.tagName,class:el.className,left:el.getBoundingClientRect().left,right:el.getBoundingClientRect().right}))})"), null, 2));
  expect(okay).toBe(true);
}

async function capture(app: RealApp, name: string): Promise<void> {
  await app.electronApp.evaluate(({ BrowserWindow }) => {
    const window = BrowserWindow.getAllWindows()[0];
    if (window.isMinimized()) window.restore();
    if (!window.isVisible()) window.show();
  });
  await app.page.bringToFront();
  await app.page.screenshot({ path: join(screenshots, name), animations: "disabled" });
  await fits(app);
}

async function nativeViews(app: RealApp, origin: string) {
  return app.electronApp.evaluate(({ BrowserWindow, WebContentsView }, site) => {
    return BrowserWindow.getAllWindows()[0].contentView.children
      .filter((view) => view instanceof WebContentsView && view.webContents.getURL().startsWith(site))
      .map((view) => ({ visible: view.getVisible(), bounds: view.getBounds() }));
  }, origin);
}

// This drives the real production renderer/preload/main/daemon, with unsigned disposable
// accounts. Messages/usage are seeded directly; nothing sends a provider turn or signs in.
it("uses Quiet pill navigation and reachable phone sheets, forms, account dialogs and native browser", async () => {
  let app: RealApp | undefined;
  let site: Server | undefined;
  const rendererErrors: string[] = [];
  mkdirSync(screenshots, { recursive: true });
  try {
    app = await launchRealApp(undefined, { isolatedAccounts: true });
    const { page, client, electronApp } = app;
    page.on("pageerror", (error) => rendererErrors.push(error.message));
    const bot = await client.call("bots.create", {
      name: "Inbox Triage with a long display name", color: "blue", shape: "circle", provider: "claude", model: "haiku",
      instructions: "Synthetic layout fixture; no real provider work.", browser: true, autonomy: "ask",
    });
    const other = await client.call("bots.create", {
      name: "Research Scout", color: "green", shape: "rounded-square", provider: "claude", model: "haiku",
      instructions: "Synthetic layout fixture; no real provider work.", browser: false, autonomy: "ask",
    });
    const exchangeId = newId();
    for (let index = 1; index <= 12; index += 1) {
      await client.call("accounts.create", { provider: "claude", label: `Layout account ${index} with a long label`, kind: "subscription" });
    }
    const accounts = await client.call("accounts.list", {});
    const db = openDb(join(app.root, "data", "tjommie.db"));
    try {
      addMessage(db, { conversationId: bot.id, author: "you", kind: "text", body: "Please review the sample inbox.", status: "done" });
      addMessage(db, { conversationId: bot.id, author: bot.id, kind: "text", body: "The sample summary is ready. These are fixture messages, with no connection to an inbox.", status: "done" });
      addMessage(db, { conversationId: bot.id, author: "system", kind: "approval", body: "Review the sample document before making a change", data: { approvalId: "phone-layout-only", tool: "Edit", detail: "sample/document.md", rule: null, status: "pending" }, status: "done" });
      addMessage(db, { conversationId: "team", author: other.id, kind: "text", body: "Sample research notes are ready to review.", status: "done" });
      db.prepare("INSERT INTO exchanges (id,from_bot_id,to_bot_id,request,reply,status,chain_id,reply_to,created_at,replied_at) VALUES (?,?,?,?,?,'replied',?,?,?,?)")
        .run(exchangeId, bot.id, other.id, "Review the sample notes", "The sample looks ready.", "phone-layout-chain", bot.id, nowIso(), nowIso());
      db.prepare("INSERT INTO conversations (id,kind,bot_id) VALUES (?,'exchange',NULL)").run(exchangeId);
      addMessage(db, { conversationId: exchangeId, author: other.id, kind: "text", body: "The sample looks ready.", status: "done" });
      addMessage(db, { conversationId: bot.id, author: "system", kind: "handoff", body: "Sample exchange", data: { exchangeId, fromBotId: bot.id, toBotId: other.id, messages: 1, status: "replied" }, status: "done" });
      if (bot.accountId) recordUsage(db, { botId: bot.id, accountId: bot.accountId, provider: "claude", model: "haiku", input: 1000, cachedInput: 100, output: 200, reasoning: 0, costUsd: 0.02, costSource: "reported" });
      for (const account of accounts) {
        replaceAccountLimits(db, { accountId: account.id, limitId: account.provider, observedAt: nowIso(), windows: [
          { windowKey: "primary", durationMinutes: 300, usedPercent: 8, resetsAt: new Date(Date.now() + 2 * 3_600_000).toISOString(), state: "ok" },
          { windowKey: "secondary", durationMinutes: 10080, usedPercent: account.provider === "claude" ? 5 : 31, resetsAt: new Date(Date.now() + 4 * 86_400_000).toISOString(), state: "ok" },
        ] });
      }
    } finally { db.close(); }

    for (const [width, height] of [[390, 844], [360, 640]] as const) {
      await resize(app, width, height);
      await visit(app, "/");
      await page.waitForURL("**/agents/");
      await page.getByRole("heading", { name: "Agents", exact: true }).waitFor();
      const nav = page.getByRole("navigation", { name: "Tjommie", exact: true });
      await nav.getByRole("link", { name: /^Agents/ }).waitFor();
      expect(await nav.getByRole("link", { name: /^Agents/ }).getAttribute("aria-current")).toBe("page");
      const navBounds = await nav.boundingBox();
      expect(navBounds?.width).toBeLessThan(width);
      expect(navBounds?.height).toBeGreaterThanOrEqual(56);
      await capture(app, `agents-${width}-dark.png`);

      await visit(app, `/chat/?bot=${bot.id}`);
      await page.getByRole("textbox", { name: `Message ${bot.name}`, exact: true }).waitFor();
      expect(await page.getByRole("complementary", { name: `About ${bot.name}` }).count()).toBe(0);
      expect(await nav.isVisible()).toBe(false);
      await page.getByRole("group", { name: "Waiting for you", exact: true }).waitFor();
      await capture(app, `chat-${width}-dark.png`);
      const remembered = await page.evaluate(() => localStorage.getItem("tjommie.panel"));
      const details = page.getByRole("button", { name: /Show details/ });
      await details.click();
      const panel = page.getByRole("complementary", { name: `About ${bot.name}` });
      await panel.waitFor();
      await eventually(async () => Math.round((await panel.boundingBox())?.width ?? 0) === width || undefined, 5000);
      expect(await page.getByRole("textbox", { name: `Message ${bot.name}` }).evaluate((el) => el.closest("[inert]") !== null)).toBe(true);
      await capture(app, `details-${width}.png`);
      await page.getByRole("button", { name: "Add a routine", exact: true }).click();
      const routine = page.getByRole("dialog", { name: "New routine", exact: true });
      await routine.waitFor();
      await eventually(async () => Math.round((await routine.boundingBox())?.width ?? 0) === width || undefined, 5000);
      await capture(app, `routine-${width}.png`);
      await routine.getByRole("button", { name: "Close", exact: true }).click();
      await panel.getByRole("button", { name: "Close details", exact: true }).click();
      await eventually(async () => await details.evaluate((element) => element === element.ownerDocument.activeElement) || undefined);
      expect(await page.evaluate(() => localStorage.getItem("tjommie.panel"))).toBe(remembered);
      await details.click();
      await page.keyboard.press("Escape");
      await panel.waitFor({ state: "hidden" });

      // Exchange opens from a real persisted handoff and returns focus on close.
      const handoff = page.locator(`[data-exchange="${exchangeId}"]`);
      await handoff.click();
      const exchange = page.getByRole("complementary", { name: "Exchange", exact: true });
      await exchange.waitFor();
      // The sheet is visible during its scale entrance; check settled bounds.
      await eventually(async () => Math.round((await exchange.boundingBox())?.width ?? 0) === width || undefined, 5000);
      await eventually(async () => await exchange.evaluate((element) => element.contains(element.ownerDocument.activeElement)) || undefined);
      await capture(app, `exchange-${width}.png`);
      await exchange.getByRole("button", { name: "Close the exchange", exact: true }).click();
      await eventually(async () => await handoff.evaluate((element) => element === element.ownerDocument.activeElement) || undefined);

      // The Rooms tab opens the rooms list, Everyone first; Everyone opens as /room/?id=team, and the tab stays current in both.
      await visit(app, "/room/");
      await page.getByRole("heading", { name: "Rooms", exact: true }).waitFor();
      expect(await nav.getByRole("link", { name: "Rooms", exact: true }).getAttribute("aria-current")).toBe("page");
      await capture(app, `rooms-${width}.png`);
      await page.getByRole("link", { name: "Everyone", exact: true }).click();
      await page.waitForURL((url) => url.pathname === "/room/" && url.searchParams.get("id") === "team");
      expect(await nav.getByRole("link", { name: "Rooms", exact: true }).getAttribute("aria-current")).toBe("page");
      await page.getByRole("combobox", { name: "Message Everyone" }).fill("@");
      await page.getByRole("listbox", { name: "Mention a bot" }).waitFor();
      await capture(app, `team-mention-${width}.png`);
      await page.keyboard.press("Escape");
      await page.getByRole("combobox", { name: "Message Everyone" }).fill("");
      await page.getByRole("heading", { name: /^Everyone/ }).getByRole("button").click();
      const members = page.getByRole("complementary", { name: "Everyone members" });
      await members.waitFor();
      await eventually(async () => Math.round((await members.boundingBox())?.width ?? 0) === width || undefined, 5000);
      await eventually(async () => await members.evaluate((element) => element.contains(element.ownerDocument.activeElement)) || undefined);
      await capture(app, `members-${width}.png`);
      await page.keyboard.press("Escape");
      await members.waitFor({ state: "hidden" });

      for (const path of ["/bots/new/", `/bots/settings/?bot=${bot.id}`]) {
        await visit(app, path);
        await page.getByRole("button", { name: /^Account and model/ }).waitFor();
        await page.getByRole("button", { name: /^Account and model/ }).click();
        const lastChoice = page.getByRole("button", { name: /Layout account 12 with a long label/ });
        await lastChoice.scrollIntoViewIfNeeded();
        const choiceBounds = await lastChoice.boundingBox();
        expect(choiceBounds?.y).toBeGreaterThanOrEqual(0);
        expect((choiceBounds?.y ?? height) + (choiceBounds?.height ?? 1)).toBeLessThanOrEqual(height);
        await capture(app, `${path.includes("new") ? "new-bot" : "bot-settings"}-${width}.png`);
        await page.keyboard.press("Escape");
        const action = page.getByRole("button", { name: path.includes("new") ? "Create bot" : "Save changes", exact: true });
        await action.scrollIntoViewIfNeeded();
        const actionBounds = await action.boundingBox();
        expect(actionBounds?.y).toBeGreaterThanOrEqual(0);
        expect((actionBounds?.y ?? height) + (actionBounds?.height ?? 1)).toBeLessThanOrEqual(height);
        await capture(app, `${path.includes("new") ? "new-bot" : "bot-settings"}-closed-${width}.png`);
      }
      await visit(app, "/settings/");
      await page.getByRole("navigation", { name: "Settings", exact: true }).getByRole("link", { name: /^Accounts\b/ }).click();
      await page.getByRole("heading", { name: "Accounts", exact: true }).waitFor();
      await capture(app, `accounts-${width}-dark.png`);
      await page.getByRole("button", { name: "Add Claude account", exact: true }).click();
      await page.getByRole("menuitem", { name: "API key", exact: true }).click();
      const keyDialog = page.getByRole("dialog", { name: "Add Claude Code API key", exact: true });
      await keyDialog.waitFor();
      await eventually(async () => Math.round((await keyDialog.boundingBox())?.width ?? 0) === width || undefined, 5000);
      await capture(app, `add-key-${width}.png`);
      await keyDialog.getByRole("button", { name: "Cancel", exact: true }).click();
      await visit(app, "/settings/?section=usage");
      await page.getByText("API value (subscriptions)", { exact: true }).waitFor();
      await page.getByText("Combined cost estimate", { exact: true }).waitFor();
      await page.getByRole("group", { name: "Range", exact: true }).getByRole("button", { name: "This year", exact: true }).click();
      await page.getByText("Combined cost estimate", { exact: true }).waitFor();
      await page.getByText(/^Months in /).waitFor();
      await capture(app, `usage-${width}-dark.png`);
    }

    // Native guests retain the desktop capability at phone width, never a JPEG stream.
    site = createServer((_request, response) => response.writeHead(200, { "content-type": "text/html" }).end('<!doctype html><title>Phone native fixture</title><body style="background:#dbecf5"><input id="name" style="width:150px;height:32px"><p>Local native phone fixture</p></body>'));
    await new Promise<void>((done) => site?.listen(0, "127.0.0.1", done));
    const address = site.address();
    if (!address || typeof address === "string") throw new Error("Fixture server unavailable");
    const origin = `http://127.0.0.1:${address.port}`;
    await resize(app, 360, 640);
    await visit(app, `/chat/?bot=${bot.id}`);
    await page.getByRole("button", { name: /Show details/ }).click();
    await page.getByRole("tab", { name: "Browser", exact: true }).click();
    await page.getByRole("button", { name: "Take control", exact: true }).click();
    await page.getByRole("textbox", { name: "Browser address" }).fill(origin);
    await page.getByRole("button", { name: "Go", exact: true }).click();
    await eventually(async () => (await nativeViews(app!, origin)).some((view) => view.visible && view.bounds.width > 200) || undefined);
    expect(await page.getByRole("button", { name: "Expand browser", exact: true }).isVisible()).toBe(false);
    await electronApp.evaluate(({ webContents }, siteOrigin) => {
      const guest = webContents.getAllWebContents().find((contents) => contents.getURL().startsWith(siteOrigin));
      if (!guest) throw new Error("Native phone fixture missing");
      guest.sendInputEvent({ type: "mouseDown", x: 40, y: 20, button: "left", clickCount: 1 });
      guest.sendInputEvent({ type: "mouseUp", x: 40, y: 20, button: "left", clickCount: 1 });
      guest.sendInputEvent({ type: "char", keyCode: "Q" });
    }, origin);
    expect(await electronApp.evaluate(async ({ webContents }, siteOrigin) => {
      const guest = webContents.getAllWebContents().find((contents) => contents.getURL().startsWith(siteOrigin));
      if (!guest) throw new Error("Native phone fixture missing");
      return guest.executeJavaScript("document.getElementById('name').value") as Promise<unknown>;
    }, origin)).toBe("Q");
    await fits(app);
    const identity = await electronApp.evaluate(({ BrowserWindow }) => {
      const handle = BrowserWindow.getAllWindows()[0].getNativeWindowHandle();
      return { handle: (handle.length === 8 ? handle.readBigUInt64LE() : BigInt(handle.readUInt32LE())).toString(), pid: process.pid };
    });
    const powershell = join(process.env.SystemRoot ?? "C:/Windows", "System32/WindowsPowerShell/v1.0/powershell.exe");
    const nativeCapture: unknown = JSON.parse(execFileSync(powershell, ["-NoProfile", "-NonInteractive", "-File", join(import.meta.dirname, "native-window-capture.ps1"), "-WindowHandle", identity.handle, "-OwnerPid", String(identity.pid), "-OutputPath", join(screenshots, "native-browser-360.png")], { windowsHide: true, encoding: "utf8" }));
    expect(nativeCapture).toMatchObject({ nativePixelSamples: expect.any(Number) });
    if (!nativeCapture || typeof nativeCapture !== "object" || !("nativePixelSamples" in nativeCapture) || typeof nativeCapture.nativePixelSamples !== "number") throw new Error("Native composite missing");
    expect(nativeCapture.nativePixelSamples).toBeGreaterThan(1000);
    // Exercise the renderer's actual occlusion observer while its phone sheet is open.
    await page.evaluate("{const overlay=document.createElement('div');overlay.id='layout-overlay';overlay.setAttribute('role','menu');document.body.append(overlay)}");
    await eventually(async () => (await nativeViews(app!, origin)).every((view) => !view.visible) || undefined);
    await page.evaluate("document.getElementById('layout-overlay').remove()");
    await eventually(async () => (await nativeViews(app!, origin)).some((view) => view.visible) || undefined);
    await resize(app, 1440, 900);
    await page.getByRole("button", { name: "Expand browser", exact: true }).click();
    await resize(app, 360, 640);
    expect(await page.getByRole("button", { name: "Restore browser", exact: true }).isVisible()).toBe(false);
    await resize(app, 1440, 900);
    await page.getByRole("button", { name: "Expand browser", exact: true }).waitFor();
    await resize(app, 360, 640);
    await page.getByRole("tab", { name: "Details", exact: true }).click();
    await eventually(async () => (await nativeViews(app!, origin)).every((view) => !view.visible) || undefined);
    expect((await client.call("browser.open", { botId: bot.id })).control).toBe("bot");

    await resize(app, 390, 844);
    await visit(app, "/settings/?section=general");
    await page.getByRole("radio", { name: "Light", exact: true }).click();
    for (const [name, path] of [["accounts", "/settings/?section=accounts"], ["usage", "/settings/?section=usage"], ["chat", `/chat/?bot=${bot.id}`]] as const) {
      await visit(app, path);
      if (name === "chat") await page.getByRole("button", { name: /Show details/ }).waitFor();
      else await page.getByRole("heading", { name: name === "accounts" ? "Accounts" : "Usage", exact: true }).waitFor();
      if (name === "usage") await page.getByText("API value (subscriptions)", { exact: true }).waitFor();
      await capture(app, `${name}-390-light.png`);
    }
    await resize(app, 1440, 900);
    await visit(app, "/agents/");
    expect(await page.getByRole("navigation", { name: "Tjommie", exact: true }).getByRole("link", { name: "New bot", exact: true }).isVisible()).toBe(true);
    await capture(app, "agents-1440-desktop.png");
    expect(rendererErrors).toEqual([]);
  } catch (error) {
    if (app) {
      await app.page.screenshot({ path: join(screenshots, "failure.png"), timeout: 5000 }).catch(() => {});
      mkdirSync(resolve(".cache"), { recursive: true });
      writeFileSync(resolve(".cache/phone-error.json"), JSON.stringify({ url: app.page.url(), active: await app.page.evaluate("({tag:document.activeElement?.tagName,label:document.activeElement?.getAttribute('aria-label')})").catch(() => null) }, null, 2));
    }
    throw error;
  } finally {
    await closeRealApp(app, false);
    if (site) await new Promise<void>((done, reject) => site?.close((error) => error ? reject(error) : done()));
  }
}, 240_000);
