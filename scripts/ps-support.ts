import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach } from "vitest";

// What the tests of the installer scripts (installer/*.ps1, installer/tjommie.nsi) share: running a PowerShell script or
// snippet in a clean environment, and temp folders that are removed afterwards. Nothing here installs, uninstalls or looks
// at a Tjommie location.

export const repo = resolve(import.meta.dirname, "..");
export const installer = join(repo, "installer");
export const windows = process.platform === "win32";

export const shell = ((): string => {
  for (const candidate of ["pwsh", "powershell"]) {
    if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
  }
  return "powershell";
})();

const folders: string[] = [];
export function temp(): string {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-installer-test-"));
  folders.push(folder);
  return folder;
}
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); });

export type Run = { status: number | null; stdout: string; stderr: string };

// Runs a script from a file, with this environment (not ours: a CI runner's own GITHUB_ACTIONS must not leak in).
export function powershell(file: string, args: string[], env: NodeJS.ProcessEnv): Run {
  const run = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", file, ...args], { encoding: "utf8", windowsHide: true, env });
  return { status: run.status, stdout: run.stdout, stderr: run.stderr };
}

// A script of our own that dot-sources `library` (a file in installer/), runs `body` and prints its result as JSON.
export function runBody(library: string, body: string): unknown {
  const file = join(temp(), "run.ps1");
  writeFileSync(file, `$ErrorActionPreference = 'Stop'\n. '${join(installer, library)}'\n${body}\n`);
  const run = powershell(file, [], { ...process.env });
  if (run.status !== 0) throw new Error(`PowerShell failed (${run.status}): ${run.stderr || run.stdout}`);
  return JSON.parse(run.stdout) as unknown;
}

// A script's code without its comment lines: comments may name the commands the tests forbid.
export function codeOf(path: string): string {
  // Line endings are normalised: git checks files out as CRLF on Windows, and a test that matches across lines must not care.
  return readFileSync(path, "utf8").replace(/\r\n/g, "\n").split("\n").filter((line) => !line.trim().startsWith("#") && !line.trim().startsWith(";")).join("\n");
}

export const scriptsIn = (extension: string): string[] => (existsSync(installer) ? readdirSync(installer).filter((name) => name.endsWith(extension)) : []);
