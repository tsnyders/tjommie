// Owner-authorized normal-app acceptance. Uses the existing Tjommie login and deletes only its own test bot.
import assert from "node:assert/strict";
import { randomUUID, createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { _electron } from "playwright-core";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { connect } from "../../../packages/client/src/index.ts";
import { packagedExecutable, provenanceFile, verifyPackagedProvenance } from "../../../packages/desktop/scripts/packaged-provenance.ts";

const executable = process.argv[2];
assert(executable && isAbsolute(executable) && existsSync(executable), "Supply an absolute installed executable path.");
const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
verifyPackagedProvenance(repo);
const hash = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
const manifest = JSON.parse(readFileSync(join(repo, provenanceFile), "utf8"));
assert.equal(hash(executable), hash(join(repo, packagedExecutable)));
const prefix = "dist-desktop/installer/win-unpacked/";
for (const [file, expected] of manifest.artifacts) {
  if (file.startsWith(prefix)) assert.equal(hash(join(dirname(executable), file.slice(prefix.length))), expected, `Installed payload differs: ${file}`);
}
const root = join(process.env.LOCALAPPDATA, "Tjommie");
const env = Object.fromEntries(Object.entries(process.env).filter(([key, value]) => value !== undefined
  && !key.startsWith("TJOMMIE_") && !["ELECTRON_RUN_AS_NODE", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY"].includes(key)));
const result = { startedAt: new Date().toISOString(), inputsSha256: manifest.inputsSha256, checks: {}, fixtureBotId: null, error: null };
let app;
let client;
let bot;
let monitor;
let monitorStopping = false;
let monitorFailure;
const newConsoleWindows = [];

async function monitorConsoles() {
  const powershell = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0");
  monitor = spawn(join(powershell, "powershell.exe"), ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", join(repo, "docs/superpowers/demos/console-snapshot.ps1"), "-DurationMs", "900000", "-IntervalMs", "50"], {
    windowsHide: true, stdio: ["ignore", "pipe", "pipe"], env: { ...env, PSModulePath: join(powershell, "Modules") },
  });
  let baseline;
  const ready = Promise.withResolvers();
  const timer = setTimeout(() => ready.reject(new Error("Console monitor did not become ready.")), 15_000);
  const lines = createInterface({ input: monitor.stdout });
  lines.on("line", (line) => {
    try {
      const windows = JSON.parse(line);
      assert(Array.isArray(windows));
      for (const window of windows) assert(Number.isInteger(window.HWND) && Number.isInteger(window.pid) && ["ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS"].includes(window.class));
      if (!baseline) { baseline = new Set(windows.map((window) => window.HWND)); ready.resolve(); }
      else for (const window of windows) {
        if (!baseline.has(window.HWND) && !newConsoleWindows.some((entry) => entry.HWND === window.HWND)) newConsoleWindows.push(window);
      }
    } catch (error) { monitorFailure = String(error); ready.reject(error); }
  });
  monitor.stderr.on("data", () => { monitorFailure = "Console monitor reported an error."; });
  monitor.on("error", (error) => { monitorFailure = String(error); ready.reject(error); });
  monitor.on("exit", () => {
    if (!monitorStopping) { monitorFailure = "Console monitor exited before acceptance finished."; ready.reject(new Error(monitorFailure)); }
  });
  try { await ready.promise; } finally { clearTimeout(timer); }
}

async function waitUntil(check, timeout = 180_000) {
  const deadline = Date.now() + timeout;
  for (;;) {
    const value = await check();
    if (value !== false && value !== null && value !== undefined) return value;
    if (Date.now() >= deadline) throw new Error(`Own folder check timed out after ${timeout} ms.`);
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}
async function launch() {
  app = await _electron.launch({ executablePath: executable, args: [], env });
  const page = await app.firstWindow();
  await page.waitForFunction(() => typeof window.tjommie?.connection === "function");
  const connection = await page.evaluate(() => window.tjommie.connection());
  assert.equal(connection.ok, true, "Normal desktop daemon must connect.");
  client = await connect(connection.port, connection.token);
}
async function quit() {
  await client?.close();
  client = undefined;
  if (app) {
    const current = app;
    app = undefined;
    await current.close();
  }
}
async function turn(fileName, content) {
  const before = client.events.length;
  const command = `node -e "require('node:fs').writeFileSync('${fileName}','${content}')"`;
  await client.call("chat.send", { botId: bot.id, text: `Use your shell tool to run exactly this command in your home folder: ${command}. Do not request elevated permissions. Do not edit notes or any other files. When it succeeds reply with exactly ${content}.`, files: [] });
  await waitUntil(async () => {
    const approvals = client.events.slice(before).filter((event) => event.event === "approval.added" && event.data.botId === bot.id);
    if (approvals.length) throw new Error("In-folder work unexpectedly requested approval; no approval was granted.");
    const activity = client.events.slice(before).some((event) => (event.event === "message.added" || event.event === "message.updated") && event.data.conversationId === bot.id && event.data.kind === "activity");
    const current = (await client.call("bots.list", {})).find((entry) => entry.id === bot.id);
    if (current?.state === "needs_setup") throw new Error(current.setupReason ?? "Codex needs setup.");
    if (current?.state === "limited") throw new Error("Codex account is limited.");
    return activity && current?.state === "idle" && existsSync(join(bot.homeFolder, fileName));
  });
  assert.equal(readFileSync(join(bot.homeFolder, fileName), "utf8"), content);
}

try {
  await monitorConsoles();
  await launch();
  const setup = (await client.call("setup.status", {})).find((entry) => entry.provider === "codex");
  assert(setup?.signedIn && setup.sandbox === "ready", "Existing managed Codex account and elevated sandbox must be ready.");
  const models = await client.call("models.list", { provider: "codex" });
  const model = models.find((entry) => entry.isDefault) ?? models[0];
  assert(model, "Codex must provide an available model.");
  bot = await client.call("bots.create", { name: `Own folder check ${randomUUID().slice(0, 8)}`, color: "blue", shape: "circle", provider: "codex", model: model.id, autonomy: "folder", browser: false, instructions: "Perform only the exact synthetic acceptance commands requested. Do not read personal files, use network tools or change configuration." });
  result.fixtureBotId = bot.id;
  assert.equal(resolve(bot.homeFolder), resolve(root, "bots", bot.id));
  await turn("own-folder-first.txt", "OWN-FOLDER-FIRST");
  result.checks["normal-root in-folder execution without approval"] = "Pass";
  await quit();
  await launch();
  const restored = (await client.call("bots.list", {})).find((entry) => entry.id === bot.id);
  assert(restored && restored.autonomy === "folder" && restored.homeFolder === bot.homeFolder);
  assert.equal(readFileSync(join(bot.homeFolder, "own-folder-first.txt"), "utf8"), "OWN-FOLDER-FIRST");
  await turn("own-folder-second.txt", "OWN-FOLDER-SECOND");
  result.checks["persistent bot and successful execution after app restart"] = "Pass";
} catch (error) {
  result.error = String(error);
  process.exitCode = 1;
} finally {
  if (bot && client) {
    try {
      await client.call("chat.stop", { botId: bot.id });
      await client.call("bots.delete", { id: bot.id });
      assert(!existsSync(bot.homeFolder), "The temporary bot folder must be removed.");
      result.checks["temporary bot cleanup"] = "Pass";
    } catch (error) { result.cleanupError = String(error); process.exitCode = 1; }
  } else if (bot) { result.cleanupError = "Normal app connection unavailable; fixture bot still needs removal."; process.exitCode = 1; }
  await quit().catch((error) => { result.quitError = String(error); process.exitCode = 1; });
  if (monitor) {
    monitorStopping = true;
    if (monitor.exitCode === null && monitor.signalCode === null) {
      const stopped = new Promise((resolve) => monitor.once("close", resolve));
      monitor.kill(); // Only the exact monitor child created above; never a terminal window or shared host.
      await stopped;
    }
  }
  result.consoleWindows = newConsoleWindows;
  if (monitorFailure || newConsoleWindows.length) {
    result.consoleError = monitorFailure ?? "A new visible console appeared during normal launch/use.";
    process.exitCode = 1;
  } else if (result.checks["persistent bot and successful execution after app restart"] === "Pass") {
    result.checks["no new visible console during startup, two turns and restart (50ms sampling)"] = "Pass";
  }
  result.finishedAt = new Date().toISOString();
  writeFileSync(join(repo, "docs/superpowers/demos/own-folder-results.json"), `${JSON.stringify(result, null, 2)}\n`);
  console.log(JSON.stringify(result));
}
