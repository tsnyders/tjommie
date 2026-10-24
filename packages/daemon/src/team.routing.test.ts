import { rmSync } from "node:fs";
import { join } from "node:path";
import { TEAM_ROOM_ID } from "@tjommie/protocol";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createBot, deleteBot, updateBot, type StoredBot } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import type { ProviderEvent, ProviderSession } from "./providers/types.ts";
import { getWorkItem, type WorkItem } from "./queue.ts";
import { createRoom } from "./rooms.ts";
import { Runner } from "./runner.ts";
import { ensureAccounts } from "./setup.ts";
import { Team, getExchange } from "./team.ts";
import { tempRoot, waitFor } from "./testing.ts";

class RoutingSession implements ProviderSession {
  sent: string[] = [];
  steered: string[] = [];
  interrupts = 0;
  listener: ((event: ProviderEvent) => void) | null = null;

  async start(): Promise<void> {}
  async send(text: string): Promise<void> { this.sent.push(text); }
  async steer(text: string): Promise<boolean> { this.steered.push(text); return true; }
  async interrupt(): Promise<void> { this.interrupts++; }
  answer(): void {}
  close(): void {}
  pid(): undefined { return undefined; }
  onEvent(listener: (event: ProviderEvent) => void): void { this.listener = listener; }
  emit(event: ProviderEvent): void { this.listener?.(event); }
  finish(text: string): void {
    this.emit({ type: "text", delta: text });
    this.emit({ type: "text_end" });
    this.emit({ type: "turn_done", ok: true, error: null });
  }
}

let root: string;
let db: Db;
let team: Team;
let runner: Runner;
let alice: StoredBot;
let bob: StoredBot;
let carol: StoredBot;
let delivered: WorkItem[];
let started: WorkItem[];
let ended: WorkItem[];
let sessions: Map<string, RoutingSession>;

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "routing.db"));
  ensureAccounts(db, join(root, "accounts"));
  const makeBot = (name: string) => createBot(db, join(root, "bots"), { name, instructions: "", color: "blue", shape: "circle", provider: "claude", model: "sonnet" });
  alice = makeBot("Alice");
  bob = makeBot("Bob");
  carol = makeBot("Carol");
  alice = updateBot(db, alice.id, { canMessage: [bob.id] });
  bob = updateBot(db, bob.id, { canMessage: [carol.id] });
  delivered = [];
  started = [];
  ended = [];
  sessions = new Map();
  runner = new Runner({
    db,
    broadcast: () => {},
    isSignedIn: () => true,
    createSession: (bot) => {
      const session = new RoutingSession();
      sessions.set(bot.id, session);
      return session;
    },
    hooks: {
      turnStarted: (item) => started.push(item),
      turnEnded: (item, result) => { ended.push(item); team.turnEnded(item, result); },
      sessionClosed: (botId) => team.sessionClosed(botId),
    },
  });
  team = new Team({
    db,
    broadcast: () => {},
    deliver: (work) => { delivered.push(work); runner.deliver(work); },
    stopItem: (id, notice) => runner.stopItem(id, notice),
    currentWork: (botId) => runner.currentWork(botId),
    stateOf: (bot) => runner.toBot(bot).state,
  });
});

afterEach(() => {
  runner.shutdown();
  db.close();
  rmSync(root, { recursive: true, force: true });
});

const ask = (from: StoredBot, to: StoredBot, message: string): WorkItem => {
  const tool = team.tools().find((entry) => entry.name === "message_bot")!;
  tool.run(from.id, tool.input.parse({ bot: to.name, message }));
  return delivered.at(-1)!;
};

const sent = async (bot: StoredBot, count = 1): Promise<RoutingSession> => {
  await waitFor(() => (sessions.get(bot.id)?.sent.length ?? 0) >= count, 5000);
  return sessions.get(bot.id)!;
};

