import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { createBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, lastItemForMessage, newChain, nextQueued, nextSteerable, recoverAfterRestart, setStatus, useChainRequest, type NewWork, type WorkSource } from "./queue.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let botId: string;

beforeEach(() => {
  const root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id;
  return () => db.close();
});

const add = (text: string, source: WorkSource = "you", extra: Omit<NewWork, "botId" | "source" | "text"> = {}) =>
  enqueue(db, { botId, source, text, ...extra, messageId: extra.messageId ?? null });

const tiedCreatedAt = "2026-09-26T10:00:00.000Z";

describe("work queue", () => {
  it("hands out queued work oldest first within a priority, even within one millisecond", () => {
    const [a, b, c] = [add("a"), add("b"), add("c")];
    db.prepare("UPDATE work_items SET created_at = ?").run(tiedCreatedAt);
    expect(nextQueued(db, botId)?.id).toBe(a.id);
    setStatus(db, a.id, "running");
    expect(nextQueued(db, botId)?.id).toBe(b.id);
    setStatus(db, b.id, "done");
    expect(nextQueued(db, botId)?.id).toBe(c.id);
    setStatus(db, c.id, "done");
    expect(nextQueued(db, botId)).toBeNull();
  });

  it("puts your messages and replies ahead of bot requests and routine runs", () => {
    const routine = add("routine", "routine");
    const request = add("request", "bot_request");
    const reply = add("reply", "bot_reply");
    const room = add("room", "room", { conversationId: "team" });
    const mine = add("mine");
    db.prepare("UPDATE work_items SET created_at = ?").run(tiedCreatedAt);
    expect(db.prepare("SELECT source, priority FROM work_items ORDER BY rowid").all()).toEqual([
      { source: "routine", priority: 1 },
      { source: "bot_request", priority: 1 },
      { source: "bot_reply", priority: 0 },
      { source: "room", priority: 0 },
      { source: "you", priority: 0 },
    ]);
    for (const item of [reply, room, mine, routine, request]) {
      expect(nextQueued(db, botId)?.id).toBe(item.id);
      setStatus(db, item.id, "done");
    }
  });

  it("defaults the conversation to the bot's own chat", () => {
    const item = enqueue(db, { botId, source: "you", text: "x" });
    expect(item.conversationId).toBe(botId);
    expect(getWorkItem(db, item.id)).toMatchObject({
      conversationId: botId,
      messageId: null,
      chainId: null,
      exchangeId: null,
      routineRunId: null,
    });
    expect(db.prepare("SELECT conversation_id FROM work_items WHERE id = ?").get(item.id)).toEqual({ conversation_id: botId });
    expect(add("y", "room", { conversationId: "team" }).conversationId).toBe("team");
  });

  it("round-trips the destination and chain, exchange and routine metadata", () => {
    const item = add("reply text", "bot_reply", {
      messageId: "m1",
      conversationId: "exchange-1",
      chainId: "chain-1",
      exchangeId: "exchange-1",
      routineRunId: "run-1",
    });
    expect(getWorkItem(db, item.id)).toEqual({
      id: item.id,
      botId,
      source: "bot_reply",
      text: "reply text",
      messageId: "m1",
      conversationId: "exchange-1",
      chainId: "chain-1",
      exchangeId: "exchange-1",
      routineRunId: "run-1",
      status: "queued",
      createdAt: item.createdAt,
    });
    expect(nextQueued(db, botId)).toEqual(item);
  });

  it("rejects stored work with a null conversation_id", () => {
    db.prepare(
      "INSERT INTO work_items (id, bot_id, source, text, status, created_at, conversation_id) VALUES (?, ?, 'you', 'broken', 'queued', ?, NULL)",
    ).run("null-conversation", botId, tiedCreatedAt);
    expect(() => getWorkItem(db, "null-conversation")).toThrow("Work item null-conversation has a null conversation_id");
  });

  it("only steers your messages and replies, and only into the same conversation", () => {
    add("room post", "room", { conversationId: "team" });
    add("request", "bot_request", { conversationId: "team" });
    add("routine", "routine", { conversationId: "team" });
    expect(nextSteerable(db, botId, botId)).toBeNull();
    expect(nextSteerable(db, botId, "team")).toBeNull();
    const otherBot = createBot(db, join(tempRoot(), "bots"), { name: "Other", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" });
    const otherWork = enqueue(db, { botId: otherBot.id, source: "you", text: "other bot", conversationId: botId });
    db.prepare("UPDATE work_items SET created_at = ? WHERE id = ?").run("2000-01-01T00:00:00.000Z", otherWork.id);
    expect(nextSteerable(db, botId, botId)).toBeNull();
    expect(nextQueued(db, botId)?.botId).toBe(botId);
    const mine = add("mine", "you");
    const reply = add("reply", "bot_reply");
    db.prepare("UPDATE work_items SET created_at = ?").run(tiedCreatedAt);
    expect(nextSteerable(db, botId, botId)?.id).toBe(mine.id);
    expect(nextSteerable(db, botId, "team")).toBeNull();
    setStatus(db, mine.id, "running");
    expect(nextSteerable(db, botId, botId)?.id).toBe(reply.id);
    setStatus(db, reply.id, "done");
    expect(nextSteerable(db, botId, botId)).toBeNull();
    expect(nextSteerable(db, otherBot.id, botId)?.id).toBe(otherWork.id);
  });

  it("puts running work back in the queue with its start time cleared", () => {
    const item = add("x");
    setStatus(db, item.id, "running");
    setStatus(db, item.id, "queued");
    expect(getWorkItem(db, item.id).status).toBe("queued");
    expect(db.prepare("SELECT started_at FROM work_items WHERE id = ?").get(item.id)).toEqual({ started_at: null });
    for (const status of ["done", "failed", "interrupted"] as const) {
      setStatus(db, item.id, "running");
      expect(getWorkItem(db, item.id).status).toBe("running");
      expect(db.prepare("SELECT started_at, ended_at FROM work_items WHERE id = ?").get(item.id)).toEqual({
        started_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/),
        ended_at: null,
      });
      setStatus(db, item.id, status);
      expect(getWorkItem(db, item.id).status).toBe(status);
      expect(db.prepare("SELECT ended_at FROM work_items WHERE id = ?").get(item.id)).toEqual({ ended_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/) });
      setStatus(db, item.id, "queued");
      expect(db.prepare("SELECT started_at, ended_at FROM work_items WHERE id = ?").get(item.id)).toEqual({ started_at: null, ended_at: null });
    }
  });

  it("finds the latest attempt for a message", () => {
    add("first try", "you", { messageId: "m1" });
    const retry = add("first try", "you", { messageId: "m1" });
    db.prepare("UPDATE work_items SET created_at = ?").run(tiedCreatedAt);
    expect(lastItemForMessage(db, "m1")?.id).toBe(retry.id);
    expect(lastItemForMessage(db, "nope")).toBeNull();
  });

  it("marks running work interrupted after a restart and leaves queued work alone (recovery after restart)", () => {
    const running = add("running");
    const queued = add("queued");
    setStatus(db, running.id, "running");
    expect(recoverAfterRestart(db).map((item) => [item.id, item.status])).toEqual([[running.id, "interrupted"]]);
    expect(getWorkItem(db, running.id).status).toBe("interrupted");
    expect(db.prepare("SELECT ended_at FROM work_items WHERE id = ?").get(running.id)).toEqual({ ended_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/) });
    expect(getWorkItem(db, queued.id).status).toBe("queued");
    expect(nextQueued(db, botId)?.id).toBe(queued.id);
    expect(recoverAfterRestart(db)).toEqual([]);
  });

  it("counts bot-to-bot requests per chain up to the limit", () => {
    const chain = newChain(db);
    expect(db.prepare("SELECT requests_made FROM chains WHERE id = ?").get(chain)).toEqual({ requests_made: 0 });
    expect([1, 2, 3].map(() => useChainRequest(db, chain, 2))).toEqual([true, true, false]);
    expect(db.prepare("SELECT requests_made FROM chains WHERE id = ?").get(chain)).toEqual({ requests_made: 2 });
    expect(useChainRequest(db, chain, 2)).toBe(false);
    expect(db.prepare("SELECT requests_made FROM chains WHERE id = ?").get(chain)).toEqual({ requests_made: 2 });
    const independentChain = newChain(db);
    expect(independentChain).not.toBe(chain);
    expect(useChainRequest(db, independentChain, 2)).toBe(true);
  });
});
