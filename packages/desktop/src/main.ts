import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, net, Notification, protocol, session, shell, Tray } from "electron";
import { appendFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir, userInfo } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { connect, type Client } from "@tjommie/client";
import { NativeBrowserMount, NativeBrowserUiCommand, type EventData } from "@tjommie/protocol";
import updater from "electron-updater";
import { navigationFor, uiFile } from "./app-protocol.ts";
import { notesFile } from "./bot-files.ts";
import { daemonCommand, daemonLaunchEnv, startDaemonProcess, stopDaemonBeforeQuit, type DaemonConnection, type RunningDaemon } from "./daemon-process.ts";
import { notificationDelivery, notificationRoute } from "./notification-delivery.ts";
import { beginGoogleConsent } from "./google-consent.ts";
import { NativeBrowserEngine } from "./native-browser.ts";
import { nativeBrowserAutomation, cancelNativeBrowserAutomation } from "./native-browser-automation.ts";
import { startNativeBrowserHost } from "./native-browser-host.ts";
import { getLoginItem, setLoginItem, startsHidden, syncLoginItem } from "./login-item.ts";
import { createUpdater, trayTemplate } from "./updater.ts";
import { RemoteWires } from "./remote-wire.ts";
import { createDeviceKeys } from "./device-keys.ts";
import { localFilePath } from "./local-file-path.ts";
import { createDesktopDiagnostics } from "./diagnostics.ts";
import { DesktopDeviceBridge, attachmentSaveName, isAttachmentUrl } from "./local-device-bridge.ts";
import { installSpeechPermissions, speechOrigin } from "./speech-permissions.ts";
import { SpeechRuntime } from "./speech-runtime.ts";
import { packagedSpeechDigest } from "./speech-resources.ts";
import { NativeLoginVault } from "./login-vault.ts";
import { NativeVaultEntry } from "./vault-entry.ts";
import { VaultRecords, WindowsLoginCredentials } from "./vault-storage.ts";
import { RemoteWorkerIsolation } from "./worker-isolation.ts";
import { startVaultHost } from "./vault-host.ts";
import { VaultCommandGate } from "./vault-command-gate.ts";
import { VaultHostAction, type VaultHostCommand } from "../../protocol/src/vault.ts";

const { autoUpdater } = updater;
const here = import.meta.dirname;
const packaged = app.isPackaged;
const packageIdentity = packaged
  ? JSON.parse(readFileSync(join(app.getAppPath(), "package.json"), "utf8")) as { appId?: string; testDataRoot?: string }
  : {};
const productName = packaged ? app.getName() : "Tjommie";
// Three ways to run. Development: the development daemon and `npm run dev` run separately. Production from source
// (TJOMMIE_DESKTOP=production): this process starts the daemon and serves the static export. Installed: the same,
// with the daemon, the UI and the icons as plain files in resources (npm run package).
const production = packaged || process.env.TJOMMIE_DESKTOP === "production";
const dataRoot = process.env.TJOMMIE_HOME ?? packageIdentity.testDataRoot ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), productName);
const assets = packaged ? join(process.resourcesPath, "assets") : join(here, "..", "assets");
const uiDir = packaged ? join(process.resourcesPath, "ui") : join(here, "..", "..", "ui", "out");
const daemonEntry = packaged ? join(process.resourcesPath, "daemon", "main.mjs") : join(here, "..", "..", "daemon", "src", "main.ts");
const appIcon = join(assets, "icon-256.png");
const APP_ID = packageIdentity.appId ?? "com.tsnyders.tjommie";
// Next needs inline bootstrap scripts; Noise WASM needs wasm-unsafe-eval; Markdown sanitization handles injected content.
const CSP =
  "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self' ws://127.0.0.1:*; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-src 'none'";
const THEMES = { dark: { color: "#070707", symbolColor: "#ACACAC" }, light: { color: "#FCFCFC", symbolColor: "#5C5C5C" } } as const;
const TITLE_BAR_HEIGHT = 44;
// A dialog's scrim is 60% black (the UI's bg-black/60), so what it covers keeps 40% of each channel: #ACACAC is #454545.
const underScrim = (hex: string): string =>
  `#${[1, 3, 5].map((at) => Math.round(Number.parseInt(hex.slice(at, at + 2), 16) * 0.4).toString(16).padStart(2, "0")).join("").toUpperCase()}`;

type ConnectionResult = ({ ok: true } & DaemonConnection) | { ok: false; error: string };

