import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { expect, it } from "vitest";
import { addDevice } from "../../daemon/src/devices-store.ts";
import { openDb } from "../../daemon/src/db.ts";
import { closeRealApp, eventually, launchRealApp, type RealApp } from "./real-app.ts";

// Delayed/failed RPCs use the production renderer and disposable daemon only.
// Paused unsigned bots cannot start a provider turn.
it("retains edited drafts, reveals keyboard mentions, dismisses native overlays and reports revoke failure", async () => {
  let app: RealApp | undefined;
  try {
    app = await launchRealApp(undefined, { isolatedAccounts: true });
    const { page, client } = app;
    const bot = await client.call("bots.create", { name: "Draft fixture", provider: "claude", model: "haiku", color: "blue", shape: "circle", instructions: "Synthetic UI fixture", autonomy: "ask", browser: false });
    await client.call("bots.pause", { id: bot.id, paused: true });
    for (let index = 0; index < 12; index++) {
      const next = await client.call("bots.create", { name: `Mention fixture ${index}`, provider: "claude", model: "haiku", color: "gray", shape: "circle", instructions: "Synthetic fixture", autonomy: "ask", browser: false });
      await client.call("bots.pause", { id: next.id, paused: true });
    }
    const db = openDb(join(app.root, "data", "tjommie.db"));
    try { addDevice(db, { name: "Disposable phone", publicKey: Buffer.alloc(32, 3).toString("base64url") }); } finally { db.close(); }
    let pending: (() => void) | undefined;
    let heldMethod: string | null = null;
    let rejectRevoke = true;
    const usageStarts: string[] = [];
    const waiting = new Set<number>();
    await page.routeWebSocket(/ws:\/\/127\.0\.0\.1:/, (socket) => {
      const server = socket.connectToServer();
      socket.onMessage((message) => {
        const request = JSON.parse(message.toString()) as { type: string; id: number; method?: string; params?: { from?: string } };
        if (request.method === "usage.query" && request.params?.from) usageStarts.push(request.params.from);
        if (request.method === "devices.revoke" && rejectRevoke) {
          socket.send(JSON.stringify({ type: "res", id: request.id, ok: false, error: { code: "internal", message: "Synthetic revocation failure" } }));
          return;
        }
        if (request.method === heldMethod) waiting.add(request.id);
        server.send(message);
      });
      server.onMessage((message) => {
        const response = JSON.parse(message.toString()) as { type: string; id?: number };
        if (response.type === "res" && response.id !== undefined && waiting.delete(response.id)) pending = () => socket.send(message);
        else socket.send(message);
      });
    });
    await app.electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].setContentSize(360, 640));
    const visit = (path: string) => page.goto(new URL(path, page.url()).href);
    await visit(`/chat/?bot=${bot.id}`);
    const input = page.getByRole("textbox", { name: `Message ${bot.name}`, exact: true });
    await input.waitFor();
    const fileA = join(app.root, "submitted-a.txt"), fileB = join(app.root, "new-b.txt");
    writeFileSync(fileA, "Synthetic attachment A"); writeFileSync(fileB, "Synthetic attachment B");
    await app.electronApp.evaluate(({ dialog }, file) => { dialog.showOpenDialog = async () => ({ canceled: false, filePaths: [file] }); }, fileA);
    await page.getByRole("button", { name: "Attach files", exact: true }).click();
    await page.getByRole("button", { name: "Remove submitted-a.txt", exact: true }).waitFor();
    heldMethod = "chat.send";
    await input.fill("Submitted message A");
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await eventually(async () => pending || undefined);
    await app.electronApp.evaluate(({ dialog }) => {
      dialog.showOpenDialog = () => new Promise((resolve) => {
        (globalThis as typeof globalThis & { uiFixturePick?: (paths: string[]) => void }).uiFixturePick = (paths) => resolve({ canceled: false, filePaths: paths });
      });
    });
    await page.getByRole("button", { name: "Attach files", exact: true }).click();
    await eventually(async () => await app!.electronApp.evaluate(() => typeof (globalThis as typeof globalThis & { uiFixturePick?: unknown }).uiFixturePick === "function") || undefined);
    await input.fill("Unsent message B");
    pending!(); pending = undefined; heldMethod = null;
    await eventually(async () => await page.getByRole("button", { name: "Send", exact: true }).isEnabled() || undefined);
    expect(await input.inputValue()).toBe("Unsent message B");
    await page.getByRole("button", { name: "Remove submitted-a.txt", exact: true }).waitFor({ state: "hidden" });
    await app.electronApp.evaluate((_electron, file) => (globalThis as typeof globalThis & { uiFixturePick: (paths: string[]) => void }).uiFixturePick([file]), fileB);
    await page.getByRole("button", { name: "Remove new-b.txt", exact: true }).waitFor();
    expect(await page.getByRole("button", { name: "Remove submitted-a.txt", exact: true }).count()).toBe(0);

    await page.getByRole("button", { name: /Show details/ }).click();
    await page.getByRole("button", { name: "Add a routine", exact: true }).click();
    let routine = page.getByRole("dialog", { name: "New routine", exact: true });
    await routine.getByLabel("Name", { exact: true }).fill("Old pending routine");
    await routine.getByLabel("What to do", { exact: true }).fill("Synthetic disabled routine");
    await routine.getByRole("switch", { name: "Routine on" }).click();
    heldMethod = "routines.create";
    await routine.getByRole("button", { name: "Save", exact: true }).click();
    await eventually(async () => pending || undefined);
    await routine.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.getByRole("button", { name: "Add a routine", exact: true }).click();
    routine = page.getByRole("dialog", { name: "New routine", exact: true });
    await routine.getByLabel("Name", { exact: true }).fill("New unsaved routine");
    pending!(); pending = undefined; heldMethod = null;
    await page.waitForTimeout(100);
    expect(await routine.getByLabel("Name", { exact: true }).inputValue()).toBe("New unsaved routine");
    // Android Back dismisses the dialog on top, leaving the parent Details sheet open.
    await page.evaluate("(()=>{const dialog=[...document.querySelectorAll('[role=dialog]')].filter(el=>el.getClientRects().length).at(-1);dialog.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));})()");
    await routine.waitFor({ state: "hidden" });
    expect(await page.getByRole("button", { name: "Close details", exact: true }).isVisible()).toBe(true);

    await visit("/room/?id=team");
    const room = page.getByRole("combobox", { name: "Message Everyone", exact: true });
    heldMethod = "room.post";
    await room.fill("Submitted room post A");
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await eventually(async () => pending || undefined);
    await room.fill("Unsent room post B");
    pending!(); pending = undefined; heldMethod = null;
    await eventually(async () => await page.getByRole("button", { name: "Send", exact: true }).isEnabled() || undefined);
    expect(await room.inputValue()).toBe("Unsent room post B");
    await room.fill("@");
    for (let step = 0; step < 12; step++) await room.press("ArrowDown");
    expect(await page.evaluate("(()=>{const list=document.getElementById('mention-list'),input=document.querySelector('[role=combobox]'),item=document.getElementById(input.getAttribute('aria-activedescendant'));const a=list.getBoundingClientRect(),b=item.getBoundingClientRect();return b.top>=a.top&&b.bottom<=a.bottom})()")).toBe(true);
    // Same interception as Android MainActivity, including Escape targeted at the listbox.
    const nativeBack = "(()=>{const overlays=[...document.querySelectorAll('[role=dialog],[role=alertdialog],[role=menu],[role=listbox]')].filter(el=>el.getClientRects().length);if(overlays.length){overlays.at(-1).dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',code:'Escape',bubbles:true,cancelable:true}));return;}window.dispatchEvent(new CustomEvent('tjommie:back',{cancelable:true}));})()";
    await page.evaluate(nativeBack);
    await page.getByRole("listbox", { name: "Mention a bot" }).waitFor({ state: "hidden" });

    await visit("/settings/?section=devices");
    await page.getByRole("button", { name: "Revoke Disposable phone", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Revoke Disposable phone?", exact: true });
    await dialog.getByRole("button", { name: "Revoke", exact: true }).click();
    await dialog.getByRole("alert").filter({ hasText: "Synthetic revocation failure" }).waitFor();
    expect(await dialog.isVisible()).toBe(true);
    expect((await client.call("devices.list", {}))[0].revokedAt).toBeNull();
    rejectRevoke = false;
    await dialog.getByRole("button", { name: "Revoke", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    expect((await client.call("devices.list", {}))[0].revokedAt).not.toBeNull();
    const calendar = await page.evaluate<string[]>("[new Date(2026,9,22,23,59,58),new Date(2026,9,22),new Date(2026,9,23)].map(date=>date.toISOString())");
    await page.clock.install({ time: new Date(calendar[0]) });
    await visit("/settings/?section=usage");
    await page.getByRole("group", { name: "Range", exact: true }).getByRole("button", { name: "Today", exact: true }).click();
    await eventually(async () => usageStarts.at(-1) === calendar[1] || undefined);
    await page.clock.fastForward(3000);
    await eventually(async () => usageStarts.at(-1) === calendar[2] || undefined);
  } finally { await closeRealApp(app, false); }
}, 120_000);
