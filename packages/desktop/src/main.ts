import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, net, Notification, protocol, session, shell, Tray } from "electron";
import { appendFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir, userInfo } from "node:os";
import { isAbsolute, join } from "node:path";
import { pathToFileURL } from "node:url";
import { connect, type Client } from "@tjommie/client";
import { navigationFor, uiFile } from "./app-protocol.ts";
import { notesFile } from "./bot-files.ts";
import { startDaemonProcess, type DaemonConnection, type RunningDaemon } from "./daemon-process.ts";

const here = import.meta.dirname;
const packaged = app.isPackaged;
// Three ways to run. Development: the development daemon and `npm run dev` run separately. Production from source
// (TJOMMIE_DESKTOP=production): this process starts the daemon and serves the static export. Installed: the same,
// with the daemon, the UI and the icons as plain files in resources (npm run package).
const production = packaged || process.env.TJOMMIE_DESKTOP === "production";
const dataRoot = process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie");
const assets = packaged ? join(process.resourcesPath, "assets") : join(here, "..", "assets");
const uiDir = packaged ? join(process.resourcesPath, "ui") : join(here, "..", "..", "ui", "out");
const daemonEntry = packaged ? join(process.resourcesPath, "daemon", "main.mjs") : join(here, "..", "..", "daemon", "src", "main.ts");
const appIcon = join(assets, "icon-256.png");
const APP_ID = "com.tsnyders.tjommie";
// Next's static export needs 'unsafe-inline' bootstrap scripts; Task 8's Markdown sanitizer, not this CSP, blocks injected script.
const CSP =
  "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self' ws://127.0.0.1:*; object-src 'none'; base-uri 'none'; frame-src 'none'";
const THEMES = { dark: { color: "#070707", symbolColor: "#ACACAC" }, light: { color: "#FCFCFC", symbolColor: "#5C5C5C" } } as const;
const TITLE_BAR_HEIGHT = 44;
// A dialog's scrim is 60% black (the UI's bg-black/60), so what it covers keeps 40% of each channel: #ACACAC is #454545.
const underScrim = (hex: string): string =>
  `#${[1, 3, 5].map((at) => Math.round(Number.parseInt(hex.slice(at, at + 2), 16) * 0.4).toString(16).padStart(2, "0")).join("").toUpperCase()}`;

type ConnectionResult = ({ ok: true } & DaemonConnection) | { ok: false; error: string };

let win: BrowserWindow | null = null;
let tray: Tray | null = null;
let quitting = false;
let daemon: RunningDaemon | null = null;
let current: Promise<DaemonConnection> | null = null;
let starting = false;
let notifier: Client | null = null;
let restarts: number[] = [];
let pageTheme: keyof typeof THEMES = "dark";
let scrim = false; // a dialog is open in the page, its scrim over everything but the caption buttons
const notifications = new Set<Notification>(); // kept alive until closed, so their click handlers fire

function log(text: string): void {
  try {
    mkdirSync(join(dataRoot, "logs"), { recursive: true });
    appendFileSync(join(dataRoot, "logs", "desktop.log"), text.endsWith("\n") ? text : `${text}\n`);
  } catch {
    // logging must never stop the app
  }
}

