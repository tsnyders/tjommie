import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import type { BrowserContext, CDPSession, Dialog, Page } from "playwright-core";
import { z } from "zod";
import type { BrowserFrame, BrowserInput, BrowserState } from "@tjommie/protocol";
import type { StoredBot } from "./bots.ts";
import { cliEnv } from "./cli.ts";
import { settleCleanup } from "./cleanup.ts";
import { RpcError } from "./server.ts";

const require = createRequire(import.meta.url);
export const browserProfile = (bot: StoredBot): string => join(bot.homeFolder, "chrome-profile");

export function findBrowser(exists: (path: string) => boolean = existsSync, env: NodeJS.ProcessEnv = process.env): { channel: "chrome" | "msedge"; path: string } {
  const roots = [env.ProgramFiles ?? "C:\\Program Files", env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", env.LOCALAPPDATA].filter((root): root is string => !!root);
  for (const [channel, vendor, name] of [["chrome", "Google", "Chrome"], ["msedge", "Microsoft", "Edge"]] as const) {
    for (const root of roots) {
      const path = join(root, vendor, name, "Application", channel === "chrome" ? "chrome.exe" : "msedge.exe");
      if (exists(path)) return { channel, path };
    }
  }
  throw new RpcError("not_installed", "Install Google Chrome or Microsoft Edge to use the bot's browser.");
}

// Resolve from the MCP package: it must use the exact same Playwright copy as the automation tools.
export async function launchBrowser(bot: StoredBot): Promise<BrowserContext> {
  const runtime = createRequire(require.resolve("@playwright/mcp/package.json"))("playwright") as typeof import("playwright-core");
  const browser = findBrowser();
  try {
    return await runtime.chromium.launchPersistentContext(browserProfile(bot), {
      executablePath: browser.path, channel: browser.channel, headless: true,
      chromiumSandbox: true, viewport: { width: 1000, height: 720 }, deviceScaleFactor: 1,
      acceptDownloads: true, downloadsPath: join(bot.homeFolder, "browser-files"),
      env: cliEnv({}), timeout: 30_000,
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (/ProcessSingleton|already in use|user data directory is already|profile.*in use/i.test(message)) {
      throw new RpcError("busy", "This bot's old browser window is still using its profile. Close that window, then open the browser panel again.");
    }
    // Browser launch errors can contain paths and environment details. Keep the app error bounded.
    throw new RpcError("bad_state", "The bot's browser could not start. Check that Chrome or Edge can run, then try again.");
  }
}

export function navigationUrl(value: string): string {
  let url: URL;
  try { url = new URL(value); } catch { throw new RpcError("bad_params", "Enter a complete http or https address."); }
  if (url.protocol !== "http:" && url.protocol !== "https:" && url.href !== "about:blank") {
    throw new RpcError("bad_params", "Only web addresses and about:blank can be opened here.");
  }
  if (url.username || url.password) throw new RpcError("bad_params", "Use the website's sign-in form instead of an address containing a password.");
  return url.href;
}

type Tab = { id: string; page: Page; title: string; pressed: Set<string>; observer: Promise<CDPSession | null>; navigating: boolean; viewportRevision: number; documentId?: string; documentRevision: number };
type Stream = { id: string; pageId: string; viewportRevision: number; documentRevision: number; cdp: CDPSession; frame: BrowserFrame | null; timer?: NodeJS.Timeout; sequence: number };
type Session = {
  bot: StoredBot; context: BrowserContext; generation: string; control: BrowserState["control"];
  tabs: Tab[]; activePageId: string | null; dialog: { pageId: string; value: Dialog } | null;
  dialogListeners: Set<() => void>;
  revision: number; automationActive: boolean; controlPending: number;
  stream: Stream | null; streamPending: Promise<Stream> | null; tail: Promise<void>; closing: boolean;
};
const Screencast = z.object({ data: z.string(), sessionId: z.number(), metadata: z.object({ deviceWidth: z.number(), deviceHeight: z.number() }) });
const DocumentNavigation = z.object({ frame: z.object({ loaderId: z.string(), parentId: z.string().optional() }) });

export function jpegDimensions(data: string): { width: number; height: number } | null {
  const bytes = Buffer.from(data, "base64");
  if (bytes.length < 4 || bytes.readUInt16BE(0) !== 0xffd8) return null;
  let offset = 2;
  while (offset + 3 < bytes.length) {
    if (bytes[offset++] !== 0xff) return null;
    while (bytes[offset] === 0xff) offset++;
    const marker = bytes[offset++];
    if (marker === 0xda || marker === 0xd9 || offset + 2 > bytes.length) return null;
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd8)) continue;
    const length = bytes.readUInt16BE(offset);
    if (length < 2 || offset + length > bytes.length) return null;
    if ([0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf].includes(marker) && length >= 7) {
      const height = bytes.readUInt16BE(offset + 3), width = bytes.readUInt16BE(offset + 5);
      return width > 0 && height > 0 ? { width, height } : null;
    }
    offset += length;
  }
  return null;
}

export class BrowserManager {
  #sessions = new Map<string, Promise<Session>>();
  #closing = new Set<string>();
  #mcpDownloads = new WeakMap<BrowserContext, number>();
  #launch: (bot: StoredBot) => Promise<BrowserContext>;

  constructor(launch: (bot: StoredBot) => Promise<BrowserContext> = launchBrowser) { this.#launch = launch; }

  async #session(bot: StoredBot): Promise<Session> {
    if (this.#closing.has(bot.id)) throw new RpcError("busy", "The browser is closing. Try again shortly.");
    const current = this.#sessions.get(bot.id);
    if (current) return current;
    const opening = this.#create(bot);
    this.#sessions.set(bot.id, opening);
    void opening.catch(() => { if (this.#sessions.get(bot.id) === opening) this.#sessions.delete(bot.id); });
    return opening;
  }

  async #create(bot: StoredBot): Promise<Session> {
    const context = await this.#launch(bot);
    context.setDefaultTimeout(10_000);
    context.setDefaultNavigationTimeout(20_000);
    const session: Session = { bot, context, generation: randomUUID(), control: "bot", tabs: [], activePageId: null, dialog: null, dialogListeners: new Set(), revision: 0, automationActive: false, controlPending: 0, stream: null, streamPending: null, tail: Promise.resolve(), closing: false };
    context.on("page", (page) => this.#addPage(session, page));
    context.on("close", () => { void this.#stopStream(session); void this.#sessions.get(bot.id)?.then((current) => { if (current === session) this.#sessions.delete(bot.id); }).catch(() => {}); });
    for (const page of context.pages()) this.#addPage(session, page);
    try {
      if (!session.tabs.length) await context.newPage();
      await Promise.all(session.tabs.map((tab) => tab.observer));
    }
    catch (error) { await context.close().catch(() => {}); throw error; }
    return session;
  }

  #addPage(session: Session, page: Page): void {
    if (session.tabs.some((tab) => tab.page === page)) return;
    const tab: Tab = { id: randomUUID(), page, title: "", pressed: new Set(), observer: Promise.resolve(null), navigating: false, viewportRevision: 0, documentRevision: 0 };
    // Dialogs can be answered by either the person or MCP; the close event keeps both views consistent.
    tab.observer = session.context.newCDPSession(page).then(async (cdp) => {
      cdp.on("Page.javascriptDialogClosed", () => { if (session.dialog?.pageId === tab.id) { session.dialog = null; session.revision++; } });
      // CDP separates document replacement from history/hash routing. Only a new top-level
      // loader invalidates cached document pixels; it does not change the input layout epoch.
      cdp.on("Page.frameNavigated", (value: unknown) => {
        const parsed = DocumentNavigation.safeParse(value);
        if (!parsed.success || parsed.data.frame.parentId || parsed.data.frame.loaderId === tab.documentId) return;
        tab.documentId = parsed.data.frame.loaderId;
        tab.documentRevision++; session.revision++;
      });
      await cdp.send("Page.enable");
      return cdp;
    }).catch(() => null);
    session.tabs.push(tab);
    session.activePageId = tab.id;
    session.revision++;
    const refresh = (): void => { void page.title().then((title) => { if (tab.title !== title) { tab.title = title; session.revision++; } }).catch(() => {}); };
    page.on("domcontentloaded", refresh);
    page.on("framenavigated", (frame) => {
      if (frame !== page.mainFrame()) return;
      session.revision++; // URL/title state changes, including history.replaceState during typing
      refresh();
    });
    page.on("dialog", (value) => {
      session.dialog = { pageId: tab.id, value };
      session.activePageId = tab.id;
      session.revision++;
      for (const notify of session.dialogListeners) notify();
    });
    page.on("close", () => {
      void tab.observer.then((cdp) => cdp?.detach()).catch(() => {});
      session.tabs = session.tabs.filter((entry) => entry !== tab);
      if (session.activePageId === tab.id) session.activePageId = session.tabs.at(-1)?.id ?? null;
      if (session.dialog?.pageId === tab.id) session.dialog = null;
      session.revision++;
      if (session.stream?.pageId === tab.id) void this.#stopStream(session);
    });
    // Downloads remain inside the bot home and never launch a desktop application.
    page.on("download", (download) => {
      if (this.#mcpDownloads.has(session.context)) return; // the active MCP backend already saves this download
      const name = download.suggestedFilename().replace(/[^a-zA-Z0-9._ -]/g, "_").slice(0, 140) || "download";
      void download.saveAs(join(session.bot.homeFolder, "browser-files", `${randomUUID()}-${name}`)).catch(() => {});
    });
    refresh();
  }

  #state(session: Session): BrowserState {
    const dialog = session.dialog;
    const active = session.tabs.find((tab) => tab.id === session.activePageId);
    return { botId: session.bot.id, generation: session.generation, control: session.control, activePageId: session.activePageId,
      revision: session.revision, viewportRevision: active?.viewportRevision ?? 0,
      busy: !!active?.navigating || (session.control === "human" && (session.automationActive || session.controlPending > 0)),
      tabs: session.tabs.map((tab) => ({ id: tab.id, url: tab.page.url(), title: tab.title })),
      dialog: dialog ? { pageId: dialog.pageId, type: dialog.value.type(), message: dialog.value.message(), defaultValue: dialog.value.defaultValue() } : null };
  }

  #check(session: Session, generation: string, pageId?: string): Tab | undefined {
    if (session.closing || session.generation !== generation) throw new RpcError("bad_state", "The browser restarted. Wait for its current view before interacting.");
    if (pageId === undefined) return undefined;
    const tab = session.tabs.find((entry) => entry.id === pageId);
    if (!tab) throw new RpcError("not_found", "This browser tab has closed.");
    return tab;
  }

  #queue<T>(session: Session, run: () => Promise<T>): Promise<T> {
    const result = session.tail.then(() => {
      if (session.closing) throw new RpcError("busy", "The browser is closing.");
      return run();
    });
    session.tail = result.then(() => {}, () => {});
    return result;
  }

  async open(bot: StoredBot): Promise<BrowserState> { return this.#state(await this.#session(bot)); }
  async context(bot: StoredBot): Promise<BrowserContext> { return (await this.#session(bot)).context; }
  async state(bot: StoredBot): Promise<BrowserState> { return this.#state(await this.#session(bot)); }

  delegateDownloads(context: BrowserContext): () => void {
    this.#mcpDownloads.set(context, (this.#mcpDownloads.get(context) ?? 0) + 1);
    let released = false;
    return () => {
      if (released) return;
      released = true;
      const remaining = (this.#mcpDownloads.get(context) ?? 1) - 1;
      if (remaining === 0) this.#mcpDownloads.delete(context); else this.#mcpDownloads.set(context, remaining);
    };
  }

  async control(bot: StoredBot, control: BrowserState["control"]): Promise<BrowserState> {
    const session = await this.#session(bot);
    // Takeover blocks newly queued bot calls immediately; input waits for an already-running call to finish.
    session.control = control;
    session.controlPending++; session.revision++;
    // Return the control state immediately so a dialog raised by the pending operation can be answered.
    // Releases and all subsequent input still serialize after that operation.
    void this.#queue(session, async () => {
      for (const tab of session.tabs) {
        for (const key of tab.pressed) await tab.page.keyboard.up(key).catch(() => {});
        tab.pressed.clear();
        for (const button of ["left", "middle", "right"] as const) await tab.page.mouse.up({ button }).catch(() => {});
      }
    }).catch(() => {}).finally(() => { session.controlPending--; session.revision++; });
    return this.#state(session);
  }

  async automation<T>(bot: StoredBot, run: () => Promise<T>): Promise<T> {
    const session = await this.#session(bot);
    return this.#queue(session, async () => {
      if (session.control === "human") throw new RpcError("busy", "The person is using this browser. Ask them to choose Resume bot in the browser panel when they are finished.");
      session.automationActive = true; session.revision++;
      try { return await run(); }
      finally { session.automationActive = false; session.revision++; }
    });
  }

  async activeIndex(bot: StoredBot): Promise<number> {
    const session = await this.#session(bot);
    return session.tabs.findIndex((tab) => tab.id === session.activePageId);
  }
  async selectAutomationIndex(bot: StoredBot, index: number): Promise<void> {
    const session = await this.#session(bot);
    const tab = session.tabs[index];
    if (tab && session.activePageId !== tab.id) { session.activePageId = tab.id; session.revision++; }
  }

  async #human<T>(bot: StoredBot, generation: string, pageId: string | undefined, action: (session: Session, tab: Tab | undefined) => Promise<T>, onDialog?: (session: Session) => T, queueUnderDialog = false): Promise<T> {
    const session = await this.#session(bot);
    if (session.control !== "human") throw new RpcError("busy", "Choose Take over before interacting with the browser.");
    this.#check(session, generation, pageId);
    if (session.dialog && onDialog && !queueUnderDialog) return onDialog(session);
    const operation = this.#queue(session, async () => {
      if (session.control !== "human") throw new RpcError("busy", "Choose Take over before interacting with the browser.");
      return action(session, this.#check(session, generation, pageId));
    });
    if (!onDialog) return operation;
    if (session.dialog) {
      // A release is accepted for ordered delivery, even if CDP cannot finish it until the modal
      // closes. The queue retains ownership; returning now leaves the dialog-answer RPC usable.
      void operation.catch(() => {});
      return onDialog(session);
    }
    let listener: () => void = () => {};
    const dialog = new Promise<T>((resolve) => {
      listener = () => resolve(onDialog(session));
      session.dialogListeners.add(listener);
    });
    // CDP input/navigation can wait for a dialog. Let the UI poll and answer it; keep the underlying
    // operation in the queue so ordinary input still cannot overtake it.
    try { return await Promise.race([operation, dialog]); }
    finally { session.dialogListeners.delete(listener); }
  }

  async navigate(bot: StoredBot, generation: string, pageId: string, url: string): Promise<BrowserState> {
    const target = navigationUrl(url);
    return this.#human(bot, generation, pageId, async (session, tab) => {
      await this.#reshapePage(session, tab!, () => tab!.page.goto(target, { waitUntil: "commit" }));
      return this.#state(session);
    }, (session) => this.#state(session));
  }
  async history(bot: StoredBot, generation: string, pageId: string, action: "back" | "forward" | "reload"): Promise<BrowserState> {
    return this.#human(bot, generation, pageId, async (session, tab) => {
      await this.#reshapePage(session, tab!, () => {
        if (action === "back") return tab!.page.goBack({ waitUntil: "commit" });
        if (action === "forward") return tab!.page.goForward({ waitUntil: "commit" });
        return tab!.page.reload({ waitUntil: "commit" });
      });
      return this.#state(session);
    }, (session) => this.#state(session));
  }

  async #reshapePage(session: Session, tab: Tab, change: () => Promise<unknown>): Promise<void> {
    tab.navigating = true;
    tab.viewportRevision++; session.revision++;
    const invalidate = async (): Promise<void> => {
      // A frame pull may already be starting a stream. Drain it before discarding its old document.
      await session.streamPending?.catch(() => {});
      if (session.stream?.pageId === tab.id) await this.#stopStream(session);
    };
    try { await invalidate(); await change(); }
    finally { await invalidate(); tab.navigating = false; session.revision++; }
  }
  async tab(bot: StoredBot, generation: string, action: "new" | "select" | "close", pageId?: string): Promise<BrowserState> {
    return this.#human(bot, generation, pageId, async (session, tab) => {
      if (action === "new") await session.context.newPage();
      else {
        if (!tab) throw new RpcError("bad_params", "Choose a browser tab.");
        if (action === "close") await tab.page.close();
        else { session.activePageId = tab.id; session.revision++; await tab.page.bringToFront(); }
      }
      if (!session.tabs.length) await session.context.newPage();
      return this.#state(session);
    }, (session) => this.#state(session));
  }
  async resize(bot: StoredBot, generation: string, pageId: string, width: number, height: number): Promise<BrowserState> {
    return this.#human(bot, generation, pageId, async (session, tab) => {
      await this.#reshapePage(session, tab!, async () => {
        await tab!.page.setViewportSize({ width, height });
        // A popup can change Chromium's shared native window surface while the original tab keeps
        // its emulated viewport. Playwright skips identical viewport overrides, so also restore the
        // content surface explicitly; never relabel a larger captured image as the requested size.
        const cdp = await tab!.observer;
        if (!cdp) throw new RpcError("bad_state", "The browser could not resize its view. Close this tab and try again.");
        const { windowId } = await cdp.send("Browser.getWindowForTarget");
        try { await cdp.send("Browser.setContentsSize", { windowId, width, height }); }
        catch (error) {
          // This experimental command is absent on older Chromium builds. Preserve the Playwright
          // viewport there; capture validation still hides a mismatched surface. Other failures matter.
          const reason = error instanceof Error ? /Protocol error \(Browser\.setContentsSize\): (.+)$/i.exec(error.message)?.[1] : undefined;
          if (!reason || !/^(?:'Browser\.setContentsSize' wasn't found|Method not found|Not supported|(?:Browser\.setContentsSize|This method) is not supported)\.?$/i.test(reason)) throw error;
        }
      });
      return this.#state(session);
    }, (session) => this.#state(session));
  }
  async input(bot: StoredBot, generation: string, pageId: string, input: BrowserInput, viewportRevision?: number): Promise<void> {
    const release = (input.kind === "key" || input.kind === "mouse") && input.action === "up";
    await this.#human(bot, generation, pageId, async (session, tab) => {
      const pointer = input.kind === "mouse" || input.kind === "wheel";
      if (!release && (session.activePageId !== pageId || (pointer && viewportRevision !== undefined && viewportRevision !== tab!.viewportRevision))) throw new RpcError("bad_state", "The browser view changed. Wait for its current view before interacting.");
      const page = tab!.page;
      if (input.kind === "mouse" || input.kind === "wheel") {
        const size = page.viewportSize();
        if (!size || (!release && (input.x >= size.width || input.y >= size.height))) throw new RpcError("bad_params", "The pointer is outside the browser view.");
        await page.mouse.move(Math.min(input.x, size.width - 1), Math.min(input.y, size.height - 1));
        if (input.kind === "wheel") await page.mouse.wheel(input.deltaX, input.deltaY);
        else if (input.action === "down") await page.mouse.down({ button: input.button });
        else if (input.action === "up") await page.mouse.up({ button: input.button });
      } else if (input.kind === "text") await page.keyboard.insertText(input.text);
      else if (input.action === "down") { await page.keyboard.down(input.key); tab!.pressed.add(input.key); }
      else { await page.keyboard.up(input.key); tab!.pressed.delete(input.key); }
    }, () => undefined, release);
  }
  async dialog(bot: StoredBot, generation: string, pageId: string, accept: boolean, promptText?: string): Promise<BrowserState> {
    const session = await this.#session(bot);
    if (session.control !== "human") throw new RpcError("busy", "Choose Take over before answering the page dialog.");
    this.#check(session, generation, pageId);
    const dialog = session.dialog;
    if (!dialog || dialog.pageId !== pageId) throw new RpcError("bad_state", "This dialog has already closed.");
    // Do not queue behind the operation waiting for this very dialog.
    if (accept) await dialog.value.accept(promptText); else await dialog.value.dismiss();
    if (session.dialog === dialog) { session.dialog = null; session.revision++; }
    return this.#state(session);
  }

  async frame(bot: StoredBot, after?: { streamId: string; sequence: number }): Promise<{ state: BrowserState; frame: BrowserFrame | null; unchanged?: boolean }> {
    const session = await this.#session(bot);
    const tab = session.tabs.find((entry) => entry.id === session.activePageId);
    if (!tab || tab.navigating) return { state: this.#state(session), frame: null };
    const stream = await this.#stream(session, tab);
    clearTimeout(stream.timer);
    stream.timer = setTimeout(() => { if (session.stream === stream) void this.#stopStream(session); }, 3000);
    stream.timer.unref();
    const frame = session.stream === stream && !tab.navigating && tab.viewportRevision === stream.viewportRevision && tab.documentRevision === stream.documentRevision && session.activePageId === stream.pageId ? stream.frame : null;
    if (frame && after?.streamId === stream.id && after.sequence === frame.sequence) return { state: this.#state(session), frame: null, unchanged: true };
    return { state: this.#state(session), frame };
  }

  async #stream(session: Session, tab: Tab): Promise<Stream> {
    if (session.stream?.pageId === tab.id && session.stream.viewportRevision === tab.viewportRevision && session.stream.documentRevision === tab.documentRevision) return session.stream;
    if (session.streamPending) { await session.streamPending; return this.#stream(session, tab); }
    const pending = (async () => {
      await this.#stopStream(session);
      const cdp = await session.context.newCDPSession(tab.page);
      const stream: Stream = { id: randomUUID(), pageId: tab.id, viewportRevision: tab.viewportRevision, documentRevision: tab.documentRevision, cdp, frame: null, sequence: 0 };
      session.stream = stream;
      cdp.on("Page.screencastFrame", (value: unknown) => {
        const parsed = Screencast.safeParse(value);
        if (!parsed.success) return;
        const { data, sessionId, metadata } = parsed.data;
        void cdp.send("Page.screencastFrameAck", { sessionId }).catch(() => {});
        if (session.stream !== stream) return;
        const viewport = tab.page.viewportSize();
        if (tab.navigating || tab.viewportRevision !== stream.viewportRevision || tab.documentRevision !== stream.documentRevision || (viewport && (metadata.deviceWidth !== viewport.width || metadata.deviceHeight !== viewport.height))) {
          stream.frame = null; // never show a capture whose physical surface disagrees with input coordinates
          return;
        }
        const pixels = jpegDimensions(data);
        if (!pixels) { stream.frame = null; return; }
        stream.frame = { data, mime: "image/jpeg", ...pixels, viewportWidth: metadata.deviceWidth, viewportHeight: metadata.deviceHeight, pageId: tab.id, viewportRevision: tab.viewportRevision, streamId: stream.id, sequence: ++stream.sequence };
      });
      try { await cdp.send("Page.startScreencast", { format: "jpeg", quality: 75, maxWidth: 1280, maxHeight: 1000, everyNthFrame: 1 }); }
      catch (error) { await this.#stopStream(session); throw error; }
      return stream;
    })();
    session.streamPending = pending;
    try { return await pending; } finally { if (session.streamPending === pending) session.streamPending = null; }
  }
  async #stopStream(session: Session): Promise<void> {
    const stream = session.stream;
    session.stream = null;
    if (!stream) return;
    clearTimeout(stream.timer);
    await stream.cdp.send("Page.stopScreencast").catch(() => {});
    await stream.cdp.detach().catch(() => {});
  }

  async close(botId: string): Promise<void> {
    const pending = this.#sessions.get(botId);
    if (!pending) return;
    this.#closing.add(botId);
    try {
      const session = await pending.catch(() => null);
      if (!session) return; // a failed launch left no live session to close and must not block deletion
      session.closing = true;
      if (session.dialog) await session.dialog.value.dismiss().catch(() => {});
      await this.#stopStream(session);
      await session.context.close(); // waits for owned Chromium and profile writes to finish
      await session.tail;
    } finally {
      if (this.#sessions.get(botId) === pending) this.#sessions.delete(botId);
      this.#closing.delete(botId);
    }
  }
  async closeAll(): Promise<void> { await settleCleanup([...this.#sessions.keys()].map((id) => this.close(id)), "Some bot browsers could not close."); }
}

export const browsers = new BrowserManager();
