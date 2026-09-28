import { execFileSync, execSync } from "node:child_process";
import { existsSync, lstatSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { connect, type Client } from "@tjommie/client";
import type { Bot, BotInput } from "@tjommie/protocol";
import { _electron, type ElectronApplication, type Page } from "playwright-core";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { realAccountsDir, tempRoot } from "../../daemon/src/testing.ts";

// The real desktop app: Electron, the daemon it starts, the UI's static export and real Claude bots. It uses the
// signed-in account folders with a disposable data root and Electron profile.
const repo = resolve(import.meta.dirname, "..", "..", "..");
const electronPath = createRequire(import.meta.url)("electron") as string;

type Bridge = { tjommie: { connection(): Promise<{ ok: true; port: number; token: string } | { ok: false; error: string }> } };

let root: string | undefined;
let userData: string | undefined;
let electronApp: ElectronApplication | undefined;
let page: Page;
let client: Client | undefined;
let quit = false;
const ownedPids = new Set<number>();
const next404s: string[] = [];
let nextResponseCount = 0;

beforeAll(async () => {
  root = tempRoot();
  userData = mkdtempSync(join(tmpdir(), "tjommie-electron-"));
  execSync("npm run build", { cwd: repo, stdio: "inherit" }); // the static export the app serves
  const env = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined && entry[0] !== "TJOMMIE_DEV"));
  electronApp = await _electron.launch({
    executablePath: electronPath,
    args: [resolve(import.meta.dirname, "..")],
    env: {
      ...env,
      TJOMMIE_DESKTOP: "production",
      TJOMMIE_HOME: root,
      TJOMMIE_PORT: "0",
      TJOMMIE_ACCOUNTS_DIR: realAccountsDir(),
      TJOMMIE_USER_DATA: userData,
      TJOMMIE_BROWSER_HEADLESS: "1",
    },
  });
  const electronPid = electronApp.process().pid;
  if (electronPid !== undefined) ownedPids.add(electronPid);
  page = await electronApp.firstWindow();
  page.on("response", (response) => {
    const url = new URL(response.url());
    if (url.protocol === "app:" && url.hostname === "tjommie" && url.pathname.split("/").some((part) => part.startsWith("__next."))) {
      nextResponseCount++;
      if (response.status() === 404) next404s.push(url.pathname);
    }
  });
  await page.getByRole("heading", { name: "Set up Tjommie" }).waitFor({ timeout: 120_000 });
  const { port, token } = await daemonConnection();
  ownedPids.add(listeningPid(port));
  client = await connect(port, token);
});

afterAll(async () => {
  await client?.close().catch(() => {});
  if (electronApp && !quit) await electronApp.close().catch(() => {});
  for (const pid of ownedPids) {
    if (isRunning(pid)) {
      try { execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"]); }
      catch { /* The process may have exited between the check and taskkill. */ }
    }
    expect(isRunning(pid)).toBe(false);
  }
  for (const folder of [root, userData]) if (folder) await remove(folder);
});

// The connection the app handed its UI, read through the same bridge.
async function daemonConnection(): Promise<{ port: number; token: string }> {
  const result = await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.connection());
  if (!result.ok) throw new Error(result.error);
  return { port: result.port, token: result.token };
}

function daemonClient(): Client {
  if (!client) throw new Error("The test couldn't reach the app's daemon.");
  return client;
}

function runningApp(): ElectronApplication {
  if (!electronApp) throw new Error("The desktop app didn't start.");
  return electronApp;
}

const claudeBot = (name: string, extra: Partial<BotInput> = {}): Promise<Bot> =>
  daemonClient().call("bots.create", {
    name,
    color: "green",
    shape: "pill",
    instructions: "You are a terse test bot. Answer in one short line.",
    provider: "claude",
    model: "haiku",
    ...extra,
  });

async function openChat(name: string): Promise<void> {
  await page.getByRole("link", { name: new RegExp(`^${name},`) }).click();
  await page.getByRole("heading", { level: 1, name }).waitFor();
  await page.waitForLoadState("networkidle");
  expect(next404s).toEqual([]);
}

async function send(name: string, message: string): Promise<void> {
  const box = page.getByRole("textbox", { name: `Message ${name}` });
  await box.fill(message);
  await box.press("Enter");
  await page.locator('[data-author="you"]', { hasText: message }).waitFor();
}

const botReply = (message: string) => page.locator('[data-author="bot"]', { hasText: message });

function listeningPid(port: number): number {
  const output = execFileSync("powershell.exe", ["-NoProfile", "-Command", `(Get-NetTCPConnection -LocalPort ${port} -State Listen).OwningProcess`], { encoding: "utf8" });
  const pid = Number(output.trim().split(/\s+/)[0]);
  if (!Number.isInteger(pid) || pid <= 0) throw new Error("The test couldn't identify its daemon process.");
  return pid;
}

