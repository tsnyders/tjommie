import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BotInput, EventData, EventName } from "@tjommie/protocol";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createApproval } from "./approvals.ts";
import { createBot, getStoredBot } from "./bots.ts";
import { addMessage, getMessage, history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { McpTokens } from "./mcp.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, getWorkItem, newChain, setStatus, type WorkItem, type WorkStatus } from "./queue.ts";
import { activityBody, botStateOf, copyIntoInbox, Runner, type RunnerHooks, type SessionFactory, type TurnResult } from "./runner.ts";
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
  closeError: Error | null = null;
  steerResult = true;
  steerPromise: Promise<boolean> | null = null;
  steerError: Error | null = null;
  interruptError: Error | null = null;

  start(): Promise<void> {
    return this.startPromise;
  }

  async send(text: string): Promise<void> {
    this.sent.push(text);
    await this.sendPromise;
  }

  async steer(text: string): Promise<boolean> {
    this.steered.push(text);
    if (this.steerError) throw this.steerError;
    if (this.steerPromise) return this.steerPromise;
    return this.steerResult;
  }

  async interrupt(): Promise<void> {
    this.interrupts++;
    if (this.interruptError) throw this.interruptError;
  }

  answer(id: string, answer: "once" | "always" | "deny"): void {
    this.answers.push({ id, answer });
  }

  close(): void {
    this.closed = true;
    if (this.closeError) throw this.closeError;
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

function harness(factory: (options: StartOptions) => FakeSession = () => new FakeSession(), signedIn = true, hooks: RunnerHooks = {}, onBroadcast?: Broadcast): Harness {
  const root = tempRoot();
  const db = openDb(join(root, "t.db"));
  ensureAccounts(db, join(root, "accounts"));
  const events: SentEvent[] = [];
  const sessions: FakeSession[] = [];
  const broadcast: Broadcast = (event, data) => {
    events.push({ event, data } as SentEvent);
    onBroadcast?.(event, data);
  };
  const createSession: SessionFactory = (_bot, options) => {
    const session = factory(options);
    sessions.push(session);
    return session;
  };
  const runner = new Runner({ db, broadcast, isSignedIn: () => signedIn, createSession, hooks });
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
  it("rejects missing paths and directories before copying attachments or saving a message", () => {
    const h = harness();
    const bot = newBot(h);
    const file = join(h.root, "valid.txt");
    writeFileSync(file, "valid");
    for (const invalid of [join(h.root, "missing.txt"), h.root]) {
      expect(() => h.runner.submit(bot.id, "Files", [file, invalid])).toThrow(expect.objectContaining({ code: "bad_params", message: expect.stringContaining(invalid) }));
      expect(history(h.db, bot.id, undefined, 20)).toEqual([]);
      expect(readdirSync(join(bot.homeFolder, "inbox"))).toEqual([]);
    }
  });
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

describe("Runner session costs", () => {
  it("preserves the cost baseline across a daemon restart and records only the resumed delta", async () => {
    const h = harness();
    const bot = newBot(h);
    h.runner.submit(bot.id, "first", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    h.sessions[0].emit({ type: "session", cliSessionId: "saved" });
    h.sessions[0].emit({ type: "usage", model: "haiku", input: 1, cachedInput: 0, output: 1, reasoning: 0, costUsd: 0.0169623, totalCostUsd: 0.0169623 });
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    h.runner.shutdown();
    h.db.close();

    const db = openDb(join(h.root, "t.db"));
    const resumed = new FakeSession();
    const options: StartOptions[] = [];
    const restarted = new Runner({
      db,
      broadcast: () => {},
      isSignedIn: () => true,
      createSession: (_bot, startOptions) => {
        options.push(startOptions);
        return resumed;
      },
    });
    runners.push(restarted);
    try {
      restarted.submit(bot.id, "continue", []);
      await waitFor(() => resumed.sent.length);
      expect(options[0]).toMatchObject({ resumeId: "saved", resumeCostUsd: 0.0169623 });
      resumed.emit({ type: "session", cliSessionId: "saved" });
      expect(db.prepare("SELECT total_cost_usd FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ total_cost_usd: 0.0169623 });

      resumed.emit({ type: "usage", model: "haiku", input: 1, cachedInput: 0, output: 1, reasoning: 0, costUsd: 0.0029083, totalCostUsd: 0.0198706 });
      resumed.emit({ type: "turn_done", ok: true, error: null });
      expect(db.prepare("SELECT total_cost_usd FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ total_cost_usd: 0.0198706 });
      const total = db.prepare("SELECT sum(cost_usd) AS cost FROM usage WHERE bot_id = ?").get(bot.id) as { cost: number };
      expect(total.cost).toBeCloseTo(0.0198706, 10);
    } finally {
      restarted.shutdown();
      db.close();
    }
  });

  it.each([
    { name: "provider", provider: "codex", accountId: "claude-main", emittedId: "saved", resumeId: null, resumeCostUsd: null },
    { name: "account", provider: "claude", accountId: "old-account", emittedId: "saved", resumeId: null, resumeCostUsd: null },
    { name: "CLI session", provider: "claude", accountId: "claude-main", emittedId: "replacement", resumeId: "saved", resumeCostUsd: 0.0169623 },
  ])("clears the saved cost when the $name changes", async ({ provider, accountId, emittedId, resumeId, resumeCostUsd }) => {
    const options: StartOptions[] = [];
    const h = harness((startOptions) => {
      options.push(startOptions);
      return new FakeSession();
    });
    const bot = newBot(h);
    h.db.prepare("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at, total_cost_usd) VALUES (?, ?, ?, ?, ?, ?)")
      .run(bot.id, provider, accountId, "saved", "then", 0.0169623);

    h.runner.submit(bot.id, "continue", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    expect(options[0]).toMatchObject({ resumeId, resumeCostUsd });
    h.sessions[0].emit({ type: "session", cliSessionId: emittedId });
    expect(h.db.prepare("SELECT provider, account_id, cli_session_id, total_cost_usd FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({
      provider: "claude",
      account_id: "claude-main",
      cli_session_id: emittedId,
      total_cost_usd: null,
    });
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

  it("interrupts a stopped resumed session without retrying it fresh", async () => {
    const resumed = new FakeSession();
    const fresh = new FakeSession();
    const available = [resumed, fresh];
    let nextSession = 0;
    const h = harness(() => {
      const session = available[nextSession++];
      if (!session) throw new Error("Unexpected extra session");
      return session;
    });
    const bot = newBot(h);
    h.db
      .prepare("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?, ?, ?, ?, ?)")
      .run(bot.id, "claude", "claude-main", "saved-session", "2026-09-25T10:00:00.000Z");
    const message = h.runner.submit(bot.id, "start", []);
    await waitFor(() => resumed.sent.length > 0);

    await h.runner.stop(bot.id);
    resumed.emit({ type: "exit", error: null });
    await new Promise<void>((resolve) => setImmediate(resolve));

    expect(resumed.interrupts).toBe(1);
    expect(h.sessions).toHaveLength(1);
    expect(itemForMessage(h.db, message.id).status).toBe("interrupted");
    expect(h.db.prepare("SELECT cli_session_id FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ cli_session_id: "saved-session" });
    expect(history(h.db, bot.id, undefined, 20).some((item) => item.kind === "notice" && item.body === "Stopped.")).toBe(true);
  });

  it("settles a successful turn as done even after a reached limit event", async () => {
    const session = new FakeSession();
    const h = harness(() => session);
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "finish this once", []);
    await waitFor(() => session.sent.length > 0);

    session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: "2099-09-25T10:00:00.000Z" });
    session.emit({ type: "turn_done", ok: true, error: null });

    expect(itemForMessage(h.db, message.id).status).toBe("done");
    expect(getMessage(h.db, message.id).status).toBe("sent");
  });

  it("settles an early Stop after startup without sending and clears it before later work", async () => {
    let releaseStart: () => void = () => {
      throw new Error("Start did not begin");
    };
    const session = new FakeSession();
    session.startPromise = new Promise<void>((resolve) => {
      releaseStart = resolve;
    });
    const h = harness(() => session);
    const bot = newBot(h);
    const stopped = h.runner.submit(bot.id, "do not send", []);

    await h.runner.stop(bot.id);
    releaseStart();
    await waitFor(() => itemForMessage(h.db, stopped.id).status === "interrupted", 2_000);

    expect(session.interrupts).toBe(1);
    expect(session.sent).toEqual([]);
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("idle");
    expect(h.runner.processId(bot.id)).toBe(4321);

    const later = h.runner.submit(bot.id, "later work", []);
    await waitFor(() => session.sent.length > 0);
    session.emit({ type: "exit", error: "later failure" });

    expect(itemForMessage(h.db, later.id).status).toBe("failed");
    expect(history(h.db, bot.id, undefined, 20).find((item) => item.kind === "error" && item.data?.retryMessageId === later.id)?.body).toBe("later failure");
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
    const streaming = addMessage(h.db, { conversationId: bot.id, author: bot.id, kind: "text", body: "partial reply", status: "streaming" });

    h.runner.start();
    await waitFor(() => session.sent.length > 0);

    expect(getWorkItem(h.db, interrupted.id).status).toBe("interrupted");
    expect(getMessage(h.db, interruptedMessage.id).status).toBe("interrupted");
    expect(getMessage(h.db, streaming.id).status).toBe("interrupted");
    expect(getWorkItem(h.db, queued.id).status).toBe("running");
    expect(session.sent[0]).toContain("next");
    expect(h.events.some((event) => event.event === "message.added" && event.data.kind === "error" && event.data.data?.retryMessageId === interruptedMessage.id)).toBe(true);
    expect(h.events.some((event) => event.event === "message.updated" && event.data.id === streaming.id && event.data.status === "interrupted")).toBe(true);
  });
});

describe("Runner idle shutdown", () => {
  it("closes an idle CLI after ten minutes while the bot is limited", async () => {
    const session = new FakeSession();
    const h = harness(() => session);
    const bot = newBot(h);
    h.runner.submit(bot.id, "hit the limit", []);
    await waitFor(() => session.sent.length > 0);

    vi.useFakeTimers();
    try {
      const resetsAt = new Date(Date.now() + 2 * 60 * 60_000).toISOString();
      session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt });
      session.emit({ type: "turn_done", ok: false, error: "Usage limit reached" });
      expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("limited");
      expect(session.closed).toBe(false);

      await vi.advanceTimersByTimeAsync(10 * 60_000);

      expect(session.closed).toBe(true);
      expect(h.runner.processId(bot.id)).toBeUndefined();
    } finally {
      h.runner.shutdown();
      vi.useRealTimers();
    }
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

describe("Runner.start", () => {
  it("recovers before listening and holds work delivered by recovery hooks until start", async () => {
    let runner: Runner | undefined;
    const h = harness(undefined, true, {
      turnEnded: (item) => runner?.deliver({ botId: item.botId, source: "bot_reply", text: "Recovery reply" }),
    });
    runner = h.runner;
    const bot = newBot(h);
    const interrupted = enqueue(h.db, { botId: bot.id, source: "routine", text: "Interrupted routine" });
    setStatus(h.db, interrupted.id, "running");

    h.runner.recover();
    h.runner.pumpAll();
    expect(getWorkItem(h.db, interrupted.id).status).toBe("interrupted");
    expect(h.sessions).toEqual([]);
    h.runner.start();
    await waitFor(() => h.sessions[0]?.sent.length);
    expect(h.sessions).toHaveLength(1);
    expect(h.sessions[0].sent[0]).toContain("Recovery reply");
    expect(getWorkItem(h.db, interrupted.id).status).toBe("interrupted");
  });

  it("reports a turn a restart cut off to the hooks, and says so where the turn was writing", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const bot = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" });
    const item = enqueue(db, { botId: bot.id, source: "room", text: "@Writer hi", conversationId: "team" });
    setStatus(db, item.id, "running");
    const ended: [string, TurnResult][] = [];
    const runner = new Runner({ db, broadcast: () => {}, isSignedIn: () => true, hooks: { turnEnded: (work, result) => ended.push([work.id, result]) } });
    runners.push(runner);
    runner.start();
    expect(ended).toEqual([[item.id, { outcome: "failed", text: null, textMessageId: null, error: "Tjommie restarted while this was running." }]]);
    expect(history(db, "team", undefined, 1)[0]).toMatchObject({ author: "system", kind: "error", body: "Tjommie restarted while this was running." });
    runner.shutdown();
  });
});

describe("Runner bot deletion", () => {
  it("keeps queued work unchanged while closing for deletion and pumps it after failure", async () => {
    const ended: WorkItem[] = [];
    const h = harness(undefined, true, { turnEnded: (item) => ended.push(item) });
    const bot = newBot(h);
    const item = enqueue(h.db, { botId: bot.id, source: "routine", text: "Queued routine" });
    const before = h.db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(bot.id);
    h.runner.closeForDeletion(bot.id);
    expect(h.db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(bot.id)).toEqual(before);
    expect(ended).toEqual([]);
    h.runner.pump(bot.id);
    await waitFor(() => h.sessions[0]?.sent.length);
    expect(h.runner.currentWork(bot.id)?.id).toBe(item.id);
  });

  it.each(["start", "send"] as const)("restarts active work after failed deletion and ignores late %s rejection", async (boundary) => {
    let rejectPending: (error: Error) => void = () => { throw new Error("No pending operation"); };
    const pending = new Promise<void>((_resolve, reject) => { rejectPending = reject; });
    const oldSession = new FakeSession();
    if (boundary === "start") oldSession.startPromise = pending;
    else oldSession.sendPromise = pending;
    const replacement = new FakeSession();
    let calls = 0;
    const ended: WorkItem[] = [];
    const started: WorkItem[] = [];
    const requeued: WorkItem[] = [];
    const h = harness(() => calls++ === 0 ? oldSession : replacement, true, {
      turnStarted: (item) => started.push(item),
      turnEnded: (item) => ended.push(item),
      turnRequeued: (item) => requeued.push(item),
    });
    const bot = newBot(h);
    const item = h.runner.deliver({ botId: bot.id, source: "routine", text: "Active routine" });
    if (boundary === "send") await waitFor(() => oldSession.sent.length);
    const before = h.db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(bot.id);
    h.runner.closeForDeletion(bot.id);
    expect(oldSession.closed).toBe(true);
    expect(h.db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(bot.id)).toEqual(before);
    expect(ended).toEqual([]);
    h.runner.pump(bot.id);
    await waitFor(() => replacement.sent.length);
    rejectPending(new Error("Old operation rejected after replacement"));
    oldSession.emit({ type: "exit", error: "Old exit" });
    oldSession.emit({ type: "text", delta: "Old text" });
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(h.runner.currentWork(bot.id)?.id).toBe(item.id);
    expect(getWorkItem(h.db, item.id).status).toBe("running");
    expect(started.map((work) => work.id)).toEqual([item.id]);
    expect(requeued).toEqual([]);
    expect(ended).toEqual([]);
    replacement.emit({ type: "turn_done", ok: true, error: null });
    expect(ended.map((work) => work.id)).toEqual([item.id]);
  });

  it("ends captured active and queued routine work after successful deletion", async () => {
    const ended: [WorkItem, TurnResult][] = [];
    const h = harness(undefined, true, { turnEnded: (item, result) => ended.push([item, result]) });
    const bot = newBot(h);
    const active = h.runner.deliver({ botId: bot.id, source: "routine", text: "Active routine" });
    await waitFor(() => h.sessions[0]?.sent.length);
    const queued = h.runner.deliver({ botId: bot.id, source: "routine", text: "Queued routine" });
    h.runner.closeForDeletion(bot.id);
    h.db.prepare("DELETE FROM bots WHERE id = ?").run(bot.id);
    h.runner.forget(bot.id);
    h.runner.forget(bot.id);
    expect(ended.map(([item, result]) => [item.id, result.outcome])).toEqual([[active.id, "interrupted"], [queued.id, "interrupted"]]);
    expect(h.runner.currentWork(bot.id)).toBeNull();
    expect(h.runner.processId(bot.id)).toBeUndefined();
  });

  it.each([false, true])("ignores pending steering after deletion (successful: %s)", async (deleted) => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    let resolveSteer: (value: boolean) => void = () => { throw new Error("No pending steering"); };
    const oldSession = new FakeSession();
    oldSession.steerPromise = new Promise<boolean>((resolve) => { resolveSteer = resolve; });
    const replacement = new FakeSession();
    let calls = 0;
    const h = harness(() => calls++ === 0 ? oldSession : replacement);
    const bot = newBot(h);
    h.runner.submit(bot.id, "First", []);
    await waitFor(() => oldSession.sent.length);
    const steered = h.runner.submit(bot.id, "Still needed", []);
    await waitFor(() => oldSession.steered.length);
    h.runner.closeForDeletion(bot.id);
    if (deleted) {
      h.db.prepare("DELETE FROM bots WHERE id = ?").run(bot.id);
      h.runner.forget(bot.id);
    } else {
      h.runner.pump(bot.id);
      await waitFor(() => replacement.steered.length);
      expect(replacement.steered).toEqual(["Still needed"]);
      expect(itemForMessage(h.db, steered.id).status).toBe("done");
    }
    resolveSteer(false);
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(logged).not.toHaveBeenCalled();
    if (!deleted) expect(itemForMessage(h.db, steered.id).status).toBe("done");
  });

  it("leaves interrupted-by-shutdown work running for startup recovery without turn-end hooks", async () => {
    const ended: WorkItem[] = [];
    const h = harness(undefined, true, { turnEnded: (item) => ended.push(item) });
    const bot = newBot(h);
    const item = h.runner.deliver({ botId: bot.id, source: "routine", text: "Recover on restart" });
    await waitFor(() => h.sessions[0]?.sent.length);
    h.runner.shutdown();
    expect(h.sessions[0].closed).toBe(true);
    expect(getWorkItem(h.db, item.id).status).toBe("running");
    expect(ended).toEqual([]);
  });

  it("expires obsolete approval cards only when failed deletion resumes the bot", async () => {
    const h = harness();
    const bot = newBot(h);
    const work = h.runner.deliver({ botId: bot.id, source: "routine", text: "Needs approval" });
    await waitFor(() => h.sessions[0]?.sent.length);
    h.sessions[0].emit({ type: "approval", id: "old-provider-request", tool: "Write", summary: "Write notes", detail: "{}", rule: null });
    const added = h.events.find((event) => event.event === "approval.added");
    if (!added) throw new Error("Missing approval card");
    h.runner.closeForDeletion(bot.id);
    expect(getMessage(h.db, added.data.messageId).data?.status).toBe("pending");
    h.runner.pump(bot.id);
    await waitFor(() => h.sessions[1]?.sent.length);
    expect(getMessage(h.db, added.data.messageId).data?.status).toBe("denied");
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("working");
    expect(getWorkItem(h.db, work.id).status).toBe("running");
    expect(h.sessions[1].answers).toEqual([]);
  });

  it("closes every session and revokes every token when one shutdown close throws", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const closed: string[] = [];
    const h = harness(undefined, true, { sessionClosed: (id) => closed.push(id) });
    const first = newBot(h);
    const second = newBot(h);
    h.runner.submit(first.id, "First", []);
    h.runner.submit(second.id, "Second", []);
    await waitFor(() => h.sessions.every((session) => session.sent.length));
    const failure = new Error("Session close failed");
    h.sessions[0].closeError = failure;
    expect(() => h.runner.shutdown()).not.toThrow();
    expect(h.sessions.every((session) => session.closed)).toBe(true);
    expect(closed).toEqual([first.id, second.id]);
    expect(logged.mock.calls.some((call) => call.includes(failure))).toBe(true);
  });
});

describe("Runner destinations and delivery", () => {
  it("starts prequeued work only when delivered, without duplicating it, and uses its persisted destination and hooks", async () => {
    const started: WorkItem[] = [];
    const ended: [WorkItem, TurnResult][] = [];
    const h = harness(undefined, true, { turnStarted: (item) => started.push(item), turnEnded: (item, result) => ended.push([item, result]) });
    const bot = newBot(h);
    const other = newBot(h);
    const queued = enqueue(h.db, { botId: bot.id, source: "bot_request", text: "Persisted request", conversationId: "team", chainId: newChain(h.db), exchangeId: "exchange-1" });
    await Promise.resolve();
    expect(h.sessions).toHaveLength(0);
    expect(started).toEqual([]);
    expect(getWorkItem(h.db, queued.id).status).toBe("queued");

    const delivered = h.runner.deliver({ ...queued, botId: other.id, text: "Stale request" });
    expect(delivered).toEqual(queued);
    expect(h.runner.currentWork(bot.id)?.id).toBe(queued.id);
    expect(h.runner.currentWork(other.id)).toBeNull();
    await waitFor(() => h.sessions[0]?.sent.length);
    h.runner.deliver(queued);
    expect(h.db.prepare("SELECT COUNT(*) AS n FROM work_items").get()).toMatchObject({ n: 1 });
    expect(h.sessions[0].sent).toEqual(["Persisted request"]);
    expect(started.map((item) => item.id)).toEqual([queued.id]);
    h.sessions[0].emit({ type: "text", delta: "The answer" });
    h.sessions[0].emit({ type: "text_end" });
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    expect(ended).toHaveLength(1);
    expect(ended[0]).toMatchObject([{ id: queued.id, conversationId: "team", exchangeId: "exchange-1" }, { outcome: "done", text: "The answer" }]);
    expect(getWorkItem(h.db, queued.id).status).toBe("done");
    expect(h.db.prepare("SELECT COUNT(*) AS n FROM work_items").get()).toMatchObject({ n: 1 });
  });

  it.each(["chat", "team", "exchange"] as const)("writes text, deltas, activity, errors and Stop to the %s destination", async (destination) => {
    const h = harness();
    const bot = newBot(h);
    const conversationId = destination === "chat" ? bot.id : destination;
    if (destination === "exchange") h.db.prepare("INSERT INTO conversations (id, kind) VALUES (?, 'exchange')").run(conversationId);
    const first = h.runner.deliver({ botId: bot.id, source: "bot_request", text: "answer here", conversationId, chainId: "chain-a", exchangeId: "request-a" });
    expect(first).toMatchObject({ botId: bot.id, conversationId, chainId: "chain-a", exchangeId: "request-a" });
    expect(h.runner.currentWork(bot.id)?.id).toBe(first.id);
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    session.emit({ type: "text", delta: "A thought" });
    session.emit({ type: "text_end" });
    session.emit({ type: "tool", id: "read", name: "Read", summary: "Read notes.md" });
    session.emit({ type: "tool_done", id: "read", ok: true });
    session.emit({ type: "text", delta: "unfinished answer" });
    session.emit({ type: "turn_done", ok: false, error: "answer failed" });

    expect(h.runner.currentWork(bot.id)).toBeNull();
    expect(getWorkItem(h.db, first.id).status).toBe("failed");
    expect(history(h.db, conversationId, undefined, 20)).toEqual(expect.arrayContaining([
      expect.objectContaining({ author: bot.id, kind: "text", body: "A thought", status: "done" }),
      expect.objectContaining({ author: bot.id, kind: "activity", body: "1 step: Read notes.md", status: "done" }),
      expect.objectContaining({ author: bot.id, kind: "text", body: "unfinished answer", status: "failed" }),
      expect.objectContaining({ author: "system", kind: "error", body: "answer failed" }),
    ]));
    const deltas = h.events.filter((event) => event.event === "message.delta");
    expect(deltas.map((event) => event.data.conversationId)).toEqual([conversationId, conversationId]);

    const second = h.runner.deliver({ botId: bot.id, source: "routine", text: "stop here", conversationId });
    await waitFor(() => session.sent.length === 2);
    await h.runner.stop(bot.id);
    session.emit({ type: "turn_done", ok: false, error: null });
    expect(getWorkItem(h.db, second.id).status).toBe("interrupted");
    expect(history(h.db, conversationId, undefined, 20).at(-1)).toMatchObject({ kind: "notice", body: "Stopped." });
    if (destination !== "chat") expect(history(h.db, bot.id, undefined, 20)).toEqual([]);
  });

  it.each(["team", "exchange"] as const)("keeps approvals and usage-limit notices in the bot's chat while writing to %s", async (conversationId) => {
    const h = harness();
    const bot = newBot(h);
    if (conversationId === "exchange") h.db.prepare("INSERT INTO conversations (id, kind) VALUES (?, 'exchange')").run(conversationId);
    h.runner.deliver({ botId: bot.id, source: "bot_request", text: "write", conversationId });
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    session.emit({ type: "text", delta: "Working here" });
    session.emit({ type: "approval", id: "approval-1", tool: "Write", summary: "Write a file", detail: "{}", rule: null });
    session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: "2099-09-25T10:00:00.000Z" });
    expect(history(h.db, bot.id, undefined, 20)).toEqual(expect.arrayContaining([
      expect.objectContaining({ kind: "approval", body: "Write a file" }),
      expect.objectContaining({ kind: "notice", body: expect.stringContaining("hit its usage limit") }),
    ]));
    expect(history(h.db, conversationId, undefined, 20)).toEqual([expect.objectContaining({ kind: "text", body: "Working here" })]);
    expect(h.events.filter((event) => event.event === "notify").map((event) => event.data.botId)).toEqual([bot.id, bot.id]);
    const approval = history(h.db, bot.id, undefined, 20).find((message) => message.kind === "approval")!;
    expect(h.events).toContainEqual({ event: "notify", data: expect.objectContaining({ kind: "approval", botId: bot.id, approvalId: approval.data?.approvalId }) });
    expect(h.events).toContainEqual({ event: "notify", data: expect.objectContaining({ kind: "limit", botId: bot.id }) });
    session.emit({ type: "turn_done", ok: true, error: null });
  });

  it("starts a separate chain for every submitted message and every retry", () => {
    const h = harness(undefined, false);
    const bot = newBot(h);
    const first = h.runner.submit(bot.id, "first", []);
    const second = h.runner.submit(bot.id, "second", []);
    const firstItem = getWorkItem(h.db, itemForMessage(h.db, first.id).id);
    const secondItem = getWorkItem(h.db, itemForMessage(h.db, second.id).id);
    setStatus(h.db, firstItem.id, "failed");
    h.runner.retry(first.id);
    const retried = getWorkItem(h.db, itemForMessage(h.db, first.id).id);
    const chainIds = [firstItem.chainId, secondItem.chainId, retried.chainId];
    expect(chainIds.every((id) => typeof id === "string" && id.length > 0)).toBe(true);
    expect(new Set(chainIds).size).toBe(3);
    expect(h.db.prepare("SELECT count(*) AS count FROM chains WHERE requests_made = 0").get()).toEqual({ count: 3 });
    expect(retried).toMatchObject({ conversationId: bot.id, source: "you", text: "first", messageId: first.id });
  });

  it("retry preserves the prior destination and routes through delivery", async () => {
    const h = harness();
    const bot = newBot(h);
    const message = addMessage(h.db, { conversationId: "team", author: "you", kind: "text", body: "room retry", status: "failed" });
    const failed = enqueue(h.db, { botId: bot.id, source: "room", text: "room retry", conversationId: "team", messageId: message.id });
    setStatus(h.db, failed.id, "failed");
    h.runner.retry(message.id);
    await waitFor(() => h.sessions[0]?.sent.length);
    expect(h.runner.currentWork(bot.id)).toMatchObject({ conversationId: "team", source: "room", messageId: message.id });
    h.sessions[0].emit({ type: "text", delta: "ROOM" });
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    expect(history(h.db, "team", undefined, 20).at(-1)).toMatchObject({ body: "ROOM", status: "done" });
  });

  it("retry preserves a failed bot request's exchange link for the completion hook but starts a new chain without the old routine run", async () => {
    const ended: [WorkItem, TurnResult][] = [];
    const h = harness(undefined, true, { turnEnded: (item, result) => ended.push([item, result]) });
    const bot = newBot(h);
    const exchangeId = "exchange-to-retry";
    h.db.prepare("INSERT INTO conversations (id, kind) VALUES (?, 'exchange')").run(exchangeId);
    const message = addMessage(h.db, { conversationId: exchangeId, author: bot.id, kind: "text", body: "Please help", status: "failed" });
    const originalChain = newChain(h.db);
    const failed = enqueue(h.db, {
      botId: bot.id, source: "bot_request", text: "Please help", messageId: message.id,
      conversationId: exchangeId, exchangeId, chainId: originalChain, routineRunId: "original-run",
    });
    setStatus(h.db, failed.id, "failed");

    h.runner.retry(message.id);
    await waitFor(() => h.sessions[0]?.sent.length);
    const retried = getWorkItem(h.db, itemForMessage(h.db, message.id).id);
    expect(retried).toMatchObject({
      botId: bot.id, source: "bot_request", text: "Please help", messageId: message.id,
      conversationId: exchangeId, exchangeId, routineRunId: null, status: "running",
    });
    expect(retried.id).not.toBe(failed.id);
    expect(retried.chainId).toBeTypeOf("string");
    expect(retried.chainId).not.toBe(originalChain);
    expect(h.db.prepare("SELECT count(*) AS count FROM chains").get()).toEqual({ count: 2 });
    expect(getWorkItem(h.db, failed.id).status).toBe("failed");
    h.sessions[0].emit({ type: "text", delta: "Here is the help" });
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    expect(ended).toHaveLength(1);
    expect(ended[0][0]).toMatchObject({ id: retried.id, conversationId: exchangeId, exchangeId, chainId: retried.chainId, routineRunId: null });
    expect(ended[0][1]).toMatchObject({ outcome: "done", text: "Here is the help", error: null });
    expect(history(h.db, exchangeId, undefined, 1)[0]).toMatchObject({ body: "Here is the help", status: "done" });
  });

  it("stopItem only interrupts its current item and resets its custom notice before later work", async () => {
    const h = harness();
    const bot = newBot(h);
    const item = h.runner.deliver({ botId: bot.id, source: "routine", text: "first", conversationId: "team", routineRunId: "run-1" });
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    h.runner.stopItem("another-item", "Wrong notice");
    expect(session.interrupts).toBe(0);
    expect(h.runner.currentWork(bot.id)).toMatchObject({ id: item.id, routineRunId: "run-1" });
    h.runner.stopItem(item.id, "Routine time limit reached.");
    expect(session.interrupts).toBe(1);
    session.emit({ type: "turn_done", ok: false, error: null });
    expect(h.runner.currentWork(bot.id)).toBeNull();
    expect(history(h.db, "team", undefined, 1)[0].body).toBe("Routine time limit reached.");

    const later = h.runner.deliver({ botId: bot.id, source: "routine", text: "second", conversationId: "team" });
    await waitFor(() => session.sent.length === 2);
    h.runner.stopItem(item.id, "Stale timer");
    expect(session.interrupts).toBe(1);
    expect(h.runner.currentWork(bot.id)?.id).toBe(later.id);
    await h.runner.stop(bot.id);
    session.emit({ type: "exit", error: null });
    expect(history(h.db, "team", undefined, 1)[0].body).toBe("Stopped.");
  });
});

describe("Runner steering boundaries", () => {
  it("steers only same-conversation messages and replies past older non-steerable queue entries", async () => {
    let releaseSend: () => void = () => { throw new Error("Send was not held"); };
    const session = new FakeSession();
    session.sendPromise = new Promise<void>((resolve) => { releaseSend = resolve; });
    const h = harness(() => session);
    const bot = newBot(h);
    const running = h.runner.deliver({ botId: bot.id, source: "room", text: "running", conversationId: "team" });
    await waitFor(() => session.sent.length > 0);
    const room = h.runner.deliver({ botId: bot.id, source: "room", text: "room waits", conversationId: "team" });
    const request = h.runner.deliver({ botId: bot.id, source: "bot_request", text: "request waits", conversationId: "team" });
    const routine = h.runner.deliver({ botId: bot.id, source: "routine", text: "routine waits", conversationId: "team" });
    const chat = h.runner.submit(bot.id, "chat waits", []);
    const replyElsewhere = h.runner.deliver({ botId: bot.id, source: "bot_reply", text: "other reply waits", conversationId: bot.id });
    const reply = h.runner.deliver({ botId: bot.id, source: "bot_reply", text: "reply now", conversationId: "team" });
    const message = addMessage(h.db, { conversationId: "team", author: "you", kind: "text", body: "steer now", status: "queued" });
    const yours = h.runner.deliver({ botId: bot.id, source: "you", text: "steer now", conversationId: "team", messageId: message.id });
    expect(session.steered).toEqual([]);
    releaseSend();
    await waitFor(() => session.steered.length === 2);
    expect(session.steered).toEqual(["reply now", "steer now"]);
    expect(getWorkItem(h.db, reply.id).status).toBe("done");
    expect(getWorkItem(h.db, yours.id).status).toBe("done");
    expect(getMessage(h.db, message.id).status).toBe("sent");
    for (const item of [room, request, routine, replyElsewhere]) expect(getWorkItem(h.db, item.id).status).toBe("queued");
    expect(getMessage(h.db, chat.id).status).toBe("queued");
    expect(h.runner.currentWork(bot.id)?.id).toBe(running.id);
  });

  it.each([false, true])("queues a declined steer and logs only an unexpected rejection (unexpected: %s)", async (unexpected) => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const session = new FakeSession();
    session.steerResult = false;
    const failure = new Error("steer transport failed");
    if (unexpected) session.steerError = failure;
    const h = harness(() => session);
    const bot = newBot(h);
    h.runner.submit(bot.id, "first", []);
    await waitFor(() => session.sent.length);
    await new Promise<void>((resolve) => setImmediate(resolve));
    const second = h.runner.submit(bot.id, "second", []);
    await waitFor(() => session.steered.length);
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(itemForMessage(h.db, second.id).status).toBe("queued");
    expect(getMessage(h.db, second.id).status).toBe("queued");
    if (unexpected) expect(logged.mock.calls.some((call) => call.includes(failure))).toBe(true);
    else expect(logged).not.toHaveBeenCalled();
    session.emit({ type: "turn_done", ok: true, error: null });
    await waitFor(() => session.sent.length === 2);
    expect(session.sent[1]).toBe("second");
    expect(itemForMessage(h.db, second.id).status).toBe("running");
  });
});

describe("Runner turn hooks", () => {
  it.each(["done", "failed", "interrupted"] as const)("reports the %s outcome with its last finished nonblank text", async (outcome) => {
    const started: WorkItem[] = [];
    const ended: [WorkItem, TurnResult][] = [];
    const h = harness(undefined, true, { turnStarted: (item) => started.push(item), turnEnded: (item, result) => ended.push([item, result]) });
    const bot = newBot(h);
    const item = h.runner.deliver({ botId: bot.id, source: "bot_request", text: "reply", conversationId: "team" });
    expect(started.map((work) => work.id)).toEqual([item.id]);
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    session.emit({ type: "text", delta: "first thought" });
    session.emit({ type: "text_end" });
    session.emit({ type: "text", delta: "finished answer" });
    session.emit({ type: "text_end" });
    const finishedMessage = history(h.db, "team", undefined, 1)[0];
    session.emit({ type: "text", delta: "  " });
    session.emit({ type: "text_end" });
    session.emit({ type: "text", delta: "final fragment" });
    if (outcome === "interrupted") await h.runner.stop(bot.id);
    const error = outcome === "failed" ? "failed after answer" : null;
    session.emit({ type: "turn_done", ok: outcome === "done", error });
    const lastMessage = history(h.db, "team", undefined, 20).find((message) => message.body === "final fragment");
    expect(lastMessage?.status).toBe(outcome);
    expect(ended).toHaveLength(1);
    expect(ended[0][0].id).toBe(item.id);
    expect(ended[0][1]).toEqual({
      outcome,
      text: outcome === "done" ? "final fragment" : "finished answer",
      textMessageId: outcome === "done" ? lastMessage?.id : finishedMessage.id,
      error,
    });
    session.emit({ type: "turn_done", ok: false, error: "duplicate terminal event" });
    expect(ended).toHaveLength(1);
  });

  it("resets result text between turns and leaves provider-started turns without a work item", async () => {
    const ended: [WorkItem, TurnResult][] = [];
    const started: WorkItem[] = [];
    const h = harness(undefined, true, { turnStarted: (item) => started.push(item), turnEnded: (item, result) => ended.push([item, result]) });
    const bot = newBot(h);
    h.runner.submit(bot.id, "first", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    session.emit({ type: "text", delta: "answer one" });
    session.emit({ type: "turn_done", ok: true, error: null });
    h.runner.submit(bot.id, "second", []);
    await waitFor(() => session.sent.length === 2);
    session.emit({ type: "turn_done", ok: true, error: null });
    expect(ended[1][1]).toEqual({ outcome: "done", text: null, textMessageId: null, error: null });
    session.emit({ type: "text", delta: "background answer" });
    expect(h.runner.currentWork(bot.id)).toBeNull();
    session.emit({ type: "turn_done", ok: true, error: null });
    expect(started).toHaveLength(2);
    expect(ended).toHaveLength(2);
    expect(history(h.db, bot.id, undefined, 1)[0]).toMatchObject({ body: "background answer", status: "done" });
    h.runner.submit(bot.id, "third", []);
    await waitFor(() => session.sent.length === 3);
    session.emit({ type: "turn_done", ok: true, error: null });
    expect(ended[2][1]).toEqual({ outcome: "done", text: null, textMessageId: null, error: null });
  });

  it("calls turnRequeued instead of turnEnded for a failed limit turn and starts its same item again", async () => {
    const started: WorkItem[] = [];
    const requeued: WorkItem[] = [];
    const ended: [WorkItem, TurnResult][] = [];
    const h = harness(undefined, true, {
      turnStarted: (item) => started.push(item),
      turnRequeued: (item) => requeued.push(item),
      turnEnded: (item, result) => ended.push([item, result]),
    });
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "wait through limit", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    const itemId = itemForMessage(h.db, message.id).id;
    session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: "2099-09-25T10:00:00.000Z" });
    session.emit({ type: "turn_done", ok: false, error: "Usage limit reached" });
    expect(getWorkItem(h.db, itemId).status).toBe("queued");
    expect(getMessage(h.db, message.id).status).toBe("queued");
    expect(requeued.map((item) => item.id)).toEqual([itemId]);
    expect(ended).toEqual([]);
    expect(h.runner.currentWork(bot.id)).toBeNull();
    h.db.prepare("UPDATE bots SET limited_until = NULL WHERE id = ?").run(bot.id);
    h.runner.pump(bot.id);
    await waitFor(() => session.sent.length === 2);
    expect(started.map((item) => item.id)).toEqual([itemId, itemId]);
    session.emit({ type: "turn_done", ok: true, error: null });
    expect(requeued).toHaveLength(1);
    expect(ended).toHaveLength(1);
    expect(ended[0][0].id).toBe(itemId);
    expect(ended[0][1].outcome).toBe("done");
  });

  it.each(["success", "stop"] as const)("ends a limited turn normally after %s without a requeue callback", async (end) => {
    const requeued: WorkItem[] = [];
    const ended: [WorkItem, TurnResult][] = [];
    const h = harness(undefined, true, { turnRequeued: (item) => requeued.push(item), turnEnded: (item, result) => ended.push([item, result]) });
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "finish once", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: "2099-09-25T10:00:00.000Z" });
    if (end === "stop") await h.runner.stop(bot.id);
    session.emit({ type: "turn_done", ok: end === "success", error: null });
    const expectedOutcome = end === "success" ? "done" : "interrupted";
    expect(itemForMessage(h.db, message.id).status).toBe(expectedOutcome);
    expect(ended.map(([item, result]) => [item.id, result.outcome])).toEqual([[itemForMessage(h.db, message.id).id, expectedOutcome]]);
    expect(requeued).toEqual([]);
    h.db.prepare("UPDATE bots SET limited_until = NULL WHERE id = ?").run(bot.id);
    h.runner.pump(bot.id);
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(session.sent).toHaveLength(1);
  });

  it("honors stopItem called synchronously by turnStarted before creating the session", async () => {
    const ended: TurnResult[] = [];
    let runner: Runner | undefined;
    const h = harness(undefined, true, {
      turnStarted: (item) => runner?.stopItem(item.id, "Already out of time."),
      turnEnded: (_item, result) => ended.push(result),
    });
    runner = h.runner;
    const bot = newBot(h);
    const item = h.runner.deliver({ botId: bot.id, source: "routine", text: "do not send", conversationId: "team" });
    await waitFor(() => ended.length);
    expect(getWorkItem(h.db, item.id).status).toBe("interrupted");
    expect(h.sessions.flatMap((session) => session.sent)).toEqual([]);
    expect(history(h.db, "team", undefined, 1)[0].body).toBe("Already out of time.");
  });

  it.each(["turnStarted", "turnEnded", "turnRequeued", "sessionClosed"] as const)("logs a throwing %s hook without leaving the bot stuck", async (boundary) => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const failure = new Error(`${boundary} failed`);
    const hooks: RunnerHooks = { [boundary]: () => { throw failure; } };
    const h = harness(undefined, true, hooks);
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "first", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    if (boundary === "turnRequeued") session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: "2099-09-25T10:00:00.000Z" });
    if (boundary === "sessionClosed") session.emit({ type: "exit", error: "session died" });
    else session.emit({ type: "turn_done", ok: boundary !== "turnRequeued", error: null });
    expect(logged.mock.calls.some((call) => call.includes(failure))).toBe(true);
    expect(h.runner.currentWork(bot.id)).toBeNull();
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe(boundary === "turnRequeued" ? "limited" : "idle");
    expect(itemForMessage(h.db, message.id).status).toBe(boundary === "turnRequeued" ? "queued" : boundary === "sessionClosed" ? "failed" : "done");
    h.db.prepare("UPDATE bots SET limited_until = NULL WHERE id = ?").run(bot.id);
    if (boundary === "turnRequeued") h.runner.pump(bot.id);
    else h.runner.submit(bot.id, "next", []);
    await waitFor(() => h.sessions.reduce((count, active) => count + active.sent.length, 0) === 2);
    expect(h.runner.currentWork(bot.id)).not.toBeNull();
  });
});

