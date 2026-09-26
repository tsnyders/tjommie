import { rmSync } from "node:fs";
import { join } from "node:path";
import { Approval, type EventData, type EventName } from "@tjommie/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getApproval } from "./approvals.ts";
import { createBot, updateBot, type StoredBot } from "./bots.ts";
import { getMessage } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, newChain, type WorkItem } from "./queue.ts";
import { listRoutines } from "./routines.ts";
import type { Broadcast } from "./server.ts";
import { setSettings } from "./settings.ts";
import { Team, getExchange } from "./team.ts";
import { tempRoot } from "./testing.ts";

let root: string;
let db: Db;
let observer: Db;
let writer: StoredBot;
let researcher: StoredBot;
let team: Team;
let current: Map<string, WorkItem>;
let delivered: WorkItem[];
let events: { event: EventName; data: EventData<EventName> }[];

function makeTeam(deliver: (work: WorkItem) => void = (work) => { delivered.push(work); }, observe?: Broadcast): Team {
  return new Team({
    db,
    broadcast: (event, data) => {
      events.push({ event, data });
      observe?.(event, data);
    },
    deliver,
    currentWork: (botId) => current.get(botId) ?? null,
    stateOf: () => "idle",
  });
}

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "team.db"));
  observer = openDb(join(root, "team.db"));
  writer = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "Writes summaries.", provider: "codex", model: "gpt-5.6-sol" });
  researcher = createBot(db, join(root, "bots"), { name: "Researcher", color: "teal", shape: "droplet", instructions: "Finds sources.", provider: "claude", model: "sonnet", canMessage: [writer.id] });
  writer = updateBot(db, writer.id, { canMessage: [researcher.id] });
  current = new Map();
  delivered = [];
  events = [];
  team = makeTeam();
});

afterEach(() => {
  vi.restoreAllMocks();
  observer.close();
  db.close();
  rmSync(root, { recursive: true, force: true });
});

function call(botId: string, name: string, args: unknown): string | Promise<string> {
  const tool = team.tools().find((entry) => entry.name === name);
  if (!tool) throw new Error(`Missing tool ${name}`);
  return tool.run(botId, tool.input.parse(args));
}

function chatTurn(): WorkItem {
  const work = enqueue(db, { botId: researcher.id, source: "you", text: "Help me write.", chainId: newChain(db) });
  current.set(researcher.id, work);
  return work;
}

const propose = () => call(writer.id, "add_routine", { name: "Morning check", days: ["mon", "wed"], times: ["08:30"], prompt: "Summarize the inbox." });

function proposal(): Approval {
  propose();
  return Approval.parse(events.find((event) => event.event === "approval.added")?.data);
}

const requestCount = (chainId: string | null): number => {
  if (chainId === null) throw new Error("Expected a chain");
  const row = db.prepare("SELECT requests_made FROM chains WHERE id = ?").get(chainId) as { requests_made: number } | undefined;
  if (!row) throw new Error("Missing chain");
  return row.requests_made;
};

function stateCounts(connection = db): Record<string, number> {
  const tables = ["approvals", "chains", "conversations", "exchanges", "messages", "routines", "work_items"];
  return Object.fromEntries(tables.map((table) => [table, (connection.prepare(`SELECT COUNT(*) AS n FROM ${table}`).get() as { n: number }).n]));
}

