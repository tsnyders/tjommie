import { BrowserWindow, WebContentsView, session, type DownloadItem, type Session, type WebContents, type WebPreferences } from "electron";
import { randomUUID } from "node:crypto";
import { lstatSync, mkdirSync, realpathSync } from "node:fs";
import { basename, join, relative, resolve } from "node:path";
import type { BrowserState } from "../../protocol/src/browser.ts";
import { NativeBrowserCommand, NativeBrowserMount, type NativeBrowserUiCommand, type NativeBrowserToolCommand } from "../../protocol/src/native-browser.ts";
import { nativeBrowserBounds, nativeBrowserFrameUrl, nativeBrowserProfile, nativeBrowserUrl } from "./native-browser-policy.ts";
import { denyNativeBrowserBluetooth, denyNativeBrowserPermissions } from "./native-browser-permissions.ts";
import { boundedProfileCleanup, checkedNativeProfile, clearNativeProfile, nativeProfileDirectories, removeNativeProfile } from "./native-browser-profiles.ts";

export type NativeBrowserBot = { id: string; homeFolder: string; name?: string; projectFolder?: string | null };
export type NativeBrowserNotice = { botId: string; message: string };
export type NativeBrowserOptions = {
  window: BrowserWindow;
  profileRoot: string;
  onState?: (state: BrowserState) => void;
  onNotice?: (notice: NativeBrowserNotice) => void;
  onFocusReturn?: (botId: string) => void;
  automation?: (engine: NativeBrowserEngine, bot: NativeBrowserBot, command: NativeBrowserToolCommand) => Promise<unknown>;
};
type Tab = {
  id: string; view: WebContentsView; contents: WebContents; ready: Promise<void>;
  keys: Set<string>; buttons: Set<"left" | "middle" | "right">; point: { x: number; y: number };
};
type Browser = {
  bot: NativeBrowserBot; generation: string; session: Session; tabs: Map<string, Tab>; active: string | null;
  control: "human" | "bot"; revision: number; viewportRevision: number; busy: number; handover: number;
  tail: Promise<void>; closing: boolean; injecting: boolean; dialog: BrowserState["dialog"];
  mount: NativeBrowserMount | null;
  downloads: Set<DownloadItem>;
  downloadListener?: (event: { preventDefault(): void }, item: DownloadItem, contents: WebContents) => void;
};

function preferences(profile: Session): WebPreferences {
  return { session: profile, nodeIntegration: false, nodeIntegrationInSubFrames: false, nodeIntegrationInWorker: false,
    contextIsolation: true, sandbox: true, webSecurity: true, webviewTag: false, allowRunningInsecureContent: false,
    navigateOnDragDrop: false, safeDialogs: true, spellcheck: true, backgroundThrottling: false };
}
function containedFolder(root: string, leaf: string): string {
  mkdirSync(root, { recursive: true });
  const base = realpathSync(root), path = resolve(base, leaf);
  mkdirSync(path, { recursive: true });
  if (lstatSync(path).isSymbolicLink() || relative(base, realpathSync(path)).startsWith("..")) throw new Error("Browser storage must stay inside its owned folder.");
  return path;
}

/** Owns native browser contents. No renderer receives an Electron object or a debugging endpoint. */
export class NativeBrowserEngine {
  #options: NativeBrowserOptions;
  #browsers = new Map<string, Browser>();
  #mounted: string | null = null;
  #occluded = false;
  #closed = false;
  #closing = new Map<string, Promise<void>>();
  #profiles = new Map<string, Session>();
  #deleting = new Map<string, Promise<void>>();
  #blockedProfiles = new Set<string>();
  #shield: WebContentsView | null = null;
  #windowChanged = (): void => { this.#present(); };