let win: BrowserWindow | null = null;
let remoteWires: RemoteWires | null = null;
const deviceKeys = createDeviceKeys(dataRoot);
let tray: Tray | null = null;
let updates: ReturnType<typeof createUpdater> | null = null;
let quitting = false;
let stopping = false;
let daemon: RunningDaemon | null = null;
let current: Promise<DaemonConnection> | null = null;
let starting = false;
let notifier: Client | null = null;
let nativeEngine: NativeBrowserEngine | null = null;
let nativeHost: ReturnType<typeof startNativeBrowserHost> | null = null;
let nativeClient: Promise<Client> | null = null;
let speech: SpeechRuntime | null = null;
let deviceBridge: DesktopDeviceBridge | null = null;
let appDocumentReady = false;
let loginVault: NativeLoginVault | null = null;
let vaultIsolation: RemoteWorkerIsolation | null = null;
const vaultCommands = new VaultCommandGate({ vault: () => loginVault, ready: () => !quitting, connection: () => vaultIsolation, resolveBot: (id) => resolveNativeBot(id, false) });
let vaultHost: ReturnType<typeof startVaultHost> | null = null;
let deviceQuitReady = false;
let deviceQuitCleanup: Promise<void> | null = null;
const disabledNativeBots = new Set<string>();
let restarts: number[] = [];
let pageTheme: keyof typeof THEMES = "dark";
let scrim = false; // a dialog is open in the page, its scrim over everything but the caption buttons
const notifications = new Set<Notification>(); // kept alive until closed, so their click handlers fire
const deliverNotification = notificationDelivery();

function log(text: string): void {
  try {
    mkdirSync(join(dataRoot, "logs"), { recursive: true });
    appendFileSync(join(dataRoot, "logs", "desktop.log"), text.endsWith("\n") ? text : `${text}\n`);
  } catch {
    // logging must never stop the app
  }
}

app.setName(productName);
if (process.env.TJOMMIE_USER_DATA || packageIdentity.testDataRoot) {
  const userData = process.env.TJOMMIE_USER_DATA ?? join(dataRoot, "electron");
  mkdirSync(userData, { recursive: true });
  app.setPath("userData", userData); // tests: their own profile and lock, including the update relaunch
}
protocol.registerSchemesAsPrivileged([{ scheme: "app", privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true } }]);

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  // Windows shows notifications under this id; the installer gives the Start menu shortcut the same one.
  app.setAppUserModelId(packaged ? APP_ID : process.execPath);
  app.on("second-instance", showWindow);
  app.on("before-quit", () => {
    if (!quitting) { deviceQuitReady = false; deviceQuitCleanup = null; }
    quitting = true;
  });
  app.on("will-quit", (event) => {
    if (deviceBridge && !deviceQuitReady) {
      event.preventDefault();
      deviceQuitCleanup ??= revokeLocalDevice().then(() => { deviceQuitReady = true; setImmediate(() => app.quit()); });
      return;
    }
    if (stopping) { event.preventDefault(); return; }
    if (!daemon && starting && current) {
      event.preventDefault();
      void current.catch(() => {}).finally(() => app.quit());
      return;
    }
    if (!daemon) {
      updates?.stop();
      remoteWires?.closeAll();
      const activeNotifier = notifier;
      notifier = null;
      void activeNotifier?.close().catch(() => {});
      updates?.installOnQuit();
      return;
    }
    event.preventDefault(); // stop the daemon first, so its bots close cleanly
    stopping = true;
    const running = daemon;
    daemon = null;
    let daemonStopped = false;
    let forcedStop = false;
    void (async () => {
      try { loginVault?.lock(); vaultIsolation?.close(); await vaultHost?.close(); vaultHost = null; await nativeEngine?.closeAll(); }
      finally {
        forcedStop = (await stopDaemonBeforeQuit(running, 20_000)) === "forced";
        daemonStopped = true;
        try { await nativeHost?.close(); }
        finally {
          nativeHost = null;
          await (await nativeClient?.catch(() => null))?.close(); nativeClient = null;
        }
      }
      if (forcedStop) throw new Error("The background service required a forced stop.");
    })().catch((error: unknown) => {
      log(`Shutdown failed: ${String(error)}`);
      if (!updates?.installationPending() || (daemonStopped && !forcedStop)) return true;
      quitting = false;
      updates?.shutdownFailed();
      if (!win || win.isDestroyed()) {
        remoteWires?.closeAll();
        createWindow();
      }
      if (daemonStopped) reconnect();
      else win?.webContents.send("daemon:failed", "Tjommie's background service could not be stopped. Restart Tjommie to reconnect.");
      showWindow();
      dialog.showErrorBox("Update not installed", daemonStopped
        ? "The background service did not stop in time. The update was not installed. Tjommie is restarting the service."
        : "Tjommie couldn't confirm its background service stopped. The update was not installed. Restart Tjommie to reconnect.");
      return false;
    }).finally(() => { stopping = false; }).then((finish = true) => {
      if (!finish) return;
      updates?.installOnQuit();
      app.quit();
    }).catch((error: unknown) => log(`Quit recovery failed: ${String(error)}`));
  });
  app.on("window-all-closed", () => {}); // the window only hides; Tjommie keeps running in the tray
  // No top-level await here: Electron emits "ready" only after this module finishes evaluating.
  app
    .whenReady()
    .then(start)
    .catch((error: unknown) => {
      log(`Startup failed: ${String(error)}`);
      dialog.showErrorBox("Tjommie couldn't start", String(error));
      app.quit();
    });
}

