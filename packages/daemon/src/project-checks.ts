import type { ProjectCheck, TaskEvaluation } from "../../protocol/src/projects.ts";
import { killTreeAndWait, launchCli, waitForCliExit, WorkerLaunchTerminationError } from "./cli.ts";

export type CheckResult = TaskEvaluation["checks"][number];
export type CheckRuntime = { run(check: ProjectCheck, cwd: string, signal: AbortSignal): Promise<CheckResult> };
export class CheckTerminationError extends Error {}

// Use only after the trusted caller verifies explicit check acceptance and Full auto autonomy.
export class HostCheckRuntime implements CheckRuntime {
  async run(check: ProjectCheck, cwd: string, signal: AbortSignal): Promise<CheckResult> {
    if (signal.aborted) throw new Error("Check cancelled.");
    const env: Record<string, string> = {};
    for (const key of ["PATH", "SystemRoot", "SystemDrive", "WINDIR", "PATHEXT", "TEMP", "TMP", "COMSPEC"]) {
      if (process.env[key] !== undefined) env[key] = process.env[key];
    }
    const child = await launchCli({ file: check.command, args: [] }, check.args, { cwd, env }).catch((error: unknown) => {
      if (error instanceof WorkerLaunchTerminationError) throw new CheckTerminationError("Could not confirm that the owned check launch stopped. Its worktree remains reserved.");
      throw new Error("The configured check could not start. Isolated hosts require an executable in their protected worker runtime.");
    });
    let output = Buffer.alloc(0), truncated = false, timedOut = false;
    const append = (chunk: Buffer): void => {
      truncated ||= output.length + chunk.length > 262_144;
      output = Buffer.concat([output, chunk.subarray(0, Math.max(0, 262_144 - output.length))]);
    };
    child.stdout.on("data", append); child.stderr.on("data", append);
    let stopped: Promise<void> | null = null;
    let failedStop: (error: CheckTerminationError) => void = () => {};
    const stopFailure = new Promise<never>((_resolve, reject) => { failedStop = reject; }); void stopFailure.catch(() => {});
    const stop = (): void => { stopped ??= killTreeAndWait(child.pid); void stopped.catch(() => failedStop(new CheckTerminationError("Could not confirm that the owned check process tree stopped."))); };
    const timer = setTimeout(() => { timedOut = true; stop(); }, check.timeoutMs);
    signal.addEventListener("abort", stop, { once: true });
    try {
      const closed = new Promise<number | null>((resolve, reject) => {
        child.once("error", () => reject(new Error("The configured check could not start.")));
        child.once("close", resolve);
      });
      const retired = waitForCliExit(child, closed.then(() => {}, () => {})); void retired.catch(() => {});
      child.stdin.on("error", () => {}); child.stdin.end();
      if (signal.aborted) stop();
      let exitCode: number | null;
      try { exitCode = await Promise.race([closed, stopFailure]); }
      catch (error) {
        if (error instanceof CheckTerminationError) throw error;
        try { await retired; } catch { throw new CheckTerminationError("Could not confirm that the owned check process tree stopped."); }
        throw error;
      }
      try { await retired; } catch { throw new CheckTerminationError("Could not confirm that the owned check process tree stopped."); }
      if (stopped) { try { await stopped; } catch { throw new CheckTerminationError("Could not confirm that the owned check process tree stopped."); } }
      return { checkId: check.id, command: check.command, args: check.args, exitCode: timedOut || signal.aborted ? null : exitCode,
        output: output.toString("utf8").slice(0, 262_000) + (timedOut ? "\nCheck timed out." : signal.aborted ? "\nCheck cancelled." : ""), truncated };
    } finally { clearTimeout(timer); signal.removeEventListener("abort", stop); }
  }
}
