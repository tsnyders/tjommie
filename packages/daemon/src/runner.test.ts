import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BotInput, EventData, EventName } from "@tjommie/protocol";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createApproval } from "./approvals.ts";
import { createBot, getStoredBot } from "./bots.ts";
import { addMessage, getMessage, history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, getWorkItem, setStatus, type WorkStatus } from "./queue.ts";
import { activityBody, botStateOf, copyIntoInbox, Runner, type SessionFactory } from "./runner.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { ensureAccounts } from "./setup.ts";
import { tempRoot, waitFor } from "./testing.ts";

type SentEvent = { [E in EventName]: { event: E; data: EventData<E> } }[EventName];

class FakeSession implements ProviderSession {
  events: ((event: ProviderEvent) => void) | null = null;
  startPromise: Promise<void> = Promise.resolve();
  sendPromise: Promise<void> = Promise.resolve();
  sent: string[] = [];
  steered: string[] = [];
  answers: { id: string; answer: "once" | "always" | "deny" }[] = [];
  interrupts = 0;
  closed = false;
  steerResult = true;

  start(): Promise<void> {
    return this.startPromise;
  }

  async send(text: string): Promise<void> {
    this.sent.push(text);
    await this.sendPromise;
  }

  async steer(text: string): Promise<boolean> {
    this.steered.push(text);
    return this.steerResult;
  }

  async interrupt(): Promise<void> {
    this.interrupts++;
  }

  answer(id: string, answer: "once" | "always" | "deny"): void {
    this.answers.push({ id, answer });
  }

  close(): void {
    this.closed = true;
  }

  pid(): number {
    return 4321;
  }

  onEvent(listener: (event: ProviderEvent) => void): void {
    this.events = listener;
  }

  emit(event: ProviderEvent): void {
    this.events?.(event);
  }
}

type Harness = {
  root: string;
  db: Db;
  runner: Runner;
  events: SentEvent[];
  sessions: FakeSession[];
};

const runners: Runner[] = [];

afterEach(() => {
  for (const runner of runners) runner.shutdown();
  runners.length = 0;
  vi.restoreAllMocks();
});

function harness(factory: (options: StartOptions) => FakeSession = () => new FakeSession(), signedIn = true): Harness {
  const root = tempRoot();
  const db = openDb(join(root, "t.db"));
  ensureAccounts(db, join(root, "accounts"));
  const events: SentEvent[] = [];
  const sessions: FakeSession[] = [];
  const broadcast: Broadcast = (event, data) => events.push({ event, data } as SentEvent);
  const createSession: SessionFactory = (_bot, options) => {
    const session = factory(options);
    sessions.push(session);
    return session;
  };
  const runner = new Runner({ db, broadcast, isSignedIn: () => signedIn, createSession });
  runners.push(runner);
  return { root, db, runner, events, sessions };
}

let botCount = 0;

function newBot(h: Harness, overrides: Partial<BotInput> = {}) {
  return createBot(h.db, join(h.root, "bots"), {
    name: `Bot ${++botCount}`,
    color: "blue",
    shape: "circle",
    instructions: "Be terse.",
    provider: "claude",
    model: "haiku",
    ...overrides,
  });
}

function itemForMessage(db: Db, messageId: string): { id: string; status: WorkStatus } {
  const row = db.prepare("SELECT id, status FROM work_items WHERE message_id = ? ORDER BY rowid DESC LIMIT 1").get(messageId) as
    | { id: string; status: WorkStatus }
    | undefined;
  if (!row) throw new Error(`No work item for ${messageId}`);
  return row;
}

describe("botStateOf", () => {
  const base = { signedIn: true, paused: false, limitedUntil: null, waiting: false, busy: false };
  const now = "2026-09-25T10:00:00.000Z";

  it("ranks states: needs setup, paused, limited, waiting, working, idle", () => {
    expect(botStateOf({ ...base, signedIn: false, paused: true }, now)).toBe("needs_setup");
    expect(botStateOf({ ...base, paused: true, busy: true }, now)).toBe("paused");
    expect(botStateOf({ ...base, limitedUntil: "2026-09-25T11:00:00.000Z", waiting: true }, now)).toBe("limited");
    expect(botStateOf({ ...base, limitedUntil: "2026-09-25T09:00:00.000Z", waiting: true, busy: true }, now)).toBe("waiting");
    expect(botStateOf({ ...base, busy: true }, now)).toBe("working");
    expect(botStateOf(base, now)).toBe("idle");
  });
});

