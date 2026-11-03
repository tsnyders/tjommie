import type { CliCommand } from "./cli.ts";
import { randomUUID } from "node:crypto";
import { killTreeAndWait, launchCli, waitForCliExit, WorkerLaunchTerminationError } from "./cli.ts";
import { CheckTerminationError } from "./project-checks.ts";
import { parseClaudeLine } from "./providers/claude-parse.ts";
import { redactProviderEvent, type ProviderEvent } from "./providers/types.ts";
import { isRecord, REVIEW_OUTPUT_LIMIT, reviewEffectivePolicy, type ReviewEffectivePolicy } from "./project-review-policy.ts";

export type ReviewCliResult = Readonly<{ text: string; policy: ReviewEffectivePolicy; costUsd: number; model: string }>;
export type ReviewCliOptions = { command: CliCommand; args: string[]; cwd: string; env: Record<string, string>; input: string;
  model: string; signal: AbortSignal; apiKey?: string; onEvent(event: ProviderEvent): void; timeoutMs?: number };

// Drains bounded output and waits for the owned tree before releasing its account or scratch profile.
export async function runReviewCli(options: ReviewCliOptions): Promise<ReviewCliResult> {
  if (options.signal.aborted) throw new Error("review_cancelled");
  const turnId = randomUUID();
  const child = await launchCli(options.command, options.args, { cwd: options.cwd, env: options.env, ...(options.apiKey ? { claudeApiKey: options.apiKey } : {}) }).catch((error: unknown) => {
    if (!(error instanceof WorkerLaunchTerminationError)) throw error;
    try { options.onEvent({ type: "usage", model: options.model, input: 0, cachedInput: 0, output: 0, reasoning: 0, costUsd: null, eventId: `${turnId}:unknown-launch` }); }
    finally { throw new CheckTerminationError("Reviewer launch retirement is unconfirmed; its account and scratch profile are retained."); }
  });
  let usageReliable = false;
  let bytes = 0, pending = "", failure: Error | null = null, policy: ReviewEffectivePolicy | null = null;
  let result: ReviewCliResult | null = null, resultSeen = false, stopped: Promise<void> | null = null;
  let rejectStop: (error: Error) => void = () => {};
  const stopFailure = new Promise<never>((_resolve, reject) => { rejectStop = reject; }); void stopFailure.catch(() => {});
  const stop = (): void => { stopped ??= killTreeAndWait(child.pid); void stopped.catch(() => rejectStop(new CheckTerminationError("Reviewer process-tree termination is unconfirmed; its account and scratch profile are retained."))); };
  const fail = (message: string, terminate = true): void => { failure ??= new Error(message); if (terminate) stop(); };
  const emit = (event: ProviderEvent): void => { try { options.onEvent(redactProviderEvent(event, options.apiKey)); } catch { usageReliable = false; fail("review_usage_record_failed"); } };
  const line = (text: string): void => {
    let raw: unknown;
    try { raw = JSON.parse(text) as unknown; } catch { fail("review_provider_protocol_invalid"); return; }
    if (!isRecord(raw)) { fail("review_provider_protocol_invalid"); return; }
    if (raw.type === "system" && raw.subtype === "init") {
      if (policy) { fail("review_provider_policy_changed"); return; }
      try { policy = reviewEffectivePolicy(raw); } catch { fail("review_effective_tools_unavailable"); }
    }
    if (raw.type === "control_request" || raw.type === "user" && isRecord(raw.message) && Array.isArray(raw.message.content) && raw.message.content.some((block) => isRecord(block) && block.type === "tool_result")) fail("review_tool_attempted");
    const parsed = parseClaudeLine(text);
    if (parsed.kind === "assistant" && parsed.tools.length || parsed.kind === "permission") fail("review_tool_attempted");
    if (parsed.kind === "rate_limit") emit({ type: "limit", state: parsed.state, usedPercent: parsed.usedPercent, resetsAt: parsed.resetsAt, ...(parsed.windows ? { windows: parsed.windows } : {}) });
    if (parsed.kind === "result") {
      if (resultSeen) { fail("review_provider_result_repeated"); return; }
      resultSeen = true;
      const usage = parsed.usage;
      if (!Object.values(usage).every((value) => Number.isSafeInteger(value) && value >= 0) || parsed.totalCostUsd === null || !Number.isFinite(parsed.totalCostUsd) || parsed.totalCostUsd < 0 || !parsed.model) {
        fail("review_usage_unavailable", false); return;
      }
      usageReliable = failure === null || ["review_cancelled", "review_timeout"].includes(failure.message);
      emit({ type: "usage", model: parsed.model, ...usage, costUsd: parsed.totalCostUsd, totalCostUsd: parsed.totalCostUsd, eventId: `${turnId}:${parsed.eventId ?? "result"}` });
      if (!parsed.ok || !policy || typeof raw.result !== "string" || Buffer.byteLength(raw.result) > 262_144) { fail("review_provider_turn_incomplete", false); return; }
      result = { text: raw.result, policy, costUsd: parsed.totalCostUsd, model: parsed.model };
    }
  };
  child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
    bytes += Buffer.byteLength(chunk);
    if (bytes > REVIEW_OUTPUT_LIMIT) { fail("review_output_limit"); return; }
    pending += chunk;
    let newline: number;
    while ((newline = pending.indexOf("\n")) !== -1) { const next = pending.slice(0, newline).replace(/\r$/, ""); pending = pending.slice(newline + 1); if (next) line(next); }
  });
  child.stderr.on("data", (chunk: Buffer) => { bytes += chunk.length; if (bytes > REVIEW_OUTPUT_LIMIT) fail("review_output_limit"); });
  child.stdin.on("error", () => { failure ??= new Error("review_provider_input_closed"); });
  const closed = new Promise<number | null>((resolve, reject) => { child.once("close", resolve); child.once("error", () => reject(new Error("review_provider_start_failed"))); });
  const timer = setTimeout(() => fail("review_timeout"), options.timeoutMs ?? 180_000);
  const abort = (): void => fail("review_cancelled");
  options.signal.addEventListener("abort", abort, { once: true });
  if (options.signal.aborted) abort();
  child.stdin.end(options.input);
  try {
    const code = await Promise.race([closed, stopFailure]);
    try { await waitForCliExit(child, Promise.resolve()); } catch { throw new CheckTerminationError("Reviewer job retirement is unconfirmed; its account and scratch profile are retained."); }
    if (pending) line(pending);
    if (stopped) { try { await stopped; } catch { throw new CheckTerminationError("Reviewer process-tree termination is unconfirmed; its account and scratch profile are retained."); } }
    if (failure) throw failure;
    if (code !== 0 || !result) throw new Error("review_provider_turn_incomplete");
    return result;
  } catch (error) {
    if (stopped) { try { await stopped; } catch { throw new CheckTerminationError("Reviewer process-tree termination is unconfirmed; its account and scratch profile are retained."); } }
    throw error;
  } finally {
    clearTimeout(timer); options.signal.removeEventListener("abort", abort);
    if (child.pid !== undefined && !usageReliable) {
      try { options.onEvent({ type: "usage", model: options.model, input: 0, cachedInput: 0, output: 0, reasoning: 0, costUsd: null, eventId: `${turnId}:unknown` }); }
      catch { throw new CheckTerminationError("Reviewer spending could not be recorded; its account and scratch profile are retained for host reconciliation."); }
    }
  }
}

