import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { basename, dirname, extname, join, resolve } from "node:path";
import { connect, type Client } from "@tjommie/client";
import type { Bot, BotInput } from "@tjommie/protocol";
import { _electron, type ElectronApplication, type Page } from "playwright-core";
import { realAccountsDir, tempRoot } from "../../daemon/src/testing.ts";

type Bridge = { tjommie: { connection(): Promise<{ ok: true; port: number; token: string } | { ok: false; error: string }> } };
type OwnedProcess = { image: string; path: string; started: string };
type Ownership = { executablePath: string; startedAt: number; ownedPids: Map<number, OwnedProcess> };

export type RealApp = { electronApp: ElectronApplication; page: Page; client: Client; root: string; userData: string };

const desktopDir = resolve(import.meta.dirname, "..");
const ownerships = new WeakMap<RealApp, Ownership>();

// Run the production app from source or a packaged executable. Only the daemon receives the path to the user's
// signed-in account folders; the data root, Electron profile and port are disposable for each launch.
export async function launchRealApp(executablePath?: string): Promise<RealApp> {
  const ownership: Ownership = {
    executablePath: resolve(executablePath ?? (createRequire(import.meta.url)("electron") as string)),
    startedAt: Date.now(),
    ownedPids: new Map(),
  };
  const root = tempRoot();
  const userData = mkdtempSync(join(tmpdir(), "tjommie-electron-"));
  const inherited = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined && entry[0] !== "TJOMMIE_DEV"));
  let electronApp: ElectronApplication | undefined;
  let client: Client | undefined;
  try {
    electronApp = await _electron.launch({
      executablePath: ownership.executablePath,
      args: executablePath ? [] : [desktopDir],
      env: {
        ...inherited,
        TJOMMIE_DESKTOP: "production",
        TJOMMIE_HOME: root,
        TJOMMIE_PORT: "0",
        TJOMMIE_ACCOUNTS_DIR: realAccountsDir(),
        TJOMMIE_USER_DATA: userData,
        TJOMMIE_BROWSER_HEADLESS: "1",
      },
    });
    rememberProcess(ownership, await electronApp.evaluate(() => process.pid));
    const page = await electronApp.firstWindow();
    await page.getByRole("heading", { name: "Set up Tjommie" }).waitFor({ timeout: 120_000 });
    const { port, token } = await daemonConnection(page);
    rememberProcess(ownership, listeningPid(port));
    client = await connect(port, token);
    const app = { electronApp, page, client, root, userData };
    ownerships.set(app, ownership);
    return app;
  } catch (error) {
    await client?.close().catch(() => {});
    await electronApp?.close().catch(() => {});
    const failures: unknown[] = [error];
    await cleanOwnedProcesses(ownership).catch((cleanup: unknown) => failures.push(cleanup));
    await removeFolders(root, userData).catch((cleanup: unknown) => failures.push(cleanup));
    throw allOf(failures, "The app didn't start, and cleaning up after it failed too.");
  }
}

// Close the client and app, then stop only processes still matching this launch's recorded identities.
export async function closeRealApp(app: RealApp | undefined, quit: boolean): Promise<void> {
  if (!app) return;
  const ownership = ownershipFor(app);
  const failures: unknown[] = [];
  await app.client.close().catch(() => {});
  if (!quit) await app.electronApp.close().catch(() => {});
  await cleanOwnedProcesses(ownership).catch((error: unknown) => failures.push(error));
  await removeFolders(app.root, app.userData).catch((error: unknown) => failures.push(error));
  ownerships.delete(app);
  if (failures.length) throw allOf(failures, "The test's processes and folders were not all cleaned up.");
}

// One failure as it is, or every failure in order, the first one first. A process that didn't exit usually still
// holds its folder, so the folder's error alone would hide the cause.
const allOf = (failures: unknown[], message: string): unknown => (failures.length === 1 ? failures[0] : new AggregateError(failures, message));