describe("Team routine transactions", () => {
  it("rolls back the proposal card when creating its approval fails", () => {
    const before = stateCounts();
    db.exec("CREATE TRIGGER reject_proposal BEFORE INSERT ON approvals BEGIN SELECT RAISE(ABORT, 'approval insert failed'); END");
    expect(propose).toThrow("approval insert failed");
    expect(stateCounts()).toEqual(before);
    expect(events).toEqual([]);
  });

  it("publishes the proposal only after both card and approval commit", () => {
    const observations: { messages: number; approvals: number }[] = [];
    team = makeTeam(undefined, () => {
      const counts = stateCounts(observer);
      observations.push({ messages: counts.messages, approvals: counts.approvals });
    });
    proposal();
    expect(events.map((event) => event.event)).toEqual(["message.added", "approval.added", "notify", "bot.updated"]);
    expect(observations).toEqual(Array.from({ length: 4 }, () => ({ messages: 1, approvals: 1 })));
  });

  it.each([
    ["routine creation", "CREATE TRIGGER reject_answer BEFORE INSERT ON routines BEGIN SELECT RAISE(ABORT, 'answer failed'); END"],
    ["routine scheduling", "CREATE TRIGGER reject_answer BEFORE UPDATE ON routines BEGIN SELECT RAISE(ABORT, 'answer failed'); END"],
    ["card update", "CREATE TRIGGER reject_answer BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'answer failed'); END"],
  ])("rolls back approval, routine and card when %s fails", (_stage, trigger) => {
    const pending = proposal();
    const before = stateCounts();
    const card = getMessage(db, pending.messageId);
    events.length = 0;
    db.exec(trigger);
    expect(() => team.answerRoutineProposal(pending.id, "once")).toThrow("answer failed");
    expect(getApproval(db, pending.id)).toEqual(pending);
    expect(getMessage(db, pending.messageId)).toEqual(card);
    expect(stateCounts()).toEqual(before);
    expect(events).toEqual([]);
  });

  it("rolls back denial if the proposal card cannot be updated", () => {
    const pending = proposal();
    events.length = 0;
    db.exec("CREATE TRIGGER reject_denial BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'denial failed'); END");
    expect(() => team.answerRoutineProposal(pending.id, "deny")).toThrow("denial failed");
    expect(getApproval(db, pending.id).status).toBe("pending");
    expect(getMessage(db, pending.messageId).data?.status).toBe("pending");
    expect(listRoutines(db, writer.id)).toEqual([]);
    expect(events).toEqual([]);
  });

  it("publishes approval results only after the routine and card commit", () => {
    const pending = proposal();
    events.length = 0;
    const observations: { approval: string; card: unknown; routines: number }[] = [];
    team = makeTeam(undefined, () => {
      observations.push({ approval: getApproval(observer, pending.id).status, card: getMessage(observer, pending.messageId).data?.status, routines: listRoutines(observer, writer.id).length });
    });
    team.answerRoutineProposal(pending.id, "once");
    expect(events.map((event) => event.event).sort()).toEqual(["approval.resolved", "bot.updated", "message.updated", "routine.updated"]);
    expect(observations).toEqual(Array.from({ length: 4 }, () => ({ approval: "allowed", card: "allowed", routines: 1 })));
    expect(() => team.answerRoutineProposal(pending.id, "once")).toThrow(/already answered/);
    expect(listRoutines(db, writer.id)).toHaveLength(1);
  });

  it.each([
    ["malformed payload", "UPDATE approvals SET payload = '{}' WHERE id = ?"],
    ["wrong approval kind", "UPDATE approvals SET kind = 'tool' WHERE id = ?"],
  ])("leaves a pending proposal intact when it has a %s", (_reason, sql) => {
    const pending = proposal();
    db.prepare(sql).run(pending.id);
    events.length = 0;
    expect(() => team.answerRoutineProposal(pending.id, "once")).toThrow();
    expect(getApproval(db, pending.id).status).toBe("pending");
    expect(getMessage(db, pending.messageId).data?.status).toBe("pending");
    expect(listRoutines(db, writer.id)).toEqual([]);
    expect(events).toEqual([]);
  });

  it("rejects a proposal payload that belongs to another bot without resolving it", () => {
    const pending = proposal();
    db.prepare("UPDATE approvals SET payload = ? WHERE id = ?").run(JSON.stringify({ ...pending.payload, botId: researcher.id }), pending.id);
    events.length = 0;
    expect(() => team.answerRoutineProposal(pending.id, "once")).toThrow();
    expect(getApproval(db, pending.id).status).toBe("pending");
    expect(stateCounts().routines).toBe(0);
    expect(events).toEqual([]);
  });
});

