// Run the installed acceptance harness while sampling visible console-window handles.
// The supplied diagnostic only observes metadata; it never closes existing windows.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";

const [executable, diagnostic] = process.argv.slice(2);
assert(executable && diagnostic, "Supply installed executable and metadata-only snapshot.ps1 paths.");
const powershell = join(process.env.SystemRoot ?? "C:/Windows", "System32/WindowsPowerShell/v1.0/powershell.exe");
const monitor = spawn(powershell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", diagnostic, "-DurationMs", "900000", "-IntervalMs", "50"], { windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
const lines = createInterface({ input: monitor.stdout });
const evidence = { intervalMs: 50, baseline: null, newWindows: [], errors: [], acceptanceExit: null };
let ready;
let stopped = false;
const readiness = new Promise((resolve, reject) => {
  ready = resolve;
  monitor.once("error", reject);
});
const timer = setTimeout(() => { evidence.errors.push("Monitor readiness timed out."); ready(); }, 15000);
lines.on("line", (line) => {
  try {
    const windows = JSON.parse(line);
    assert(Array.isArray(windows) && windows.every((entry) => Number.isSafeInteger(entry.HWND) && Number.isSafeInteger(entry.pid) && typeof entry.class === "string"));
    if (evidence.baseline === null) { evidence.baseline = windows; clearTimeout(timer); ready(); }
    else for (const entry of windows) {
      if (!evidence.baseline.some((old) => old.HWND === entry.HWND) && !evidence.newWindows.some((old) => old.HWND === entry.HWND)) evidence.newWindows.push(entry);
    }
  } catch { evidence.errors.push("Malformed monitor output."); ready(); }
});
monitor.stderr.on("data", () => { evidence.errors.push("Monitor wrote stderr."); ready(); });
monitor.on("exit", () => { if (!stopped) { evidence.errors.push("Monitor exited before acceptance finished."); ready(); } });
try {
  await readiness;
  assert(evidence.baseline !== null && evidence.errors.length === 0, "Console monitor did not become ready.");
  const acceptance = spawn(process.execPath, ["--disable-warning=ExperimentalWarning", join(import.meta.dirname, "verify-installed.mjs"), executable], { windowsHide: true, stdio: "inherit" });
  evidence.acceptanceExit = await new Promise((resolve, reject) => { acceptance.once("error", reject); acceptance.once("exit", (code) => resolve(code)); });
} catch (error) {
  evidence.errors.push(error instanceof Error ? error.message : "Acceptance launch failed.");
} finally {
  clearTimeout(timer);
  stopped = true;
  monitor.kill(); // Only this explicitly owned observer child; never a window host or baseline process.
  await new Promise((resolve) => { if (monitor.exitCode !== null || monitor.signalCode !== null) resolve(); else monitor.once("exit", resolve); });
  lines.close();
  writeFileSync(join(import.meta.dirname, "installed-console-results.json"), `${JSON.stringify(evidence, null, 2)}\n`);
}
assert(evidence.acceptanceExit === 0 && evidence.errors.length === 0 && evidence.newWindows.length === 0, "Installed acceptance or sampled console-window check failed; inspect evidence.");
console.log("Installed acceptance passed with zero new sampled console windows.");
