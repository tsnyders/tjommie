import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { createBot, getStoredBot } from "../bots.ts";
import { resolveCli } from "../cli.ts";
import { openDb } from "../db.ts";
import { Runner } from "../runner.ts";
import { accountEnv, ensureAccounts } from "../setup.ts";
import { firstOf, realAccount, realAccountsDir, tempRoot, waitFor } from "../testing.ts";
import { ClaudeSession } from "./claude.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}) {
  const cwd = tempRoot();
  const claude = new ClaudeSession(resolveCli("claude")!, {
    cwd,
    extraDirs: [],
    model: "haiku",
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("claude")),
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  claude.onEvent((event) => events.push(event));
  return { claude, events, cwd };
}

const text = (events: ProviderEvent[]) => events.map((e) => (e.type === "text" ? e.delta : "")).join("");

describe("ClaudeSession against the real CLI", () => {
  it("answers, reports its session id and the turn's cost", async () => {
    const { claude, events } = session();
    await claude.start();
    await claude.send("Reply with exactly: PONG");
    expect(await waitFor(() => firstOf(events, "turn_done"))).toEqual({ type: "turn_done", ok: true, error: null });
    expect(text(events)).toContain("PONG");
    expect(firstOf(events, "session")?.cliSessionId).toMatch(/^[0-9a-f-]{36}$/);
    expect(firstOf(events, "usage")?.costUsd).toBeGreaterThan(0);
    claude.close();
  });

  it("asks before writing in Always ask mode, then writes once allowed", async () => {
    const { claude, events, cwd } = session({ autonomy: "ask" });
    await claude.start();
    await claude.send("Use the Write tool to create hello.txt containing the word hi. Then reply DONE.");
    const approval = await waitFor(() => firstOf(events, "approval"));
    expect(approval).toMatchObject({ tool: "Write", rule: "Write" });
    claude.answer(approval.id, "once");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(existsSync(join(cwd, "hello.txt"))).toBe(true);
    claude.close();
  });

  it("stops a running turn", async () => {
    const { claude, events, cwd } = session({ autonomy: "full" });
    const db = openDb(join(cwd, "stop.db"));
    ensureAccounts(db, realAccountsDir());
    const bot = createBot(db, join(cwd, "bots"), {
      name: "Stop test",
      color: "blue",
      shape: "circle",
      instructions: "You are a terse test bot.",
      provider: "claude",
      model: "haiku",
      autonomy: "full",
    });
    const runner = new Runner({ db, broadcast: () => {}, isSignedIn: () => true, createSession: () => claude });
    try {
      runner.start();
      const message = runner.submit(bot.id, `Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('late'), 30000)". Then tell me what it printed.`, []);
      await waitFor(() => firstOf(events, "tool"));
      expect(firstOf(events, "turn_done")).toBeUndefined();
      await runner.stop(bot.id);
      // Claude can acknowledge interrupt without a result. The 10 s fallback then reports a quiet exit.
      const terminal = await waitFor(() => events.find((event) => event.type === "turn_done" || event.type === "exit"), 20_000);
      expect(terminal).toMatchObject(terminal.type === "exit" ? { error: null } : { ok: false });
      expect(runner.toBot(getStoredBot(db, bot.id)).state).toBe("idle");
      expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "interrupted" });
      expect(db.prepare("SELECT kind, body FROM messages WHERE conversation_id = ? AND author = 'system'").all(bot.id))
        .toEqual([{ kind: "notice", body: "Stopped." }]);
    } finally {
      runner.shutdown();
      await waitFor(() => firstOf(events, "exit"), 20_000);
      db.close();
    }
  });
});
