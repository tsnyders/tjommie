import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { basename, join } from "node:path";
import type { Approval, ApprovalDecision, Bot, BotState, Message } from "@tjommie/protocol";
import { allowRules, createApproval, expirePendingApprovals, resolveApproval } from "./approvals.ts";
import { botPrompt, getStoredBot, listStoredBots, setLimitedUntil, type StoredBot } from "./bots.ts";
import { addMessage, buildRecap, getMessage, history, updateMessage } from "./chat.ts";
import { resolveCli } from "./cli.ts";
import { newId, nowIso, type Db } from "./db.ts";
import { ClaudeSession } from "./providers/claude.ts";
import { CodexSession } from "./providers/codex.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, lastItemForMessage, nextQueued, recoverAfterRestart, setStatus, type WorkItem } from "./queue.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { accountEnv, accountFor } from "./setup.ts";

export type SessionFactory = (bot: StoredBot, options: StartOptions) => ProviderSession;

export const cliSessionFactory: SessionFactory = (bot, options) => {
  const cmd = resolveCli(bot.provider);
  if (!cmd) throw new RpcError("not_installed", `${bot.provider === "claude" ? "Claude Code" : "Codex"} is not installed`);
  return bot.provider === "claude" ? new ClaudeSession(cmd, options) : new CodexSession(cmd, options);
};

const IDLE_SHUTDOWN_MS = 10 * 60_000;
const RECAP_MESSAGES = 20;
const UNKNOWN_RESET_MS = 60 * 60_000;
const MAX_TIMER_MS = 2 ** 31 - 1;
const TURN_EVENTS = new Set<ProviderEvent["type"]>(["text", "text_end", "tool", "tool_done", "approval"]);

type ActivityItem = { id: string; name: string; summary: string; status: "running" | "done" | "failed" };

// What the runner knows about a bot that has (or recently had) a CLI session.
type Live = {
  session: ProviderSession | null;
  fresh: boolean; // started without resuming
  busy: boolean; // a turn is running
  sent: boolean; // the current turn's first message has reached the CLI, so more can be steered in
  steering: boolean;
  item: WorkItem | null; // null while the CLI runs a turn it started by itself
  text: { id: string; body: string } | null;
  activity: { id: string; items: ActivityItem[] } | null;
  stopRequested: boolean;
  limitHit: boolean;
  lastResetsAt: string | null;
  reload: boolean;
  approvals: Map<string, string>; // Tjommie approval id -> the provider's own request id
  idleTimer: NodeJS.Timeout | null;
  limitTimer: NodeJS.Timeout | null;
};

export function botStateOf(
  state: { signedIn: boolean; paused: boolean; limitedUntil: string | null; waiting: boolean; busy: boolean },
  now: string,
): BotState {
  if (!state.signedIn) return "needs_setup";
  if (state.paused) return "paused";
  if (state.limitedUntil !== null && state.limitedUntil > now) return "limited";
  if (state.waiting) return "waiting";
  if (state.busy) return "working";
  return "idle";
}

export function activityBody(items: { summary: string }[]): string {
  return `${items.length} ${items.length === 1 ? "step" : "steps"}: ${items.at(-1)?.summary ?? ""}`.slice(0, 160);
}

// Copies a file the person attached into the bot's inbox, never overwriting an earlier one.
export function copyIntoInbox(homeFolder: string, file: string): string {
  const inbox = join(homeFolder, "inbox");
  mkdirSync(inbox, { recursive: true });
  const original = basename(file);
  let name = original;
  if (existsSync(join(inbox, name))) {
    const stamp = Date.now();
    let collision = 0;
    do {
      name = `${stamp}${collision === 0 ? "" : `-${collision}`}-${original}`;
      collision++;
    } while (existsSync(join(inbox, name)));
  }
  copyFileSync(file, join(inbox, name));
  return name;
}

export class Runner {
  #db: Db;
  #broadcast: Broadcast;
  #isSignedIn: (provider: Bot["provider"]) => boolean;
  #createSession: SessionFactory;
  #live = new Map<string, Live>();

