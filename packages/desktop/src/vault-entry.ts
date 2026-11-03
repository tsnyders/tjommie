import { BrowserWindow, ipcMain, session } from "electron";
import { randomUUID } from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";
import { z } from "zod";
import { LoginOrigin, type LoginEntry } from "../../protocol/src/vault.ts";
import type { TrustedVaultEntry, VaultConfirmation, VaultEntryInput } from "./login-vault.ts";
import { validateVaultSecret } from "./vault-storage.ts";

const EntryInput = z.object({ name: z.string().trim().min(1).max(100), origin: LoginOrigin, username: z.string().min(1).max(320), password: z.string().min(1).max(2400) }).strict();
const Submit = "tjommie:private-login-submit", Ready = "tjommie:private-login-ready", Init = "tjommie:private-login-init";
export class NativeVaultEntry implements TrustedVaultEntry {
  readonly #parent: () => BrowserWindow | null;
  #window: BrowserWindow | null = null;
  constructor(parent: BrowserWindow | (() => BrowserWindow | null)) { this.#parent = typeof parent === "function" ? parent : () => parent; }
  async entry(botId: string, previous: LoginEntry | null, signal: AbortSignal): Promise<VaultEntryInput | null> {
    const result = await this.#show({ kind: "entry", botId, name: previous?.name ?? "", origin: previous?.origin ?? "" }, signal);
    if (result === null) return null;
    const input = EntryInput.parse(result), secret = { username: input.username, password: input.password }; validateVaultSecret(secret);
    return { name: input.name, origin: input.origin, secret };
  }
  async confirm(request: VaultConfirmation, signal: AbortSignal): Promise<boolean> { return await this.#show(request, signal) === true; }
  #show(input: unknown, signal: AbortSignal): Promise<unknown> {
    const parent = this.#parent();
    if (signal.aborted || !parent || parent.isDestroyed()) return Promise.resolve(null);
    if (this.#window) return Promise.reject(new Error("A native Logins window is already open."));
    const html = fileURLToPath(new URL("./vault-entry.html", import.meta.url));
    const profile = session.fromPartition(`tjommie-login-${randomUUID()}`);
    profile.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
    profile.setPermissionCheckHandler(() => false);
    const window = new BrowserWindow({ parent, modal: true, width: 480, height: 620, show: false, resizable: false, autoHideMenuBar: true, title: "Tjommie Logins",
      webPreferences: { preload: fileURLToPath(new URL("./vault-entry-preload.cjs", import.meta.url)), session: profile,
        contextIsolation: true, sandbox: true, nodeIntegration: false, webSecurity: true, devTools: false, spellcheck: false, webviewTag: false } });
    this.#window = window; window.setContentProtection(true); window.removeMenu();
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (event) => event.preventDefault());
    window.webContents.on("will-attach-webview", (event) => event.preventDefault());
    return new Promise<unknown>((done) => {
      let settled = false;
      const finish = (value: unknown): void => {
        if (settled) return; settled = true; clearTimeout(timer); signal.removeEventListener("abort", cancel);
        ipcMain.removeHandler(Submit); ipcMain.removeHandler(Ready);
        if (this.#window === window) this.#window = null;
        if (!window.isDestroyed()) window.destroy();
        void profile.clearStorageData().catch(() => {}); done(value);
      };
      const cancel = (): void => finish(null);
      const trusted = (event: Electron.IpcMainInvokeEvent): boolean => event.sender === window.webContents && event.senderFrame === window.webContents.mainFrame && event.senderFrame.url === pathToFileURL(html).href;
      ipcMain.handle(Ready, (event) => { if (!trusted(event)) throw new Error("Not a native Logins window."); window.webContents.send(Init, input); return {}; });
      ipcMain.handle(Submit, (event, raw: unknown) => {
        if (!trusted(event)) throw new Error("Not a native Logins window.");
        if (raw === null) { cancel(); return {}; }
        if (raw === true && typeof input === "object" && input !== null && "kind" in input && input.kind !== "entry") { finish(true); return {}; }
        const parsed = EntryInput.safeParse(raw);
        if (!parsed.success || typeof input !== "object" || input === null || !("kind" in input) || input.kind !== "entry") throw new Error("Check the login fields in this native window.");
        try { validateVaultSecret({ username: parsed.data.username, password: parsed.data.password }); }
        catch { throw new Error("Check the login fields in this native window."); }
        finish(parsed.data); return {};
      });
      const timer = setTimeout(cancel, 120_000); timer.unref();
      signal.addEventListener("abort", cancel, { once: true }); window.once("closed", cancel);
      window.once("ready-to-show", () => { if (!settled && !signal.aborted) window.show(); });
      void window.loadFile(html).catch(cancel);
    });
  }
  close(): void { this.#window?.close(); }
}