// Quit as the tray action does. A quit must stop both the Electron main process and its daemon.
export async function quitApp(app: RealApp): Promise<void> {
  const closed = app.electronApp.waitForEvent("close");
  await app.electronApp.evaluate(({ app: electron }) => electron.quit()).catch(() => {});
  await closed;
  const ownership = ownershipFor(app);
  for (const pid of [...ownership.ownedPids.keys()]) await waitForOwnedProcessExit(ownership, pid);
}

// Record a replacement daemon after a reconnect. The pid must have this launch's executable and start time.
export function rememberDaemon(app: RealApp, port: number): number {
  const pid = listeningPid(port);
  rememberProcess(ownershipFor(app), pid);
  return pid;
}

// The restart test deliberately kills its daemon tree, with the same identity guard used by teardown.
export async function terminateOwnedProcess(app: RealApp, pid: number): Promise<void> {
  const ownership = ownershipFor(app);
  const identity = ownership.ownedPids.get(pid);
  if (!identity || !stillOwnsProcess(pid, identity)) throw new Error("The process no longer belongs to this test.");
  try {
    execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"]);
  } catch {
    // The process can exit after the identity check and before taskkill runs.
  }
  await waitForOwnedProcessExit(ownership, pid);
}

// Read the connection through the same bridge as the UI; never put the token in test output.
export async function daemonConnection(page: Page): Promise<{ port: number; token: string }> {
  const result = await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.connection());
  if (!result.ok) throw new Error(result.error);
  return { port: result.port, token: result.token };
}

export const claudeBot = (client: Client, name: string, extra: Partial<BotInput> = {}): Promise<Bot> =>
  client.call("bots.create", {
    name,
    color: "green",
    shape: "pill",
    instructions: "You are a terse test bot. Answer in one short line.",
    provider: "claude",
    model: "haiku",
    ...extra,
  });

// Playwright's default name match is a substring, so "Writer" could otherwise match "CV Writer".
export async function openChat(page: Page, name: string): Promise<void> {
  await page.getByRole("link", { name: new RegExp(`^${name},`) }).click();
  await page.getByRole("heading", { level: 1, name, exact: true }).waitFor();
  await page.getByRole("textbox", { name: `Message ${name}` }).waitFor();
}

export async function send(page: Page, name: string, text: string): Promise<void> {
  const box = page.getByRole("textbox", { name: `Message ${name}` });
  await box.fill(text);
  await box.press("Enter");
  await page.locator('[data-author="you"]', { hasText: text }).waitFor();
}

export const botReply = (page: Page, text: string) => page.locator('[data-author="bot"]', { hasText: text });

export function listeningPid(port: number): number {
  const output = execFileSync("powershell.exe", ["-NoProfile", "-Command", `(Get-NetTCPConnection -LocalPort ${port} -State Listen).OwningProcess`], { encoding: "utf8" });
  const pid = Number(output.trim().split(/\s+/)[0]);
  if (!Number.isInteger(pid) || pid <= 0) throw new Error("The test couldn't identify its daemon process.");
  return pid;
}

export function refused(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      done(false);
    });
    socket.once("error", () => done(true));
  });
}

export async function eventually<T>(check: () => Promise<T | undefined>, timeoutMs = 60_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check().catch(() => undefined);
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`Timed out after ${timeoutMs} ms`);
    await new Promise((done) => setTimeout(done, 500));
  }
}

function ownershipFor(app: RealApp): Ownership {
  const ownership = ownerships.get(app);
  if (!ownership) throw new Error("The app's process ownership was not recorded.");
  return ownership;
}