  constructor(deps: { db: Db; broadcast: Broadcast; isSignedIn: (provider: Bot["provider"]) => boolean; createSession?: SessionFactory }) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#isSignedIn = deps.isSignedIn;
    this.#createSession = deps.createSession ?? cliSessionFactory;
  }

  // Call once at startup: settle work a restart cut off, then carry on with whatever is queued.
  start(): void {
    for (const item of recoverAfterRestart(this.#db)) {
      if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "interrupted" }));
      this.#notice(item.botId, "error", "Tjommie restarted while this was running.", item.messageId ? { retryMessageId: item.messageId } : null);
    }
    for (const approval of expirePendingApprovals(this.#db)) this.#showApprovalStatus(approval);
    for (const bot of listStoredBots(this.#db)) {
      if (bot.limitedUntil) this.#armLimitTimer(bot.id, bot.limitedUntil);
      this.pump(bot.id);
    }
  }

  toBot(bot: StoredBot): Bot {
    const live = this.#live.get(bot.id);
    const state = botStateOf(
      {
        signedIn: this.#isSignedIn(bot.provider),
        paused: bot.paused,
        limitedUntil: bot.limitedUntil,
        waiting: (live?.approvals.size ?? 0) > 0,
        busy: live?.busy ?? false,
      },
      nowIso(),
    );
    return { ...bot, state };
  }

  processId(botId: string): number | undefined {
    return this.#live.get(botId)?.session?.pid();
  }

  submit(botId: string, text: string, files: string[]): Message {
    const bot = getStoredBot(this.#db, botId);
    const attached = files.map((file) => copyIntoInbox(bot.homeFolder, file));
    const message = addMessage(this.#db, {
      conversationId: botId,
      author: "you",
      kind: "text",
      body: text,
      data: attached.length ? { files: attached } : null,
      status: "queued",
    });
    this.#emitMessage(message, "message.added");
    const prompt = attached.length ? `${text}\n\n[Attached files, now in your inbox folder: ${attached.join(", ")}]` : text;
    enqueue(this.#db, { botId, source: "you", text: prompt, messageId: message.id });
    const live = this.#liveFor(botId);
    if (!live.busy) this.pump(botId);
    else if (live.sent) void this.#steerQueued(botId); // otherwise the turn steers it in as soon as its first message is sent
    return getMessage(this.#db, message.id);
  }

  retry(messageId: string): Message {
    const previous = lastItemForMessage(this.#db, messageId);
    if (!previous) throw new RpcError("not_found", "There's nothing to retry for this message");
    if (previous.status !== "failed" && previous.status !== "interrupted") {
      throw new RpcError("bad_state", "Only failed or interrupted work can be retried");
    }
    enqueue(this.#db, { botId: previous.botId, source: previous.source, text: previous.text, messageId });
    this.#emitMessage(updateMessage(this.#db, messageId, { status: "queued" }));
    this.pump(previous.botId);
    return getMessage(this.#db, messageId);
  }

  async stop(botId: string): Promise<void> {
    const live = this.#live.get(botId);
    if (!live?.busy || !live.session) return;
    live.stopRequested = true;
    await live.session.interrupt();
  }

  answerApproval(approvalId: string, decision: ApprovalDecision): Approval {
    const approval = resolveApproval(this.#db, approvalId, decision);
    const live = this.#live.get(approval.botId);
    const providerId = live?.approvals.get(approvalId);
    if (live && providerId !== undefined) {
      live.approvals.delete(approvalId);
      live.session?.answer(providerId, decision);
    }
    this.#showApprovalStatus(approval);
    this.#emitBot(approval.botId);
    return approval;
  }

  // Settings changed. The next turn restarts the CLI with the new settings. The conversation carries on
  // unless the provider changed.
  botChanged(botId: string, providerChanged: boolean): void {
    if (providerChanged) this.#db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(botId);
    const live = this.#live.get(botId);
    if (!live?.session) return;
    if (live.busy) live.reload = true;
    else this.#closeSession(live);
  }

  // Call before deleting a bot.
  forget(botId: string): void {
    const live = this.#live.get(botId);
    if (!live) return;
    if (live.idleTimer) clearTimeout(live.idleTimer);
    if (live.limitTimer) clearTimeout(live.limitTimer);
    this.#closeSession(live);
    this.#live.delete(botId);
  }

  pump(botId: string): void {
    const live = this.#liveFor(botId);
    if (live.busy) return;
    let bot: StoredBot;
    try {
      bot = getStoredBot(this.#db, botId);
    } catch {
      return; // deleted
    }
    if (this.toBot(bot).state !== "idle") {
      this.#emitBot(botId);
      return;
    }
    const item = nextQueued(this.#db, botId);
    if (!item) {
      this.#armIdle(live);
      return;
    }
    live.busy = true; // set before any await, so a second pump can't start another turn
    live.sent = false;
    live.item = item;
    live.stopRequested = false;
    live.limitHit = false;
    if (live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    setStatus(this.#db, item.id, "running");
    if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "sent" }));
    this.#emitBot(botId);
    this.#startTurn(bot, item).catch((error: unknown) => this.#finishTurn(botId, false, error instanceof Error ? error.message : String(error)));
  }

  pumpAll(): void {
    for (const bot of listStoredBots(this.#db)) this.pump(bot.id);
  }

  shutdown(): void {
    for (const botId of [...this.#live.keys()]) this.forget(botId);
  }

  #liveFor(botId: string): Live {
    let live = this.#live.get(botId);
    if (!live) {
      live = {
        session: null,
        fresh: false,
        busy: false,
        sent: false,
        steering: false,
        item: null,
        text: null,
        activity: null,
        stopRequested: false,
        limitHit: false,
        lastResetsAt: null,
        reload: false,
        approvals: new Map(),
        idleTimer: null,
        limitTimer: null,
      };
      this.#live.set(botId, live);
    }
    return live;
  }

  async #startTurn(bot: StoredBot, item: WorkItem): Promise<void> {
    const live = this.#liveFor(bot.id);
    let text = item.text;
    let session = live.session;
    if (!session) {
      const account = accountFor(this.#db, bot.provider);
      const saved = this.#db.prepare("SELECT provider, account_id, cli_session_id FROM sessions WHERE bot_id = ?").get(bot.id) as
        | { provider: string; account_id: string; cli_session_id: string }
        | undefined;
      const resumeId = saved && saved.provider === bot.provider && saved.account_id === account.id ? saved.cli_session_id : null;
      const created = this.#createSession(bot, {
        cwd: bot.projectFolder ?? bot.homeFolder,
        extraDirs: bot.projectFolder ? [bot.homeFolder] : [],
        model: bot.model,
        instructions: botPrompt(bot),
        autonomy: bot.autonomy,
        allowRules: allowRules(this.#db, bot.id),
        resumeId,
        env: accountEnv(account),
      });
      created.onEvent((event) => this.#onEvent(bot.id, created, event));
      live.session = created;
      live.fresh = resumeId === null;
      try {
        await created.start();
      } catch (error) {
        live.session = null;
        created.close();
        if (!live.busy || live.item?.id !== item.id) return; // an exit already settled this turn
        if (resumeId === null) throw error;
        this.#db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(bot.id); // couldn't resume: start fresh with a recap
        return this.#startTurn(bot, item);
      }
      if (!live.busy || live.item?.id !== item.id || live.session !== created) return;
      if (live.fresh) {
        const recap = buildRecap(history(this.#db, bot.id, item.messageId ?? undefined, RECAP_MESSAGES), bot.name);
        if (recap) text = `${recap}\n\n${text}`;
      }
      session = created;
    }
    await session.send(text);
    if (!live.busy || live.item?.id !== item.id || live.session !== session) return;
    live.sent = true;
    await this.#steerQueued(bot.id); // anything sent while the turn was starting goes in now, in order
  }

  // Steers queued messages into the running turn, oldest first. One loop at a time keeps the order.
  async #steerQueued(botId: string): Promise<void> {
    const live = this.#liveFor(botId);
    if (live.steering) return;
    live.steering = true;
    try {
      for (let item = nextQueued(this.#db, botId); item && live.busy && live.sent; item = nextQueued(this.#db, botId)) {
        const session = live.session;
        if (!session) break;
        setStatus(this.#db, item.id, "done"); // claim it first, so a turn that ends meanwhile can't pick it up too
        if (!(await session.steer(item.text).catch(() => false))) {
          setStatus(this.#db, item.id, "queued"); // it runs as the next turn instead
          break;
        }
        if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "sent" }));
      }
    } finally {
      live.steering = false;
    }
    if (!live.busy) this.pump(botId);
  }

  #onEvent(botId: string, session: ProviderSession, event: ProviderEvent): void {
    const live = this.#live.get(botId);
    if (!live || live.session !== session) return; // a session we closed or replaced
    if (!live.busy && TURN_EVENTS.has(event.type)) {
      // The CLI started a turn by itself, for example after a background task finished.
      live.busy = true;
      live.sent = true;
      live.item = null;
      this.#emitBot(botId);
    }
    switch (event.type) {
      case "session":
        this.#saveSession(botId, event.cliSessionId);
        return;
      case "text":
        return this.#appendText(botId, live, event.delta);
      case "text_end":
        return this.#flushText(live, "done");
      case "tool":
        return this.#addTool(botId, live, event);
      case "tool_done":
        return this.#toolDone(live, event.id, event.ok);
      case "approval":
        return this.#onApproval(botId, live, event);
      case "usage":
        return this.#recordUsage(botId, event);
      case "limit":
        return this.#onLimit(botId, live, event);
      case "turn_done":
        return this.#finishTurn(botId, event.ok, event.error);
      case "exit":
        return this.#onExit(botId, live, event.error);
    }
  }

  #saveSession(botId: string, cliSessionId: string): void {
    const bot = getStoredBot(this.#db, botId);
    this.#db
      .prepare(
        `INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(bot_id) DO UPDATE SET provider = excluded.provider, account_id = excluded.account_id,
           cli_session_id = excluded.cli_session_id, updated_at = excluded.updated_at`,
      )
      .run(botId, bot.provider, accountFor(this.#db, bot.provider).id, cliSessionId, nowIso());
  }

  #appendText(botId: string, live: Live, delta: string): void {
    if (!live.text) {
      if (live.activity) this.#closeActivity(live);
      const message = addMessage(this.#db, { conversationId: botId, author: botId, kind: "text", body: "", status: "streaming" });
      live.text = { id: message.id, body: "" };
      this.#emitMessage(message, "message.added");
    }
    live.text.body += delta;
    this.#broadcast("message.delta", { id: live.text.id, conversationId: botId, delta });
  }

  #flushText(live: Live, status: Message["status"]): void {
    if (!live.text) return;
    this.#emitMessage(updateMessage(this.#db, live.text.id, { body: live.text.body, status }));
    live.text = null;
  }

  #addTool(botId: string, live: Live, event: { id: string; name: string; summary: string }): void {
    this.#flushText(live, "done");
    const item: ActivityItem = { id: event.id, name: event.name, summary: event.summary, status: "running" };
    if (live.activity) {
      live.activity.items.push(item);
      this.#saveActivity(live, "streaming");
      return;
    }
    const message = addMessage(this.#db, {
      conversationId: botId,
      author: botId,
      kind: "activity",
      body: activityBody([item]),
      data: { items: [item] },
      status: "streaming",
    });
    live.activity = { id: message.id, items: [item] };
    this.#emitMessage(message, "message.added");
  }

  #toolDone(live: Live, id: string, ok: boolean): void {
    const item = live.activity?.items.find((activity) => activity.id === id);
    if (!item) return;
    item.status = ok ? "done" : "failed";
    this.#saveActivity(live, "streaming");
  }

  #saveActivity(live: Live, status: Message["status"]): void {
    if (!live.activity) return;
    this.#emitMessage(
      updateMessage(this.#db, live.activity.id, {
        body: activityBody(live.activity.items),
        data: { items: live.activity.items },
        status,
      }),
    );
  }

  #closeActivity(live: Live): void {
    this.#saveActivity(live, "done");
    live.activity = null;
  }

  #onApproval(botId: string, live: Live, event: { id: string; tool: string; summary: string; detail: string; rule: string | null }): void {
    if (event.rule && allowRules(this.#db, botId).includes(event.rule)) {
      live.session?.answer(event.id, "once"); // "Always allow" was chosen for this before
      return;
    }
    this.#flushText(live, "done");
    const approvalId = newId();
    const message = addMessage(this.#db, {
      conversationId: botId,
      author: botId,
      kind: "approval",
      body: event.summary,
      data: { approvalId, tool: event.tool, detail: event.detail, rule: event.rule, status: "pending" },
      status: "done",
    });
    const approval = createApproval(this.#db, {
      id: approvalId,
      botId,
      tool: event.tool,
      summary: event.summary,
      detail: event.detail,
      rule: event.rule,
      messageId: message.id,
    });
    live.approvals.set(approvalId, event.id);
    this.#emitMessage(message, "message.added");
    this.#broadcast("approval.added", approval);
    this.#broadcast("notify", { botId, title: `${getStoredBot(this.#db, botId).name} is waiting for you`, body: event.summary });
    this.#emitBot(botId);
  }

  #showApprovalStatus(approval: Approval): void {
    const message = getMessage(this.#db, approval.messageId);
    this.#emitMessage(updateMessage(this.#db, message.id, { data: { ...message.data, status: approval.status } }));
    this.#broadcast("approval.resolved", approval);
  }

  #recordUsage(botId: string, event: Extract<ProviderEvent, { type: "usage" }>): void {
    const bot = getStoredBot(this.#db, botId);
    this.#db
      .prepare(
        `INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        newId(),
        botId,
        accountFor(this.#db, bot.provider).id,
        bot.provider,
        event.model,
        event.input,
        event.cachedInput,
        event.output,
        event.reasoning,
        event.costUsd,
        event.costUsd === null ? null : "reported",
        nowIso(),
      );
  }

  #onLimit(botId: string, live: Live, event: Extract<ProviderEvent, { type: "limit" }>): void {
    if (event.resetsAt) live.lastResetsAt = event.resetsAt;
    if (event.state !== "reached" || live.limitHit) return; // "ok" and "near" feed the Usage page in Phase 2
    live.limitHit = true;
    const until = event.resetsAt ?? live.lastResetsAt ?? new Date(Date.now() + UNKNOWN_RESET_MS).toISOString();
    setLimitedUntil(this.#db, botId, until);
    const name = getStoredBot(this.#db, botId).name;
    const when = new Date(until);
    const at = `${when.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })} on ${when.toLocaleDateString([], { weekday: "long" })}`;
    this.#notice(botId, "notice", `${name} hit its usage limit. It will pick up again at ${at}.`, null);
    this.#broadcast("notify", { botId, title: `${name} hit its usage limit`, body: `It will pick up again at ${at}.` });
    this.#armLimitTimer(botId, until);
    this.#emitBot(botId);
  }

  #armLimitTimer(botId: string, until: string): void {
    const live = this.#liveFor(botId);
    if (live.limitTimer) clearTimeout(live.limitTimer);
    const delay = Math.min(Math.max(0, Date.parse(until) - Date.now()) + 60_000, MAX_TIMER_MS);
    live.limitTimer = setTimeout(() => {
      live.limitTimer = null;
      setLimitedUntil(this.#db, botId, null);
      this.#emitBot(botId);
      this.pump(botId);
    }, delay);
    live.limitTimer.unref();
  }

  #finishTurn(botId: string, ok: boolean, error: string | null): void {
    const live = this.#live.get(botId);
    if (!live?.busy) return;
    const outcome = ok ? "done" : live.stopRequested ? "interrupted" : "failed";
    this.#flushText(live, outcome);
    if (live.activity) this.#closeActivity(live);
    const item = live.item;
    if (item && live.limitHit) {
      setStatus(this.#db, item.id, "queued"); // runs again once the limit resets
      if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "queued" }));
    } else if (item) {
      setStatus(this.#db, item.id, outcome);
      if (outcome === "interrupted") this.#notice(botId, "notice", "Stopped.", null);
      if (outcome === "failed") this.#notice(botId, "error", error ?? "Something went wrong.", item.messageId ? { retryMessageId: item.messageId } : null);
    }
    for (const approvalId of live.approvals.keys()) {
      this.#showApprovalStatus(resolveApproval(this.#db, approvalId, "deny")); // the turn that asked is over
    }
    live.approvals.clear();
    live.busy = false;
    live.sent = false;
    live.item = null;
    live.stopRequested = false;
    live.limitHit = false;
    if (live.reload) {
      live.reload = false;
      this.#closeSession(live);
    }
    this.#emitBot(botId);
    this.pump(botId);
  }

  #onExit(botId: string, live: Live, error: string | null): void {
    live.session = null;
    if (live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    if (!live.busy) return;
    live.limitHit = false; // an exit is terminal even if a limit event arrived first
    this.#finishTurn(botId, false, error ?? "The CLI stopped unexpectedly.");
  }

  #closeSession(live: Live): void {
    const session = live.session;
    live.session = null; // its exit event is ignored from now on
    session?.close();
  }

  #armIdle(live: Live): void {
    if (!live.session || live.idleTimer) return;
    live.idleTimer = setTimeout(() => {
      live.idleTimer = null;
      if (!live.busy) this.#closeSession(live);
    }, IDLE_SHUTDOWN_MS);
    live.idleTimer.unref();
  }

  #notice(botId: string, kind: "notice" | "error", body: string, data: Record<string, unknown> | null): void {
    this.#emitMessage(
      addMessage(this.#db, { conversationId: botId, author: "system", kind, body: body.slice(0, 500), data, status: "done" }),
      "message.added",
    );
  }

  #emitMessage(message: Message, event: "message.added" | "message.updated" = "message.updated"): void {
    this.#broadcast(event, message);
  }

  #emitBot(botId: string): void {
    try {
      this.#broadcast("bot.updated", this.toBot(getStoredBot(this.#db, botId)));
    } catch {
      // the bot was deleted
    }
  }
}
