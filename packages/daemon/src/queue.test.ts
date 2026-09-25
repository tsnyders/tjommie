import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { createBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, lastItemForMessage, nextQueued, recoverAfterRestart, setStatus } from "./queue.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let botId: string;

beforeEach(() => {
  const root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id;
});

const add = (text: string, messageId: string | null = null) => enqueue(db, { botId, source: "you", text, messageId });

describe("work queue", () => {
  it("hands out queued work oldest first, even within one millisecond", () => {
    const [a, b, c] = [add("a"), add("b"), add("c")];
    expect(nextQueued(db, botId)?.id).toBe(a.id);
    setStatus(db, a.id, "running");
    expect(nextQueued(db, botId)?.id).toBe(b.id);
    setStatus(db, b.id, "done");
    expect(nextQueued(db, botId)?.id).toBe(c.id);
  });

  it("puts running work back in the queue with its start time cleared", () => {
    const item = add("x");
    setStatus(db, item.id, "running");
    setStatus(db, item.id, "queued");
    expect(getWorkItem(db, item.id).status).toBe("queued");
    expect(db.prepare("SELECT started_at FROM work_items WHERE id = ?").get(item.id)).toEqual({ started_at: null });
  });

  it("finds the latest attempt for a message", () => {
    add("first try", "m1");
    const retry = add("first try", "m1");
    expect(lastItemForMessage(db, "m1")?.id).toBe(retry.id);
    expect(lastItemForMessage(db, "nope")).toBeNull();
  });

  it("marks running work interrupted after a restart and leaves queued work alone (recovery after restart)", () => {
    const running = add("running");
    const queued = add("queued");
    setStatus(db, running.id, "running");
    expect(recoverAfterRestart(db).map((item) => [item.id, item.status])).toEqual([[running.id, "interrupted"]]);
    expect(getWorkItem(db, running.id).status).toBe("interrupted");
    expect(nextQueued(db, botId)?.id).toBe(queued.id);
  });
});
