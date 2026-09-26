import { existsSync, rmSync } from "node:fs";
import { join } from "node:path";
import { TEAM_ROOM_ID, type EventData, type EventName } from "@tjommie/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBot, deleteBot, getStoredBot, updateBot, type StoredBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, newChain, setStatus, type WorkItem } from "./queue.ts";
import type { Broadcast } from "./server.ts";
import { Team, getExchange } from "./team.ts";
import { tempRoot } from "./testing.ts";

vi.mock("node:fs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:fs")>();
  return { ...actual, rmSync: vi.fn(actual.rmSync) };
});

let root: string;
let db: Db;
let observer: Db;
let writer: StoredBot;
let researcher: StoredBot;
let planner: StoredBot;
let team: Team;
let current: Map<string, WorkItem>;
let delivered: WorkItem[];
let events: { event: EventName; data: EventData<EventName> }[];

function makeTeam(observe?: Broadcast): Team {
  return new Team({
    db,
    broadcast: (event, data) => {
      events.push({ event, data });
      observe?.(event, data);
    },
    deliver: (work) => { delivered.push(work); },
    currentWork: (botId) => current.get(botId) ?? null,
    stateOf: () => "idle",
  });
}

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "team.db"));
  observer = openDb(join(root, "team.db"));
  writer = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "Write summaries.", provider: "codex", model: "gpt-5.6-sol" });
  researcher = createBot(db, join(root, "bots"), { name: "Researcher", color: "teal", shape: "droplet", instructions: "Find sources.", provider: "claude", model: "sonnet", canMessage: [writer.id] });
  planner = createBot(db, join(root, "bots"), { name: "Planner", color: "blue", shape: "circle", instructions: "Make plans.", provider: "codex", model: "gpt-5.6-sol", canMessage: [writer.id] });
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

function request(from: StoredBot, to: StoredBot, conversationId = from.id): WorkItem {
  const work = enqueue(db, { botId: from.id, source: "you", text: "Ask for help.", conversationId, chainId: newChain(db) });
  current.set(from.id, work);
  const tool = team.tools().find((entry) => entry.name === "message_bot");
  if (!tool) throw new Error("Missing message_bot tool");
  tool.run(from.id, tool.input.parse({ bot: to.name, message: `Help ${from.name}.` }));
  const queued = delivered.at(-1);
  if (!queued?.exchangeId) throw new Error("No exchange request was delivered");
  return queued;
}

function exchangeId(work: WorkItem): string {
  if (!work.exchangeId) throw new Error("Missing exchange id");
  return work.exchangeId;
}

function state(connection = db): Record<string, unknown[]> {
  return Object.fromEntries(["bots", "chains", "conversations", "exchanges", "messages", "work_items"].map((table) => [table, connection.prepare(`SELECT * FROM ${table} ORDER BY rowid`).all()]));
}

function replies(connection = db): { bot_id: string; conversation_id: string; exchange_id: string; text: string }[] {
  return connection.prepare("SELECT bot_id, conversation_id, exchange_id, text FROM work_items WHERE source = 'bot_reply' ORDER BY rowid").all() as { bot_id: string; conversation_id: string; exchange_id: string; text: string }[];
}

function clearEffects(): void {
  delivered.length = 0;
  events.length = 0;
}