function processIdentity(pid: number): OwnedProcess | undefined {
  try {
    const command = `$p = Get-Process -Id ${pid} -ErrorAction SilentlyContinue; if ($p) { [pscustomobject]@{ Image = $p.ProcessName; Path = $p.Path; Started = $p.StartTime.ToUniversalTime().ToString('o') } | ConvertTo-Json -Compress }`;
    const output = execFileSync("powershell.exe", ["-NoProfile", "-Command", command], { encoding: "utf8" }).trim();
    if (!output) return undefined;
    const value: unknown = JSON.parse(output);
    if (typeof value !== "object" || value === null || !("Image" in value) || !("Path" in value) || !("Started" in value)) return undefined;
    if (typeof value.Image !== "string" || typeof value.Path !== "string" || typeof value.Started !== "string") return undefined;
    return { image: value.Image, path: value.Path, started: value.Started };
  } catch {
    return undefined;
  }
}

function rememberProcess(ownership: Ownership, pid: number): void {
  const identity = processIdentity(pid);
  const startedAt = identity ? Date.parse(identity.started) : NaN;
  const executableName = basename(ownership.executablePath, extname(ownership.executablePath)).toLowerCase();
  const imageAllowed = identity !== undefined && ["electron", "node", executableName].includes(identity.image.toLowerCase());
  const pathMatches = identity !== undefined && resolve(identity.path).toLowerCase() === ownership.executablePath.toLowerCase();
  const startedAfterTest = Number.isFinite(startedAt) && startedAt >= ownership.startedAt;
  if (!identity || !imageAllowed || !pathMatches || !startedAfterTest) {
    throw new Error(`The test couldn't verify ownership of its process (found=${identity !== undefined}, image=${identity?.image ?? "none"}, pathMatches=${pathMatches}, startedAfterTest=${startedAfterTest}).`);
  }
  ownership.ownedPids.set(pid, identity);
}

function stillOwnsProcess(pid: number, identity: OwnedProcess): boolean {
  const current = processIdentity(pid);
  return current !== undefined && current.image === identity.image && current.path.toLowerCase() === identity.path.toLowerCase() && current.started === identity.started;
}

async function waitForOwnedProcessExit(ownership: Ownership, pid: number): Promise<void> {
  const identity = ownership.ownedPids.get(pid);
  if (!identity) return;
  await eventually(async () => stillOwnsProcess(pid, identity) ? undefined : true, 5_000);
  ownership.ownedPids.delete(pid);
}

async function cleanOwnedProcesses(ownership: Ownership): Promise<void> {
  const errors: unknown[] = [];
  for (const [pid, identity] of ownership.ownedPids) {
    if (!stillOwnsProcess(pid, identity)) {
      ownership.ownedPids.delete(pid);
      continue;
    }
    try {
      execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"]);
    } catch {
      // The process may have exited between the identity check and taskkill.
    }
    try {
      await waitForOwnedProcessExit(ownership, pid);
    } catch (error) {
      errors.push(error);
    }
  }
  if (errors.length) throw new AggregateError(errors, "The test's processes did not all exit.");
}

async function removeFolders(root: string, userData: string): Promise<void> {
  const errors: unknown[] = [];
  for (const folder of [root, userData]) {
    try {
      await remove(folder);
    } catch (error) {
      errors.push(error);
    }
  }
  if (errors.length) throw new AggregateError(errors, "The disposable folders could not be removed.");
}

// Windows can hold a file briefly after its process exits. Never remove anything outside the OS temp folder.
async function remove(folder: string): Promise<void> {
  const disposable = resolve(folder);
  if (!existsSync(disposable)) return;
  if (dirname(disposable) !== resolve(tmpdir()) || lstatSync(disposable).isSymbolicLink()) {
    throw new Error("Refusing to remove a folder outside the disposable temp root.");
  }
  for (let attempt = 0; attempt < 20 && existsSync(disposable); attempt++) {
    try {
      rmSync(disposable, { recursive: true, force: true });
    } catch {
      await new Promise((done) => setTimeout(done, 500));
    }
  }
  if (existsSync(disposable)) throw new Error("The disposable folder could not be removed.");
}