describe("activityBody", () => {
  it("counts steps and shows the latest one", () => {
    expect(activityBody([{ summary: "Read notes.md" }])).toBe("1 step: Read notes.md");
    expect(activityBody([{ summary: "Read notes.md" }, { summary: "Run npm test" }])).toBe("2 steps: Run npm test");
  });
});

describe("copyIntoInbox", () => {
  it("copies three same-name attachments without overwriting under a fixed clock", () => {
    vi.spyOn(Date, "now").mockReturnValue(1_797_000_000_000);
    const root = tempRoot();
    const home = join(root, "home");
    mkdirSync(home);
    const file = join(root, "CV final ñ.pdf");

    writeFileSync(file, "one");
    const first = copyIntoInbox(home, file);
    writeFileSync(file, "two");
    const second = copyIntoInbox(home, file);
    writeFileSync(file, "three");
    const third = copyIntoInbox(home, file);

    expect(first).toBe("CV final ñ.pdf");
    expect(second).toMatch(/^1797000000000-CV final ñ\.pdf$/);
    expect(third).toMatch(/^1797000000000-\d+-CV final ñ\.pdf$/);
    expect(new Set([first, second, third]).size).toBe(3);
    expect(readFileSync(join(home, "inbox", first), "utf8")).toBe("one");
    expect(readFileSync(join(home, "inbox", second), "utf8")).toBe("two");
    expect(readFileSync(join(home, "inbox", third), "utf8")).toBe("three");
  });
});

describe("Runner queueing", () => {
  it("starts one session and steers a second immediate message in order", async () => {
    let releaseStart = (): void => {};
    const session = new FakeSession();
    session.startPromise = new Promise<void>((resolve) => {
      releaseStart = resolve;
    });
    const h = harness(() => session);
    const bot = newBot(h);

    const first = h.runner.submit(bot.id, "first", []);
    const second = h.runner.submit(bot.id, "second", []);
    expect(h.sessions).toHaveLength(1);
    releaseStart();

    await waitFor(() => session.sent.length === 1 && session.steered.length === 1);
    expect(session.sent).toEqual(["first"]);
    expect(session.steered).toEqual(["second"]);
    expect(itemForMessage(h.db, second.id).status).toBe("done");

    session.emit({ type: "turn_done", ok: true, error: null });
    expect(itemForMessage(h.db, first.id).status).toBe("done");
  });

  it.each(["queued", "running", "done"] as const)("rejects retry when the latest item is %s", (status) => {
    const h = harness(undefined, false);
    const bot = newBot(h);
    const message = addMessage(h.db, { conversationId: bot.id, author: "you", kind: "text", body: status, status: "done" });
    const item = enqueue(h.db, { botId: bot.id, source: "you", text: status, messageId: message.id });
    if (status !== "queued") setStatus(h.db, item.id, status);

    const before = (h.db.prepare("SELECT count(*) AS count FROM work_items WHERE message_id = ?").get(message.id) as { count: number }).count;
    let thrown: unknown;
    try {
      h.runner.retry(message.id);
    } catch (error) {
      thrown = error;
    }

    expect(thrown).toBeInstanceOf(RpcError);
    expect((thrown as RpcError).code).toBe("bad_state");
    expect((h.db.prepare("SELECT count(*) AS count FROM work_items WHERE message_id = ?").get(message.id) as { count: number }).count).toBe(before);
  });

  it.each(["failed", "interrupted"] as const)("allows retry when the latest item is %s", (status) => {
    const h = harness(undefined, false);
    const bot = newBot(h);
    const message = addMessage(h.db, { conversationId: bot.id, author: "you", kind: "text", body: status, status });
    const item = enqueue(h.db, { botId: bot.id, source: "you", text: status, messageId: message.id });
    setStatus(h.db, item.id, status);

    expect(h.runner.retry(message.id).status).toBe("queued");
    expect(itemForMessage(h.db, message.id).status).toBe("queued");
  });
});

