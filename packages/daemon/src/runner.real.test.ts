import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BotInput, EventData, EventName, Message } from "@tjommie/protocol";
import { afterEach, describe, expect, it } from "vitest";
import { createBot, getStoredBot } from "./bots.ts";
import { killTree } from "./cli.ts";
import { openDb, type Db } from "./db.ts";
import { McpTokens } from "./mcp.ts";
import type { ProviderEvent } from "./providers/types.ts";
import type { WorkItem } from "./queue.ts";
import { Runner, cliSessionFactory, type RunnerHooks, type TurnResult } from "./runner.ts";
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
function harness(root = tempRoot(), db: Db = openDb(join(root, "t.db")), hooks: RunnerHooks = {}) {
  ensureAccounts(db, realAccountsDir());
  const events: SentEvent[] = [];
  const providerEvents: ProviderEvent[] = [];
  let sessions = 0;
  const broadcast: Broadcast = (event, data) => events.push({ event, data } as SentEvent);
  const runner = new Runner({
    db,
    broadcast,
    isSignedIn: () => true,
    createSession: (bot, options) => {
      sessions++;
      const session = cliSessionFactory(bot, options);
      session.onEvent((event) => providerEvents.push(event));
      return session;
    },
    hooks,
  });
  runners.push(runner);
  runner.start();
  return {
    root,
    db,
    runner,
    events,
    providerEvents,
    sessions: () => sessions,
    state: (botId: string) => runner.toBot(getStoredBot(db, botId)).state,
    replies: (botId: string) =>
      events
        .filter((event): event is Extract<SentEvent, { event: "message.updated" }> =>
          event.event === "message.updated" && event.data.author === botId && event.data.kind === "text" && event.data.status === "done",
        )
        .map((event) => event.data.body),
    // Finished texts written into one conversation: a bot's chat, "team" or an exchange.
    said: (conversationId: string) =>
      events
        .filter((event): event is Extract<SentEvent, { event: "message.updated" }> =>
          event.event === "message.updated" && event.data.conversationId === conversationId && event.data.kind === "text" && event.data.status === "done",
        )
        .map((event) => event.data.body),
    firstAdded: (conversationId: string, kind: Message["kind"]) =>
      events.find(
        (event): event is Extract<SentEvent, { event: "message.added" }> =>
          event.event === "message.added" && event.data.conversationId === conversationId && event.data.kind === kind,
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

  it("stops a Codex long command and stays idle for five seconds after interrupt", async () => {
    const h = harness();
    const model = (await listModels(h.db, "codex")).find((item) => item.isDefault)?.id;
    if (!model) throw new Error("Codex did not report a default model");
    const bot = newBot(h, { provider: "codex", model, autonomy: "full" });
    const started = join(bot.homeFolder, "stop-started.txt");
    const finished = join(bot.homeFolder, "stop-finished.txt");
    const command = `Set-Content -LiteralPath '${started.replaceAll("'", "''")}' -Value started; Start-Sleep -Seconds 60; Set-Content -LiteralPath '${finished.replaceAll("'", "''")}' -Value finished`;
    const message = h.runner.submit(bot.id, `Run this exact PowerShell command in the foreground using your shell tool. Wait for it to finish, then say DONE. Command: ${command}`, []);
    await waitFor(() => existsSync(started));
    expect(h.state(bot.id)).toBe("working");
    expect(h.providerEvents.some((event) => event.type === "tool")).toBe(true);
    const interruptFrom = h.providerEvents.length;
    await h.runner.stop(bot.id);
    await waitFor(() => h.state(bot.id) === "idle", 20_000);
    const idleFrom = h.events.length;
    expect(h.db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "interrupted" });
    await new Promise<void>((resolve) => setTimeout(resolve, 5_000));
    expect(h.state(bot.id)).toBe("idle");
    expect(h.events.slice(idleFrom).some((event) => event.event === "bot.updated" && event.data.id === bot.id && event.data.state === "working")).toBe(false);
    expect(existsSync(finished)).toBe(false);
    const eventOrder = h.providerEvents.slice(interruptFrom).map((event) => event.type);
    const evidence = join(h.root, "codex-stop-event-order.json");
    writeFileSync(evidence, JSON.stringify({ eventOrder, stableIdleMs: 5_000, itemStatus: "interrupted" }, null, 2));
    console.log("Codex provider events after turn/interrupt:", eventOrder.join(", "), "Evidence:", evidence);
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
    const bot = newBot(h, { autonomy: "ask" });
    const message = h.runner.submit(bot.id, "Use the Write tool to create crash.txt containing the word hi. Then reply DONE.", []);
    // An unanswered permission request holds the real turn open, regardless of response length or kill latency.
    const approval = await waitFor(() => h.events.find((event): event is Extract<SentEvent, { event: "approval.added" }> =>
      event.event === "approval.added" && event.data.botId === bot.id,
    ));
    expect(approval.data.tool).toBe("Write");
    expect(h.state(bot.id)).toBe("waiting");
    expect(h.db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "running" });
    const pid = h.runner.processId(bot.id);
    expect(pid).toBeDefined();
    killTree(pid);
    const error = await waitFor(() => h.firstAdded(bot.id, "error"), 20_000);
    expect(error.data).toEqual({ retryMessageId: message.id });
    await waitFor(() => h.state(bot.id) === "idle", 20_000);
    expect(h.runner.processId(bot.id)).toBeUndefined();
    expect(h.db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "failed" });
    expect(h.db.prepare("SELECT status FROM approvals WHERE id = ?").get(approval.data.id)).toEqual({ status: "denied" });
    expect(existsSync(join(bot.homeFolder, "crash.txt"))).toBe(false);
  });

  it("keeps a conversation across a restart (resume) and across a fresh session (recap)", async () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const first = harness(root, db);
    const bot = newBot(first);
    first.runner.submit(bot.id, "Remember the code word ZEBRA. Reply with just OK.", []);
    await waitFor(() => first.replies(bot.id).length > 0);
    await waitFor(() => first.state(bot.id) === "idle");
    const sessionCost = () => (db.prepare("SELECT total_cost_usd FROM sessions WHERE bot_id = ?").get(bot.id) as { total_cost_usd: number }).total_cost_usd;
    const turnCosts = () => (db.prepare("SELECT cost_usd FROM usage WHERE bot_id = ? ORDER BY rowid").all(bot.id) as { cost_usd: number }[]).map((row) => row.cost_usd);
    const firstTotal = sessionCost();
    expect(firstTotal).toBeGreaterThan(0);
    expect(turnCosts()).toEqual([firstTotal]);
    first.runner.shutdown();

    const second = harness(root, db); // the same database: a restarted daemon
    second.runner.submit(bot.id, "What was the code word? Reply with just the word.", []);
    await waitFor(() => second.replies(bot.id).some((reply) => /ZEBRA/i.test(reply)));
    await waitFor(() => second.state(bot.id) === "idle");
    expect(turnCosts()).toHaveLength(2);
    expect(turnCosts()[1]).toBeCloseTo(sessionCost() - firstTotal, 10);
    expect(turnCosts()[1]).toBeGreaterThan(0);

    db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(bot.id); // the next turn has to start a fresh CLI session
    second.runner.botChanged(bot.id, false);
    const before = second.replies(bot.id).length;
    second.runner.submit(bot.id, "Say the code word again. Reply with just the word.", []);
    await waitFor(() => {
      const replies = second.replies(bot.id);
      const latest = replies.at(-1);
      return replies.length > before && latest !== undefined && /ZEBRA/i.test(latest);
    });
    await waitFor(() => second.state(bot.id) === "idle");
    expect(turnCosts()).toHaveLength(3);
    expect(turnCosts()[2]).toBe(sessionCost());
    expect(turnCosts()[2]).toBeGreaterThan(0);
  });

  it("writes a turn where its work belongs, and doesn't steer a chat message into a Team room answer", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "full" });
    h.runner.deliver({ botId: bot.id, source: "room", text: `${LONG_COMMAND(8)} End your reply with the word ROOM.`, conversationId: "team" });
    await waitFor(() => h.firstAdded("team", "activity"));
    const message = h.runner.submit(bot.id, "Reply with exactly: CHAT", []);
    expect(h.db.prepare("SELECT status FROM messages WHERE id = ?").get(message.id)).toEqual({ status: "queued" });
    await waitFor(() => h.said(bot.id).some((text) => text.includes("CHAT")));
    await waitFor(() => h.state(bot.id) === "idle");
    expect(h.said("team").some((text) => text.includes("ROOM"))).toBe(true);
    expect(h.said("team").some((text) => text.includes("CHAT"))).toBe(false);
    expect(h.said(bot.id).some((text) => text.includes("ROOM"))).toBe(false);
  });

  it("tells the hooks how each turn went, and revokes the session's MCP token when it closes (token revoked on close)", async () => {
    const tokens = new McpTokens();
    const started: WorkItem[] = [];
    const ended: [WorkItem, TurnResult][] = [];
    let token = "";
    const h = harness(undefined, undefined, {
      mcpServers: (bot) => {
        token = tokens.issue(bot.id);
        return [];
      },
      sessionClosed: (botId) => tokens.revoke(botId),
      turnStarted: (item) => started.push(item),
      turnEnded: (item, result) => ended.push([item, result]),
    });
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "Reply with exactly: PONG", []);
    await waitFor(() => ended.length > 0);
    expect(started).toHaveLength(1);
    expect(ended).toHaveLength(1);
    expect(started[0]).toMatchObject({ botId: bot.id, messageId: message.id });
    expect(ended[0][0].id).toBe(started[0].id);
    expect(ended[0][1]).toMatchObject({ outcome: "done", text: expect.stringContaining("PONG"), error: null });
    expect(h.db.prepare("SELECT conversation_id, body, status FROM messages WHERE id = ?").get(ended[0][1].textMessageId)).toEqual({
      conversation_id: bot.id,
      body: ended[0][1].text,
      status: "done",
    });
    expect(tokens.botFor(token)).toBe(bot.id);
    h.runner.forget(bot.id);
    expect(tokens.botFor(token)).toBeNull();
  });
});
