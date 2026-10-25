import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, cpSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { extractFile } from "@electron/asar";
import { _electron, type ElectronApplication } from "playwright-core";
import { describe, expect, it } from "vitest";
import { tempRoot } from "../../daemon/src/testing.ts";
import { windowsAclDriver, type AclTarget } from "../../daemon/src/windows-acl.ts";
import { daemonConnection, eventually, listeningPid, refused } from "./real-app.ts";
import type { UpdateState } from "./updater.ts";

const repo = resolve(import.meta.dirname, "..", "..", "..");
const appId = "com.tsnyders.tjommie.updatetest";
const productName = "Tjommie Update Test";
const versions = ["0.3.0-test", "0.3.1-test"] as const;

type ProcessInfo = { pid: number; mainWindow: number; main: boolean };
type ProcessSnapshot = { processes: ProcessInfo[]; visibleConhosts: number };
type Bridge = { tjommie: { updates: { state(): Promise<UpdateState>; check(): Promise<void>; restart(): void } } };

function run(file: string, args: string[], env = process.env): Promise<void> {
  return new Promise((resolveRun, reject) => {
    const child = spawn(file, args, { cwd: repo, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    let output = "";
    child.stdout.on("data", (data: Buffer) => { output = (output + data.toString()).slice(-4_000); });
    child.stderr.on("data", (data: Buffer) => { output = (output + data.toString()).slice(-4_000); });
    child.once("error", reject);
    child.once("close", (code) => code === 0 ? resolveRun() : reject(new Error(`${basename(file)} exited ${code}: ${output}`)));
  });
}

function snapshot(exe: string, since: Date): ProcessSnapshot {
  const path = exe.replaceAll("'", "''");
  const command = `$since=[datetime]::Parse('${since.toISOString()}'); $p=@(Get-Process -Name 'Tjommie Update Test' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq '${path}' }); $hosts=@(Get-Process -Name 'conhost' -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 -and $_.StartTime.ToUniversalTime() -ge $since }); [pscustomobject]@{ processes=@($p | ForEach-Object { $cmd=(Get-CimInstance Win32_Process -Filter ('ProcessId=' + $_.Id) -ErrorAction SilentlyContinue).CommandLine; [pscustomobject]@{ pid=$_.Id; mainWindow=[int64]$_.MainWindowHandle; main=[bool]($cmd -and $cmd -notmatch '--type=' -and $cmd -notmatch '--token-stdin') } }); visibleConhosts=$hosts.Count } | ConvertTo-Json -Compress -Depth 4`;
  return JSON.parse(execFileSync("powershell.exe", ["-NoProfile", "-Command", command], { encoding: "utf8", windowsHide: true })) as ProcessSnapshot;
}

function ownerLastWriteSnapshot(): string {
  const local = process.env.LOCALAPPDATA;
  if (!local) throw new Error("LOCALAPPDATA is required for the owner-data safety check.");
  const hash = createHash("sha256");
  const visit = (path: string): void => {
    if (!existsSync(path)) { hash.update(`missing:${path}\n`); return; }
    const entry = lstatSync(path, { bigint: true });
    hash.update(`${path}:${entry.mtimeNs}\n`);
    if (entry.isDirectory()) for (const name of readdirSync(path).sort()) visit(join(path, name));
  };
  visit(join(local, "Tjommie"));
  visit(join(local, "Programs", "Tjommie"));
  return hash.digest("hex");
}

function desktopLogMatches(root: string, pattern: RegExp): string[] {
  const file = join(root, "logs", "desktop.log");
  return existsSync(file) ? [...readFileSync(file, "utf8").matchAll(pattern)].map((match) => match[1] ?? "") : [];
}

function versionAt(exe: string): string {
  const packageFile = extractFile(join(dirname(exe), "resources", "app.asar"), "package.json");
  return (JSON.parse(packageFile.toString()) as { version: string }).version;
}

function clearTestDataAcl(root: string): void {
  if (dirname(resolve(root)) !== resolve(tmpdir())) throw new Error("Refusing ACL cleanup outside the disposable test folder.");
  const targets: AclTarget[] = [
    ["accounts", "claude-main"], ["logs"], ["data"], ["shared-files"], ["daemon-token"],
  ].map((parts) => {
    const path = join(root, ...parts);
    return { path, folder: parts[0] !== "daemon-token", mode: "removeDeny" as const };
  }).filter((target) => existsSync(target.path));
  const result = windowsAclDriver.runSync(targets);
  if (result.failures.length) throw new Error(`Disposable ACL cleanup failed: ${result.failures.join("; ")}`);
}

function clearTestLoginItem(): void {
  const key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
  const args = ["delete", key, "/v", appId, "/f"];
  spawnSync("reg.exe", args, { windowsHide: true, stdio: "ignore" });
  const checked = spawnSync("reg.exe", ["query", key, "/v", appId], { windowsHide: true, stdio: "ignore" });
  if (checked.error) throw checked.error;
  if (checked.status === 0) throw new Error("The test login item remains registered.");
}

async function feedServer(feed: string, allowed: Set<string>): Promise<{ server: Server; url: string }> {
  const server = createServer((request, response) => {
    const name = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname.slice(1));
    if (!allowed.has(name)) { response.writeHead(404).end(); return; }
    const file = join(feed, name);
    const size = statSync(file).size;
    const range = /^bytes=(\d+)-(\d*)$/.exec(request.headers.range ?? "");
    const start = range ? Number(range[1]) : 0;
    const end = range && range[2] ? Number(range[2]) : size - 1;
    if (start > end || end >= size) { response.writeHead(416).end(); return; }
    response.writeHead(range ? 206 : 200, {
      "accept-ranges": "bytes",
      "content-length": end - start + 1,
      ...(range ? { "content-range": `bytes ${start}-${end}/${size}` } : {}),
    });
    createReadStream(file, { start, end }).pipe(response);
  });
  await new Promise<void>((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("The local update feed did not bind a TCP port.");
  return { server, url: `http://127.0.0.1:${address.port}` };
}

describe("packaged silent Windows update", () => {
  it("installs a local release, stops the daemon and relaunches hidden", async () => {
    if (process.platform !== "win32") throw new Error("The packaged updater test requires Windows.");
    const ownerBefore = ownerLastWriteSnapshot();
    const root = tempRoot();
    const out = join(root, "dist-desktop", "installer");
    const installDir = join(root, productName);
    const installedExe = join(installDir, `${productName}.exe`);
    const feed = join(root, "feed");
    const oldInstaller = join(root, "old-installer.exe");
    let launchedAfter = new Date();
    let server: Server | undefined;
    let electronApp: ElectronApplication | undefined;
    try {
      mkdirSync(feed);
      for (const version of versions) {
        await run(process.execPath, ["--disable-warning=ExperimentalWarning", "packages/desktop/scripts/package.ts"], {
          ...process.env,
          TJOMMIE_PACKAGE_APP_ID: appId,
          TJOMMIE_PACKAGE_PRODUCT: productName,
          TJOMMIE_PACKAGE_VERSION: version,
          TJOMMIE_PACKAGE_TEST_ROOT: root,
        });
        const installerName = `Tjommie-Setup-${version}.exe`;
        const source = join(out, installerName);
        if (!existsSync(source)) throw new Error(`Test installer missing: ${installerName}`);
        if (version === versions[0]) cpSync(source, oldInstaller);
        else {
          for (const name of [installerName, `${installerName}.blockmap`, "latest.yml"]) cpSync(join(out, name), join(feed, name));
        }
      }
      const allowed = new Set(readdirSync(feed));
      const started = await feedServer(feed, allowed);
      server = started.server;
      await run(oldInstaller, ["/S", `/D=${installDir}`]);
      if (!existsSync(installedExe)) throw new Error("The test installer did not use the disposable install directory.");
      expect(versionAt(installedExe)).toBe(versions[0]);
      launchedAfter = new Date();

      const env = {
        ...process.env,
        TJOMMIE_HOME: root,
        TJOMMIE_USER_DATA: join(root, "electron"),
        TJOMMIE_ACCOUNTS_DIR: join(root, "accounts"),
        TJOMMIE_PORT: "0",
        TJOMMIE_UPDATE_FEED: started.url,
        TJOMMIE_BROWSER_HEADLESS: "1",
      };
      electronApp = await _electron.launch({ executablePath: installedExe, args: ["--hidden"], env });
      const page = await Promise.race([
        electronApp.firstWindow(),
        electronApp.waitForEvent("close").then(() => { throw new Error("The test app exited before opening a window."); }),
        new Promise<never>((_resolve, reject) => setTimeout(() => reject(new Error("The test app did not open a window within 30 seconds.")), 30_000)),
      ]);
      const initialMainPid = await electronApp.evaluate(() => process.pid);
      await page.waitForLoadState("load");
      const { port } = await eventually(() => daemonConnection(page).catch(() => undefined), 120_000);
      await eventually(async () => desktopLogMatches(root, /Update test window ready: (\d+)/g).includes(String(initialMainPid)) ? true : undefined, 30_000);
      expect(await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().every((window) => !window.isVisible()))).toBe(true);
      const daemonPid = listeningPid(port);
      const firstDaemonCount = desktopLogMatches(root, /Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/g).length;
      expect(firstDaemonCount).toBeGreaterThan(0);
      await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.check());
      const ready = await eventually(async () => {
        const state = await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.state());
        return state.status === "ready" ? state : undefined;
      }, 180_000);
      expect(ready).toEqual({ status: "ready", version: versions[1] });
      const db = join(root, "data", "tjommie.db");
      const dbBefore = statSync(db).mtimeMs;
      await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.restart());
      await eventually(async () => {
        const oldMainGone = !snapshot(installedExe, launchedAfter).processes.some((process) => process.pid === initialMainPid);
        return oldMainGone && await refused(port) ? true : undefined;
      }, 120_000);
      await eventually(async () => snapshot(installedExe, launchedAfter).processes.some((process) => process.pid === daemonPid) ? undefined : true, 30_000);
      await eventually(async () => versionAt(installedExe) === versions[1] ? true : undefined, 120_000);
      const relaunchedPort = await eventually(async () => {
        const ports = desktopLogMatches(root, /Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/g);
        return ports.length > firstDaemonCount ? Number(ports.at(-1)) : undefined;
      }, 120_000);
      const relaunchedDaemonPid = listeningPid(relaunchedPort);
      expect(snapshot(installedExe, launchedAfter).processes.some((process) => process.pid === relaunchedDaemonPid)).toBe(true);
      await eventually(async () => statSync(db).mtimeMs > dbBefore ? true : undefined, 30_000);
      const relaunchedMainPid = await eventually(async () => {
        const pids = desktopLogMatches(root, /Update test window ready: (\d+)/g);
        return pids.find((pid) => Number(pid) !== initialMainPid);
      }, 120_000);
      const relaunched = snapshot(installedExe, launchedAfter);
      const main = relaunched.processes.filter((process) => process.main && process.pid !== initialMainPid);
      expect(main).toHaveLength(1);
      expect(main[0]?.pid).toBe(Number(relaunchedMainPid));
      expect(main[0]?.mainWindow).toBe(0);
      expect(relaunched.visibleConhosts).toBe(0);
      expect(versionAt(installedExe)).toBe(versions[1]);
    } catch (error) {
      console.error("Packaged update assertion failed before teardown:", error);
      throw error;
    } finally {
      try {
        await electronApp?.close().catch(() => {});
        if (existsSync(installDir)) {
          for (const process of snapshot(installedExe, launchedAfter).processes) {
            try { execFileSync("taskkill", ["/PID", String(process.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" }); } catch { /* A test process may have exited. */ }
          }
          const uninstaller = readdirSync(installDir).find((name) => /^Uninstall Tjommie Update Test\.exe$/.test(name));
          if (uninstaller) await run(join(installDir, uninstaller), ["/S"]);
        }
        clearTestLoginItem();
        const closing = server;
        if (closing) await new Promise<void>((resolveClose) => closing.close(() => resolveClose()));
        clearTestDataAcl(root);
        for (let attempt = 0; attempt < 20 && existsSync(root); attempt++) {
          try { rmSync(root, { recursive: true, force: true }); }
          catch { await new Promise((resolveDelay) => setTimeout(resolveDelay, 500)); }
        }
        if (existsSync(root)) throw new Error("The disposable update-test folder could not be removed.");
      } finally {
        try { rmSync(join(process.env.LOCALAPPDATA!, "tjommie-updatetest-updater"), { recursive: true, force: true }); }
        finally { expect(ownerLastWriteSnapshot(), "Owner Tjommie LastWriteTime changed during the update test.").toBe(ownerBefore); }
      }
    }
  }, 1_200_000);
});
