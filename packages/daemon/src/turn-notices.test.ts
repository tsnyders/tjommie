import { rmSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, expect, it } from "vitest";
import { createBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, newChain, setStatus } from "./queue.ts";
import type { TurnResult } from "./runner.ts";
import { tempRoot } from "./testing.ts";
import { turnNotice } from "./turn-notices.ts";

let db: Db;
let root: string;
let botId: string;
const result = (outcome: TurnResult["outcome"], text: string | null = null, error: string | null = null): TurnResult =>
  ({ outcome, text, error, textMessageId: null });

beforeEach(() => {
  root = tempRoot();
  db = openDb(":memory:");
  botId = createBot(db, join(root, "bots"), { name: "Writer", color: "purple", shape: "cloud", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id;
});
afterEach(() => { db.close(); rmSync(root, { recursive: true, force: true }); });

it("announces a finished message once the chain has no other work", () => {
  const item = enqueue(db, { botId, source: "you", text: "Write", chainId: newChain(db) });
  setStatus(db, item.id, "running");
  expect(turnNotice(db, item, result("done"))).toMatchObject({ kind: "done" });
  setStatus(db, item.id, "done");
  expect(turnNotice(db, item, result("done", "A finished reply"))).toEqual({ kind: "done", botId, title: "Writer is done", body: "A finished reply" });
  expect(turnNotice(db, item, result("done", "x".repeat(141)))?.body).toHaveLength(140);
  expect(turnNotice(db, item, result("done", `${"x".repeat(139)}😀y`))?.body).toBe(`${"x".repeat(139)}😀`);
  expect(turnNotice(db, { ...item, chainId: null, status: "running" }, result("done"))).toEqual({ kind: "done", botId, title: "Writer is done", body: "" });
});

it("re-reads a chain assigned during a turn before announcing completion", () => {
  const item = enqueue(db, { botId, source: "you", text: "Delegate" });
  const chainId = newChain(db);
  db.prepare("UPDATE work_items SET chain_id = ? WHERE id = ?").run(chainId, item.id);
  enqueue(db, { botId, source: "bot_request", text: "Research", chainId });
  expect(turnNotice(db, item, result("done"))).toBeNull();
});

it("waits through a bot request and the open exchange, then announces the asker's reply", () => {
  const chainId = newChain(db);
  const first = enqueue(db, { botId, source: "you", text: "Delegate", chainId });
  const asked = enqueue(db, { botId, source: "bot_request", text: "Research", chainId });
  db.prepare("INSERT INTO exchanges(id,from_bot_id,to_bot_id,request,status,chain_id,reply_to,created_at) VALUES('exchange',?,?,?,?,?,?,?)")
    .run(botId, botId, "Research", "waiting", chainId, botId, "now");
  setStatus(db, first.id, "done");
  expect(turnNotice(db, first, result("done", "First turn"))).toBeNull();
  setStatus(db, asked.id, "done");
  expect(turnNotice(db, asked, result("done", "Research"))).toBeNull();
  expect(turnNotice(db, first, result("done", "First turn"))).toBeNull();
  const reply = enqueue(db, { botId, source: "bot_reply", text: "Result", chainId });
  db.prepare("UPDATE exchanges SET status = 'replied' WHERE id = 'exchange'").run();
  expect(turnNotice(db, reply, result("done", "Final answer"))).toEqual({ kind: "done", botId, title: "Writer is done", body: "Final answer" });
});

it("waits for the last of three room bots and routes the notice to the room", () => {
  const chainId = newChain(db);
  const others = ["Researcher", "Analyst"].map((name) => createBot(db, join(root, "bots"), { name, color: "purple", shape: "cloud", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id);
  const items = [botId, ...others].map((memberId, index) => enqueue(db, { botId: memberId, source: "room", text: `Part ${index}`, conversationId: "team", chainId }));
  setStatus(db, items[0].id, "done");
  expect(turnNotice(db, items[0], result("done"))).toBeNull();
  setStatus(db, items[1].id, "done");
  expect(turnNotice(db, items[1], result("done"))).toBeNull();
  setStatus(db, items[2].id, "done");
  expect(turnNotice(db, items[2], result("done", "Last reply"))).toEqual({ kind: "done", botId: others[1], roomId: "team", title: "Done in Everyone", body: "Last reply" });
});

it("announces direct failures, with a fallback body, and skips delegated failures", () => {
  const direct = enqueue(db, { botId, source: "you", text: "Write" });
  const delegated = enqueue(db, { botId, source: "bot_request", text: "Research" });
  expect(turnNotice(db, direct, result("failed", null, "CLI exited"))).toEqual({ kind: "failed", botId, title: "Writer ran into a problem", body: "CLI exited" });
  expect(turnNotice(db, direct, result("failed"))).toMatchObject({ body: "Something went wrong." });
  const room = enqueue(db, { botId, source: "room", text: "Report", conversationId: "team" });
  expect(turnNotice(db, room, result("failed", null, "Room failed"))).toEqual({ kind: "failed", botId, roomId: "team", title: "Writer ran into a problem", body: "Room failed" });
  expect(turnNotice(db, delegated, result("failed", null, "CLI exited"))).toBeNull();
});

it("skips interruptions and routine completion, including routine chains", () => {
  const chainId = newChain(db);
  const routine = enqueue(db, { botId, source: "routine", text: "Digest", chainId, routineRunId: "run" });
  const reply = enqueue(db, { botId, source: "bot_reply", text: "Answer", chainId });
  setStatus(db, routine.id, "done");
  expect(turnNotice(db, routine, result("done"))).toBeNull();
  expect(turnNotice(db, reply, result("done"))).toBeNull();
  expect(turnNotice(db, reply, result("failed", null, "Reply failed"))).toMatchObject({ kind: "failed", body: "Reply failed" });
  expect(turnNotice(db, reply, result("interrupted"))).toBeNull();
});
