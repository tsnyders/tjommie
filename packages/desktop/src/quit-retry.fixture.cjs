const { app, BrowserWindow } = require("electron");
const { spawn } = require("node:child_process");
const { join } = require("node:path");
const root = process.argv[2];
app.setName("Tjommie Quit Regression");
app.setAppUserModelId("com.tsnyders.tjommie.quitrepro");
app.setPath("userData", join(root, "user-data"));
app.setPath("crashDumps", join(root, "crash"));
app.setAppLogsPath(join(root, "logs"));
let quitting = false, deviceQuitReady = false, deviceQuitCleanup = null, stopping = false, daemon = null;
let cleanupCount = 0, stopCount = 0, recoveryCount = 0, backendPid = null, backendExit = null;
const deviceBridge = {};
async function revokeLocalDevice() { cleanupCount++; await Promise.allSettled([Promise.resolve(), Promise.resolve()]); }
app.on("before-quit", () => { if (!quitting) { deviceQuitReady = false; deviceQuitCleanup = null; } quitting = true; });
app.on("will-quit", event => {
  /* PRODUCTION_DEVICE_QUIT_BRANCH */
  if (stopping) { event.preventDefault(); return; }
  if (!daemon) return;
  event.preventDefault(); stopping = true;
  const running = daemon; daemon = null; stopCount++;
  const stopped = new Promise(done => running.once("close", (code, signal) => { backendExit = { code, signal }; done(); }));
  running.stdin.end();
  void stopped.finally(() => { stopping = false; }).then(() => app.quit());
});
app.on("window-all-closed", () => {});
app.on("quit", () => process.stdout.write(`${JSON.stringify({ event: "quit", cleanupCount, stopCount, recoveryCount, backendPid, backendExit, electron: process.versions.electron })}\n`));
app.whenReady().then(async () => {
  daemon = spawn(process.execPath, ["-e", "process.stdin.resume();process.stdin.once('end',()=>process.exit(0));process.stdout.write('ready');"], { env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" }, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
  backendPid = daemon.pid; daemon.stdin.on("error", () => {});
  await new Promise((done, failed) => { daemon.once("error", failed); daemon.stdout.once("data", done); });
  const window = new BrowserWindow({ show: false, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false } });
  await window.loadURL("data:text/html,<title>Isolated quit regression</title>");
  setImmediate(() => {
    setTimeout(() => { recoveryCount++; app.quit(); }, 2000);
    app.quit();
  });
}).catch(error => { process.stderr.write(`${error.message}\n`); daemon?.stdin.end(); app.exit(1); });