describe("Team two-phase bot deletion", () => {
  it("captures incoming and outgoing exchanges without changing state or publishing effects", () => {
    const incoming = request(researcher, writer);
    const outgoing = request(writer, researcher);
    clearEffects();
    const before = state();
    const handle = team.botDeleting(writer.id);
    expect(handle).toBeDefined();
    expect(state()).toEqual(before);
    expect(getExchange(db, exchangeId(incoming)).status).toBe("waiting");
    expect(getExchange(db, exchangeId(outgoing)).status).toBe("waiting");
    expect(delivered).toEqual([]);
    expect(events).toEqual([]);
  });

  it("keeps a busy bot and its exchanges untouched when deletion never completes", () => {
    const pending = request(researcher, writer);
    clearEffects();
    const before = state();
    team.botDeleting(writer.id);
    vi.mocked(rmSync).mockImplementationOnce(() => { throw new Error("locked"); });
    expect(() => deleteBot(db, writer.id)).toThrow("The bot folder is busy");
    expect(getStoredBot(db, writer.id)).toEqual(writer);
    expect(existsSync(writer.homeFolder)).toBe(true);
    expect(getExchange(db, exchangeId(pending)).status).toBe("waiting");
    expect(state()).toEqual(before);
    expect(delivered).toEqual([]);
    expect(events).toEqual([]);
  });

  it("refuses to finalize a handle while its bot still exists", () => {
    request(researcher, writer);
    const handle = team.botDeleting(writer.id);
    clearEffects();
    const before = state();
    expect(() => team.botDeleted(handle)).toThrow();
    expect(state()).toEqual(before);
    expect(delivered).toEqual([]);
    expect(events).toEqual([]);
  });

  it("fails every incoming exchange after deletion and tells each asker once across repeated handles", () => {
    const first = request(researcher, writer, TEAM_ROOM_ID);
    const second = request(planner, writer);
    const handle = team.botDeleting(writer.id);
    const secondHandle = team.botDeleting(writer.id);
    deleteBot(db, writer.id);
    clearEffects();
    team.botDeleted(handle);
    for (const work of [first, second]) expect(getExchange(db, exchangeId(work))).toMatchObject({ status: "failed", reply: "Writer was deleted" });
    expect(replies()).toEqual([
      expect.objectContaining({ bot_id: researcher.id, conversation_id: TEAM_ROOM_ID, exchange_id: first.exchangeId, text: expect.stringContaining("Writer was deleted") }),
      expect.objectContaining({ bot_id: planner.id, conversation_id: planner.id, exchange_id: second.exchangeId, text: expect.stringContaining("Writer was deleted") }),
    ]);
    expect(delivered).toHaveLength(2);
    const after = state();
    const published = events.length;
    expect(published).toBeGreaterThan(0);
    team.botDeleted(handle);
    team.botDeleted(secondHandle);
    expect(state()).toEqual(after);
    expect(events).toHaveLength(published);
    expect(delivered).toHaveLength(2);
  });

  it("recovers captured exchanges after their rows cascade away with the deleted bot", () => {
    const pending = request(researcher, writer);
    const handle = team.botDeleting(writer.id);
    db.exec("CREATE TRIGGER cascade_exchanges AFTER DELETE ON bots BEGIN DELETE FROM exchanges WHERE from_bot_id = OLD.id OR to_bot_id = OLD.id; END");
    deleteBot(db, writer.id);
    expect(() => getExchange(db, exchangeId(pending))).toThrow("No such exchange");
    clearEffects();
    team.botDeleted(handle);
    expect(getExchange(db, exchangeId(pending))).toMatchObject({ status: "failed", reply: "Writer was deleted", fromBotId: researcher.id, toBotId: writer.id });
    expect(replies()).toEqual([expect.objectContaining({ bot_id: researcher.id, exchange_id: pending.exchangeId, text: expect.stringContaining("Writer was deleted") })]);
    expect(delivered).toHaveLength(1);
  });

  it("fails queued and running requests from a deleted asker without sending it a reply", () => {
    const queued = request(writer, researcher);
    const running = request(writer, researcher);
    setStatus(db, running.id, "running");
    const handle = team.botDeleting(writer.id);
    deleteBot(db, writer.id);
    clearEffects();
    team.botDeleted(handle);
    for (const work of [queued, running]) expect(getExchange(db, exchangeId(work)).status).toBe("failed");
    expect(db.prepare("SELECT id FROM work_items WHERE bot_id = ? AND source = 'bot_request' AND status IN ('queued', 'running')").all(researcher.id)).toEqual([]);
    expect(replies()).toEqual([]);
    expect(delivered).toEqual([]);
    const after = state();
    const published = events.length;
    team.turnEnded(running, { outcome: "done", text: "A late answer.", textMessageId: null, error: null });
    expect(state()).toEqual(after);
    expect(events).toHaveLength(published);
  });

  it("rolls back all failures and notices when a reply insert fails and keeps the handle reusable", () => {
    const first = request(researcher, writer);
    const second = request(planner, writer);
    const handle = team.botDeleting(writer.id);
    deleteBot(db, writer.id);
    clearEffects();
    const before = state();
    db.exec(`CREATE TRIGGER reject_deleted_reply BEFORE INSERT ON work_items WHEN NEW.source = 'bot_reply' AND NEW.bot_id = '${planner.id}' BEGIN SELECT RAISE(ABORT, 'deletion reply failed'); END`);
    expect(() => team.botDeleted(handle)).toThrow("deletion reply failed");
    expect(state()).toEqual(before);
    expect(events).toEqual([]);
    expect(delivered).toEqual([]);
    db.exec("DROP TRIGGER reject_deleted_reply");
    team.botDeleted(handle);
    for (const work of [first, second]) expect(getExchange(db, exchangeId(work)).status).toBe("failed");
    expect(replies()).toHaveLength(2);
    expect(delivered).toHaveLength(2);
  });

  it("broadcasts deletion results only after all failures and notifications commit", () => {
    const first = request(researcher, writer);
    const second = request(planner, writer);
    const observations: { statuses: string[]; replies: number; notices: number }[] = [];
    team = makeTeam(() => {
      observations.push({
        statuses: [getExchange(observer, exchangeId(first)).status, getExchange(observer, exchangeId(second)).status],
        replies: replies(observer).length,
        notices: (observer.prepare("SELECT COUNT(*) AS n FROM messages WHERE kind = 'notice' AND conversation_id = ?").get(TEAM_ROOM_ID) as { n: number }).n,
      });
    });
    const handle = team.botDeleting(writer.id);
    deleteBot(db, writer.id);
    clearEffects();
    team.botDeleted(handle);
    expect(events.length).toBeGreaterThan(0);
    expect(observations).toEqual(Array.from({ length: events.length }, () => ({ statuses: ["failed", "failed"], replies: 2, notices: 2 })));
  });
});