function isRunning(pid: number): boolean {
  try { process.kill(pid, 0); return true; }
  catch { return false; }
}

function refused(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      done(false);
    });
    socket.once("error", () => done(true));
  });
}

async function eventually<T>(check: () => Promise<T | undefined>, timeoutMs = 60_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check().catch(() => undefined);
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`Timed out after ${timeoutMs} ms`);
    await new Promise((done) => setTimeout(done, 500));
  }
}

function tokenAbsentFromLog(token: string): void {
  if (!root) throw new Error("The test data root wasn't created.");
  const logFile = join(root, "logs", "desktop.log");
  expect(existsSync(logFile)).toBe(true);
  // Compare a boolean so an assertion failure cannot print the token or the log.
  expect(readFileSync(logFile, "utf8").includes(token)).toBe(false);
}

// Windows can hold a file for a moment after its process exits.
async function remove(folder: string): Promise<void> {
  const disposable = resolve(folder);
  if (!existsSync(disposable)) return;
  if (dirname(disposable) !== resolve(tmpdir()) || lstatSync(disposable).isSymbolicLink()) {
    throw new Error("Refusing to remove a folder outside the disposable temp root.");
  }
  for (let attempt = 0; attempt < 20 && existsSync(disposable); attempt++) {
    try { rmSync(disposable, { recursive: true, force: true }); }
    catch { await new Promise((done) => setTimeout(done, 500)); }
  }
  expect(existsSync(disposable)).toBe(false);
}

describe("the desktop app with real bots", () => {
  it("sends a message and shows the reply", async () => {
    await claudeBot("Echo");
    await openChat("Echo");
    expect(nextResponseCount).toBeGreaterThan(0);
    await send("Echo", "Reply with exactly: PONG");
    await botReply("PONG").waitFor({ timeout: 180_000 });
  });

  it("asks before a change and carries on after Allow once", async () => {
    const bot = await claudeBot("Careful", { autonomy: "ask" });
    await openChat("Careful");
    await send("Careful", "Use the Write tool to create hello.txt containing the word hi. Then reply DONE.");
    const card = page.getByRole("group", { name: "Waiting for you" });
    await card.waitFor({ timeout: 180_000 });
    await card.getByRole("button", { name: "Allow once" }).click();
    await page.getByText(/^Allowed once: /).waitFor();
    await botReply("DONE").waitFor({ timeout: 180_000 });
    expect(existsSync(join(bot.homeFolder, "hello.txt"))).toBe(true);
  });

  it("stops a bot that's working", async () => {
    await claudeBot("Sleeper", { autonomy: "full" });
    await openChat("Sleeper");
    await send("Sleeper", `Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('done'), 60000)". Then tell me what it printed.`);
    await page.getByRole("button", { name: /^\d+ steps?: / }).waitFor({ timeout: 180_000 }); // the command is running
    await page.getByRole("button", { name: "Stop" }).click();
    await page.getByText("Stopped.", { exact: true }).waitFor({ timeout: 30_000 });
    await page.getByRole("link", { name: "Sleeper, Idle" }).waitFor({ timeout: 30_000 });
  });

  it("reconnects after the daemon dies", async () => {
    const before = await daemonConnection();
    const pid = listeningPid(before.port);
    ownedPids.add(pid);
    execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"]);
    // The app starts a new daemon with a new token, and hands the UI the new connection.
    const after = await eventually(async () => {
      const next = await daemonConnection();
      return next.token === before.token ? undefined : next;
    });
    const nextPid = listeningPid(after.port);
    ownedPids.add(nextPid);
    expect(nextPid).not.toBe(pid);
    tokenAbsentFromLog(before.token);
    tokenAbsentFromLog(after.token);
    await page.getByText("Reconnecting to Tjommie").waitFor({ state: "hidden", timeout: 60_000 });
    // The UI's calls and events now go to the new daemon.
    await openChat("Echo");
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Pause bot" }).click();
    await page.getByRole("link", { name: "Echo, Paused" }).waitFor();
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Resume bot" }).click();
    await page.getByRole("link", { name: "Echo, Idle" }).waitFor();
  });

  it("quitting Tjommie stops its daemon", async () => {
    const { port, token } = await daemonConnection();
    tokenAbsentFromLog(token);
    const closed = runningApp().waitForEvent("close");
    await runningApp()
      .evaluate(({ app }) => app.quit())
      .catch(() => {}); // the app may exit before it answers
    await closed;
    quit = true;
    expect(await refused(port)).toBe(true);
    tokenAbsentFromLog(token);
    expect(next404s).toEqual([]);
  });
});
