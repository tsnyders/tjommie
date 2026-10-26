import { execFileSync, spawn, spawnSync } from "node:child_process";
import { appendFileSync, createReadStream, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { extractFile } from "@electron/asar";
import { _electron, type ElectronApplication } from "playwright-core";
import { describe, expect, it } from "vitest";
import { tempRoot } from "../../daemon/src/testing.ts";
import { windowsAclDriver, type AclTarget } from "../../daemon/src/windows-acl.ts";
import { assertOwnerUnchanged, ownerChangedPaths, ownerLastWriteSnapshot } from "./owner-safety.ts";
import { assertTestDownloadedUpdate, assertTestInstaller, assertTestUninstaller, assertTestUpdate, checkTestRegistration, nsisSpawnOptions, readRegistryKeys, testIdentity, testRegistryKeys } from "../scripts/update-test-guards.ts";
import { daemonConnection, eventually, listeningPid, refused } from "./real-app.ts";
import type { UpdateState } from "./updater.ts";

const repo = resolve(import.meta.dirname, "..", "..", "..");
const appId = testIdentity.appId;
const productName = testIdentity.product;
const versions = ["0.3.0-test", "0.3.1-test"] as const;
const daemonListening = /Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/g;

type ProcessInfo = { pid: number; mainWindow: number; main: boolean };
type ProcessSnapshot = { processes: ProcessInfo[]; visibleConhosts: number };
type Bridge = { tjommie: { updates: { state(): Promise<UpdateState>; check(): Promise<void>; restart(): void } } };

function run(file: string, args: string[], env = process.env): Promise<void> {
  return new Promise((resolveRun, reject) => {
    const child = spawn(file, args, { cwd: repo, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"],
      ...(args.at(-1)?.startsWith("/D=") ? nsisSpawnOptions(file) : {}),
    });
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

function relaunchedDaemonPort(root: string, previousCount: number): number | undefined {
  const ports = desktopLogMatches(root, daemonListening);
  return ports.length > previousCount ? Number(ports.at(-1)) : undefined;
}

function hasTestDatabaseSidecar(root: string): boolean {
  return ["-wal", "-shm"].some((suffix) => existsSync(join(root, "data", `tjommie.db${suffix}`)));
}

it("owner safety snapshot narrows live exceptions and detects all other writes", () => {
  const root = tempRoot();
  const local = join(root, "Local");
  const roaming = join(root, "Roaming");
  const data = join(local, "Tjommie");
  const install = join(local, "Programs", "Tjommie");
  const cache = join(local, "tjommie-updater");
  const profile = join(roaming, "Tjommie");
  try {
    for (const path of [join(data, "logs"), join(data, "data"), join(data, "bots"), install, cache, join(profile, "GPUCache")]) mkdirSync(path, { recursive: true });
    writeFileSync(join(data, "logs", "desktop.log"), "start");
    for (const file of [join(data, "data", "tjommie.db-wal"), join(data, "data", "tjommie.db-shm"), join(profile, "GPUCache", "data_1")]) writeFileSync(file, "start");
    const before = ownerLastWriteSnapshot(local, roaming, true, []);
    for (const file of [join(data, "logs", "desktop.log"), join(data, "data", "tjommie.db-wal"), join(data, "data", "tjommie.db-shm"), join(profile, "GPUCache", "data_1")]) appendFileSync(file, "changed");
    writeFileSync(join(data, "logs", "new.log"), "new log");
    expect(ownerChangedPaths(before, ownerLastWriteSnapshot(local, roaming, true, []))).toEqual([]);
    for (const path of [join(data, "settings.json"), join(data, "bots", "other.db-wal"), join(profile, "Cookies-shm"), join(profile, "Preferences"), join(profile, "GPUCache", "data_2"), join(cache, "update.exe"), join(install, "Tjommie.exe")]) {
      const baseline = ownerLastWriteSnapshot(local, roaming, true, []);
      writeFileSync(path, "unexpected");
      expect(ownerChangedPaths(baseline, ownerLastWriteSnapshot(local, roaming, true, []))).toContain(path);
      rmSync(path);
    }
    const settings = join(data, "settings.json");
    writeFileSync(settings, "start");
    const existing = ownerLastWriteSnapshot(local, roaming, true, []);
    utimesSync(settings, new Date(), new Date(Date.now() + 10_000));
    expect(ownerChangedPaths(existing, ownerLastWriteSnapshot(local, roaming, true, []))).toContain(settings);
    const deleting = ownerLastWriteSnapshot(local, roaming, true, []);
    rmSync(settings);
    expect(ownerChangedPaths(deleting, ownerLastWriteSnapshot(local, roaming, true, []))).toContain(settings);
    const beforeTransient = ownerLastWriteSnapshot(local, roaming, true, []);
    const transient = join(profile, "transient");
    writeFileSync(transient, "transient"); rmSync(transient);
    utimesSync(profile, new Date(), new Date(Date.now() + 20_000));
    expect(ownerChangedPaths(beforeTransient, ownerLastWriteSnapshot(local, roaming, true, []))).toContain(profile);
    const closed = ownerLastWriteSnapshot(local, roaming, false, []);
    appendFileSync(join(data, "data", "tjommie.db-wal"), "closed owner");
    expect(ownerChangedPaths(closed, ownerLastWriteSnapshot(local, roaming, false, []))).toContain(join(data, "data", "tjommie.db-wal"));
    expect(ownerChangedPaths(before, ownerLastWriteSnapshot(local, roaming, false, []))).toContain(join(data, "data", "tjommie.db-wal"));
  } finally {
    if (dirname(resolve(root)) !== resolve(tmpdir())) throw new Error("Refusing safety fixture cleanup outside the disposable folder.");
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

it("requires a new daemon listening event after the old daemon exits", () => {
  const root = tempRoot();
  try {
    mkdirSync(join(root, "logs"), { recursive: true });
    const log = join(root, "logs", "desktop.log");
    writeFileSync(log, "Tjommie daemon listening on http://127.0.0.1:40101\n");
    const firstCount = desktopLogMatches(root, daemonListening).length;
    expect(relaunchedDaemonPort(root, firstCount)).toBeUndefined();
    expect(hasTestDatabaseSidecar(root)).toBe(false);
    appendFileSync(log, "Tjommie daemon listening on http://127.0.0.1:40202\n");
    expect(relaunchedDaemonPort(root, firstCount)).toBe(40202);
    mkdirSync(join(root, "data"));
    writeFileSync(join(root, "data", "tjommie.db-wal"), "reopened by test daemon");
    expect(hasTestDatabaseSidecar(root)).toBe(true);
  } finally {
    if (dirname(resolve(root)) !== resolve(tmpdir())) throw new Error("Refusing relaunch fixture cleanup outside the disposable folder.");
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

describe("packaged silent Windows update", () => {
  it("installs a local release, stops the daemon and relaunches hidden", async () => {
    if (process.platform !== "win32") throw new Error("The packaged updater test requires Windows.");
    const ownerBefore = ownerLastWriteSnapshot();
    if (ownerBefore.running) throw new Error("Close the owner's Tjommie before running the packaged update test.");
    const root = mkdtempSync(join(tmpdir(), "tjommie-update-test-"));
    const out = join(root, "dist-desktop", "installer");
    const installDir = join(root, productName);
    const installedExe = join(installDir, `${productName}.exe`);
    const feed = join(root, "feed");
    const manifest = join(feed, "latest.yml");
    const pending = join(process.env.LOCALAPPDATA!, `${testIdentity.packageName}-updater`, "pending");
    const oldInstaller = join(root, "old-installer.exe");
    let launchedAfter = new Date();
    let server: Server | undefined;
    let electronApp: ElectronApplication | undefined;
    const primaryFailures: unknown[] = [];
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
        assertTestInstaller(source, installDir);
        if (version === versions[0]) cpSync(source, oldInstaller);
        else {
          assertTestUpdate(join(out, "latest.yml"), source, installDir, version);
          for (const name of [installerName, `${installerName}.blockmap`, "latest.yml"]) cpSync(join(out, name), join(feed, name));
        }
      }
      const allowed = new Set(readdirSync(feed));
      assertTestUpdate(manifest, join(feed, `Tjommie-Setup-${versions[1]}.exe`), installDir, versions[1]);
      const started = await feedServer(feed, allowed);
      server = started.server;
      assertTestInstaller(oldInstaller, installDir);
      checkTestRegistration(installDir, false);
      const previousUninstaller = join(installDir, `Uninstall ${productName}.exe`);
      if (existsSync(previousUninstaller)) assertTestUninstaller(previousUninstaller, installDir);
      await run(oldInstaller, ["/S", `/D=${installDir}`]);
      if (!existsSync(installedExe)) throw new Error("The test installer did not use the disposable install directory.");
      expect(versionAt(installedExe)).toBe(versions[0]);
      checkTestRegistration(installDir, true);
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
      const firstDaemonCount = desktopLogMatches(root, daemonListening).length;
      expect(firstDaemonCount).toBeGreaterThan(0);
      checkTestRegistration(installDir, true);
      assertTestUninstaller(join(installDir, `Uninstall ${productName}.exe`), installDir);
      assertTestUpdate(manifest, join(feed, `Tjommie-Setup-${versions[1]}.exe`), installDir, versions[1]);
      await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.check());
      const ready = await eventually(async () => {
        const state = await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.state());
        return state.status === "ready" ? state : undefined;
      }, 180_000);
      expect(ready).toEqual({ status: "ready", version: versions[1] });
      checkTestRegistration(installDir, true);
      assertTestUninstaller(join(installDir, `Uninstall ${productName}.exe`), installDir);
      assertTestDownloadedUpdate(manifest, pending, installDir, versions[1]);
      await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.restart());
      await eventually(async () => {
        const oldMainGone = !snapshot(installedExe, launchedAfter).processes.some((process) => process.pid === initialMainPid);
        return oldMainGone && await refused(port) ? true : undefined;
      }, 120_000);
      await eventually(async () => snapshot(installedExe, launchedAfter).processes.some((process) => process.pid === daemonPid) ? undefined : true, 30_000);
      await eventually(async () => hasTestDatabaseSidecar(root) ? undefined : true, 30_000);
      await eventually(async () => versionAt(installedExe) === versions[1] ? true : undefined, 120_000);
      const relaunchedPort = await eventually(async () => relaunchedDaemonPort(root, firstDaemonCount), 120_000);
      await eventually(async () => hasTestDatabaseSidecar(root) ? true : undefined, 30_000);
      const relaunchedDaemonPid = listeningPid(relaunchedPort);
      expect(relaunchedDaemonPid).not.toBe(daemonPid);
      expect(snapshot(installedExe, launchedAfter).processes.some((process) => process.pid === relaunchedDaemonPid)).toBe(true);
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
      primaryFailures.push(error);
      console.error("Packaged update assertion failed before teardown:", error);
      throw error;
    } finally {
      const cleanupFailures: unknown[] = [];
      let uninstallFinished = true;
      // Closing a downloaded update can itself hand off to NSIS. A failed guard leaves only the PID-scoped forced stop below.
      if (electronApp) {
        try {
          checkTestRegistration(installDir, true);
          assertTestUninstaller(join(installDir, `Uninstall ${productName}.exe`), installDir);
          assertTestDownloadedUpdate(manifest, pending, installDir, versions[1]);
          await electronApp.close();
        } catch (error) { cleanupFailures.push(error); }
      }
      if (existsSync(installDir)) {
        try {
          for (const process of snapshot(installedExe, launchedAfter).processes) {
            try { execFileSync("taskkill", ["/PID", String(process.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" }); } catch { /* A test process may have exited. */ }
          }
        } catch (error) { cleanupFailures.push(error); }
        try {
          const uninstaller = readdirSync(installDir).find((name) => /^Uninstall Tjommie Update Test\.exe$/.test(name));
          if (uninstaller) {
            checkTestRegistration(installDir, true);
            assertTestUninstaller(join(installDir, uninstaller), installDir);
            await run(join(installDir, uninstaller), ["/S", `/D=${installDir}`]);
          }
          // The NSIS launcher can exit before its temporary child; retain the fixture until that child finishes.
          await eventually(async () => readRegistryKeys(testRegistryKeys()).every((entry) => !entry.present) ? true : undefined, 30_000);
        } catch (error) { uninstallFinished = false; cleanupFailures.push(error); }
      }
      try { clearTestLoginItem(); } catch (error) { cleanupFailures.push(error); }
      try { const closing = server; if (closing) await new Promise<void>((resolveClose) => closing.close(() => resolveClose())); }
      catch (error) { cleanupFailures.push(error); }
      try { clearTestDataAcl(root); } catch (error) { cleanupFailures.push(error); }
      try {
        if (!uninstallFinished) throw new Error(`Test uninstall did not finish; retained its executable and evidence at ${root}. No registry entries were repaired.`);
        if (dirname(resolve(root)) !== resolve(tmpdir())) throw new Error("Refusing update-test cleanup outside the disposable folder.");
        let cleanupError: unknown;
        for (let attempt = 0; attempt < 60 && existsSync(root); attempt++) {
          try { rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); }
          catch (error) { cleanupError = error; await new Promise((resolveDelay) => setTimeout(resolveDelay, 500)); }
        }
        if (existsSync(root)) throw new Error("The disposable update-test folder could not be removed.", { cause: cleanupError });
      } catch (error) { cleanupFailures.push(error); }
      try { rmSync(join(process.env.LOCALAPPDATA!, "tjommie-updatetest-updater"), { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); }
      catch (error) { cleanupFailures.push(error); }
      try { console.info(assertOwnerUnchanged(ownerBefore, ownerLastWriteSnapshot())); }
      catch (error) { console.error(error); cleanupFailures.push(error); }
      if (cleanupFailures.length) throw new AggregateError([...primaryFailures, ...cleanupFailures], "Owner-folder safety or disposable cleanup failed.", { cause: primaryFailures[0] });
    }
  }, 1_200_000);
});