describe("Runner turn settlement", () => {
  it("restarts a failed resumed session fresh with a recap", async () => {
    const resumed = new FakeSession();
    const fresh = new FakeSession();
    let rejectResumedSend: (reason: Error) => void = () => {
      throw new Error("Resumed send did not start");
    };
    resumed.sendPromise = new Promise<void>((_resolve, reject) => {
      rejectResumedSend = reject;
    });
    const available = [resumed, fresh];
    const options: StartOptions[] = [];
    let nextSession = 0;
    const h = harness((startOptions) => {
      options.push(startOptions);
      const session = available[nextSession++];
      if (!session) throw new Error("Unexpected extra session");
      return session;
    });
    const bot = newBot(h);
    addMessage(h.db, { conversationId: bot.id, author: "you", kind: "text", body: "Remember ZEBRA", status: "done" });
    addMessage(h.db, { conversationId: bot.id, author: bot.id, kind: "text", body: "Noted ZEBRA", status: "done" });
    h.db
      .prepare("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?, ?, ?, ?, ?)")
      .run(bot.id, "claude", "claude-main", "missing-session", "2026-09-25T10:00:00.000Z");

    const message = h.runner.submit(bot.id, "continue", []);
    await waitFor(() => resumed.sent.length > 0);
    expect(options[0]?.resumeId).toBe("missing-session");
    expect(resumed.sent).toEqual(["continue"]);

    resumed.emit({ type: "exit", error: null });
    await waitFor(() => fresh.sent.length > 0, 2_000);

    expect(options.map((item) => item.resumeId)).toEqual(["missing-session", null]);
    expect(fresh.sent[0]).toContain("[Tjommie: this is a new session of an ongoing chat.");
    expect(fresh.sent[0]).toContain("Person: Remember ZEBRA");
    expect(fresh.sent[0]).toContain(`${bot.name}: Noted ZEBRA`);
    expect(fresh.sent[0]).toContain("continue");
    expect(h.db.prepare("SELECT cli_session_id FROM sessions WHERE bot_id = ?").get(bot.id)).toBeUndefined();
    expect(h.db.prepare("SELECT count(*) AS count FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ count: 1 });
    expect(itemForMessage(h.db, message.id).status).toBe("running");
    expect(history(h.db, bot.id, undefined, 20).some((item) => item.kind === "error")).toBe(false);

    rejectResumedSend(new Error("old session stopped"));
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(itemForMessage(h.db, message.id).status).toBe("running");
    expect(history(h.db, bot.id, undefined, 20).some((item) => item.kind === "error")).toBe(false);

    fresh.emit({ type: "exit", error: null });
    expect(h.sessions).toHaveLength(2);
    expect(itemForMessage(h.db, message.id).status).toBe("failed");
    expect(history(h.db, bot.id, undefined, 20).find((item) => item.kind === "error")?.data).toEqual({ retryMessageId: message.id });
  });

  it("fails with Retry when a fresh session exits before reporting its session", async () => {
    const session = new FakeSession();
    const options: StartOptions[] = [];
    const h = harness((startOptions) => {
      options.push(startOptions);
      return session;
    });
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "start", []);
    await waitFor(() => session.sent.length > 0);
    expect(options[0]?.resumeId).toBeNull();

    session.emit({ type: "exit", error: null });

    expect(h.sessions).toHaveLength(1);
    expect(itemForMessage(h.db, message.id).status).toBe("failed");
    expect(history(h.db, bot.id, undefined, 20).find((item) => item.kind === "error")?.data).toEqual({ retryMessageId: message.id });
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("idle");
  });

  it("fails partial assistant text and offers Retry when the CLI exits mid-turn without an error", async () => {
    const session = new FakeSession();
    const h = harness(() => session);
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "start", []);
    await waitFor(() => session.sent.length > 0);

    session.emit({ type: "text", delta: "partial reply" });
    session.emit({ type: "exit", error: null });

    expect(itemForMessage(h.db, message.id).status).toBe("failed");
    expect(history(h.db, bot.id, undefined, 20).find((item) => item.author === bot.id && item.kind === "text")).toMatchObject({ body: "partial reply", status: "failed" });
    expect(history(h.db, bot.id, undefined, 20).find((item) => item.kind === "error")?.data).toEqual({ retryMessageId: message.id });
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("idle");
  });

  it("interrupts an open turn when a requested stop is followed only by exit", async () => {
    const session = new FakeSession();
    const h = harness(() => session);
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "start", []);
    await waitFor(() => session.sent.length > 0);
    session.emit({ type: "text", delta: "partial reply" });

    await h.runner.stop(bot.id);
    session.emit({ type: "exit", error: null });

    expect(session.interrupts).toBe(1);
    expect(itemForMessage(h.db, message.id).status).toBe("interrupted");
    expect(history(h.db, bot.id, undefined, 20).find((item) => item.author === bot.id && item.kind === "text")?.status).toBe("interrupted");
    expect(history(h.db, bot.id, undefined, 20).some((item) => item.kind === "error")).toBe(false);
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("idle");
  });

  it("treats exit as terminal when a limit event arrived first", async () => {
    const session = new FakeSession();
    const h = harness(() => session);
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "start", []);
    await waitFor(() => session.sent.length > 0);

    session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: "2099-09-25T10:00:00.000Z" });
    session.emit({ type: "exit", error: null });

    expect(itemForMessage(h.db, message.id).status).toBe("failed");
    expect(history(h.db, bot.id, undefined, 20).find((item) => item.kind === "error")?.data).toEqual({ retryMessageId: message.id });
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("limited");
  });

  it("recovers interrupted work and continues queued work on start", async () => {
    const session = new FakeSession();
    const h = harness(() => session);
    const bot = newBot(h);
    const interruptedMessage = addMessage(h.db, { conversationId: bot.id, author: "you", kind: "text", body: "old", status: "sent" });
    const interrupted = enqueue(h.db, { botId: bot.id, source: "you", text: "old", messageId: interruptedMessage.id });
    setStatus(h.db, interrupted.id, "running");
    const queuedMessage = addMessage(h.db, { conversationId: bot.id, author: "you", kind: "text", body: "next", status: "queued" });
    const queued = enqueue(h.db, { botId: bot.id, source: "you", text: "next", messageId: queuedMessage.id });

    h.runner.start();
    await waitFor(() => session.sent.length > 0);

    expect(getWorkItem(h.db, interrupted.id).status).toBe("interrupted");
    expect(getMessage(h.db, interruptedMessage.id).status).toBe("interrupted");
    expect(getWorkItem(h.db, queued.id).status).toBe("running");
    expect(session.sent[0]).toContain("next");
    expect(h.events.some((event) => event.event === "message.added" && event.data.kind === "error" && event.data.data?.retryMessageId === interruptedMessage.id)).toBe(true);
  });
});