app.setName("Tjommie");
if (process.env.TJOMMIE_USER_DATA) app.setPath("userData", process.env.TJOMMIE_USER_DATA); // tests: their own profile and lock
protocol.registerSchemesAsPrivileged([{ scheme: "app", privileges: { standard: true, secure: true, supportFetchAPI: true } }]);

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  // Windows shows notifications under this id; the installer gives the Start menu shortcut the same one.
  app.setAppUserModelId(packaged ? APP_ID : process.execPath);
  app.on("second-instance", showWindow);
  app.on("before-quit", () => {
    quitting = true;
  });
  app.on("will-quit", (event) => {
    const activeNotifier = notifier;
    notifier = null;
    void activeNotifier?.close().catch(() => {});
    if (!daemon) return;
    event.preventDefault(); // stop the daemon first, so its bots close cleanly
    const running = daemon;
    daemon = null;
    void running.stop().finally(() => app.quit());
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
  // Tjommie's window needs no browser permissions; notifications go through the main process.
  session.defaultSession.setPermissionRequestHandler((_wc, _permission, callback) => callback(false));
  session.defaultSession.setPermissionCheckHandler(() => false);
  if (production) protocol.handle("app", serveUi);
  registerIpc();
  createWindow();
  createTray();
  reconnect(false); // the UI asks for this first connection itself
}

async function serveUi(request: Request): Promise<Response> {
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
      execPath: process.execPath,
      args: ["--disable-warning=ExperimentalWarning", daemonEntry],
      env: process.env,
      // Only an isolated desktop test may redirect the daemon's account folders.
      testAccountsDir: process.env.TJOMMIE_USER_DATA ? process.env.TJOMMIE_ACCOUNTS_DIR || undefined : undefined,
    },
    log,
  );
  daemon = running;
  void running.exited.then(({ code }) => onDaemonExit(running, code));
  return running.connection;
}

// The daemon stopped on its own. Start a new one with a new token, at most three times a minute.
function onDaemonExit(running: RunningDaemon, code: number | null): void {
  if (quitting || daemon !== running) return;
  daemon = null;
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
    client.subscribe((event) => {
      if (event.event !== "notify" || !Notification.isSupported()) return;
      if (win?.isVisible() && win.isFocused()) return; // the person is already looking at Tjommie
      const note = new Notification({ title: event.data.title, body: event.data.body, icon: appIcon });
      notifications.add(note);
      note.on("close", () => notifications.delete(note));
      note.on("click", () => {
        notifications.delete(note);
        showWindow();
        win?.webContents.send("navigate", `/chat/?bot=${encodeURIComponent(event.data.botId)}`);
      });
      note.show();
    });
  } catch (error) {
    log(`Notifications are off until the next restart: ${String(error)}`);
  }
}

function registerIpc(): void {
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
    if (typeof path !== "string" || !isAbsolute(path) || !existsSync(path) || !statSync(path).isDirectory()) return;
    await shell.openPath(path);
  });
  ipcMain.on("file:show", (_event, path: unknown) => {
    if (typeof path === "string" && isAbsolute(path) && existsSync(path) && statSync(path).isFile()) shell.showItemInFolder(path);
  });
  ipcMain.on("theme:set", (_event, theme: unknown) => {
    pageTheme = theme === "light" ? "light" : "dark";
    paintTitleBar();
    win?.setBackgroundColor(THEMES[pageTheme].color);
  });
  ipcMain.on("dialog:scrim", (_event, open: unknown) => {
    if (typeof open !== "boolean") return;
    scrim = open;
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
  const window = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 960,
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
  window.once("ready-to-show", () => window.show());
  // A reload starts the page with no dialog open, so a scrim left over from before it goes.
  window.webContents.on("did-navigate", () => {
    if (!scrim) return;
    scrim = false;
    paintTitleBar();
  });
  window.on("close", (event) => {
    if (quitting) return;
    event.preventDefault();
    window.hide();
    hintTray();
  });
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (navigationFor(url, production) === "browser") void shell.openExternal(url);
    return { action: "deny" };
  });
  window.webContents.on("will-navigate", (event, url) => {
    const decision = navigationFor(url, production);
    if (decision === "stay") return;
    event.preventDefault();
    if (decision === "browser") void shell.openExternal(url);
  });
  void window.loadURL(production ? "app://tjommie/" : "http://localhost:3000/");
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
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: "Open Tjommie", click: showWindow },
      { type: "separator" },
      { label: "Quit Tjommie", click: () => app.quit() },
    ]),
  );
  tray.on("click", showWindow);
}

function showWindow(): void {
  if (!win) return;
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