function start(): void {
  const speechDirectory = join(process.resourcesPath, "speech");
  speech = new SpeechRuntime({ runtimePath: join(speechDirectory, "whisper-cli.exe"), runtimeSha256: packaged ? packagedSpeechDigest(speechDirectory) : null, dataDir: join(app.getPath("userData"), "speech"), emit: (event) => deviceBridge?.emitSpeech(event) });
  deviceBridge = new DesktopDeviceBridge({
    window: () => win, ready: () => appDocumentReady && !quitting && !!win && !win.isDestroyed() && !win.isMinimized(), production, speech,
    chooseSave: async (info) => {
      if (!win || win.isDestroyed()) throw new Error("The app window is closed.");
      const result = await dialog.showSaveDialog(win, { title: "Download file", defaultPath: attachmentSaveName(info.name), properties: ["showOverwriteConfirmation"] });
      return result.canceled ? null : result.filePath ?? null;
    },
  });
  installSpeechPermissions(session.defaultSession, () => win && !win.isDestroyed() ? win.webContents : null, () => deviceBridge?.isCapturing() ?? false, production);
  // Protocol handlers have no sender identity; check the actual requesting frame before serving a capability.
  session.defaultSession.webRequest.onBeforeRequest({ urls: ["app://tjommie/*"] }, (request, callback) => callback({ cancel: isAttachmentUrl(request.url) && !deviceBridge?.permitsAttachmentRequest(request) }));
  session.defaultSession.on("will-download", (event, item) => { if (isAttachmentUrl(item.getURL())) event.preventDefault(); });
  protocol.handle("app", serveUi);
  // The disposable packaged smoke uses a real app identity, so never register it at Windows sign-in.
  if (!process.env.TJOMMIE_USER_DATA && !packageIdentity.testDataRoot) {
    try { syncLoginItem(app); } catch (error) { log(`Could not set start at login: ${String(error)}`); }
  }
  updates = createUpdater(packaged, autoUpdater, {
    TJOMMIE_UPDATES: (process.env.TJOMMIE_USER_DATA && APP_ID === "com.tsnyders.tjommie") ||
      (packageIdentity.testDataRoot && !process.env.TJOMMIE_UPDATE_FEED) ? "off" : process.env.TJOMMIE_UPDATES,
    TJOMMIE_UPDATE_FEED: packageIdentity.testDataRoot ? process.env.TJOMMIE_UPDATE_FEED : undefined,
  }, () => app.quit());
  updates.onState((state) => {
    if (win && !win.isDestroyed()) win.webContents.send("updates:state", state);
    refreshTray();
  });
  registerIpc();
  createWindow();
  try {
    loginVault = new NativeLoginVault({
      records: new VaultRecords(join(dataRoot, "data", "logins.sqlite")), credentials: new WindowsLoginCredentials(dataRoot),
      isolation: {
        unavailableReason: () => vaultIsolation?.unavailableReason() ?? Promise.resolve("The protected provider host is unavailable."),
        acquire: () => vaultIsolation?.acquire() ?? Promise.reject(new Error("The protected provider host is unavailable.")),
      },
      entry: new NativeVaultEntry(() => win),
      browser: {
        destination: async (target, owner) => { const bot = await resolveNativeBot(target.botId); return vaultEngine().vaultDestination(bot, target, owner); },
        begin: async (target, owner, origin, check) => { const bot = await resolveNativeBot(target.botId); check(); return vaultEngine().beginVault(bot, target, owner, origin, check); },
        fill: async (botId, epoch, secret, check) => { const bot = await resolveNativeBot(botId); check(); await vaultEngine().writeVault(bot, epoch, secret, check); },
        finish: async (botId, generation, epoch, mode, check) => { const bot = await resolveNativeBot(botId); check(); await vaultEngine().finishVault(bot, generation, epoch, mode, check); },
      },
    });
  } catch { log("Logins storage could not be opened. Other app features remain available."); }
  createTray();
  reconnect(false); // the UI asks for this first connection itself
}

async function serveUi(request: Request): Promise<Response> {
  if (isAttachmentUrl(request.url)) return deviceBridge?.serve(request) ?? new Response(null, { status: 404 });
  if (!production) return new Response(null, { status: 404 });
  const file = uiFile(uiDir, request.url);
  // Anything missing gets Next's not-found page, which keeps the rail, so the window (which has no Back button)
  // never strands the person. Pages plan 1E adds show it until then.
  const served = file ?? uiFile(uiDir, "app://tjommie/404.html");
  if (!served) return new Response("Not found", { status: 404, headers: { "content-type": "text/plain" } });
  const response = await net.fetch(pathToFileURL(served).toString());
  const headers = new Headers(response.headers);
  headers.set("content-security-policy", CSP);
  return new Response(response.body, { status: file ? response.status : 404, headers });
}