describe("Runner approvals", () => {
  it("updates the card and broadcasts the resolved approval when answered", async () => {
    const session = new FakeSession();
    const h = harness(() => session);
    const bot = newBot(h);
    h.runner.submit(bot.id, "write", []);
    await waitFor(() => session.sent.length > 0);

    session.emit({ type: "approval", id: "provider-a1", tool: "Write", summary: "Write hello.txt", detail: "{}", rule: "Write" });
    const added = h.events.find((event) => event.event === "approval.added");
    if (!added) throw new Error("approval.added was not broadcast");
    expect(getMessage(h.db, added.data.messageId).data?.status).toBe("pending");

    const resolved = h.runner.answerApproval(added.data.id, "always");

    expect(resolved.status).toBe("allowed");
    expect(session.answers).toEqual([{ id: "provider-a1", answer: "always" }]);
    expect(getMessage(h.db, added.data.messageId).data?.status).toBe("allowed");
    expect(h.events.some((event) => event.event === "message.updated" && event.data.id === added.data.messageId && event.data.data?.status === "allowed")).toBe(true);
    expect(h.events.some((event) => event.event === "approval.resolved" && event.data.id === added.data.id && event.data.status === "allowed")).toBe(true);
  });

  it("updates and broadcasts every approval card expired at startup", () => {
    const h = harness(undefined, false);
    const bot = newBot(h);
    const card = addMessage(h.db, {
      conversationId: bot.id,
      author: bot.id,
      kind: "approval",
      body: "Write hello.txt",
      data: { approvalId: "a-restart", tool: "Write", detail: "{}", rule: "Write", status: "pending" },
      status: "done",
    });
    createApproval(h.db, { id: "a-restart", botId: bot.id, tool: "Write", summary: "Write hello.txt", detail: "{}", rule: "Write", messageId: card.id });

    h.runner.start();

    expect(getMessage(h.db, card.id).data?.status).toBe("denied");
    expect(h.events.some((event) => event.event === "message.updated" && event.data.id === card.id && event.data.data?.status === "denied")).toBe(true);
    expect(h.events.some((event) => event.event === "approval.resolved" && event.data.id === "a-restart" && event.data.status === "denied")).toBe(true);
  });
});
