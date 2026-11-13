import { mkdtempSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

// Helpers shared by the desktop's *.test.ts and *.real.test.ts files. Not used by the app itself.

// File-mocked tests can load a second helper module, so keep the registry on the worker process.
const processWithTempRoots = process as NodeJS.Process & { __tjommieTempRoots?: string[] };
const tempRoots = processWithTempRoots.__tjommieTempRoots ??= [];

// A fresh folder whose path has a space and an accented letter, like many Windows profile paths.
export const tempRoot = (): string => {
  const root = mkdtempSync(join(tmpdir(), "tjommie test ñ-"));
  tempRoots.push(root);
  return root;
};

export function cleanupTempRoots(): void {
  for (const root of tempRoots.splice(0)) {
    try { rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }); }
    catch { /* Locked temp files must not fail or noise a test. */ }
  }
}

// Use the real Tjommie accounts so CLI credential refreshes stay where the daemon reads them: the accounts folder of the
// data root (TJOMMIE_HOME, else %LOCALAPPDATA%\Tjommie). Tests never read or copy credential contents. Databases, bots and
// browser profiles stay disposable. Unlike the old TypeScript helper, this only computes the path; it creates no folders.
export const realAccountsDir = (): string =>
  join(process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie"), "accounts");
