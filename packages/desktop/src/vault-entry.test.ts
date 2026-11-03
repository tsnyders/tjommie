import type { BrowserWindow } from "electron";
import type { EventEmitter } from "node:events";
import { pathToFileURL } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NativeVaultEntry } from "./vault-entry.ts";

type Invoke = (event: Electron.IpcMainInvokeEvent, raw?: unknown) => unknown;
type WindowFixture = EventEmitter & {
  options: Electron.BrowserWindowConstructorOptions; destroyed: boolean; loaded: string;
  webContents: EventEmitter & { mainFrame: { url: string }; send: ReturnType<typeof vi.fn>; setWindowOpenHandler: ReturnType<typeof vi.fn> };
  destroy(): void; show: ReturnType<typeof vi.fn>; setContentProtection: ReturnType<typeof vi.fn>;
};
const fixtures = vi.hoisted(() => ({ handlers: new Map<string, Invoke>(), windows: [] as WindowFixture[], profiles: [] as { clearStorageData: ReturnType<typeof vi.fn>; setPermissionRequestHandler: ReturnType<typeof vi.fn>; setPermissionCheckHandler: ReturnType<typeof vi.fn> }[] }));
vi.mock("electron", async () => {
  const { EventEmitter } = await import("node:events");
  return {
    ipcMain: { handle: (name: string, invoke: Invoke) => fixtures.handlers.set(name, invoke), removeHandler: (name: string) => fixtures.handlers.delete(name) },
    session: { fromPartition: vi.fn(() => { const profile = { clearStorageData: vi.fn(async () => {}), setPermissionRequestHandler: vi.fn(), setPermissionCheckHandler: vi.fn() }; fixtures.profiles.push(profile); return profile; }) },
    BrowserWindow: class extends EventEmitter {
      readonly options: Electron.BrowserWindowConstructorOptions; destroyed = false; loaded = "";
      readonly webContents = Object.assign(new EventEmitter(), { mainFrame: { url: "" }, send: vi.fn(), setWindowOpenHandler: vi.fn() });
      show = vi.fn(); setContentProtection = vi.fn(); removeMenu = vi.fn();
      constructor(options: Electron.BrowserWindowConstructorOptions) { super(); this.options = options; fixtures.windows.push(this); }
      isDestroyed(): boolean { return this.destroyed; }
      destroy(): void { this.destroyed = true; this.emit("closed"); }
      close(): void { this.destroy(); }
      async loadFile(path: string): Promise<void> { this.loaded = path; this.webContents.mainFrame.url = pathToFileURL(path).href; }
    },
  };
});
afterEach(() => { for (const window of fixtures.windows.splice(0)) window.destroy(); fixtures.profiles.splice(0); fixtures.handlers.clear(); });
const parent = (): BrowserWindow => ({ isDestroyed: () => false }) as BrowserWindow;
const windowNow = (): WindowFixture => { const window = fixtures.windows.at(-1); if (!window) throw new Error("Missing native fixture window."); return window; };
const trusted = (window: WindowFixture): Electron.IpcMainInvokeEvent => ({ sender: window.webContents, senderFrame: window.webContents.mainFrame }) as unknown as Electron.IpcMainInvokeEvent;
const invoke = (name: "ready" | "submit", event: Electron.IpcMainInvokeEvent, raw?: unknown): unknown => {
  const handler = fixtures.handlers.get(`tjommie:private-login-${name}`); if (!handler) throw new Error("Missing native fixture handler."); return handler(event, raw);
};

describe("trusted native Logins entry", () => {
  it("accepts secret input only from its exact native main frame and sends only blank-entry metadata", async () => {
    const entry = new NativeVaultEntry(parent()), operation = entry.entry("bot-1", null, new AbortController().signal), window = windowNow();
    expect(window.options.webPreferences).toMatchObject({ sandbox: true, contextIsolation: true, nodeIntegration: false, webSecurity: true, devTools: false, webviewTag: false });
    expect(window.setContentProtection).toHaveBeenCalledWith(true);
    expect(() => invoke("submit", { ...trusted(window), sender: {} } as Electron.IpcMainInvokeEvent, true)).toThrow("Not a native Logins");
    expect(() => invoke("submit", { ...trusted(window), senderFrame: { url: window.webContents.mainFrame.url } } as unknown as Electron.IpcMainInvokeEvent, true)).toThrow("Not a native Logins");
    const original = window.webContents.mainFrame.url; window.webContents.mainFrame.url = "https://example.test";
    expect(() => invoke("ready", trusted(window))).toThrow("Not a native Logins"); window.webContents.mainFrame.url = original;
    invoke("ready", trusted(window));
    expect(window.webContents.send).toHaveBeenCalledWith("tjommie:private-login-init", { kind: "entry", botId: "bot-1", name: "", origin: "" });
    invoke("submit", trusted(window), { name: "Synthetic", origin: "https://example.test", username: "synthetic-user", password: "synthetic-password" });
    expect(await operation).toEqual({ name: "Synthetic", origin: "https://example.test", secret: { username: "synthetic-user", password: "synthetic-password" } });
    expect(window.destroyed).toBe(true); expect(fixtures.handlers.size).toBe(0);
    expect(fixtures.profiles[0]?.clearStorageData).toHaveBeenCalledTimes(1);
  });

  it("aborts without revealing fields, denies navigation, and suppresses a late ready-to-show event", async () => {
    const entry = new NativeVaultEntry(parent()), abort = new AbortController(), operation = entry.entry("bot-1", null, abort.signal), window = windowNow();
    const preventDefault = vi.fn(); window.webContents.emit("will-navigate", { preventDefault }); window.webContents.emit("will-attach-webview", { preventDefault });
    expect(preventDefault).toHaveBeenCalledTimes(2);
    expect(window.webContents.setWindowOpenHandler.mock.calls[0]?.[0]()).toEqual({ action: "deny" });
    abort.abort(); expect(await operation).toBeNull(); window.emit("ready-to-show"); expect(window.show).not.toHaveBeenCalled();
    expect(fixtures.handlers.size).toBe(0);
  });

  it("uses the current owner window getter and rejects fields on confirmation-only dialogs", async () => {
    let owner = parent(); const first = owner, entry = new NativeVaultEntry(() => owner);
    const operation = entry.confirm({ kind: "unlock" }, new AbortController().signal), window = windowNow();
    expect(window.options.parent).toBe(first);
    expect(() => invoke("submit", trusted(window), { name: "Synthetic", origin: "https://example.test", username: "x", password: "y" })).toThrow("Check the login fields");
    invoke("submit", trusted(window), true); expect(await operation).toBe(true);
    owner = parent(); const next = entry.confirm({ kind: "unlock" }, new AbortController().signal), replacement = windowNow();
    expect(replacement.options.parent).toBe(owner); expect(replacement.options.parent).not.toBe(first);
    invoke("submit", trusted(replacement), null); expect(await next).toBe(false);
  });
});
