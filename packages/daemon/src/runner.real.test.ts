import { existsSync } from "node:fs";
import { join } from "node:path";
import type { BotInput, EventData, EventName, Message } from "@tjommie/protocol";
import { afterEach, describe, expect, it } from "vitest";
import { createBot, getStoredBot } from "./bots.ts";
import { killTree } from "./cli.ts";
import { openDb, type Db } from "./db.ts";
import { Runner, cliSessionFactory } from "./runner.ts";
import type { Broadcast } from "./server.ts";
import { ensureAccounts, listModels } from "./setup.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

type SentEvent = { [E in EventName]: { event: E; data: EventData<E> } }[EventName];

const runners: Runner[] = [];

afterEach(() => {
  for (const runner of runners) runner.shutdown();
  runners.length = 0;
});

// A runner on a fresh database in a spaced, accented folder, using the real signed-in account folders.
function harness(root = tempRoot(), db: Db = openDb(join(root, "t.db"))) {
  ensureAccounts(db, realAccountsDir());
  const events: SentEvent[] = [];
  let sessions = 0;
  const broadcast: Broadcast = (event, data) => events.push({ event, data } as SentEvent);
  const runner = new Runner({
    db,
    broadcast,
    isSignedIn: () => true,
    createSession: (bot, options) => {
      sessions++;
      return cliSessionFactory(bot, options);
    },
  });
  runners.push(runner);
  runner.start();
  return {
    root,
    db,
    runner,
    events,
    sessions: () => sessions,
    state: (botId: string) => runner.toBot(getStoredBot(db, botId)).state,
    replies: (botId: string) =>
      events
        .filter((event): event is Extract<SentEvent, { event: "message.updated" }> =>
          event.event === "message.updated" && event.data.author === botId && event.data.kind === "text" && event.data.status === "done",
        )
        .map((event) => event.data.body),
    firstAdded: (botId: string, kind: Message["kind"]) =>
      events.find(
        (event): event is Extract<SentEvent, { event: "message.added" }> =>
          event.event === "message.added" && event.data.conversationId === botId && event.data.kind === kind,
      )?.data,
  };
}

let count = 0;
const newBot = (h: ReturnType<typeof harness>, overrides: Partial<BotInput> = {}) =>
  createBot(h.db, join(h.root, "bots"), {
    name: `Bot ${++count}`,
    color: "blue",
    shape: "circle",
    instructions: "You are a terse test bot. Answer in one short line.",
    provider: "claude",
    model: "haiku",
    ...overrides,
  });

const LONG_COMMAND = (seconds: number) =>
  `Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('done'), ${seconds * 1000})". Then tell me what it printed.`;

describe("Runner against the real CLIs", () => {
  it("a Claude bot answers, streams, records usage and saves its session", async () => {
    const h = harness();
    const bot = newBot(h);
    h.runner.submit(bot.id, "Reply with exactly: PONG", []);
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("PONG")));
    await waitFor(() => h.state(bot.id) === "idle");
    expect(h.events.some((event) => event.event === "message.delta")).toBe(true);
    expect((h.db.prepare("SELECT cost_usd FROM usage WHERE bot_id = ?").get(bot.id) as { cost_usd: number }).cost_usd).toBeGreaterThan(0);
    expect(h.db.prepare("SELECT provider FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ provider: "claude" });
  });

  it("a Codex bot answers", async () => {
    const h = harness();
    const model = (await listModels(h.db, "codex")).find((item) => item.isDefault)?.id;
    if (!model) throw new Error("Codex did not report a default model");
    const bot = newBot(h, { provider: "codex", model });
    h.runner.submit(bot.id, "Reply with exactly: PONG", []);
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("PONG")));
    await waitFor(() => h.state(bot.id) === "idle");
  });

  it("two quick messages to an idle bot start one session and reach it in order (double send)", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "full" });
    const first = h.runner.submit(bot.id, LONG_COMMAND(8), []);
    const second = h.runner.submit(bot.id, "Also end your reply with the word BANANA.", []);
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("BANANA")));
    await waitFor(() => h.state(bot.id) === "idle");
    expect(h.sessions()).toBe(1);
    const statuses = h.db.prepare("SELECT status FROM messages WHERE id IN (?, ?)").all(first.id, second.id) as { status: string }[];
    expect(statuses.map((status) => status.status)).toEqual(["sent", "sent"]);
  });

  it("turns a permission request into a card; Always allow saves the rule and the turn carries on", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "ask" });
    h.runner.submit(bot.id, "Use the Write tool to create hello.txt containing the word hi. Then reply DONE.", []);
    const added = await waitFor(() => h.events.find((event): event is Extract<SentEvent, { event: "approval.added" }> => event.event === "approval.added"));
    expect(h.state(bot.id)).toBe("waiting");
    expect(h.events.some((event) => event.event === "notify")).toBe(true);
    h.runner.answerApproval(added.data.id, "always");
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("DONE")));
    expect(existsSync(join(bot.homeFolder, "hello.txt"))).toBe(true);
    expect(h.db.prepare("SELECT rule FROM allow_rules WHERE bot_id = ?").all(bot.id)).toEqual([{ rule: "Write" }]);
  });

  it("gets back to Idle with a Retry when the CLI dies mid-turn (CLI killed mid-turn)", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "full" });
    const message = h.runner.submit(bot.id, "Write a response of at least 2,000 words. Start immediately and do not use tools.", []);
    await waitFor(() => h.events.find((event) => event.event === "message.delta" && event.data.conversationId === bot.id));
    killTree(h.runner.processId(bot.id));
    const error = await waitFor(() => h.firstAdded(bot.id, "error"));
    expect(error.data).toEqual({ retryMessageId: message.id });
    await waitFor(() => h.state(bot.id) === "idle");
    expect(h.db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "failed" });
    const partial = h.db
      .prepare("SELECT body, status FROM messages WHERE conversation_id = ? AND author = ? AND kind = 'text' ORDER BY seq DESC LIMIT 1")
      .get(bot.id, bot.id) as { body: string; status: string };
    expect(partial.body.length).toBeGreaterThan(0);
    expect(partial.status).toBe("failed");
  });

  it("keeps a conversation across a restart (resume) and across a fresh session (recap)", async () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const first = harness(root, db);
    const bot = newBot(first);
    first.runner.submit(bot.id, "Remember the code word ZEBRA. Reply with just OK.", []);
    await waitFor(() => first.replies(bot.id).length > 0);
    await waitFor(() => first.state(bot.id) === "idle");
    first.runner.shutdown();

    const second = harness(root, db); // the same database: a restarted daemon
    second.runner.submit(bot.id, "What was the code word? Reply with just the word.", []);
    await waitFor(() => second.replies(bot.id).some((reply) => /ZEBRA/i.test(reply)));
    await waitFor(() => second.state(bot.id) === "idle");

    db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(bot.id); // the next turn has to start a fresh CLI session
    second.runner.botChanged(bot.id, false);
    const before = second.replies(bot.id).length;
    second.runner.submit(bot.id, "Say the code word again. Reply with just the word.", []);
    await waitFor(() => {
      const replies = second.replies(bot.id);
      const latest = replies.at(-1);
      return replies.length > before && latest !== undefined && /ZEBRA/i.test(latest);
    });
  });
});