  constructor(options: NativeBrowserOptions) {
    this.#options = options;
    options.window.on("resize", this.#windowChanged);
    options.window.on("hide", this.#windowChanged);
    options.window.on("show", this.#windowChanged);
    options.window.once("closed", () => { void this.closeAll().catch(() => {}); });
  }

  async open(bot: NativeBrowserBot): Promise<BrowserState> {
    if (this.#closed || this.#options.window.isDestroyed()) throw new Error("The desktop browser host is closed.");
    const hash = basename(nativeBrowserProfile(this.#options.profileRoot, bot.id));
    if (this.#deleting.has(hash) || this.#blockedProfiles.has(hash)) throw new Error("The bot browser profile is being deleted or needs cleanup to be retried.");
    let browser = this.#browsers.get(bot.id);
    if (!browser) {
      const path = nativeBrowserProfile(this.#options.profileRoot, bot.id);
      const storage = containedFolder(this.#options.profileRoot, basename(path));
      const profile = session.fromPath(storage);
      this.#profiles.set(hash, profile);
      browser = { bot: { ...bot }, generation: randomUUID(), session: profile, tabs: new Map(), active: null,
        control: "bot", revision: 0, viewportRevision: 0, busy: 0, handover: 0, tail: Promise.resolve(), closing: false,
        injecting: false, dialog: null, mount: null, downloads: new Set() };
      this.#browsers.set(bot.id, browser);
      this.#configureSession(browser);
      this.#newTab(browser);
    } else if (resolve(browser.bot.homeFolder) !== resolve(bot.homeFolder)) throw new Error("The browser bot home changed; close its browser before reopening it.");
    if (browser.closing) throw new Error("The bot browser is closing.");
    await Promise.all([...browser.tabs.values()].map((tab) => tab.ready));
    return this.state(bot.id);
  }

  state(botId: string): BrowserState {
    const browser = this.#get(botId);
    return { botId, generation: browser.generation, control: browser.control, revision: browser.revision,
      viewportRevision: browser.viewportRevision, busy: browser.busy > 0 || browser.handover > 0, activePageId: browser.active,
      tabs: [...browser.tabs.values()].filter((tab) => !tab.contents.isDestroyed()).map((tab) => ({ id: tab.id, url: tab.contents.getURL() || "about:blank", title: tab.contents.getTitle() })),
      dialog: browser.dialog ? { ...browser.dialog } : null };
  }

  async execute(bot: NativeBrowserBot, raw: NativeBrowserCommand): Promise<unknown> {
    const command = NativeBrowserCommand.parse(raw);
    if (command.kind === "close") { await this.closeBot(bot.id); return {}; }
    if (command.kind === "delete_profile") { await this.deleteProfile(bot.id); return {}; }
    if (command.kind === "cancel_tools") throw new Error("Tool cancellation belongs to the private desktop host bridge.");
    if (command.kind === "tool") {
      if (!this.#options.automation) throw new Error("Native browser automation is unavailable.");
      return this.#options.automation(this, bot, command);
    }
    return this.act(bot, command, "human");
  }

  async act(bot: NativeBrowserBot, command: NativeBrowserUiCommand, actor: "human" | "bot", check?: () => void): Promise<BrowserState> {
    check?.();
    await this.open(bot);
    check?.();
    const browser = this.#get(bot.id);
    if (command.kind === "open" || command.kind === "state") return this.state(bot.id);
    if (command.kind === "control") return this.control(bot, command.control);
    this.#generation(browser, command.generation);
    // Dialog dismissal is an unblocker and must never wait behind a blocked browser action.
    if (command.kind === "dialog") {
      if (browser.control !== actor) throw new Error("Browser control changed; the current controller must answer this dialog.");
      const tab = this.#tab(browser, command.pageId);
      if (!browser.dialog || browser.dialog.pageId !== tab.id) throw new Error("There is no dialog in that tab.");
      await tab.contents.debugger.sendCommand("Page.handleJavaScriptDialog", { accept: command.accept, promptText: command.promptText });
      return this.state(bot.id);
    }
    return this.#enqueue(browser, async () => {
      check?.();
      this.#requireControl(browser, actor);
      this.#generation(browser, command.generation);
      if (command.kind === "tab") {
        for (const tab of browser.tabs.values()) { await this.#release(browser, tab); check?.(); }
        if (command.action === "new") { const tab = this.#newTab(browser); await tab.ready; check?.(); }
        else {
          const tab = this.#tab(browser, command.pageId);
          if (command.action === "close") await this.#destroyTab(browser, tab);
          else browser.active = tab.id;
        }
        this.#changed(browser); this.#present();
      } else {
        const tab = this.#tab(browser, command.pageId);
        await this.#release(browser, tab);
        check?.();
        if (command.kind === "navigate") {
          // loadURL's completion may wait for a dialog or slow subresource. State events report progress.
          void tab.contents.loadURL(nativeBrowserUrl(command.url)).catch((error: unknown) => this.#notice(browser, String(error)));
        } else if (command.kind === "history") {
          if (command.action === "reload") tab.contents.reload();
          else if (command.action === "back" && tab.contents.navigationHistory.canGoBack()) tab.contents.navigationHistory.goBack();
          else if (command.action === "forward" && tab.contents.navigationHistory.canGoForward()) tab.contents.navigationHistory.goForward();
        }
        this.#changed(browser);
      }
      return this.state(bot.id);
    });
  }

  async control(bot: NativeBrowserBot, control: "human" | "bot"): Promise<BrowserState> {
    await this.open(bot);
    const browser = this.#get(bot.id);
    browser.control = control;
    for (const tab of browser.tabs.values()) if (!tab.contents.isDestroyed()) tab.contents.emit("tjommie-browser-control", control);
    const handover = ++browser.handover;
    this.#changed(browser); this.#present();
    if (browser.dialog && control === "bot") await this.#dismiss(browser);
    return this.#enqueue(browser, async () => {
      for (const tab of browser.tabs.values()) await this.#release(browser, tab);
      if (browser.handover === handover) browser.handover = 0;
      this.#changed(browser); this.#present();
      if (control === "human" && browser.control === "human" && browser.handover === 0 && browser.active) {
        const tab = this.#tab(browser);
        if (this.#interactive(browser, tab)) tab.contents.focus();
      }
      return this.state(bot.id);
    });
  }

  async withAutomation<T>(bot: NativeBrowserBot, target: { generation?: string; pageId?: string }, task: (contents: WebContents) => Promise<T>): Promise<T> {
    await this.open(bot);
    const browser = this.#get(bot.id);
    this.#requireControl(browser, "bot");
    return this.#enqueue(browser, async () => {
      this.#requireControl(browser, "bot");
      if (target.generation) this.#generation(browser, target.generation);
      const tab = this.#tab(browser, target.pageId);
      await tab.ready;
      browser.injecting = true;
      try { return await task(tab.contents); }
      finally { browser.injecting = false; this.#changed(browser); }
    });
  }

  async mount(bot: NativeBrowserBot, raw: NativeBrowserMount): Promise<BrowserState> {
    const mount = NativeBrowserMount.parse(raw);
    await this.open(bot);
    const browser = this.#get(bot.id);
    if (!mount.visible) {
      browser.mount = mount;
      if (this.#mounted === bot.id) this.#mounted = null;
      this.#present();
      return this.control(bot, "bot");
    }
    if (this.#mounted && this.#mounted !== bot.id) {
      const previous = this.#get(this.#mounted);
      previous.mount = previous.mount ? { ...previous.mount, visible: false } : null;
      this.#mounted = null; this.#present();
      void this.control(previous.bot, "bot").catch((error: unknown) => this.#notice(previous, `Browser handback failed: ${String(error)}`));
    }
    // Geometry is owner UI state, not a page command. It must not leave a stale native surface
    // covering the app while a bot action is waiting. Control's own barrier still gates input.
    browser.mount = mount; this.#mounted = bot.id;
    this.#present(); this.#changed(browser);
    return this.state(bot.id);
  }

  setOccluded(value: boolean): void { this.#occluded = value; this.#present(); }

  hideBot(botId: string): void {
    const browser = this.#browsers.get(botId);
    if (!browser || browser.closing) return;
    if (browser.mount) browser.mount = { ...browser.mount, visible: false };
    if (this.#mounted === botId) this.#mounted = null;
    this.#present();
    void this.control(browser.bot, "bot").catch((error: unknown) => this.#notice(browser, `Browser handback failed: ${String(error)}`));
  }

  async resize(bot: NativeBrowserBot, width: number, height: number, check?: () => void): Promise<BrowserState> {
    check?.();
    if (!Number.isInteger(width) || !Number.isInteger(height) || width < 240 || width > 2560 || height < 180 || height > 1600) throw new Error("Browser dimensions must be 240–2560 by 180–1600.");
    await this.open(bot);
    const browser = this.#get(bot.id);
    return this.#enqueue(browser, async () => {
      check?.();
      this.#requireControl(browser, "bot");
      if (browser.mount?.visible) throw new Error("The visible browser follows its panel size. Resize the Tjommie panel, or hide it before requesting a fixed bot viewport.");
      const tab = this.#tab(browser), bounds = tab.view.getBounds();
      tab.view.setBounds({ ...bounds, width, height }); browser.viewportRevision++;
      this.#changed(browser); return this.state(bot.id);
    });
  }

  async closeBot(botId: string): Promise<void> {
    const existing = this.#closing.get(botId);
    if (existing) return existing;
    const closing = this.#closeBot(botId);
    this.#closing.set(botId, closing);
    try { await closing; } finally { this.#closing.delete(botId); }
  }

  async deleteProfile(botId: string): Promise<void> {
    const hash = basename(nativeBrowserProfile(this.#options.profileRoot, botId));
    return this.#deleteProfile(hash, botId);
  }

  async pruneProfiles(validBotIds: readonly string[]): Promise<void> {
    const valid = new Set(validBotIds.map((id) => basename(nativeBrowserProfile(this.#options.profileRoot, id))));
    const errors: unknown[] = [];
    for (const hash of await nativeProfileDirectories(this.#options.profileRoot)) {
      // Recheck live IDs immediately before reserving deletion, including opens during the scan.
      if (valid.has(hash) || this.#deleting.has(hash) || [...this.#browsers.keys()].some((id) => basename(nativeBrowserProfile(this.#options.profileRoot, id)) === hash)) continue;
      try { await this.#deleteProfile(hash); } catch (error) { errors.push(error); }
    }
    if (errors.length) throw new AggregateError(errors, "Could not safely prune all orphaned browser profiles.");
  }

  #deleteProfile(hash: string, botId?: string): Promise<void> {
    const existing = this.#deleting.get(hash);
    if (existing) return existing;
    this.#blockedProfiles.add(hash);
    // Reserve synchronously before the first filesystem await so no open can race cleanup.
    const task = Promise.resolve().then(async () => {
      if (botId) await boundedProfileCleanup(this.closeBot(botId), "close guest", 5000);
      let profile = this.#profiles.get(hash);
      const path = await checkedNativeProfile(this.#options.profileRoot, hash);
      // Unopened startup orphans can be deleted without creating a Session that locks them.
      if (!profile && (!path || await removeNativeProfile(this.#options.profileRoot, hash))) return;
      if (!profile) { profile = session.fromPath(path!); this.#profiles.set(hash, profile); }
      denyNativeBrowserPermissions(profile);
      await clearNativeProfile(profile);
      if (!await removeNativeProfile(this.#options.profileRoot, hash)) this.#options.onNotice?.({ botId: botId ?? hash,
        message: "Browser sign-in and storage were cleared. Locked profile files will be removed on a later startup." });
    });
    this.#deleting.set(hash, task);
    return task.then(() => { this.#blockedProfiles.delete(hash); }).finally(() => { this.#deleting.delete(hash); });
  }

  async #closeBot(botId: string): Promise<void> {
    const browser = this.#browsers.get(botId);
    if (!browser) return;
    browser.closing = true;
    if (this.#mounted === botId) this.#mounted = null;
    const errors: unknown[] = [];
    for (const item of browser.downloads) { try { item.cancel(); } catch (error) { errors.push(error); } }
    browser.downloads.clear();
    for (const tab of [...browser.tabs.values()]) {
      try { await this.#destroyTab(browser, tab); } catch (error) { errors.push(error); }
    }
    try { browser.session.flushStorageData(); await browser.session.cookies.flushStore(); } catch (error) { errors.push(error); }
    if (browser.downloadListener) browser.session.removeListener("will-download", browser.downloadListener);
    denyNativeBrowserPermissions(browser.session);
    this.#browsers.delete(botId); this.#present();
    if (errors.length) throw new AggregateError(errors, "Could not completely close the bot browser.");
  }

  async closeAll(): Promise<void> {
    this.#closed = true;
    const results = await Promise.allSettled([...this.#browsers.keys()].map((id) => this.closeBot(id)));
    if (this.#shield) { this.#shield.webContents.close({ waitForBeforeUnload: false }); this.#shield = null; }
    this.#options.window.removeListener("resize", this.#windowChanged);
    this.#options.window.removeListener("hide", this.#windowChanged);
    this.#options.window.removeListener("show", this.#windowChanged);
    const errors = results.flatMap((result) => result.status === "rejected" ? [result.reason] : []);
    if (errors.length) throw new AggregateError(errors, "Could not completely close native browsers.");
  }

  #get(botId: string): Browser {
    const browser = this.#browsers.get(botId);
    if (!browser) throw new Error("The bot browser is not open.");
    return browser;
  }
  #tab(browser: Browser, id = browser.active ?? undefined): Tab {
    const tab = id ? browser.tabs.get(id) : undefined;
    if (!tab || tab.contents.isDestroyed()) throw new Error("That browser tab is closed.");
    return tab;
  }
  #generation(browser: Browser, generation: string): void { if (browser.generation !== generation) throw new Error("The browser session changed; refresh its state."); }
  #requireControl(browser: Browser, actor: "human" | "bot"): void {
    if (browser.closing) throw new Error("The bot browser is closing.");
    if (browser.control !== actor || browser.handover > 0) throw new Error(actor === "bot" ? "The person is controlling this browser. Wait until they return control." : "Take control of the browser first.");
  }
  #enqueue<T>(browser: Browser, task: () => Promise<T>): Promise<T> {
    const result = browser.tail.then(async () => {
      if (browser.closing) throw new Error("The bot browser is closing.");
      browser.busy++; this.#changed(browser);
      try { return await task(); }
      finally { browser.busy--; if (!browser.closing) this.#changed(browser); }
    });
    browser.tail = result.then(() => {}, () => {});
    return result;
  }
  #changed(browser: Browser): void {
    browser.revision++;
    if (!browser.closing) this.#options.onState?.(this.state(browser.bot.id));
  }
  #notice(browser: Browser, message: string): void { this.#options.onNotice?.({ botId: browser.bot.id, message: message.slice(0, 2000) }); }

  #newTab(browser: Browser, inherited?: WebPreferences, supplied?: WebContents): Tab {
    // Chromium can supply an existing child contents for POST/opener-preserving window.open.
    // Replacing it loses that relationship and Electron rejects the created window.
    const view = supplied ? new WebContentsView({ webContents: supplied })
      : new WebContentsView({ webPreferences: { ...inherited, ...preferences(browser.session), preload: undefined } });
    view.setBackgroundColor("#ffffff"); view.setBounds({ x: 0, y: 44, width: 1000, height: 700 }); view.setVisible(false);
    const tab: Tab = { id: randomUUID(), view, contents: view.webContents, ready: Promise.resolve(), keys: new Set(), buttons: new Set(), point: { x: 0, y: 0 } };
    browser.tabs.set(tab.id, tab); browser.active = tab.id;
    this.#options.window.contentView.addChildView(view);
    this.#observe(browser, tab);
    tab.ready = (async () => {
      if (!inherited) await tab.contents.loadURL("about:blank");
      tab.contents.debugger.attach("1.3");
      await tab.contents.debugger.sendCommand("Page.enable");
    })();
    void tab.ready.catch((error: unknown) => this.#notice(browser, `Browser initialization failed: ${String(error)}`));
    this.#changed(browser); this.#present();
    return tab;
  }

  #observe(browser: Browser, tab: Tab): void {
    const contents = tab.contents;
    const changed = (): void => this.#changed(browser);
    contents.on("did-navigate", changed); contents.on("did-navigate-in-page", changed);
    contents.on("page-title-updated", changed); contents.on("did-start-loading", changed); contents.on("did-stop-loading", changed);
    const guard = (event: { preventDefault(): void }, url: string, isMainFrame: boolean): void => { try { nativeBrowserFrameUrl(url, isMainFrame); } catch { event.preventDefault(); this.#notice(browser, "That page tried to open an unsupported browser address."); } };
    contents.on("will-navigate", (event, url) => guard(event, url, true));
    contents.on("will-frame-navigate", (event) => guard(event, event.url, event.isMainFrame));
    contents.on("will-redirect", (event) => guard(event, event.url, event.isMainFrame));
    contents.on("will-attach-webview", (event) => event.preventDefault());
    denyNativeBrowserBluetooth(contents);
    contents.on("before-input-event", (event, input) => {
      if (browser.injecting) return;
      if (!this.#interactive(browser, tab)) { event.preventDefault(); return; }
      if (input.type === "keyDown" && input.key === "Tab" && input.shift) {
        event.preventDefault(); this.#options.window.webContents.focus(); this.#options.onFocusReturn?.(browser.bot.id); return;
      }
      if (input.type === "keyDown") tab.keys.add(input.key);
      if (input.type === "keyUp") tab.keys.delete(input.key);
    });
    contents.on("before-mouse-event", (event, input) => {
      if (browser.injecting) return;
      if (!this.#interactive(browser, tab)) { event.preventDefault(); return; }
      tab.point = { x: input.x, y: input.y };
      if (input.type === "mouseDown" && input.button) tab.buttons.add(input.button);
      if (input.type === "mouseUp" && input.button) tab.buttons.delete(input.button);
    });
    contents.on("blur", () => { if (!browser.dialog) void this.#release(browser, tab).catch(() => {}); });
    contents.on("destroyed", () => {
      browser.tabs.delete(tab.id);
      if (browser.active === tab.id) browser.active = [...browser.tabs.keys()].at(-1) ?? null;
      if (browser.dialog?.pageId === tab.id) browser.dialog = null;
      if (!browser.closing) { this.#changed(browser); this.#present(); }
    });
    contents.debugger.on("message", (_event, method, params: unknown) => {
      if (method === "Page.javascriptDialogOpening" && params && typeof params === "object") {
        const data = params as Record<string, unknown>;
        browser.dialog = { pageId: tab.id, type: String(data.type ?? "alert"), message: String(data.message ?? "").slice(0, 10000), defaultValue: String(data.defaultPrompt ?? "").slice(0, 10000) };
        this.#changed(browser);
      } else if (method === "Page.javascriptDialogClosed") {
        browser.dialog = null; this.#changed(browser);
        void this.#release(browser, tab).catch(() => {});
      }
    });
    contents.setWindowOpenHandler((details) => {
      try { nativeBrowserUrl(details.url); } catch { return { action: "deny" }; }
      if (browser.tabs.size >= 30 || browser.closing) { this.#notice(browser, "The browser tab limit is 30."); return { action: "deny" }; }
      return { action: "allow", overrideBrowserWindowOptions: { show: false, webPreferences: preferences(browser.session) },
        createWindow: (options) => this.#newTab(browser, options.webPreferences, (options as typeof options & { webContents?: WebContents }).webContents).contents };
    });
  }

  #interactive(browser: Browser, tab: Tab): boolean {
    return browser.control === "human" && browser.handover === 0 && browser.active === tab.id && this.#mounted === browser.bot.id && !this.#occluded && !!browser.mount?.visible && this.#options.window.isVisible();
  }
  async #release(browser: Browser, tab: Tab): Promise<void> {
    if (tab.contents.isDestroyed() || browser.dialog?.pageId === tab.id) return;
    const prior = browser.injecting; browser.injecting = true;
    try {
      for (const key of tab.keys) tab.contents.sendInputEvent({ type: "keyUp", keyCode: key });
      for (const button of tab.buttons) tab.contents.sendInputEvent({ type: "mouseUp", ...tab.point, button, clickCount: 1 });
      tab.keys.clear(); tab.buttons.clear();
    } finally { browser.injecting = prior; }
  }
  async #dismiss(browser: Browser): Promise<void> {
    if (!browser.dialog) return;
    const tab = browser.tabs.get(browser.dialog.pageId);
    if (tab && !tab.contents.isDestroyed()) await tab.contents.debugger.sendCommand("Page.handleJavaScriptDialog", { accept: false });
  }
  async #destroyTab(browser: Browser, tab: Tab): Promise<void> {
    if (browser.dialog?.pageId === tab.id) await this.#dismiss(browser).catch(() => {});
    if (!this.#options.window.isDestroyed()) this.#options.window.contentView.removeChildView(tab.view);
    if (!tab.contents.isDestroyed()) tab.contents.close({ waitForBeforeUnload: false });
    browser.tabs.delete(tab.id);
    if (browser.active === tab.id) browser.active = [...browser.tabs.keys()].at(-1) ?? null;
  }

  #present(): void {
    const owner = this.#options.window;
    if (owner.isDestroyed()) return;
    const [width, height] = owner.getContentSize();
    this.#shield?.setVisible(false);
    for (const browser of this.#browsers.values()) {
      const mount = browser.mount;
      const bounds = mount ? nativeBrowserBounds(mount.bounds, width, height) : null;
      const visible = !this.#occluded && owner.isVisible() && this.#mounted === browser.bot.id && !!mount?.visible && !!bounds?.width && !!bounds.height;
      for (const tab of browser.tabs.values()) {
        if (tab.contents.isDestroyed()) continue;
        const show = visible && browser.active === tab.id;
        if (show && bounds) {
          const previous = tab.view.getBounds();
          tab.view.setBounds(bounds);
          if (previous.width !== bounds.width || previous.height !== bounds.height) browser.viewportRevision++;
        }
        tab.view.setVisible(show);
      }
      if (visible && bounds && (browser.control !== "human" || browser.handover > 0)) {
        const shield = this.#getShield();
        owner.contentView.addChildView(shield); shield.setBounds(bounds); shield.setVisible(true);
        // Move retained focus out of the remote page while the bot owns input.
        owner.webContents.focus();
      }
    }
  }
  #getShield(): WebContentsView {
    if (!this.#shield) {
      const shield = new WebContentsView({ webPreferences: { nodeIntegration: false, contextIsolation: true, sandbox: true, webSecurity: true, webviewTag: false, partition: `native-browser-shield-${randomUUID()}` } });
      shield.setBackgroundColor("#12000000");
      shield.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
      void shield.webContents.loadURL("data:text/html,<html><body style='margin:0;background:transparent;cursor:default'></body></html>");
      this.#shield = shield;
    }
    return this.#shield;
  }

  #configureSession(browser: Browser): void {
    denyNativeBrowserPermissions(browser.session, (permission) => this.#notice(browser, `Browser permission '${permission}' is unavailable in Tjommie.`));
    browser.downloadListener = (event, item, contents) => {
      if (browser.closing || ![...browser.tabs.values()].some((tab) => tab.contents === contents)) { event.preventDefault(); return; }
      if (item.getTotalBytes() > 100 * 1024 * 1024) { event.preventDefault(); this.#notice(browser, "Downloads are limited to 100 MB."); return; }
      try {
        const directory = containedFolder(browser.bot.homeFolder, "browser-files");
        const name = basename(item.getFilename()).replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").slice(0, 160) || "download";
        item.setSavePath(join(directory, `${randomUUID()}-${name}`));
        browser.downloads.add(item);
        item.on("updated", () => { if (item.getReceivedBytes() > 100 * 1024 * 1024) item.cancel(); });
        item.once("done", (_event, status) => { browser.downloads.delete(item); this.#notice(browser, status === "completed" ? `Downloaded ${name} to the bot's browser-files folder.` : `Download ${status}: ${name}`); });
      } catch (error) { event.preventDefault(); this.#notice(browser, `Download failed: ${String(error)}`); }
    };
    browser.session.on("will-download", browser.downloadListener);
  }
}