export async function reviewCliVersion(command: CliCommand, cwd: string, env: Record<string, string>, signal: AbortSignal): Promise<string> {
  if (signal.aborted) throw new Error("review_cancelled");
  const child = await launchCli(command, ["--version"], { cwd, env }); child.stdin.end();
  let output = "", invalid = false, stopped: Promise<void> | null = null;
  const stop = (): void => { stopped ??= killTreeAndWait(child.pid); void stopped.catch(() => {}); };
  const append = (chunk: Buffer): void => { if (output.length + chunk.length > 4096) { invalid = true; stop(); } else output += chunk.toString("utf8"); };
  child.stdout.on("data", append); child.stderr.on("data", append);
  const timer = setTimeout(() => { invalid = true; stop(); }, 10_000);
  signal.addEventListener("abort", stop, { once: true });
  if (signal.aborted) stop();
  try {
    const code = await new Promise<number | null>((resolve, reject) => { child.once("close", resolve); child.once("error", () => reject(new Error("review_cli_unavailable"))); });
    try { await waitForCliExit(child, Promise.resolve()); } catch { throw new CheckTerminationError("Reviewer version-probe job retirement is unconfirmed."); }
    if (stopped) { try { await stopped; } catch { throw new CheckTerminationError("Reviewer version-probe process exit is unconfirmed."); } }
    const version = output.trim().match(/^(2\.1\.(\d+)) \(Claude Code\)$/);
    if (invalid || signal.aborted || code !== 0 || !version || Number(version[2]) < 314) throw new Error("review_cli_version_unsupported");
    return version[1];
  } catch (error) { if (error instanceof WorkerLaunchTerminationError) throw new CheckTerminationError("Reviewer version-probe retirement is unconfirmed."); throw error; }
  finally { clearTimeout(timer); signal.removeEventListener("abort", stop); }
}