// Opens a new connection: a new daemon with a new token in production, the development daemon otherwise.
// After a restart the UI gets it pushed when it's ready, or the reason it failed.
function reconnect(push = true): void {
  starting = true;
  const next = openConnection();
  current = next;
  next.then(
    (connection) => {
      if (current !== next) return;
      starting = false;
      if (push) win?.webContents.send("daemon:connection", connection);
    },
    (error: unknown) => {
      if (current !== next) return;
      starting = false;
      const reason = error instanceof Error ? error.message : String(error);
      log(`The daemon didn't start: ${reason}`);
      if (push) win?.webContents.send("daemon:failed", reason);
    },
  );
}

async function openConnection(): Promise<DaemonConnection> {
  const connection = production ? await launchDaemon() : developmentConnection();
  void listenForNotifications(connection);
  return connection;
}

function developmentConnection(): DaemonConnection {
  const tokenFile = join(dataRoot, "daemon-token");
  if (!existsSync(tokenFile)) throw new Error("Start the development daemon first: npm start -w @tjommie/daemon");
  return { port: Number(process.env.TJOMMIE_PORT ?? 47100), token: readFileSync(tokenFile, "utf8").trim() };
}

async function launchDaemon(): Promise<DaemonConnection> {
  const running = await startDaemonProcess(
    {
      ...daemonCommand(process.env, packaged, process.execPath, daemonEntry),
      env: daemonLaunchEnv(process.env, dataRoot, !!packageIdentity.testDataRoot),
      // Only an isolated desktop test may redirect the daemon's account folders.
      testAccountsDir: packageIdentity.testDataRoot
        ? join(dataRoot, "accounts")
        : process.env.TJOMMIE_USER_DATA ? process.env.TJOMMIE_ACCOUNTS_DIR || undefined : undefined,
    },
    log,
  );
  daemon = running;
  await vaultHost?.close(); vaultHost = null;
  loginVault?.lock(); vaultIsolation?.close();
  vaultIsolation = new RemoteWorkerIsolation(running.connection.port, running.desktopHostToken);
  void running.exited.then(({ code }) => onDaemonExit(running, code));
  await nativeHost?.close();
  const previousNativeClient = nativeClient;
  nativeClient = null;
  await (await previousNativeClient?.catch(() => null))?.close();
  const attachedClient = await connectNativeClient(running).catch(() => { log("The native browser client will retry its background connection."); return null; });
  if (attachedClient && nativeEngine) {
    await attachedClient.call("bots.list", {}).then((bots) => nativeEngine?.pruneProfiles(bots.map((bot) => bot.id)))
      .catch(() => { log("Some deleted native browser profile data could not be cleaned up yet."); });
  }
  nativeHost = startNativeBrowserHost({
    port: running.connection.port, token: running.desktopHostToken,
    execute: async (identity, command) => {
      if (!nativeEngine) throw new Error("The native browser is not ready.");
      if (command.kind === "close") { await nativeEngine.closeBot(identity.id); return {}; }
      if (command.kind === "delete_profile") { await nativeEngine.deleteProfile(identity.id); return {}; }
      if (command.kind === "cancel_tools") { await cancelNativeBrowserAutomation(identity.id); return {}; }
      const bot = await resolveNativeBot(identity.id);
      const result = await nativeEngine.execute(bot, command);
      if (disabledNativeBots.has(bot.id)) { await nativeEngine.closeBot(bot.id); throw new Error("This bot's browser is off."); }
      return result;
    },
  });
  vaultHost = startVaultHost({ port: running.connection.port, token: running.desktopHostToken, execute: executeVault, onError: () => loginVault?.lock() });
  return running.connection;
}

function connectNativeClient(running: RunningDaemon): Promise<Client> {
  if (nativeClient) return nativeClient;
  const opening = connect(running.connection.port, running.connection.token, { recordEvents: false });
  nativeClient = opening;
  const retry = (): void => {
    if (nativeClient !== opening) return;
    nativeClient = null;
    loginVault?.lock(); vaultIsolation?.close(); vaultIsolation = null;
    if (!quitting && daemon === running) setTimeout(() => {
      if (!quitting && daemon === running && !nativeClient) void connectNativeClient(running).catch(() => {});
    }, 1000);
  };
  void opening.then((client) => {
    if (daemon === running && !vaultIsolation) vaultIsolation = new RemoteWorkerIsolation(running.connection.port, running.desktopHostToken);
    const changedBots = new Set<string>();
    client.subscribe((event) => {
      if (event.event === "bot.deleted" || event.event === "bot.updated") changedBots.add(event.data.id);
      if (event.event === "bot.deleted") { disabledNativeBots.add(event.data.id); void nativeEngine?.closeBot(event.data.id).catch(() => {}); }
      if (event.event === "bot.updated") {
        if (event.data.browser) disabledNativeBots.delete(event.data.id);
        else { disabledNativeBots.add(event.data.id); void nativeEngine?.closeBot(event.data.id).catch(() => {}); }
      }
    });
    void client.call("bots.list", {}).then((bots) => {
      if (nativeClient !== opening) return;
      for (const bot of bots) if (bot.browser && !changedBots.has(bot.id)) disabledNativeBots.delete(bot.id);
    }).catch(() => {});
    void client.disconnected.then(retry);
  }, retry);
  return opening;
}

