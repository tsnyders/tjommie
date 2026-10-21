import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { promisify } from "node:util";
import type { Approval } from "@tjommie/protocol";
import { expect, it, vi } from "vitest";
import { createBot, getStoredBot } from "./bots.ts";
import { cliEnv } from "./cli.ts";
import { openDb } from "./db.ts";
import { JsonRpcLines } from "./providers/codex-rpc.ts";
import type { ProviderSession } from "./providers/types.ts";
import { getWorkItem } from "./queue.ts";
import { cliSessionFactory, Runner } from "./runner.ts";
import { checkSetup, ensureAccounts, listModels } from "./setup.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null ? value as Record<string, unknown> : {};
}

const execute = promisify(execFile);

async function timerPid(marker: string): Promise<number | null> {
  // Only the exact synthetic command contains this fresh UUID; never return other process metadata.
  const psFolder = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0");
  const script = `@(Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" | Where-Object { $_.CommandLine -like '*${marker}*' } | Select-Object -ExpandProperty ProcessId) | ConvertTo-Json -Compress`;
  const { stdout } = await execute(join(psFolder, "powershell.exe"), ["-NoProfile", "-NonInteractive", "-Command", script], {
    windowsHide: true,
    timeout: 10_000,
    env: cliEnv({ PSModulePath: join(psFolder, "Modules") }),
  });
  const result: unknown = stdout.trim() ? JSON.parse(stdout) as unknown : [];
  const matches: unknown[] = Array.isArray(result) ? result : [result];
  expect(matches.length).toBeLessThanOrEqual(1);
  const pid = matches[0];
  if (pid === undefined) return null;
  if (typeof pid !== "number" || !Number.isSafeInteger(pid) || pid <= 0) throw new Error("Invalid synthetic command PID");
  return pid;
}

