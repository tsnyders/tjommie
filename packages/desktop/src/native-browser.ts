import { BrowserWindow, WebContentsView, session, nativeImage, type DownloadItem, type Session, type WebContents, type WebPreferences } from "electron";
import { randomUUID } from "node:crypto";
import { lstatSync, mkdirSync, realpathSync } from "node:fs";
import { basename, join, relative, resolve } from "node:path";
import type { BrowserFrame, BrowserState } from "../../protocol/src/browser.ts";
import { NativeBrowserCommand, NativeBrowserMount, type NativeBrowserUiCommand, type NativeBrowserToolCommand, type NativeBrowserRemoteCommand } from "../../protocol/src/native-browser.ts";
import { NativeBrowserAccess } from "./native-browser-access.ts";
import { captureNativeBrowserFrame } from "./native-browser-frame.ts";
import { nativeBrowserBounds, nativeBrowserFrameUrl, nativeBrowserProfile, nativeBrowserUrl } from "./native-browser-policy.ts";
import { denyNativeBrowserBluetooth, denyNativeBrowserPermissions } from "./native-browser-permissions.ts";
import { boundedProfileCleanup, checkedNativeProfile, clearNativeProfile, nativeProfileDirectories, removeNativeProfile } from "./native-browser-profiles.ts";
import { LoginOrigin, type VaultFillTarget } from "../../protocol/src/vault.ts";
import { prepareNativeLogin, fillNativeLogin, clearNativeLogin, type NativeLoginFields } from "./native-browser-login.ts";
import type { VaultSecret } from "./vault-storage.ts";

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
  fixedViewport: boolean;
  keys: Set<string>; buttons: Set<"left" | "middle" | "right">; point: { x: number; y: number };
};
type Browser = {
  bot: NativeBrowserBot; generation: string; session: Session; tabs: Map<string, Tab>; active: string | null;
  access: NativeBrowserAccess; revision: number; viewportRevision: number; busy: number; handover: number; frameSequence: number;
  tail: Promise<void>; closing: boolean; injecting: boolean; dialog: BrowserState["dialog"];
  mount: NativeBrowserMount | null;
  downloads: Set<DownloadItem>;
  login?: { fields: NativeLoginFields; privateEpoch: number; controlEpoch: number; pageId: string };
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
async function bounded<T>(operation: Promise<T>, milliseconds = 5000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([operation, new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error("The native browser operation timed out.")), milliseconds); })]); }
  finally { clearTimeout(timer); }
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
  #controllers = new Map<string, { closed: boolean; timer?: ReturnType<typeof setTimeout> }>();
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
        access: new NativeBrowserAccess(), revision: 0, viewportRevision: 0, busy: 0, handover: 0, frameSequence: 0, tail: Promise.resolve(), closing: false,
        injecting: false, dialog: null, mount: null, downloads: new Set() };
      this.#browsers.set(bot.id, browser);
      this.#configureSession(browser);
      this.#newTab(browser);
    } else if (resolve(browser.bot.homeFolder) !== resolve(bot.homeFolder)) throw new Error("The browser bot home changed; close its browser before reopening it.");
    if (browser.closing) throw new Error("The bot browser is closing.");
    await Promise.all([...browser.tabs.values()].map((tab) => tab.ready));
    return this.state(bot.id);
  }

  state(botId: string, owner = "local"): BrowserState {
    const browser = this.#get(botId);
    return { botId, generation: browser.generation, control: browser.access.control, controlEpoch: browser.access.controlEpoch,
      privateEpoch: browser.access.privateEpoch, privateActive: browser.access.privateActive, canControl: browser.access.owner === owner, revision: browser.revision,
      viewportRevision: browser.viewportRevision, busy: browser.busy > 0 || browser.handover > 0, activePageId: browser.active,
      tabs: [...browser.tabs.values()].filter((tab) => !tab.contents.isDestroyed()).map((tab) => ({ id: tab.id, url: browser.access.privateActive ? "about:blank" : tab.contents.getURL() || "about:blank", title: browser.access.privateActive ? "Private interaction" : tab.contents.getTitle() })),
      dialog: !browser.access.privateActive && browser.dialog ? { ...browser.dialog } : null };
  }

  async execute(bot: NativeBrowserBot, raw: NativeBrowserCommand): Promise<unknown> {
    const command = NativeBrowserCommand.parse(raw);
    if (command.kind === "remote") return this.remote(bot, command);
    if (command.kind === "close") { await this.closeBot(bot.id); return {}; }
    if (command.kind === "delete_profile") { await this.deleteProfile(bot.id); return {}; }
    if (command.kind === "cancel_tools") throw new Error("Tool cancellation belongs to the private desktop host bridge.");
    if (command.kind === "tool") {
      if (!this.#options.automation) throw new Error("Native browser automation is unavailable.");
      return this.#options.automation(this, bot, command);
    }
    return this.act(bot, command, "human");
  }

  async act(bot: NativeBrowserBot, command: NativeBrowserUiCommand, actor: "human" | "bot", check?: () => void, owner = "local"): Promise<BrowserState> {
    check?.();
    await this.open(bot);
    check?.();
    const browser = this.#get(bot.id);
    if (command.kind === "open" || command.kind === "state") return this.state(bot.id, owner);
    if (command.kind === "control") return this.control(bot, command.control, owner);
    const ticket = browser.access.ticket();
    const authorized = (): void => { check?.(); if (actor === "human") browser.access.require(owner, ticket.controlEpoch); else browser.access.check(ticket, true); };
    authorized();
    this.#generation(browser, command.generation);
    // Dialog dismissal is an unblocker and must never wait behind a blocked browser action.
    if (command.kind === "dialog") {
      authorized();
      const tab = this.#tab(browser, command.pageId);
      if (!browser.dialog || browser.dialog.pageId !== tab.id) throw new Error("There is no dialog in that tab.");
      await tab.contents.debugger.sendCommand("Page.handleJavaScriptDialog", { accept: command.accept, promptText: command.promptText });
      authorized(); return this.state(bot.id, owner);
    }
    return this.#enqueue(browser, async () => {
      authorized();
      this.#requireControl(browser, actor);
      this.#generation(browser, command.generation);
      if (command.kind === "tab") {
        for (const tab of browser.tabs.values()) { await this.#release(browser, tab); check?.(); }
        if (command.action === "new") {
          const tab = this.#newTab(browser); await tab.ready;
          if (actor === "bot") check?.();
          else if (browser.access.owner !== owner || browser.access.privateActive) throw new Error("Browser control changed while opening the tab.");
        }
        else {
          const tab = this.#tab(browser, command.pageId);
          if (command.action === "close") await this.#destroyTab(browser, tab);
          else browser.active = tab.id;
        }
        if (browser.access.owner !== null) browser.access.changedPage(); this.#changed(browser); this.#present();
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
      return this.state(bot.id, owner);
    });
  }

  async control(bot: NativeBrowserBot, control: "human" | "bot", owner = "local", explicit = true, check?: () => void): Promise<BrowserState> {
    await this.open(bot);
    check?.();
    const browser = this.#get(bot.id);
    if (control === "human") browser.access.take(owner);
    else {
      if (browser.access.privateActive && explicit) throw new Error("Finish the private interaction before explicitly resuming the bot.");
      if (!browser.access.release(owner)) {
        if (explicit && browser.access.owner !== null) throw new Error("Another device is controlling this browser.");
        return this.state(bot.id, owner);
      }
    }
    for (const tab of browser.tabs.values()) if (!tab.contents.isDestroyed()) tab.contents.emit("tjommie-browser-control", browser.access.control);
    const handover = ++browser.handover;
    this.#changed(browser); this.#present();
    if (browser.dialog && control === "bot") await this.#dismiss(browser);
    // Ownership and held-input release cannot wait behind a browser tool or a frame.
    {
      for (const tab of browser.tabs.values()) await this.#release(browser, tab);
      if (browser.handover === handover) browser.handover = 0;
      this.#changed(browser); this.#present();
      if (control === "human" && browser.access.owner === owner && browser.handover === 0 && browser.active) {
        const tab = this.#tab(browser);
        if (this.#interactive(browser, tab)) tab.contents.focus();
      }
      return this.state(bot.id, owner);
    }
  }

  async withAutomation<T>(bot: NativeBrowserBot, target: { generation?: string; pageId?: string }, task: (contents: WebContents) => Promise<T>): Promise<T> {
    await this.open(bot);
    const browser = this.#get(bot.id);
    this.#requireControl(browser, "bot");
    const ticket = browser.access.ticket();
    return this.#enqueue(browser, async () => {
      browser.access.check(ticket, true);
      this.#requireControl(browser, "bot");
      if (target.generation) this.#generation(browser, target.generation);
      const tab = this.#tab(browser, target.pageId);
      await tab.ready;
      browser.injecting = true;
      try { const value = await task(tab.contents); browser.access.check(ticket, true); return value; }
      finally { browser.injecting = false; this.#changed(browser); }
    });
  }

  async remote(bot: NativeBrowserBot, command: NativeBrowserRemoteCommand): Promise<unknown> {
    const { controllerId: owner, action } = command;
    let lifetime = this.#controllers.get(owner);
    if (!lifetime) { lifetime = { closed: false }; this.#controllers.set(owner, lifetime); }
    if (action.kind === "disconnect") {
      lifetime.closed = true;
      clearTimeout(lifetime.timer);
      lifetime.timer = setTimeout(() => this.#controllers.delete(owner), 60_000);
      lifetime.timer.unref();
      const browser = this.#browsers.get(bot.id);
      if (browser && !browser.closing) await this.control(bot, "bot", owner, false);
      return {};
    }
    const connection = (): void => { if (lifetime.closed) throw new Error("This browser controller disconnected."); };
    connection();
    await this.open(bot);
    connection();
    const browser = this.#get(bot.id);
    if (action.kind === "state") return this.state(bot.id, owner);
    if (action.kind === "frame") return this.frame(bot, owner, connection, action.thumbnail);
    if ("generation" in action && action.generation) this.#generation(browser, action.generation);
    if (action.kind === "control") {
      if (action.control === "bot") {
        if (action.controlEpoch === undefined) throw new Error("Use the current browser control epoch.");
        browser.access.require(owner, action.controlEpoch);
      }
      return this.control(bot, action.control, owner, !action.releaseOnly, connection);
    }
    const check = (): void => {
      connection();
      this.#generation(browser, action.generation);
      browser.access.require(owner, action.controlEpoch);
      if (browser.access.privateActive) throw new Error("Remote input is disabled during a private interaction.");
    };
    check();
    if (action.kind !== "input" && action.kind !== "resize") {
      const { controlEpoch: _epoch, ...ui } = action;
      return this.act(bot, ui, "human", check, owner);
    }
    return this.#enqueue(browser, async () => {
      check();
      const tab = this.#tab(browser, action.pageId);
      if (browser.active !== tab.id) throw new Error("The active browser page changed.");
      if (action.kind === "resize") {
        if (browser.mount?.visible && this.#options.window.isVisible()) throw new Error("The browser follows the local Computer panel size while that panel is visible.");
        await this.#release(browser, tab); check();
        await this.#resizeTab(tab, action.width, action.height); check();
        browser.viewportRevision++; this.#changed(browser);
        return this.state(bot.id, owner);
      }
      if (action.viewportRevision !== browser.viewportRevision) throw new Error("The browser viewport changed; wait for its current frame.");
      const input = action.input, { width, height } = tab.view.getBounds();
      if ((input.kind === "mouse" || input.kind === "wheel") && (input.x >= width || input.y >= height)) throw new Error("Browser input must be inside the current viewport.");
      browser.injecting = true;
      try {
        const modifiers: Array<"shift" | "control" | "alt" | "meta"> = [];
        if (tab.keys.has("Shift")) modifiers.push("shift");
        if (tab.keys.has("Control")) modifiers.push("control");
        if (tab.keys.has("Alt")) modifiers.push("alt");
        if (tab.keys.has("Meta")) modifiers.push("meta");
        if (input.kind === "text") await bounded(tab.contents.insertText(input.text));
        else if (input.kind === "key") {
          const aliases: Record<string, string> = { " ": "Space", ArrowLeft: "Left", ArrowRight: "Right", ArrowUp: "Up", ArrowDown: "Down" };
          const key = aliases[input.key] ?? input.key;
          tab.contents.sendInputEvent({ type: input.action === "down" ? "keyDown" : "keyUp", keyCode: key, modifiers });
          if (input.action === "down" && input.key.length === 1 && !modifiers.some((value) => value === "control" || value === "alt" || value === "meta")) tab.contents.sendInputEvent({ type: "char", keyCode: input.key, modifiers });
          if (input.action === "down") tab.keys.add(key); else tab.keys.delete(key);
        } else if (input.kind === "wheel") tab.contents.sendInputEvent({ type: "mouseWheel", x: Math.round(input.x), y: Math.round(input.y), deltaX: input.deltaX, deltaY: input.deltaY, canScroll: true, modifiers });
        else {
          tab.point = { x: Math.round(input.x), y: Math.round(input.y) };
          if (input.action !== "move" && !input.button) throw new Error("A pointer press or release needs a button.");
          tab.contents.sendInputEvent({ type: input.action === "move" ? "mouseMove" : input.action === "down" ? "mouseDown" : "mouseUp", ...tab.point, button: input.button, clickCount: 1, modifiers });
          if (input.button && input.action === "down") tab.buttons.add(input.button);
          if (input.button && input.action === "up") tab.buttons.delete(input.button);
        }
        check(); return {};
      } finally { browser.injecting = false; }
    });
  }

  async frame(bot: NativeBrowserBot, owner = "local", authorized?: () => void, thumbnail = false): Promise<{ state: BrowserState; frame: BrowserFrame | null }> {
    authorized?.();
    await this.open(bot);
    const browser = this.#get(bot.id), ticket = browser.access.ticket();
    if (browser.access.privateActive || !browser.active) return { state: this.state(bot.id, owner), frame: null };
    const pageId = browser.active, viewportRevision = browser.viewportRevision;
    const check = (): void => {
      authorized?.();
      browser.access.check(ticket);
      if (browser.closing || browser.active !== pageId || browser.viewportRevision !== viewportRevision) throw new Error("The browser page or viewport changed; this frame was discarded.");
    };
    return this.#enqueue(browser, async () => {
      check(); const tab = this.#tab(browser, pageId); await tab.ready; check();
      let data = await captureNativeBrowserFrame(tab.contents.debugger, check);
      const image = nativeImage.createFromBuffer(Buffer.from(data, "base64")); let size = image.getSize();
      if (size.width < 1 || size.height < 1 || size.width > 4096 || size.height > 4096) throw new Error("The browser returned an invalid frame size.");
      if (thumbnail) {
        const scale = Math.min(1, 320 / size.width, 240 / size.height);
        const small = image.resize({ width: Math.max(1, Math.round(size.width * scale)), height: Math.max(1, Math.round(size.height * scale)) });
        const bytes = small.toJPEG(50); if (bytes.length > 64 * 1024) throw new Error("The browser thumbnail exceeded 64 KiB.");
        data = bytes.toString("base64"); size = small.getSize();
      }
      const raw: unknown = await bounded(tab.contents.debugger.sendCommand("Page.getLayoutMetrics")); check();
      const metrics = raw && typeof raw === "object" && "cssVisualViewport" in raw ? raw.cssVisualViewport : null;
      if (!metrics || typeof metrics !== "object" || !("clientWidth" in metrics) || !("clientHeight" in metrics) || typeof metrics.clientWidth !== "number" || typeof metrics.clientHeight !== "number") throw new Error("The browser did not report its CSS viewport.");
      const viewport = { width: Math.round(metrics.clientWidth), height: Math.round(metrics.clientHeight) };
      if (viewport.width < 1 || viewport.height < 1 || viewport.width > 4096 || viewport.height > 4096) throw new Error("The browser CSS viewport exceeds its limit.");
      return { state: { ...this.state(bot.id, owner), busy: browser.busy > 1 || browser.handover > 0 }, frame: { data, mime: "image/jpeg", width: size.width, height: size.height,
        viewportWidth: viewport.width, viewportHeight: viewport.height, pageId, viewportRevision, sequence: ++browser.frameSequence,
        streamId: `${browser.generation}:${pageId}:${ticket.privateEpoch}`, ...ticket } };
    });
  }

  async beginPrivate(bot: NativeBrowserBot, owner: string, controlEpoch: number): Promise<number> {
    await this.open(bot);
    const browser = this.#get(bot.id), epoch = browser.access.beginPrivate(owner, controlEpoch);
    for (const tab of browser.tabs.values()) {
      tab.contents.emit("tjommie-browser-control", "human");
      await this.#release(browser, tab);
      for (const method of ["Page.stopScreencast", "Runtime.terminateExecution", "Network.disable", "Runtime.disable"]) {
        let timer: ReturnType<typeof setTimeout> | undefined;
        try { await Promise.race([tab.contents.debugger.sendCommand(method), new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error("The browser could not enter private mode safely.")), 1000); })]); }
        finally { clearTimeout(timer); }
      }
    }
    this.#changed(browser); this.#present();
    return epoch;
  }

  async endPrivate(bot: NativeBrowserBot, owner: string, privateEpoch: number, safeToResume: () => Promise<void>): Promise<BrowserState> {
    const browser = this.#get(bot.id);
    // The trusted fill host must verify cleared/submitted fields before explicit owner resume.
    await safeToResume();
    browser.access.endPrivate(owner, privateEpoch);
    this.#changed(browser); return this.state(bot.id, owner);
  }

  vaultDestination(bot: NativeBrowserBot, target: VaultFillTarget, owner: string): string {
    const browser = this.#get(bot.id); this.#generation(browser, target.generation);
    browser.access.require(owner, target.controlEpoch);
    if (browser.access.privateActive || browser.active !== target.pageId || target.botId !== bot.id) throw new Error("The login destination changed. Refresh the browser before filling.");
    const url = new URL(this.#tab(browser, target.pageId).contents.getURL());
    return LoginOrigin.parse(url.protocol === "https:" && !url.username && !url.password ? url.origin : "");
  }

  // These methods are owner-main-process capabilities, never browser tools or renderer commands.
  async beginVault(bot: NativeBrowserBot, target: VaultFillTarget, owner: string, origin: string, check: () => void): Promise<number> {
    check(); if (this.vaultDestination(bot, target, owner) !== origin) throw new Error("This login belongs to a different HTTPS origin.");
    const browser = this.#get(bot.id);
    // A host-native confirmation transfers remote control to the person at the host.
    if (owner !== "local") { browser.access.release(owner); browser.access.take("local"); }
    const epoch = await this.beginPrivate(bot, "local", browser.access.controlEpoch);
    const controlEpoch = browser.access.controlEpoch;
    const current = (): void => {
      check();
      if (browser.closing || browser.access.privateEpoch !== epoch || !browser.access.privateActive || browser.access.owner !== "local" || browser.access.controlEpoch !== controlEpoch || browser.active !== target.pageId) throw new Error("The private login destination changed.");
    };
    const tab = this.#tab(browser, target.pageId); current();
    const fields = await prepareNativeLogin(tab.contents.debugger, origin, current); current();
    browser.login = { fields, privateEpoch: epoch, controlEpoch, pageId: tab.id }; return epoch;
  }

  async writeVault(bot: NativeBrowserBot, epoch: number, secret: VaultSecret, check: () => void): Promise<void> {
    const browser = this.#get(bot.id), login = browser.login;
    if (!login || login.privateEpoch !== epoch) throw new Error("The private login interaction changed.");
    const current = (): void => {
      check();
      if (browser.closing || !browser.access.privateActive || browser.access.privateEpoch !== epoch || browser.access.owner !== "local" || browser.access.controlEpoch !== login.controlEpoch || browser.active !== login.pageId) throw new Error("The private login destination changed.");
    };
    current(); await fillNativeLogin(this.#tab(browser, login.pageId).contents.debugger, login.fields, secret, current); current();
  }

  async finishVault(bot: NativeBrowserBot, generation: string, epoch: number, mode: "clear" | "close", check: () => void): Promise<void> {
    const browser = this.#get(bot.id); this.#generation(browser, generation); check();
    if (!browser.access.privateActive || browser.access.privateEpoch !== epoch) throw new Error("The private login interaction changed.");
    if (mode === "close") { await this.closeBot(bot.id); return; }
    const login = browser.login;
    if (!login || login.privateEpoch !== epoch) throw new Error("Close this private browser before resuming; its fields cannot be verified.");
    browser.access.take("local");
    const current = (): void => { check(); if (browser.closing || !browser.access.privateActive || browser.access.privateEpoch !== epoch) throw new Error("The private login interaction changed."); };
    await this.endPrivate(bot, "local", epoch, () => clearNativeLogin(this.#tab(browser, login.pageId).contents.debugger, login.fields, current));
    browser.login = undefined;
    // The person still explicitly returns browser control after reviewing the cleared page.
  }

  async mount(bot: NativeBrowserBot, raw: NativeBrowserMount): Promise<BrowserState> {
    const mount = NativeBrowserMount.parse(raw);
    await this.open(bot);
    const browser = this.#get(bot.id);
    if (!mount.visible) {
      browser.mount = mount;
      if (this.#mounted === bot.id) this.#mounted = null;
      this.#present();
      return this.control(bot, "bot", "local", false);
    }
    if (this.#mounted && this.#mounted !== bot.id) {
      const previous = this.#get(this.#mounted);
      previous.mount = previous.mount ? { ...previous.mount, visible: false } : null;
      this.#mounted = null; this.#present();
      void this.control(previous.bot, "bot", "local", false).catch((error: unknown) => this.#notice(previous, `Browser handback failed: ${String(error)}`));
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
    void this.control(browser.bot, "bot", "local", false).catch((error: unknown) => this.#notice(browser, `Browser handback failed: ${String(error)}`));
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
      const tab = this.#tab(browser);
      await this.#resizeTab(tab, width, height); check?.(); browser.viewportRevision++;
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
    for (const controller of this.#controllers.values()) clearTimeout(controller.timer);
    this.#controllers.clear();
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
    if (browser.access.control !== actor || browser.handover > 0 || actor === "bot" && browser.access.privateActive) throw new Error(actor === "bot" ? "The person is controlling this browser. Wait until they return control." : "Take control of the browser first.");
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
  #notice(browser: Browser, message: string): void { if (!browser.access.privateActive) this.#options.onNotice?.({ botId: browser.bot.id, message: message.slice(0, 2000) }); }

  #newTab(browser: Browser, inherited?: WebPreferences, supplied?: WebContents): Tab {
    // Chromium can supply an existing child contents for POST/opener-preserving window.open.
    // Replacing it loses that relationship and Electron rejects the created window.
    const view = supplied ? new WebContentsView({ webContents: supplied })
      : new WebContentsView({ webPreferences: { ...inherited, ...preferences(browser.session), preload: undefined } });
    view.setBackgroundColor("#ffffff"); view.setBounds({ x: 0, y: 44, width: 1000, height: 700 }); view.setVisible(false);
    const tab: Tab = { id: randomUUID(), view, contents: view.webContents, ready: Promise.resolve(), fixedViewport: false, keys: new Set(), buttons: new Set(), point: { x: 0, y: 0 } };
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
    const pageChanged = (): void => {
      if (browser.access.owner !== null) browser.access.changedPage();
      void this.#release(browser, tab).catch(() => {}); this.#changed(browser);
    };
    contents.on("did-start-navigation", (_event, _url, _inPlace, mainFrame) => { if (mainFrame) pageChanged(); });
    contents.on("did-navigate", changed); contents.on("did-navigate-in-page", pageChanged);
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
    return browser.access.owner === "local" && browser.handover === 0 && browser.active === tab.id && this.#mounted === browser.bot.id && !this.#occluded && !!browser.mount?.visible && this.#options.window.isVisible();
  }
  async #resizeTab(tab: Tab, width: number, height: number): Promise<void> {
    await tab.ready;
    // Hidden native views retain their previous renderer size until explicitly resized through CDP.
    await bounded(tab.contents.debugger.sendCommand("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 0, mobile: false }));
    tab.fixedViewport = true; tab.view.setBounds({ ...tab.view.getBounds(), width, height });
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
          if (tab.fixedViewport) {
            tab.fixedViewport = false; browser.viewportRevision++;
            void tab.contents.debugger.sendCommand("Emulation.clearDeviceMetricsOverride").then(() => {
              browser.viewportRevision++; this.#changed(browser);
            }).catch(() => this.#notice(browser, "The native browser viewport could not be restored."));
          }
        }
        tab.view.setVisible(show);
      }
      if (visible && bounds && (browser.access.owner !== "local" || browser.handover > 0)) {
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