// The daemon stopped on its own. Start a new one with a new token, at most three times a minute.
function onDaemonExit(running: RunningDaemon, code: number | null): void {
  if (quitting || daemon !== running) return;
  daemon = null;
  loginVault?.lock(); vaultIsolation?.close(); vaultIsolation = null;
  log(`The daemon stopped (exit code ${code}).`);
  const now = Date.now();
  restarts = restarts.filter((at) => now - at < 60_000);
  if (restarts.length >= 3) {
    const reason = "Tjommie's background service keeps stopping.";
    current = Promise.reject(new Error(reason));
    current.catch(() => {});
    win?.webContents.send("daemon:failed", reason);
    return;
  }
  restarts.push(now);
  reconnect();
}

async function listenForNotifications(connection: DaemonConnection): Promise<void> {
  const previous = notifier;
  notifier = null;
  await previous?.close().catch(() => {});
  try {
    const client = await connect(connection.port, connection.token, { recordEvents: false });
    notifier = client;
    const showNotice = async (notice: EventData<"notify">): Promise<boolean> => {
      if (!Notification.isSupported()) return false;
      if (win?.isVisible() && win.isFocused()) return true; // already visible in the app's persisted history
      const note = new Notification({ title: notice.title, body: notice.body, icon: appIcon });
      notifications.add(note);
      note.on("close", () => notifications.delete(note));
      note.on("click", () => {
        notifications.delete(note);
        showWindow();
        win?.webContents.send("navigate", notificationRoute(notice));
      });
      // show() merely requests delivery. Only the OS-backed show event permits acknowledgement.
      return new Promise<boolean>((resolve) => {
        const finish = (shown: boolean): void => {
          clearTimeout(timer);
          note.removeListener("show", shownByOs);
          note.removeListener("failed", failedToShow);
          if (!shown) notifications.delete(note);
          resolve(shown);
        };
        const shownByOs = (): void => finish(true);
        const failedToShow = (): void => finish(false);
        const timer = setTimeout(failedToShow, 10_000);
        timer.unref();
        note.once("show", shownByOs);
        note.once("failed", failedToShow);
        try { note.show(); } catch { failedToShow(); }
      });
    };
    const deliver = (notice: EventData<"notify">) =>
      deliverNotification(notice, showNotice, (runId) => client.call("notifications.ack", { runId }));
    client.subscribe((event) => {
      if (event.event !== "notify") return;
      void deliver(event.data).catch((error: unknown) => log(`Could not acknowledge a routine notification: ${String(error)}`));
    });
    // Subscribe before fetching: a failure arriving during the fetch is coalesced by its run id.
    const pending = await client.call("notifications.pending", {});
    for (const notice of pending) await deliver(notice);
  } catch (error) {
    log(`Notifications are off until the next restart: ${String(error)}`);
  }
}

function vaultEngine(): NativeBrowserEngine { if (!nativeEngine) throw new Error("The native browser is unavailable."); return nativeEngine; }

async function executeVault(command: VaultHostCommand): Promise<unknown> {
  return vaultCommands.execute(command);
}

async function resolveNativeBot(id: unknown, requireBrowser = true) {
  if (typeof id !== "string" || !id || id.length > 256) throw new Error("Invalid browser bot.");
  if (!nativeClient && daemon) void connectNativeClient(daemon).catch(() => {});
  if (!nativeClient) throw new Error("The native browser needs the desktop background service.");
  const client = await nativeClient;
  const bot = (await client.call("bots.list", {})).find((value) => value.id === id);
  if (!bot || requireBrowser && (!bot.browser || disabledNativeBots.has(id))) throw new Error("This bot's browser is off or the bot no longer exists.");
  return bot;
}

