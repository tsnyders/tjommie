import { spawn } from "node:child_process";
import type { ProjectCheck, TaskEvaluation } from "../../protocol/src/projects.ts";
import { killTreeAndWait } from "./cli.ts";

export type CheckResult = TaskEvaluation["checks"][number];
export type CheckRuntime = { run(check: ProjectCheck, cwd: string, signal: AbortSignal): Promise<CheckResult> };
export class CheckTerminationError extends Error {}

// Use only after the trusted caller verifies explicit check acceptance and Full auto autonomy.
export class HostCheckRuntime implements CheckRuntime {
  async run(check: ProjectCheck, cwd: string, signal: AbortSignal): Promise<CheckResult> {
    if (signal.aborted) throw new Error("Check cancelled.");
    const env: NodeJS.ProcessEnv = {};
    for (const key of ["PATH", "SystemRoot", "SystemDrive", "WINDIR", "PATHEXT", "TEMP", "TMP", "COMSPEC"]) {
      if (process.env[key] !== undefined) env[key] = process.env[key];
    }
    const child = spawn(check.command, check.args, { cwd, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    let output = Buffer.alloc(0), truncated = false, timedOut = false;
    const append = (chunk: Buffer): void => {
      truncated ||= output.length + chunk.length > 262_144;
      output = Buffer.concat([output, chunk.subarray(0, Math.max(0, 262_144 - output.length))]);
    };
    child.stdout.on("data", append); child.stderr.on("data", append);
    let stopped: Promise<void> | null = null;
    const stop = (): void => { stopped ??= killTreeAndWait(child.pid); void stopped.catch(() => {}); };
    const timer = setTimeout(() => { timedOut = true; stop(); }, check.timeoutMs);
    signal.addEventListener("abort", stop, { once: true });
    try {
      const exitCode = await new Promise<number | null>((resolve, reject) => {
        child.once("error", () => reject(new Error("The configured check could not start.")));
        child.once("close", resolve);
      });
      if (stopped) { try { await stopped; } catch { throw new CheckTerminationError("Could not confirm that the owned check process tree stopped."); } }
      return { checkId: check.id, command: check.command, args: check.args, exitCode: timedOut || signal.aborted ? null : exitCode,
        output: output.toString("utf8").slice(0, 262_000) + (timedOut ? "\nCheck timed out." : signal.aborted ? "\nCheck cancelled." : ""), truncated };
    } finally { clearTimeout(timer); signal.removeEventListener("abort", stop); }
  }
}
