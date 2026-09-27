import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { expect, it } from "vitest";
import { startDaemon } from "./daemon.ts";
import { dataPaths } from "./paths.ts";
import { tempRoot } from "./testing.ts";
import { windowsAclDriver } from "./windows-acl.ts";

const sid = process.platform === "win32" ? windowsAclDriver.lookupSid() : null;

function icacls(args: string[]): void {
  const result = spawnSync("icacls.exe", args, { encoding: "utf8", windowsHide: true });
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
    expect(windowsAclDriver.sandboxBinReady(sandboxBin, sid!)).toBe(true);
    daemon = await startDaemon({ root, port: 0, auth: { mode: "development" } });
    for (const folder of [join(paths.accounts, "claude-main"), codexHome, paths.logs, paths.data]) {
      expect(windowsAclDriver.hasDeny(folder, sid!, true)).toBe(true);
    }
    expect(windowsAclDriver.hasAllow(sandbox, sid!, true)).toBe(true);
    expect(windowsAclDriver.sandboxBinReady(sandboxBin, sid!)).toBe(true);
    expect(windowsAclDriver.hasDeny(sandboxBin, sid!, true)).toBe(false);
    expect(windowsAclDriver.hasDeny(paths.token, sid!, false)).toBe(true);
    expect(windowsAclDriver.hasDeny(paths.db, sid!, false)).toBe(true);
    expect(windowsAclDriver.hasDeny(paths.bots, sid!, true)).toBe(false);
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
