import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterAll, afterEach } from "vitest";

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

// The helpers log to %TEMP%\tjommie-installer-logs. A test run must not write there, so every helper run in this process logs to a
// folder of its own, removed afterwards (tests that read a log pass their own TJOMMIE_HELPER_LOG_DIR).
const helperLogs = mkdtempSync(join(tmpdir(), "tjommie-helper-logs-"));
process.env.TJOMMIE_HELPER_LOG_DIR = helperLogs;
afterAll(() => rmSync(helperLogs, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }));

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

// The PowerShell engines the installer really runs its helpers under: a 32-bit NSIS installer's $SYSDIR is SysWOW64, so the helpers run
// in 32-bit Windows PowerShell 5.1; the 64-bit one is the same code on the other side of WOW64. Tests that only use pwsh 7 can miss
// what only 5.1 does, so the helper tests run under these (those that exist on this machine).
export const windowsPowerShells: Array<[string, string]> = (
  [["32-bit Windows PowerShell 5.1", "SysWOW64"], ["64-bit Windows PowerShell 5.1", "System32"]] as Array<[string, string]>
).map(([label, folder]): [string, string] => [label, join(process.env.SystemRoot ?? "C:\\Windows", folder, "WindowsPowerShell", "v1.0", "powershell.exe")]).filter(([, path]) => existsSync(path));

// Runs a script with a given engine exactly as an installer's nsExec does: -File, no profile, never interactive.
export function runWith(engine: string, file: string, args: string[], env: NodeJS.ProcessEnv): Run {
  const run = spawnSync(engine, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", file, ...args], { encoding: "utf8", windowsHide: true, env });
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