function registerIpc(): void {
  ipcMain.handle("vault:command", async (event, raw: unknown) => {
    if (!win || win.isDestroyed() || !win.isVisible() || win.isMinimized() || !appDocumentReady || quitting || event.sender !== win.webContents || event.senderFrame !== win.webContents.mainFrame || !speechOrigin(event.senderFrame.url, production) || !speechOrigin(event.sender.getURL(), production)) throw new Error("Untrusted Logins request.");
    return executeVault({ controllerId: "local", action: VaultHostAction.parse(raw) });
  });
  const localDevice = (): DesktopDeviceBridge => { if (!deviceBridge) throw new Error("Local device features are not ready."); return deviceBridge; };
  ipcMain.handle("attachments:start-save", (event, info: unknown) => localDevice().startSave(event, info));
  ipcMain.handle("attachments:append-save", (event, id: unknown, offset: unknown, bytes: unknown) => localDevice().appendSave(event, id, offset, bytes));
  ipcMain.handle("attachments:finish-save", (event, id: unknown) => localDevice().finishSave(event, id));
  ipcMain.handle("attachments:cancel-save", (event, id: unknown) => localDevice().cancelSave(event, id));
  ipcMain.handle("attachments:preview", (event, info: unknown) => localDevice().preview(event, info));
  ipcMain.handle("attachments:respond", (event, previewId: unknown, requestId: unknown, bytes: unknown) => localDevice().respond(event, previewId, requestId, bytes));
  ipcMain.handle("attachments:close-preview", (event, id: unknown) => localDevice().closePreview(event, id));
  ipcMain.handle("attachments:invalidate", (event) => localDevice().invalidateFrom(event));
  ipcMain.handle("speech:status", (event) => localDevice().status(event));
  ipcMain.handle("speech:download-model", (event) => localDevice().downloadModel(event));
  ipcMain.handle("speech:cancel-model-download", (event) => localDevice().cancelModelDownload(event));
  ipcMain.handle("speech:remove-model", (event) => localDevice().removeModel(event));
  ipcMain.handle("speech:start", (event, input: unknown) => localDevice().startSpeech(event, input));
  ipcMain.handle("speech:append-pcm", (event, input: unknown) => localDevice().appendPcm(event, input));
  ipcMain.handle("speech:stop", (event, input: unknown) => localDevice().stopSpeech(event, input));
  ipcMain.handle("speech:cancel", (event, input: unknown) => localDevice().cancelSpeech(event, input));
  const diagnostics = createDesktopDiagnostics({
    window: () => win, production, appVersion: () => app.getVersion(), platform: process.platform,
    choose: async () => {
      if (!win || win.isDestroyed()) throw new Error("The app window is closed.");
      const result = await dialog.showSaveDialog(win, { title: "Save diagnostic report", defaultPath: "tjommie-diagnostics.json", filters: [{ name: "JSON report", extensions: ["json"] }], properties: ["showOverwriteConfirmation"] });
      return result.canceled ? null : result.filePath ?? null;
    },
    watch: (invalidate) => {
      const contents = win?.webContents;
      if (!contents || contents.isDestroyed()) throw new Error("The app window is closed.");
      contents.once("destroyed", invalidate);
      contents.on("did-start-navigation", invalidate);
      return () => { if (!contents.isDestroyed()) { contents.removeListener("destroyed", invalidate); contents.removeListener("did-start-navigation", invalidate); } };
    },
  });
  ipcMain.handle("diagnostics:info", (event) => diagnostics.info(event));
  ipcMain.handle("diagnostics:save", (event, report: unknown) => diagnostics.saveReport(event, report));
  const remote = (event: Electron.IpcMainInvokeEvent): RemoteWires => {
    if (event.sender !== win?.webContents || event.senderFrame !== win.webContents.mainFrame || !remoteWires) throw new Error("Untrusted remote bridge.");
    return remoteWires;
  };
  ipcMain.handle("remote:open", (event, endpoint: unknown, timeoutMs: unknown) => remote(event).open(endpoint, timeoutMs));
  ipcMain.handle("remote:send", (event, id: unknown, frame: unknown) => remote(event).send(id, frame));
  ipcMain.on("remote:close", (event, id: unknown) => {
    if (event.sender === win?.webContents && event.senderFrame === win.webContents.mainFrame) remoteWires?.close(id);
  });
  ipcMain.handle("remote:seal", (event, name: unknown, value: unknown) => { remote(event); return deviceKeys.seal(name, value); });
  ipcMain.handle("remote:read", (event, name: unknown) => { remote(event); return deviceKeys.read(name); });
  ipcMain.handle("remote:delete", (event, name: unknown) => { remote(event); return deviceKeys.delete(name); });
  ipcMain.handle("login-item:get", () => getLoginItem(app));
  ipcMain.handle("login-item:set", (_event, on: unknown) => {
    if (typeof on !== "boolean") throw new Error("Invalid start at login setting.");
    if (process.env.TJOMMIE_USER_DATA && APP_ID === "com.tsnyders.tjommie") return;
    setLoginItem(app, on);
  });
  ipcMain.handle("updates:state", () => updates?.state() ?? { status: "unsupported" });
  ipcMain.handle("updates:check", () => updates?.check());
  ipcMain.on("updates:restart", () => updates?.restart());
  ipcMain.handle("native-browser:command", async (event, botId: unknown, command: unknown) => {
    if (event.sender !== win?.webContents || event.senderFrame !== win.webContents.mainFrame) throw new Error("Untrusted browser host.");
    const value = NativeBrowserUiCommand.parse(command);
    const bot = await resolveNativeBot(botId);
    if (!nativeEngine) throw new Error("The native browser is not ready.");
    const result = await nativeEngine.execute(bot, value);
    if (disabledNativeBots.has(bot.id)) { await nativeEngine.closeBot(bot.id); throw new Error("This bot's browser is off."); }
    return result;
  });
  ipcMain.handle("native-browser:mount", async (event, botId: unknown, mount: unknown) => {
    if (event.sender !== win?.webContents || event.senderFrame !== win.webContents.mainFrame) throw new Error("Untrusted browser host.");
    const value = NativeBrowserMount.parse(mount);
    if ((!value.visible || value.occluded) && typeof botId === "string" && botId.length > 0 && botId.length <= 256 && nativeEngine) {
      // Concealment must not depend on a live daemon or wait behind page navigation.
      if (value.visible) nativeEngine.occludeBot(botId);
      else nativeEngine.hideBot(botId);
      try { return nativeEngine.state(botId); } catch { /* A first mount has not opened a guest yet. */ }
    }
    const bot = await resolveNativeBot(botId);
    if (!nativeEngine) throw new Error("The native browser is not ready.");
    const result = await nativeEngine.mount(bot, value);
    if (disabledNativeBots.has(bot.id)) { await nativeEngine.closeBot(bot.id); throw new Error("This bot's browser is off."); }
    return result;
  });
  ipcMain.handle("google:connect", async (_event, services: unknown): Promise<{ attempt: string }> => {
    if (!current) throw new Error("Tjommie is still starting.");
    const connection = await current;
    const client = await connect(connection.port, connection.token, { recordEvents: false });
    try {
      return await beginGoogleConsent({
        services,
        begin: (requested) => client.call("google.begin", { services: requested }),
        cancel: (attempt) => client.call("google.cancel", { attempt }),
        open: (url) => shell.openExternal(url),
      });
    } finally { await client.close().catch(() => {}); }
  });
  ipcMain.handle("daemon:connection", async (): Promise<ConnectionResult> => {
    try {
      if (!current) throw new Error("Tjommie is still starting.");
      return { ok: true, ...(await current) };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  });
  ipcMain.handle("daemon:restart", () => {
    if (daemon || starting) return; // a running or starting daemon needs no second launch
    restarts = [];
    reconnect();
  });
  ipcMain.handle("files:pick", async () => {
    const options: Electron.OpenDialogOptions = { properties: ["openFile", "multiSelections"] };
    const result = win ? await dialog.showOpenDialog(win, options) : await dialog.showOpenDialog(options);
    return result.canceled ? [] : result.filePaths;
  });
  ipcMain.handle("folder:open", async (_event, path: unknown) => {
    if (!localFilePath(path) || !existsSync(path) || !statSync(path).isDirectory()) return;
    await shell.openPath(path);
  });
  ipcMain.on("file:show", (_event, path: unknown) => {
    if (localFilePath(path) && existsSync(path) && statSync(path).isFile()) shell.showItemInFolder(path);
  });
  ipcMain.on("theme:set", (_event, theme: unknown) => {
    pageTheme = theme === "light" ? "light" : "dark";
    paintTitleBar();
    win?.setBackgroundColor(THEMES[pageTheme].color);
  });
  ipcMain.on("dialog:scrim", (_event, open: unknown) => {
    if (_event.sender !== win?.webContents) return;
    if (typeof open !== "boolean") return;
    scrim = open;
    nativeEngine?.setOccluded(open || !win?.isVisible());
    paintTitleBar();
  });
  ipcMain.handle("app:info", () => ({ username: userInfo().username }));
  ipcMain.handle("folder:pick", async () => {
    const options: Electron.OpenDialogOptions = { properties: ["openDirectory"] };
    const result = win ? await dialog.showOpenDialog(win, options) : await dialog.showOpenDialog(options);
    return result.canceled ? null : (result.filePaths[0] ?? null);
  });
  ipcMain.handle("notes:open", async (_event, homeFolder: unknown) => {
    const file = typeof homeFolder === "string" ? notesFile(join(dataRoot, "bots"), homeFolder) : null;
    if (file) await shell.openPath(file);
  });
}

function createWindow(): void {
  appDocumentReady = false;
  const window = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 360,
    minHeight: 600,
    show: false,
    title: "Tjommie",
    icon: appIcon,
    backgroundColor: THEMES.dark.color,
    titleBarStyle: "hidden",
    titleBarOverlay: { ...THEMES.dark, height: TITLE_BAR_HEIGHT },
    webPreferences: { preload: join(here, "preload.cjs"), contextIsolation: true, sandbox: true, nodeIntegration: false },
  });
  win = window;
  remoteWires = new RemoteWires(window.webContents);
  nativeEngine = new NativeBrowserEngine({
    window, profileRoot: join(dataRoot, "data", "native-browser"), automation: nativeBrowserAutomation,
    onState: (state) => { if (!window.isDestroyed()) window.webContents.send("native-browser:state", state); },
    onFocusReturn: (botId) => { if (!window.isDestroyed()) window.webContents.send("native-browser:focus-return", botId); },
    onNotice: (notice) => { log(`Native browser notice for ${notice.botId}: ${notice.message}`); },
  });
  window.on("hide", () => { nativeEngine?.setOccluded(true); void revokeLocalDevice(); });
  window.on("minimize", () => { void revokeLocalDevice(); });
  window.on("blur", () => { if (speech?.isRecordingAudio()) void deviceBridge?.cancelCapture().catch(() => log("Temporary speech cleanup failed.")); });
  window.on("show", () => nativeEngine?.setOccluded(scrim));
  window.once("ready-to-show", () => {
    if (!startsHidden(process.argv)) window.show();
    if (packageIdentity.testDataRoot) log(`Update test window ready: ${process.pid}`);
  });
  // A reload starts the page with no dialog open, so a scrim left over from before it goes.
  window.webContents.on("did-navigate", (_event, url) => {
    appDocumentReady = speechOrigin(url, production);
    if (!scrim) return;
    scrim = false;
    nativeEngine?.setOccluded(!window.isVisible());
    paintTitleBar();
  });
  window.webContents.on("did-navigate-in-page", (_event, url, isMainFrame) => { if (isMainFrame) appDocumentReady = speechOrigin(url, production); });
  window.webContents.on("did-start-navigation", (_event, url, _inPlace, isMainFrame) => {
    if (!isMainFrame || navigationFor(url, production) !== "stay" || isAttachmentUrl(url)) return;
    appDocumentReady = false; void revokeLocalDevice();
  });
  window.webContents.on("render-process-gone", () => { appDocumentReady = false; void revokeLocalDevice(); });
  window.webContents.on("destroyed", () => { appDocumentReady = false; void revokeLocalDevice(); });
  window.on("close", (event) => {
    if (quitting) return;
    event.preventDefault();
    window.hide();
    hintTray();
  });
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (isAttachmentUrl(url)) return { action: "deny" };
    if (navigationFor(url, production) === "browser") void shell.openExternal(url);
    return { action: "deny" };
  });
  window.webContents.on("will-navigate", (event, url) => {
    if (isAttachmentUrl(url)) { event.preventDefault(); return; }
    const decision = navigationFor(url, production);
    if (decision === "stay") return;
    event.preventDefault();
    if (decision === "browser") void shell.openExternal(url);
  });
  void window.loadURL(production ? "app://tjommie/" : "http://localhost:3000/");
}