describe("Team request transactions", () => {
  it("logs a broadcast failure and still delivers the committed request", () => {
    const work = chatTurn();
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    team = makeTeam(undefined, () => { throw new Error("broadcast failed"); });
    expect(call(researcher.id, "message_bot", { bot: "Writer", message: "Please summarize." })).toBe("Sent. Writer will reply.");
    expect(delivered).toHaveLength(1);
    expect(getWorkItem(db, delivered[0].id).status).toBe("queued");
    expect(getExchange(db, delivered[0].exchangeId!).status).toBe("waiting");
    expect(requestCount(work.chainId)).toBe(1);
    expect(log).toHaveBeenCalledTimes(4);
    expect(log).toHaveBeenCalledWith("Tjommie: a Team broadcast failed", expect.objectContaining({ message: "broadcast failed" }));
  });

  it.each([
    ["exchange insertion", "CREATE TRIGGER reject_request BEFORE INSERT ON exchanges BEGIN SELECT RAISE(ABORT, 'request failed'); END"],
    ["conversation insertion", "CREATE TRIGGER reject_request BEFORE INSERT ON conversations WHEN NEW.kind = 'exchange' BEGIN SELECT RAISE(ABORT, 'request failed'); END"],
    ["handoff insertion", "CREATE TRIGGER reject_request BEFORE INSERT ON messages WHEN NEW.kind = 'handoff' BEGIN SELECT RAISE(ABORT, 'request failed'); END"],
    ["queue insertion", "CREATE TRIGGER reject_request BEFORE INSERT ON work_items WHEN NEW.source = 'bot_request' BEGIN SELECT RAISE(ABORT, 'request failed'); END"],
  ])("rolls back quota and all request state when %s fails", (_stage, trigger) => {
    const work = chatTurn();
    const before = stateCounts();
    db.exec(trigger);
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Please summarize." })).toThrow("request failed");
    expect(stateCounts()).toEqual(before);
    expect(requestCount(work.chainId)).toBe(0);
    expect(delivered).toEqual([]);
    expect(events).toEqual([]);
  });

  it("publishes requests and wakes delivery only after every request row commits", () => {
    const work = chatTurn();
    const observations: Record<string, number>[] = [];
    team = makeTeam((item) => {
      expect(getWorkItem(observer, item.id)).toEqual(item);
      delivered.push(item);
    }, () => { observations.push(stateCounts(observer)); });
    call(researcher.id, "message_bot", { bot: "Writer", message: "Please summarize." });
    const expected = { approvals: 0, chains: 1, conversations: 4, exchanges: 1, messages: 4, routines: 0, work_items: 2 };
    expect(stateCounts()).toEqual(expected);
    expect(events.map((event) => event.event)).toEqual(Array.from({ length: 4 }, () => "message.added"));
    expect(observations).toEqual(Array.from({ length: 4 }, () => expected));
    expect(requestCount(work.chainId)).toBe(1);
    expect(delivered).toHaveLength(1);
  });

  it("fails the exchange, refunds quota and closes queued work when delivery throws", () => {
    const work = chatTurn();
    let attempted: WorkItem | null = null;
    team = makeTeam((item) => {
      attempted = item;
      throw new Error("delivery failed");
    });
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Please summarize." })).toThrow("delivery failed");
    if (attempted === null) throw new Error("Delivery was not attempted");
    const request = attempted as WorkItem;
    if (request.exchangeId === null) throw new Error("Request has no exchange");
    expect(getExchange(db, request.exchangeId).status).toBe("failed");
    expect(requestCount(work.chainId)).toBe(0);
    expect(db.prepare("SELECT id FROM work_items WHERE exchange_id = ? AND status IN ('queued', 'running')").all(request.exchangeId)).toEqual([]);
    expect(db.prepare("SELECT json_extract(data, '$.status') AS status FROM messages WHERE kind = 'handoff' AND json_extract(data, '$.exchangeId') = ?").all(request.exchangeId)).toEqual([{ status: "failed" }, { status: "failed" }]);
    team = makeTeam();
    expect(call(researcher.id, "message_bot", { bot: "Writer", message: "Try again." })).toBe("Sent. Writer will reply.");
    expect(requestCount(work.chainId)).toBe(1);
  });

  it("fails closed for an unknown current chain without leaving request state", () => {
    const work = chatTurn();
    current.set(researcher.id, { ...work, chainId: "missing-chain" });
    const before = stateCounts();
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Please summarize." })).toThrow(/^Limit reached, ask the user/);
    expect(stateCounts()).toEqual(before);
    expect(events).toEqual([]);
    expect(delivered).toEqual([]);
  });
});

describe("Team autonomous chains", () => {
  it("reuses one chain with no current work and stops at the chain limit", () => {
    setSettings(db, { chainLimit: 2 });
    call(researcher.id, "message_bot", { bot: "Writer", message: "First." });
    call(researcher.id, "message_bot", { bot: "Writer", message: "Second." });
    expect(delivered[0].chainId).toBe(delivered[1].chainId);
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Third." })).toThrow(/^Limit reached, ask the user/);
    expect(delivered).toHaveLength(2);
    expect(stateCounts().chains).toBe(1);
    expect(requestCount(delivered[0].chainId)).toBe(2);
  });

  it("gives a closed autonomous session a fresh chain", () => {
    setSettings(db, { chainLimit: 1 });
    call(researcher.id, "message_bot", { bot: "Writer", message: "First session." });
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Over limit." })).toThrow(/^Limit reached/);
    team.sessionClosed(researcher.id);
    expect(call(researcher.id, "message_bot", { bot: "Writer", message: "New session." })).toBe("Sent. Writer will reply.");
    expect(delivered[0].chainId).not.toBe(delivered[1].chainId);
    expect(requestCount(delivered[0].chainId)).toBe(1);
    expect(requestCount(delivered[1].chainId)).toBe(1);
  });

  it("does not poison its autonomous chain when the first request rolls back", () => {
    setSettings(db, { chainLimit: 1 });
    db.exec("CREATE TRIGGER reject_first BEFORE INSERT ON work_items WHEN NEW.source = 'bot_request' BEGIN SELECT RAISE(ABORT, 'queue failed'); END");
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Will fail." })).toThrow("queue failed");
    expect(stateCounts().chains).toBe(0);
    expect(events).toEqual([]);
    db.exec("DROP TRIGGER reject_first");
    expect(call(researcher.id, "message_bot", { bot: "Writer", message: "Retry." })).toBe("Sent. Writer will reply.");
    expect(delivered).toHaveLength(1);
    expect(requestCount(delivered[0].chainId)).toBe(1);
  });
});
