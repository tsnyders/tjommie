import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { expect, it } from "vitest";
import { startDaemon } from "./daemon.ts";
import { dataPaths } from "./paths.ts";
import { tempRoot } from "./testing.ts";
import { protectCodexDataAsync, windowsAclDriver } from "./windows-acl.ts";

const sid = process.platform === "win32" ? windowsAclDriver.runSync([]).sid : null;

function icacls(args: string[]): void {
  const result = spawnSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "icacls.exe"), args, { encoding: "utf8", windowsHide: true });
  if (result.error || result.status !== 0) throw new Error(`Could not prepare disposable ACL: ${result.error?.message ?? result.stderr}`);
}

it.skipIf(!sid)("protects a disposable daemon root with real Windows deny ACEs", async () => {
  const root = tempRoot();
  const paths = dataPaths(root);
  const codexHome = join(paths.accounts, "codex-main");
  const sandbox = join(codexHome, ".sandbox");
  const sandboxBin = join(codexHome, ".sandbox-bin");
  let daemon: Awaited<ReturnType<typeof startDaemon>> | null = null;
  try {
    mkdirSync(join(paths.accounts, "claude-main"));
    mkdirSync(sandbox, { recursive: true });
    mkdirSync(sandboxBin);
    icacls([sandboxBin, "/inheritance:d"]);
    icacls([sandboxBin, "/grant", `*${sid!}:(OI)(CI)(RX)`]);
    expect(windowsAclDriver.runSync([{ path: sandboxBin, folder: true, mode: "verifyBin" }]).failures).toEqual([]);
    daemon = await startDaemon({ root, port: 0, auth: { mode: "development" } });
    const checks = [
      ...[join(paths.accounts, "claude-main"), codexHome, paths.logs, paths.data, paths.sharedFiles, paths.token, paths.db]
        .map((path) => ({ path, folder: ![paths.token, paths.db].includes(path), mode: "checkDeny" as const })),
      { path: sandbox, folder: true, mode: "checkAllow" as const },
      { path: sandboxBin, folder: true, mode: "verifyBin" as const },
      { path: sandboxBin, folder: true, mode: "checkNoDeny" as const },
      { path: paths.bots, folder: true, mode: "checkNoDeny" as const },
    ];
    expect(windowsAclDriver.runSync(checks).failures).toEqual([]);
    const botHome = join(paths.bots, "provider-switch-fixture");
    mkdirSync(botHome);
    const note = join(botHome, "notes.md");
    const inbox = join(botHome, "inbox");
    mkdirSync(inbox);
    writeFileSync(note, "fixture");
    const bot = { id: "provider-switch-fixture", homeFolder: botHome, provider: "claude" as const };
    expect((await protectCodexDataAsync(paths, [bot])).ready).toBe(true);
    expect(windowsAclDriver.runSync([{ path: botHome, folder: true, mode: "checkDeny" }]).failures).toEqual([]);
    expect((await protectCodexDataAsync(paths, [{ ...bot, provider: "codex" }])).ready).toBe(true);
    expect(windowsAclDriver.runSync([
      { path: botHome, folder: true, mode: "checkNoDeny" },
      { path: note, folder: false, mode: "checkNoDeny" },
      { path: inbox, folder: true, mode: "checkNoDeny" },
    ]).failures).toEqual([]);
    expect((await protectCodexDataAsync(paths, [bot])).ready).toBe(true);
    expect(windowsAclDriver.runSync([{ path: botHome, folder: true, mode: "checkDeny" }]).failures).toEqual([]);
  } finally {
    await daemon?.close();
    const disposable = resolve(root);
    if (dirname(disposable) !== resolve(tmpdir()) || lstatSync(disposable).isSymbolicLink()) throw new Error("Refusing to remove a non-disposable test root");
    for (let attempt = 0; attempt < 20 && existsSync(disposable); attempt++) {
      try { rmSync(disposable, { recursive: true, force: true }); }
      catch (error) {
        if (!error || typeof error !== "object" || !("code" in error) || !["EPERM", "EBUSY", "ENOTEMPTY"].includes(String(error.code))) throw error;
        await new Promise<void>((done) => setTimeout(done, 500));
      }
    }
    expect(existsSync(disposable)).toBe(false);
  }
});