it.each([false, true])("runs queued Codex work after Stop (wait for actual command PID: %s)", async (waitForCommand) => {
  const root = tempRoot();
  const db = openDb(join(root, "t.db"));
  ensureAccounts(db, realAccountsDir());
  const order: Record<string, unknown>[] = [];
  const startedAt = Date.now();
  const handleLine = JsonRpcLines.prototype.handleLine;
  const trace = vi.spyOn(JsonRpcLines.prototype, "handleLine").mockImplementation(function (this: JsonRpcLines, line: string) {
    let message: Record<string, unknown> = {};
    try { message = record(JSON.parse(line) as unknown); } catch { /* Ignore stray CLI logs. */ }
    const params = record(message.params);
    const turn = record(params.turn);
    const item = record(params.item);
    if (typeof message.method === "string" && !message.method.startsWith("codex/event/")) {
      // Keep only ordering metadata, never prompt, command output or account contents.
      order.push({ elapsedMs: Date.now() - startedAt, method: message.method, turnId: params.turnId ?? turn.id, itemId: params.itemId ?? item.id, type: item.type, status: turn.status ?? item.status });
    }
    handleLine.call(this, line);
  });
  const sessions: ProviderSession[] = [];
  const approvals: Approval[] = [];
  let toolsStarted = 0;
  let toolsSucceeded = 0;
  let sandboxReady = false;
  const runner = new Runner({
    db,
    isSignedIn: () => true,
    codexSandboxReady: () => sandboxReady,
    broadcast: (event, data) => {
      if (event === "approval.added") approvals.push(data as Approval);
    },
    createSession: (bot, options) => {
      const session = cliSessionFactory(bot, options);
      sessions.push(session);
      session.onEvent((event) => {
        if (event.type === "tool") toolsStarted++;
        if (event.type === "tool_done" && event.ok) toolsSucceeded++;
      });
      return session;
    },
  });
  const timer = (word: string, ms: number) => `node -e "setTimeout(() => console.log('${word}'), ${ms})"`;
  // Ask mode is read-only. Find the actual command by its unique marker without writing sandbox files.
  const commandMarker = `TJOMMIE-STOP-${randomUUID()}`;
  const stopCommand = waitForCommand
    ? `node -e "console.log('${commandMarker}:'+process.pid);setTimeout(() => console.log('done'),60000)"`
    : timer("done", 60000);
  const allowed = new Set([timer("first", 8000), stopCommand]);
  let answered = 0;
  const approveTimers = () => {
    for (const approval of approvals.splice(0)) {
      expect(approval.tool).toBe("Shell");
      expect(allowed.has(approval.detail)).toBe(true);
      runner.answerApproval(approval.id, "once");
      answered++;
    }
  };
  try {
    const setup = (await checkSetup(db)).find((entry) => entry.provider === "codex");
    expect(setup?.sandbox).toBe("ready");
    sandboxReady = setup?.sandbox === "ready";
    const model = (await listModels(db, "codex")).find((entry) => entry.isDefault)?.id;
    if (!model) throw new Error("Codex did not report a default model");
    const bot = createBot(db, join(root, "bots"), { name: "Stop regression", color: "blue", shape: "circle", provider: "codex", model, autonomy: "ask", instructions: "Answer briefly and follow the requested test marker exactly." });
    const state = () => runner.toBot(getStoredBot(db, bot.id)).state;
    const first = runner.submit(bot.id, `Run this exact shell command and wait for it: ${timer("first", 8000)}. Then tell me what it printed.`, []);
    await waitFor(() => { approveTimers(); return toolsStarted > 0 && answered > 0; });
    runner.submit(bot.id, "Also end your answer with STOP-REGRESSION-STEER-OK.", []);
    await waitFor(() => { approveTimers(); return state() === "idle"; });
    expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(first.id)).toEqual({ status: "done" });
    expect(toolsSucceeded).toBeGreaterThan(0);
    expect(db.prepare("SELECT id FROM messages WHERE author = ? AND body LIKE '%STOP-REGRESSION-STEER-OK%' AND status = 'done'").get(bot.id)).toBeDefined();
    const before = toolsStarted;
    const approvedBefore = answered;
    const stopped = runner.submit(bot.id, `Run this exact command in the foreground and wait: ${stopCommand}. Do not put it in the background.`, []);
    await waitFor(() => { approveTimers(); return toolsStarted > before && answered > approvedBefore; });
    let commandPid: number | null = null;
    if (waitForCommand) {
      const deadline = Date.now() + 30_000;
      while (commandPid === null && Date.now() < deadline) {
        approveTimers();
        commandPid = await timerPid(commandMarker);
        if (commandPid === null) {
          expect(state()).not.toBe("idle");
          await new Promise<void>((resolve) => setTimeout(resolve, 200));
        }
      }
      expect(commandPid).not.toBeNull();
      order.push({ action: "command-started", pid: commandPid });
    }
    const stoppedSession = sessions.at(-1)!;
    const stoppedPid = stoppedSession.pid();
    expect(stoppedPid).toBeDefined();
    order.push({ action: "stop" });
    await runner.stop(bot.id);
    await waitFor(() => state() === "idle", 20_000);
    await stoppedSession.waitForExit?.();
    expect(() => process.kill(stoppedPid!, 0)).toThrow();
    if (commandPid !== null) {
      expect(() => process.kill(commandPid, 0)).toThrow(/ESRCH/);
      order.push({ action: "command-stopped", pid: commandPid });
    }
    expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(stopped.id)).toEqual({ status: "interrupted" });
    await new Promise<void>((resolve) => setTimeout(resolve, 5_000));
    order.push({ action: "after-stop", state: state(), answered });
    expect(state()).toBe("idle");
    const followup = runner.deliver({ botId: bot.id, source: "routine", text: "Reply with exactly STOP-REGRESSION-ROUTINE-OK." });
    await waitFor(() => getWorkItem(db, followup.id).status === "done", 90_000);
    expect(state()).toBe("idle");
    expect(db.prepare("SELECT id FROM messages WHERE author = ? AND body IN ('STOP-REGRESSION-ROUTINE-OK.', 'STOP-REGRESSION-ROUTINE-OK') AND status = 'done'").get(bot.id)).toBeDefined();
    expect(db.prepare("SELECT id FROM messages WHERE status = 'streaming'").all()).toEqual([]);
    expect(answered).toBeGreaterThan(0);
    expect(sessions).toHaveLength(2);
    const interrupted = order.find((entry) => entry.method === "turn/completed" && entry.status === "interrupted");
    const lateCommand = order.find((entry) => entry.method === "item/started" && entry.turnId === interrupted?.turnId && Number(entry.elapsedMs) > Number(interrupted?.elapsedMs));
    expect(lateCommand).toBeUndefined();
  } finally {
    const evidence = join(root, "codex-stop-order.json");
    writeFileSync(evidence, JSON.stringify(order, null, 2));
    console.log("Codex Stop ordering metadata:", evidence);
    runner.shutdown();
    await Promise.all(sessions.map((session) => session.waitForExit?.()));
    trace.mockRestore();
    db.close();
  }
});