async function revokeLocalDevice(): Promise<void> {
  vaultCommands.invalidate();
  try { await deviceBridge?.invalidate(); }
  catch { log("Temporary local file or speech cleanup failed."); }
}

// The caption buttons in the page's theme. Windows draws them above the page, where a dialog's scrim can't reach, so
// while a dialog is open they're dimmed the way the scrim dims the page.
function paintTitleBar(): void {
  const { color, symbolColor } = THEMES[pageTheme];
  const shade = (hex: string): string => (scrim ? underScrim(hex) : hex);
  win?.setTitleBarOverlay({ color: shade(color), symbolColor: shade(symbolColor), height: TITLE_BAR_HEIGHT });
}

function createTray(): void {
  tray = new Tray(nativeImage.createFromPath(join(assets, "tray-32.png")));
  tray.setToolTip("Tjommie");
  refreshTray();
  tray.on("click", showWindow);
}

function refreshTray(): void {
  tray?.setContextMenu(Menu.buildFromTemplate(trayTemplate(
    updates?.state() ?? { status: "unsupported" }, showWindow, () => updates?.restart(), () => app.quit(),
  )));
}

function showWindow(): void {
  if (!win || win.isDestroyed()) return;
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

// The first time the window closes, say that Tjommie keeps running in the tray.
function hintTray(): void {
  const marker = join(app.getPath("userData"), "tray-hint-shown");
  if (existsSync(marker) || !Notification.isSupported()) return;
  mkdirSync(app.getPath("userData"), { recursive: true });
  writeFileSync(marker, "");
  new Notification({ title: "Tjommie is still running", body: "Your bots and routines keep working. Quit from the Tjommie icon in the taskbar tray.", icon: appIcon }).show();
}