describe("Team reply routing with Runner", () => {
  it("writes a custom room reply there and links to it from the bot chat", async () => {
    const room = createRoom(db, { name: "Planning", botIds: [alice.id] });
    team.postToRoom(room.id, "@Alice prepare a report");
    const session = await sent(alice);
    session.finish("Report ready");
    const answer = history(db, room.id, undefined, 10).at(-1)!;
    expect(answer).toMatchObject({ author: alice.id, body: "Report ready" });
    expect(history(db, alice.id, undefined, 10)).toContainEqual(expect.objectContaining({
      kind: "handoff", body: "Answered in Planning", data: { roomId: room.id, roomMessageId: answer.id },
    }));
  });

  it("routes nested replies into the asking exchange and sends its later answer as a follow-up", async () => {
    runner.submit(alice.id, "Prepare a report", []);
    const aliceSession = await sent(alice);
    const first = ask(alice, bob, "Write the report");
    const bobSession = await sent(bob);
    const nested = ask(bob, carol, "Find a source");
    const carolSession = await sent(carol);
    expect(nested.chainId).toBe(first.chainId);

    bobSession.finish("First draft");
    const initialReply = delivered.at(-1)!;
    expect(initialReply).toMatchObject({ botId: alice.id, source: "bot_reply", conversationId: alice.id, exchangeId: first.exchangeId });
    await waitFor(() => aliceSession.steered.length === 1, 5000);

    carolSession.finish("Verified source");
    const nestedReply = delivered.at(-1)!;
    expect(nestedReply).toMatchObject({ botId: bob.id, source: "bot_reply", conversationId: first.conversationId, exchangeId: nested.exchangeId, chainId: first.chainId });
    await sent(bob, 2);
    expect(bobSession.sent[1]).toContain("Carol replied");
    expect(bobSession.sent[1]).toContain("Verified source");
    bobSession.finish("Report with the verified source");
    const followUp = delivered.at(-1)!;
    expect(followUp).toMatchObject({ botId: alice.id, source: "bot_reply", conversationId: alice.id, exchangeId: first.exchangeId, chainId: first.chainId });
    expect(followUp.text).toContain("Bob followed up");
    await waitFor(() => aliceSession.steered.length === 2, 5000);
    expect(aliceSession.steered[1]).toContain("Report with the verified source");
    expect(getExchange(db, first.exchangeId!)).toMatchObject({ status: "replied", reply: "First draft" });
    expect(getExchange(db, nested.exchangeId!)).toMatchObject({ status: "replied", reply: "Verified source" });
    expect(history(db, alice.id, undefined, 20).find((message) => message.kind === "handoff")?.data).toMatchObject({ messages: 3, status: "replied" });
  });

  it("answers the original requester when failed exchange work is retried on a new chain", async () => {
    runner.submit(alice.id, "Prepare a report", []);
    const aliceSession = await sent(alice);
    const request = ask(alice, bob, "Write the report");
    const bobSession = await sent(bob);
    bobSession.emit({ type: "turn_done", ok: false, error: "Temporary failure" });
    expect(getExchange(db, request.exchangeId!).status).toBe("failed");
    await waitFor(() => aliceSession.steered.length === 1, 5000);

    expect(request.messageId).toBeTypeOf("string");
    runner.retry(request.messageId!);
    const retry = runner.currentWork(bob.id)!;
    expect(retry).toMatchObject({ conversationId: request.conversationId, exchangeId: request.exchangeId });
    expect(retry.chainId).not.toBe(request.chainId);
    await sent(bob, 2);
    bobSession.finish("Recovered report");
    const reply = delivered.at(-1)!;
    expect(reply).toMatchObject({ botId: alice.id, source: "bot_reply", conversationId: alice.id, exchangeId: request.exchangeId });
    expect(reply.text).toContain("Bob replied");
    expect(reply.text).toContain("Recovered report");
    await waitFor(() => aliceSession.steered.length === 2, 5000);
    expect(getExchange(db, request.exchangeId!)).toMatchObject({ status: "replied", reply: "Recovered report" });
    expect(getWorkItem(db, retry.id).status).toBe("done");
  });

  it("fails a deleted requester's queued and running exchanges without reviving them on late output", async () => {
    runner.submit(alice.id, "Prepare reports", []);
    await sent(alice);
    const running = ask(alice, bob, "First report");
    const bobSession = await sent(bob);
    const queued = ask(alice, bob, "Second report");
    expect(getWorkItem(db, running.id).status).toBe("running");
    expect(getWorkItem(db, queued.id).status).toBe("queued");

    const deleting = team.botDeleting(alice.id);
    deleteBot(db, alice.id);
    team.botDeleted(deleting);
    runner.forget(alice.id);
    expect(getWorkItem(db, queued.id).status).toBe("failed");
    expect(getWorkItem(db, running.id).status).toBe("running");
    expect(bobSession.interrupts).toBe(1);
    expect(getExchange(db, queued.exchangeId!).status).toBe("failed");
    expect(getExchange(db, running.exchangeId!).status).toBe("failed");
    expect(() => bobSession.finish("Finished for the room")).not.toThrow();
    expect(getWorkItem(db, running.id).status).toBe("done");
    expect(getExchange(db, running.exchangeId!)).toMatchObject({ status: "failed", reply: "Alice was deleted" });
    expect(delivered).toHaveLength(2);
    expect(bobSession.sent).toHaveLength(1);
    expect(history(db, TEAM_ROOM_ID, undefined, 20).some((message) => message.body === "Finished for the room")).toBe(false);
  });

  it("routes help requested from the Team room back there and records the later room answer in the bot's chat", async () => {
    team.postToRoom(TEAM_ROOM_ID, "@Alice prepare a report");
    const roomTurn = delivered[0];
    const aliceSession = await sent(alice);
    const request = ask(alice, bob, "Write the report");
    const bobSession = await sent(bob);
    aliceSession.finish("I have asked Bob for a report");
    bobSession.finish("The finished report");
    const reply = delivered.at(-1)!;
    expect(reply).toMatchObject({ botId: alice.id, source: "bot_reply", conversationId: TEAM_ROOM_ID, exchangeId: request.exchangeId, chainId: roomTurn.chainId });
    await sent(alice, 2);
    aliceSession.finish("Here is Bob's report");
    const answer = history(db, TEAM_ROOM_ID, undefined, 20).at(-1)!;
    expect(answer).toMatchObject({ author: alice.id, body: "Here is Bob's report" });
    const handoffs = history(db, alice.id, undefined, 20).filter((message) => message.body === "Answered in Everyone");
    expect(handoffs).toHaveLength(2);
    expect(handoffs.at(-1)?.data).toMatchObject({ roomMessageId: answer.id });
  });

  it("steers a nested reply without a separate turn hook or duplicate upstream answer", async () => {
    runner.submit(alice.id, "Prepare a report", []);
    await sent(alice);
    const first = ask(alice, bob, "Write the report");
    const bobSession = await sent(bob);
    const nested = ask(bob, carol, "Find a source");
    const carolSession = await sent(carol);
    carolSession.finish("The source");
    const steered = delivered.at(-1)!;
    await waitFor(() => getWorkItem(db, steered.id).status === "done", 5000);
    expect(steered).toMatchObject({ botId: bob.id, conversationId: first.conversationId, exchangeId: nested.exchangeId });
    expect(bobSession.steered).toHaveLength(1);
    expect(started.some((item) => item.id === steered.id)).toBe(false);
    expect(ended.some((item) => item.id === steered.id)).toBe(false);
    expect(delivered.filter((item) => item.botId === alice.id && item.source === "bot_reply")).toEqual([]);

    bobSession.finish("Report using the source");
    const upstream = delivered.filter((item) => item.botId === alice.id && item.source === "bot_reply");
    expect(upstream).toHaveLength(1);
    expect(upstream[0]).toMatchObject({ exchangeId: first.exchangeId, conversationId: alice.id });
    expect(ended.filter((item) => item.botId === bob.id).map((item) => item.id)).toEqual([first.id]);
    expect(history(db, TEAM_ROOM_ID, undefined, 20).filter((message) => message.author === bob.id && message.body === "Report using the source")).toHaveLength(1);
  });
});