describe("Runner session hooks", () => {
  it("settles Stop during a rejected resumed start without a fresh attempt and revokes its token", async () => {
    let rejectStart: (error: Error) => void = () => { throw new Error("Start was not held"); };
    const session = new FakeSession();
    session.startPromise = new Promise<void>((_resolve, reject) => { rejectStart = reject; });
    const options: StartOptions[] = [];
    const tokens = new McpTokens();
    let token = "";
    const closed: string[] = [];
    const ended: TurnResult[] = [];
    const h = harness((startOptions) => { options.push(startOptions); return session; }, true, {
      mcpServers: (bot) => { token = tokens.issue(bot.id); return []; },
      sessionClosed: (botId) => { closed.push(botId); tokens.revoke(botId); },
      turnEnded: (_item, result) => ended.push(result),
    });
    const bot = newBot(h);
    h.db.prepare("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?, 'claude', 'claude-main', 'saved', 'then')").run(bot.id);
    const message = h.runner.submit(bot.id, "never send", []);
    await h.runner.stop(bot.id);
    rejectStart(new Error("start rejected after Stop"));
    await waitFor(() => ended.length);
    expect(options.map((option) => option.resumeId)).toEqual(["saved"]);
    expect(h.sessions).toHaveLength(1);
    expect(session.sent).toEqual([]);
    expect(session.closed).toBe(true);
    expect(itemForMessage(h.db, message.id).status).toBe("interrupted");
    expect(ended[0]).toEqual({ outcome: "interrupted", text: null, textMessageId: null, error: null });
    expect(tokens.botFor(token)).toBeNull();
    expect(closed).toEqual([bot.id]);
    expect(h.db.prepare("SELECT cli_session_id FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ cli_session_id: "saved" });
    expect(history(h.db, bot.id, undefined, 20).some((item) => item.kind === "error")).toBe(false);
  });

  it.each([
    { boundary: "exit", throws: false },
    { boundary: "start", throws: false },
    { boundary: "exit", throws: true },
    { boundary: "start", throws: true },
  ] as const)("brackets the same resumed item's $boundary fallback with requeue and start hooks (requeue hook throws: $throws)", async ({ boundary, throws }) => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const hookFailure = new Error("requeue hook failed");
    const lifecycle: ["started" | "requeued" | "ended", string][] = [];
    const ended: TurnResult[] = [];
    let rejectStart: (error: Error) => void = () => { throw new Error("Start was not held"); };
    const resumed = new FakeSession();
    if (boundary === "start") resumed.startPromise = new Promise<void>((_resolve, reject) => { rejectStart = reject; });
    const fresh = new FakeSession();
    const available = [resumed, fresh];
    let nextSession = 0;
    const h = harness(() => {
      const session = available[nextSession++];
      if (!session) throw new Error("Unexpected extra session");
      return session;
    }, true, {
      turnStarted: (item) => lifecycle.push(["started", item.id]),
      turnRequeued: (item) => {
        lifecycle.push(["requeued", item.id]);
        if (throws) throw hookFailure;
      },
      turnEnded: (item, result) => { lifecycle.push(["ended", item.id]); ended.push(result); },
    });
    const bot = newBot(h);
    h.db.prepare("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?, 'claude', 'claude-main', 'missing', 'then')").run(bot.id);
    const message = h.runner.submit(bot.id, "continue the same item", []);
    const itemId = itemForMessage(h.db, message.id).id;
    expect(lifecycle).toEqual([["started", itemId]]);
    if (boundary === "exit") {
      await waitFor(() => resumed.sent.length);
      resumed.emit({ type: "exit", error: null });
    } else rejectStart(new Error("resume start rejected"));
    await waitFor(() => fresh.sent.length);

    expect(lifecycle).toEqual([["started", itemId], ["requeued", itemId], ["started", itemId]]);
    expect(ended).toEqual([]);
    expect(h.runner.currentWork(bot.id)?.id).toBe(itemId);
    expect(getWorkItem(h.db, itemId).status).toBe("running");
    if (throws) expect(logged.mock.calls.some((call) => call.includes(hookFailure))).toBe(true);
    else expect(logged).not.toHaveBeenCalled();
    fresh.emit({ type: "text", delta: "final answer" });
    fresh.emit({ type: "turn_done", ok: true, error: null });
    expect(lifecycle).toEqual([["started", itemId], ["requeued", itemId], ["started", itemId], ["ended", itemId]]);
    expect(ended).toEqual([{ outcome: "done", text: "final answer", textMessageId: history(h.db, bot.id, undefined, 1)[0].id, error: null }]);
    expect(getWorkItem(h.db, itemId).status).toBe("done");
    expect(h.sessions).toHaveLength(2);
  });

  it.each([false, true])("retries a rejected resumed start only once with a renewed token (fresh startup fails: %s)", async (freshFails) => {
    const tokens = new McpTokens();
    const issued: string[] = [];
    const options: StartOptions[] = [];
    const closed: string[] = [];
    const ended: TurnResult[] = [];
    const h = harness((startOptions) => {
      options.push(startOptions);
      const session = new FakeSession();
      if (options.length === 1 || freshFails) session.startPromise = Promise.reject(new Error(options.length === 1 ? "resume rejected" : "fresh rejected"));
      return session;
    }, true, {
      mcpServers: (bot) => { issued.push(tokens.issue(bot.id)); return []; },
      sessionClosed: (botId) => { closed.push(botId); tokens.revoke(botId); },
      turnEnded: (_item, result) => ended.push(result),
    });
    const bot = newBot(h);
    addMessage(h.db, { conversationId: bot.id, author: "you", kind: "text", body: "Remember ZEBRA", status: "done" });
    h.db.prepare("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?, 'claude', 'claude-main', 'missing', 'then')").run(bot.id);
    const message = h.runner.submit(bot.id, "continue", []);
    if (freshFails) await waitFor(() => ended.length);
    else await waitFor(() => h.sessions[1]?.sent.length);
    expect(options.map((option) => option.resumeId)).toEqual(["missing", null]);
    expect(h.sessions).toHaveLength(2);
    expect(h.sessions[0].closed).toBe(true);
    expect(tokens.botFor(issued[0])).toBeNull();
    expect(h.db.prepare("SELECT cli_session_id FROM sessions WHERE bot_id = ?").get(bot.id)).toBeUndefined();
    if (freshFails) {
      expect(closed).toEqual([bot.id, bot.id]);
      expect(tokens.botFor(issued[1])).toBeNull();
      expect(itemForMessage(h.db, message.id).status).toBe("failed");
      expect(ended[0].error).toBe("fresh rejected");
      expect(h.runner.currentWork(bot.id)).toBeNull();
    } else {
      expect(closed).toEqual([bot.id]);
      expect(tokens.botFor(issued[1])).toBe(bot.id);
      expect(h.sessions[1].sent[0]).toContain("Person: Remember ZEBRA");
      expect(h.sessions[1].sent[0]).toContain("continue");
      expect(itemForMessage(h.db, message.id).status).toBe("running");
      expect(ended).toEqual([]);
      h.sessions[1].emit({ type: "exit", error: null });
      expect(itemForMessage(h.db, message.id).status).toBe("failed");
      expect(h.sessions).toHaveLength(2);
      expect(tokens.botFor(issued[1])).toBeNull();
    }
  });

  it("constructs MCP servers once per session and revokes the old token when settings close it", async () => {
    const options: StartOptions[] = [];
    const tokens = new McpTokens();
    const issued: string[] = [];
    const closed: string[] = [];
    const h = harness((startOptions) => { options.push(startOptions); return new FakeSession(); }, true, {
      mcpServers: (bot) => {
        const token = tokens.issue(bot.id);
        issued.push(token);
        return [{ name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: { Authorization: `Bearer ${token}` }, autoAllow: true }];
      },
      sessionClosed: (botId) => { closed.push(botId); tokens.revoke(botId); },
    });
    const bot = newBot(h);
    h.runner.submit(bot.id, "first", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    expect(options[0].mcpServers).toEqual([{ name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: { Authorization: `Bearer ${issued[0]}` }, autoAllow: true }]);
    expect(tokens.botFor(issued[0])).toBe(bot.id);
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    h.runner.submit(bot.id, "reuse", []);
    await waitFor(() => h.sessions[0].sent.length === 2);
    expect(issued).toHaveLength(1);
    h.runner.botChanged(bot.id, false);
    expect(closed).toEqual([]);
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    expect(closed).toEqual([bot.id]);
    expect(tokens.botFor(issued[0])).toBeNull();
    h.runner.submit(bot.id, "new settings", []);
    await waitFor(() => h.sessions[1]?.sent.length);
    expect(issued).toHaveLength(2);
    expect(tokens.botFor(issued[1])).toBe(bot.id);
    h.runner.forget(bot.id);
    expect(tokens.botFor(issued[1])).toBeNull();
    expect(closed).toEqual([bot.id, bot.id]);
    h.sessions[1].emit({ type: "exit", error: null });
    expect(closed).toHaveLength(2);
  });

  it.each(["factory", "start", "mcpServers"] as const)("revokes an issued MCP token when %s fails before startup", async (boundary) => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const tokens = new McpTokens();
    let token = "";
    const closed: string[] = [];
    const ended: TurnResult[] = [];
    const failure = new Error(`${boundary} startup failed`);
    const h = harness(() => {
      if (boundary === "factory") throw failure;
      const session = new FakeSession();
      session.startPromise = Promise.reject(failure);
      return session;
    }, true, {
      mcpServers: (bot) => {
        token = tokens.issue(bot.id);
        if (boundary === "mcpServers") throw failure;
        return [];
      },
      sessionClosed: (botId) => { closed.push(botId); tokens.revoke(botId); },
      turnEnded: (_item, result) => ended.push(result),
    });
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "cannot start", []);
    await waitFor(() => ended.length);
    expect(token.length).toBeGreaterThan(0);
    expect(tokens.botFor(token)).toBeNull();
    expect(closed).toEqual([bot.id]);
    expect(h.sessions.flatMap((session) => session.sent)).toEqual([]);
    expect(h.sessions.every((session) => session.closed)).toBe(true);
    expect(itemForMessage(h.db, message.id).status).toBe("failed");
    expect(ended[0]).toEqual({ outcome: "failed", text: null, textMessageId: null, error: failure.message });
    expect(h.runner.toBot(getStoredBot(h.db, bot.id)).state).toBe("idle");
    if (boundary === "mcpServers") expect(logged.mock.calls.some((call) => call.includes(failure))).toBe(true);
  });

  it("revokes once when a live session exits and ignores later events from that session", async () => {
    const closed: string[] = [];
    const h = harness(undefined, true, { sessionClosed: (botId) => closed.push(botId) });
    const bot = newBot(h);
    h.runner.submit(bot.id, "first", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    const oldSession = h.sessions[0];
    oldSession.emit({ type: "exit", error: null });
    expect(closed).toEqual([bot.id]);
    h.runner.submit(bot.id, "next", []);
    await waitFor(() => h.sessions[1]?.sent.length);
    oldSession.emit({ type: "exit", error: "late duplicate" });
    oldSession.emit({ type: "text", delta: "stale" });
    expect(closed).toEqual([bot.id]);
    expect(h.runner.currentWork(bot.id)?.text).toBe("next");
    expect(history(h.db, bot.id, undefined, 20).some((message) => message.body === "stale")).toBe(false);
  });

  it("adds exactly the last 20 chat messages only for a fresh session", async () => {
    const options: StartOptions[] = [];
    const h = harness((startOptions) => { options.push(startOptions); return new FakeSession(); });
    const bot = newBot(h);
    for (let index = 0; index < 25; index++) addMessage(h.db, { conversationId: bot.id, author: "you", kind: "text", body: `history-${index}`, status: "sent" });
    h.runner.submit(bot.id, "fresh request", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    const session = h.sessions[0];
    expect(session.sent[0].split("\n").filter((line) => line.startsWith("Person: history-"))).toEqual(Array.from({ length: 20 }, (_, index) => `Person: history-${index + 5}`));
    expect(session.sent[0]).toContain("fresh request");
    session.emit({ type: "session", cliSessionId: "remembered" });
    session.emit({ type: "turn_done", ok: true, error: null });
    h.runner.submit(bot.id, "same session", []);
    await waitFor(() => session.sent.length === 2);
    expect(session.sent[1]).toBe("same session");
    session.emit({ type: "turn_done", ok: true, error: null });
    h.runner.botChanged(bot.id, false);
    h.runner.submit(bot.id, "resumed request", []);
    await waitFor(() => h.sessions[1]?.sent.length);
    expect(options[1].resumeId).toBe("remembered");
    expect(h.sessions[1].sent).toEqual(["resumed request"]);
  });
});

describe("Runner unexpected error boundaries", () => {
  it("logs a rejected stopItem interrupt and can settle the turn afterwards", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const failure = new Error("interrupt transport failed");
    const h = harness();
    const bot = newBot(h);
    const item = h.runner.deliver({ botId: bot.id, source: "routine", text: "run" });
    await waitFor(() => h.sessions[0]?.sent.length);
    h.sessions[0].interruptError = failure;
    h.runner.stopItem(item.id, "Routine timed out.");
    await waitFor(() => logged.mock.calls.some((call) => call.includes(failure)));
    h.sessions[0].emit({ type: "exit", error: null });
    expect(getWorkItem(h.db, item.id).status).toBe("interrupted");
    expect(history(h.db, bot.id, undefined, 1)[0].body).toBe("Routine timed out.");
    expect(h.runner.currentWork(bot.id)).toBeNull();
  });

  it.each([new Error("bot broadcast failed"), new RpcError("not_found", "broadcast target missing")])("logs an unexpected bot broadcast error and still starts and settles the work: %s", async (failure) => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const h = harness(undefined, true, {}, (event) => { if (event === "bot.updated") throw failure; });
    const bot = newBot(h);
    const message = h.runner.submit(bot.id, "still run", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    expect(logged.mock.calls.some((call) => call.includes(failure))).toBe(true);
    expect(itemForMessage(h.db, message.id).status).toBe("done");
    expect(h.runner.currentWork(bot.id)).toBeNull();
  });

  it("does not log a bot deleted by the turn-ended hook as an unexpected error", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    let db: Db | undefined;
    const h = harness(undefined, true, { turnEnded: (item) => db?.prepare("DELETE FROM bots WHERE id = ?").run(item.botId) });
    db = h.db;
    const bot = newBot(h);
    h.runner.submit(bot.id, "last turn", []);
    await waitFor(() => h.sessions[0]?.sent.length);
    h.sessions[0].emit({ type: "turn_done", ok: true, error: null });
    expect(h.db.prepare("SELECT id FROM bots WHERE id = ?").get(bot.id)).toBeUndefined();
    expect(logged).not.toHaveBeenCalled();
    expect(h.runner.currentWork(bot.id)).toBeNull();
  });
});
