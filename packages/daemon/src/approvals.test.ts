import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { allowRules, createApproval, expirePendingApprovals, getApproval, resolveApproval } from "./approvals.ts";
import { createBot } from "./bots.ts";
import { addMessage } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let botId: string;
let n = 0;

beforeEach(() => {
  const root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" }).id;
});

afterEach(() => db.close());

function approval(rule: string | null = "Write") {
  const message = addMessage(db, { conversationId: botId, author: botId, kind: "approval", body: "hello.txt", status: "done" });
  return createApproval(db, { id: `a${++n}`, botId, tool: "Write", summary: "hello.txt", detail: "{}", rule, messageId: message.id });
}

describe("approvals store", () => {
  it("creates a pending approval", () => {
    expect(approval()).toMatchObject({ botId, tool: "Write", summary: "hello.txt", rule: "Write", status: "pending" });
  });

  it("keeps routine proposals, with their payload, pending after a restart; other approvals are kind tool", () => {
    const message = addMessage(db, { conversationId: botId, author: botId, kind: "approval", body: "Add a routine", status: "done" });
    const routine = createApproval(db, { id: "r1", botId, kind: "routine", tool: "Routine", summary: "s", detail: "d", rule: null, payload: { name: "Morning check" }, messageId: message.id });
    expect(routine).toMatchObject({ kind: "routine", payload: { name: "Morning check" } });
    expect(approval()).toMatchObject({ kind: "tool", payload: null });
    expirePendingApprovals(db);
    expect(getApproval(db, "r1").status).toBe("pending");
  });

  it("allows once without saving the rule", () => {
    const a = approval();
    expect(resolveApproval(db, a.id, "once").status).toBe("allowed");
    expect(allowRules(db, botId)).toEqual([]);
  });

  it("saves the rule on Always allow, once per rule", () => {
    resolveApproval(db, approval().id, "always");
    resolveApproval(db, approval().id, "always");
    expect(allowRules(db, botId)).toEqual(["Write"]);
  });

  it("denies, and refuses to answer the same approval twice", () => {
    const a = approval();
    expect(resolveApproval(db, a.id, "deny").status).toBe("denied");
    expect(() => resolveApproval(db, a.id, "once")).toThrow(/already answered/);
  });

  it("closes every pending approval after a restart", () => {
    const pending = approval();
    const answered = approval();
    resolveApproval(db, answered.id, "once");
    expect(expirePendingApprovals(db).map((a) => a.id)).toEqual([pending.id]);
    expect(getApproval(db, pending.id).status).toBe("denied");
  });
});
