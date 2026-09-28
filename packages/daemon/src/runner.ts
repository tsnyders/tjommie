import { copyFileSync, existsSync, mkdirSync, statSync } from "node:fs";
import { basename, join } from "node:path";
import { ActivityData, ApprovalData, type ActivityItem, type Approval, type ApprovalDecision, type Bot, type BotState, type ErrorData, type Message, type UserMessageData } from "@tjommie/protocol";
import { allowRules, createApproval, expirePendingApprovals, resolveApproval } from "./approvals.ts";
import { botPrompt, getStoredBot, listStoredBots, setLimitedUntil, type StoredBot } from "./bots.ts";
import { closeBrowser } from "./browser.ts";
import { addMessage, buildRecap, getMessage, history, updateMessage } from "./chat.ts";
import { killTree, resolveCli } from "./cli.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { ClaudeSession } from "./providers/claude.ts";
import { CodexSession } from "./providers/codex.ts";
import type { McpServer, ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, getWorkItem, lastItemForMessage, newChain, nextQueued, nextSteerable, recoverAfterRestart, setStatus, type NewWork, type WorkItem } from "./queue.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { accountEnv, accountFor } from "./setup.ts";

export type SessionFactory = (bot: StoredBot, options: StartOptions) => ProviderSession;

export const cliSessionFactory: SessionFactory = (bot, options) => {
  const cmd = resolveCli(bot.provider);
  if (!cmd) throw new RpcError("not_installed", `${bot.provider === "claude" ? "Claude Code" : "Codex"} is not installed`);
  return bot.provider === "claude" ? new ClaudeSession(cmd, options) : new CodexSession(cmd, options);
};

// The last finished text is the answer passed back to a bot that requested this work.
export type TurnResult = { outcome: "done" | "failed" | "interrupted"; text: string | null; textMessageId: string | null; error: string | null };

export type RunnerHooks = {
  mcpServers?: (bot: StoredBot) => McpServer[];
  sessionClosed?: (botId: string) => void;
  turnStarted?: (item: WorkItem) => void;
  turnEnded?: (item: WorkItem, result: TurnResult) => void;
  // A limit or failed resume restarts the item; clear its timer until turnStarted re-arms it.
  turnRequeued?: (item: WorkItem) => void;
  retryRoutine?: (message: Message) => Message;
};

const IDLE_SHUTDOWN_MS = 10 * 60_000;
const RECAP_MESSAGES = 20;
const UNKNOWN_RESET_MS = 60 * 60_000;
const MAX_TIMER_MS = 2 ** 31 - 1;
const TURN_EVENTS = new Set<ProviderEvent["type"]>(["text", "tool", "approval"]);
const RESTARTED = "Tjommie restarted while this was running.";

function retryData(item: WorkItem): ErrorData | null {
  if (!item.messageId) return null;
  return { retryMessageId: item.messageId, ...(item.source === "room" ? { retryBotId: item.botId } : {}) };
}

// What the runner knows about a bot that has (or recently had) a CLI session.
type Live = {
  session: ProviderSession | null;
  fresh: boolean; // started without resuming
  sawSession: boolean; // the CLI reported its session id since it started
  busy: boolean; // a turn is running
  sent: boolean; // the current turn's first message has reached the CLI, so more can be steered in
  steering: boolean;
  steeringItem: WorkItem | null;
  item: WorkItem | null; // null while the CLI runs a turn it started by itself
  text: { id: string; body: string } | null;
  lastText: { id: string; body: string } | null;
  activity: { id: string; items: ActivityItem[] } | null;
  stopRequested: boolean;
  stopNotice: string | null;
  stopPending: Promise<boolean | void> | null;
  limitHit: boolean;
  lastResetsAt: string | null;
  reload: boolean;
  approvals: Map<string, string>; // Tjommie approval id -> the provider's own request id
  idleTimer: NodeJS.Timeout | null;
  limitTimer: NodeJS.Timeout | null;
};

type Deleting = {
  live: Live;
  items: WorkItem[];
  pid: number | undefined;
  processExited: boolean;
  closed: Promise<void>;
  settled: boolean;
  restoreWhenClosed: boolean;
};

export function botStateOf(
  state: { signedIn: boolean; setupReason?: string | null; paused: boolean; limitedUntil: string | null; waiting: boolean; busy: boolean },
  now: string,
): BotState {
  if (!state.signedIn || state.setupReason) return "needs_setup";
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
  #setupIssue: (bot: StoredBot) => string | null;
  #isHeld: (botId: string) => boolean;
  #codexSandboxReady: () => boolean;
  #createSession: SessionFactory;
  #hooks: RunnerHooks;
  #live = new Map<string, Live>();
  #deleting = new Map<string, Deleting>();
  #recovered = false;
  #ready = true;

  constructor(deps: { db: Db; broadcast: Broadcast; isSignedIn: (provider: Bot["provider"]) => boolean; setupIssue?: (bot: StoredBot) => string | null; isHeld?: (botId: string) => boolean; codexSandboxReady?: () => boolean; createSession?: SessionFactory; hooks?: RunnerHooks }) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#isSignedIn = deps.isSignedIn;
    this.#setupIssue = deps.setupIssue ?? (() => null);
    this.#isHeld = deps.isHeld ?? (() => false);
    this.#codexSandboxReady = deps.codexSandboxReady ?? (() => false);
    this.#createSession = deps.createSession ?? cliSessionFactory;
    this.#hooks = deps.hooks ?? {};
  }

  // Settle interrupted work before listening. Hooks may queue replies, but cannot start a CLI yet.
  recover(): void {
    if (this.#recovered) return;
    this.#ready = false;
    for (const item of recoverAfterRestart(this.#db)) {
      this.#setChatMessageStatus(item, "interrupted");
      this.#notice(item.conversationId, "error", RESTARTED, retryData(item));
      this.#safely(() => this.#hooks.turnEnded?.(item, { outcome: "failed", text: null, textMessageId: null, error: RESTARTED }));
    }
    const streaming = this.#db.prepare("SELECT id FROM messages WHERE status = 'streaming' ORDER BY created_at, rowid").all() as { id: string }[];
    for (const row of streaming) {
      const message = getMessage(this.#db, row.id);
      const activity = message.kind === "activity" ? ActivityData.safeParse(message.data) : null;
      const data = activity?.success
        ? { items: activity.data.items.map((item) => item.status === "running" ? { ...item, status: "failed" as const } : item) } satisfies ActivityData
        : undefined;
      this.#emitMessage(updateMessage(this.#db, row.id, { data, status: "interrupted" }));
    }
    for (const approval of expirePendingApprovals(this.#db)) this.#showApprovalStatus(approval);
    this.#recovered = true;
  }

  // Start pumping after the daemon knows the listening address used by its MCP server.
  start(): void {
    this.recover();
    this.#ready = true;
    for (const bot of listStoredBots(this.#db)) {
      if (bot.limitedUntil) this.#armLimitTimer(bot.id, bot.limitedUntil);
      this.pump(bot.id);
    }
  }

  toBot(bot: StoredBot): Bot {
    const live = this.#live.get(bot.id);
    const signedIn = this.#isSignedIn(bot.provider);
    const setupReason = signedIn ? this.#setupIssue(bot) : `Sign in to ${bot.provider === "codex" ? "Codex" : "Claude"}`;
    const state = botStateOf(
      {
        signedIn,
        setupReason,
        paused: bot.paused,
        limitedUntil: bot.limitedUntil,
        waiting: (live?.approvals.size ?? 0) > 0 || this.#db.prepare("SELECT 1 FROM approvals WHERE bot_id = ? AND kind = 'routine' AND status = 'pending' LIMIT 1").get(bot.id) !== undefined,
        busy: live?.busy ?? false,
      },
      nowIso(),
    );
    return { ...bot, state, setupReason };
  }

  processId(botId: string): number | undefined {
    return this.#live.get(botId)?.session?.pid();
  }

  // Null when idle or in a turn the CLI started by itself.
  currentWork(botId: string): WorkItem | null {
    const live = this.#live.get(botId);
    return live?.busy ? live.item : null;
  }

  submit(botId: string, text: string, files: string[]): Message {
    const bot = getStoredBot(this.#db, botId);
    for (const file of files) {
      let isFile = false;
      try { isFile = statSync(file).isFile(); } catch { /* Report the selected path as a parameter error. */ }
      if (!isFile) throw new RpcError("bad_params", `Attachment is missing or is not a file: ${file}`);
    }
    const attached = files.map((file) => copyIntoInbox(bot.homeFolder, file));
    const message = addMessage(this.#db, {
      conversationId: botId,
      author: "you",
      kind: "text",
      body: text,
      data: attached.length ? { files: attached } satisfies UserMessageData : null,
      status: "queued",
    });
    this.#emitMessage(message, "message.added");
    const prompt = attached.length ? `${text}\n\n[Attached files, now in your inbox folder: ${attached.join(", ")}]` : text;
    this.deliver({ botId, source: "you", text: prompt, messageId: message.id, chainId: newChain(this.#db) });
    return getMessage(this.#db, message.id);
  }

  // Start or steer new work, or wake work already queued atomically with its exchange.
  deliver(work: NewWork | WorkItem): WorkItem {
    const item = "id" in work ? getWorkItem(this.#db, work.id) : enqueue(this.#db, work);
    const deleting = this.#deleting.get(item.botId);
    if (deleting) {
      if (!deleting.items.some((captured) => captured.id === item.id)) deleting.items.push(item);
      return item;
    }
    const live = this.#liveFor(item.botId);
    if (!live.busy) this.pump(item.botId);
    else if (live.sent) {
      void this.#steerQueued(item.botId).catch((error: unknown) => console.error("Tjommie: queued steering failed", error));
    }
    return item;
  }

  retry(messageId: string, botId?: string): Message {
    let previous = lastItemForMessage(this.#db, messageId, botId);
    if (!previous) throw new RpcError("not_found", "There's nothing to retry for this message");
    if (previous.source === "room" && botId === undefined) {
      const bots = this.#db.prepare("SELECT DISTINCT bot_id FROM work_items WHERE message_id = ?").all(messageId) as { bot_id: string }[];
      const failed = bots.map((bot) => lastItemForMessage(this.#db, messageId, bot.bot_id)!).filter((item) => item.status === "failed" || item.status === "interrupted");
      if (failed.length > 1) throw new RpcError("bad_params", "Choose the bot whose room turn you want to retry");
      if (failed.length === 1) previous = failed[0];
    }
    if (previous.status !== "failed" && previous.status !== "interrupted") {
      throw new RpcError("bad_state", "Only failed or interrupted work can be retried");
    }
    if (previous.source === "routine") {
      if (!this.#hooks.retryRoutine) throw new RpcError("bad_state", "Routine retry is unavailable");
      return this.#hooks.retryRoutine(getMessage(this.#db, messageId));
    }
    this.#setChatMessageStatus(previous, "queued");
    this.deliver({ botId: previous.botId, source: previous.source, text: previous.text, messageId, conversationId: previous.conversationId, exchangeId: previous.exchangeId, chainId: newChain(this.#db) });
    return getMessage(this.#db, messageId);
  }

  async stop(botId: string): Promise<void> {
    const live = this.#live.get(botId);
    if (!live?.busy) return;
    const session = live.session;
    const item = live.item;
    live.stopRequested = true;
    try {
      const pending = session?.interrupt() ?? Promise.resolve();
      if (this.#live.get(botId) === live && live.session === session && live.item === item && live.busy) live.stopPending = pending;
      await pending;
    } catch (error) {
      if (this.#live.get(botId) === live && live.session === session && live.item === item && live.busy) {
        live.stopRequested = false;
        live.stopNotice = null;
      }
      throw error;
    }
  }

  // A stale routine timer must never interrupt a later item on the same bot.
  async stopItem(itemId: string, notice: string): Promise<void> {
    for (const [botId, live] of this.#live) {
      if (!live.busy || live.item?.id !== itemId) continue;
      live.stopNotice = notice;
      await this.stop(botId);
      return;
    }
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
    else this.#closeSession(botId, live);
  }

  // Release folder locks without changing persisted work; only explicit cancellation may resume it.
  closeForDeletion(botId: string): Promise<void> {
    const existing = this.#deleting.get(botId);
    if (existing) {
      if (existing.restoreWhenClosed) throw new RpcError("busy", "The bot is still closing. Try again shortly.");
      return existing.closed;
    }
    const live = this.#liveFor(botId);
    const rows = this.#db.prepare("SELECT id FROM work_items WHERE bot_id = ? AND status IN ('queued', 'running') ORDER BY created_at, rowid").all(botId) as { id: string }[];
    const items = rows.map((row) => getWorkItem(this.#db, row.id));
    if (live.steeringItem && !items.some((item) => item.id === live.steeringItem?.id)) items.push(live.steeringItem);
    const session = live.session;
    const detached: Deleting = { live, items, pid: session?.pid(), processExited: false, closed: Promise.resolve(), settled: false, restoreWhenClosed: false };
    this.#deleting.set(botId, detached);
    this.#live.delete(botId);
    this.#closeActivity(live, "failed");
    this.#releaseSession(botId, live);
    const exited = Promise.resolve(session?.waitForExit?.()).then(() => { detached.processExited = true; });
    detached.closed = Promise.all([exited, closeBrowser(botId)]).then(() => {
      detached.settled = true;
    });
    return detached.closed;
  }

  // Call only after deletion succeeds. Captured work survives the database cascade long enough for hooks.
  forget(botId: string): void {
    if (!this.#deleting.has(botId) && this.#live.has(botId)) {
      void this.closeForDeletion(botId).catch((error: unknown) => console.error("Tjommie: closing a deleted bot failed", error));
    }
    const detached = this.#deleting.get(botId);
    if (!detached) return;
    this.#deleting.delete(botId);
    for (const item of detached.items) {
      this.#safely(() => this.#hooks.turnEnded?.(item, { outcome: "interrupted", text: null, textMessageId: null, error: "The bot was deleted." }));
    }
  }

  cancelDeletion(botId: string): void {
    const detached = this.#deleting.get(botId);
    if (!detached) return;
    if (!detached.settled) {
      if (!detached.restoreWhenClosed) {
        detached.restoreWhenClosed = true;
        void detached.closed.then(() => {
          if (this.#deleting.get(botId) === detached) this.cancelDeletion(botId);
        }).catch((error: unknown) => console.error("Tjommie: restoring a bot after deletion failed", error));
      }
      return;
    }
    this.#deleting.delete(botId);
    this.#flushText(detached.live, "interrupted");
    this.#closeActivity(detached.live, "failed");
    // Use a new object so pending callbacks from the closed session cannot change resumed work.
    const live: Live = { ...detached.live, session: null, sent: false, steering: false, steeringItem: null,
      text: null, activity: null, lastText: null, stopPending: null, idleTimer: null, limitTimer: null };
    this.#live.set(botId, live);
    if (detached.live.steeringItem) setStatus(this.#db, detached.live.steeringItem.id, "queued");
    const bot = getStoredBot(this.#db, botId);
    // These cards refer to the closed provider session, so cannot be answered by its replacement.
    this.#expireApprovals(live);
    if (bot.limitedUntil) this.#armLimitTimer(botId, bot.limitedUntil);
    if (live.busy && live.stopRequested) {
      this.#finishTurn(botId, false, null);
      return;
    }
    if (live.busy && live.item) {
      // Keep the same work and its routine timer after a failed folder removal.
      this.#launchTurn(bot, live.item, live);
      return;
    }
    live.busy = false;
    this.pump(botId);
  }

  pump(botId: string): void {
    if (!this.#ready || this.#deleting.has(botId)) return;
    const live = this.#liveFor(botId);
    if (live.busy) return;
    let bot: StoredBot;
    try {
      bot = getStoredBot(this.#db, botId);
    } catch (error) {
      if (!(error instanceof RpcError && error.code === "not_found")) console.error("Tjommie: loading a bot failed", error);
      return;
    }
    // A routine proposal changes the displayed state, but does not block work.
    if (!this.#isSignedIn(bot.provider) || this.#setupIssue(bot) || bot.paused || (bot.limitedUntil !== null && bot.limitedUntil > nowIso()) || live.approvals.size > 0 || live.busy) {
      this.#armIdle(botId, live);
      this.#emitBot(botId);
      return;
    }
    const item = nextQueued(this.#db, botId);
    if (!item) {
      this.#armIdle(botId, live);
      return;
    }
    live.busy = true; // set before any await, so a second pump can't start another turn
    live.sent = false;
    live.item = item;
    live.stopRequested = false;
    live.stopNotice = null;
    live.stopPending = null;
    live.limitHit = false;
    live.lastText = null;
    if (live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    setStatus(this.#db, item.id, "running");
    this.#setChatMessageStatus(item, "sent");
    this.#emitBot(botId);
    this.#safely(() => this.#hooks.turnStarted?.(item));
    this.#launchTurn(bot, item, live);
  }

  pumpAll(): void {
    for (const bot of listStoredBots(this.#db)) this.pump(bot.id);
  }

  setupChanged(): void {
    for (const bot of listStoredBots(this.#db)) {
      if (this.#isHeld(bot.id)) continue;
      if (!this.#setupIssue(bot)) continue;
      const live = this.#live.get(bot.id);
      if (!live?.session) continue;
      if (live.busy) {
        live.reload = true;
        const item = live.item;
        if (item) void this.stopItem(item.id, "Codex's sandbox is no longer ready. Set it up again to continue.")
          .catch((error: unknown) => console.error("Could not stop a bot after setup changed.", error));
      } else this.#closeSession(bot.id, live);
    }
    this.pumpAll();
  }

  shutdown(): void {
    this.#ready = false;
    const live = [...this.#live];
    this.#live.clear();
    for (const deleting of this.#deleting.values()) {
      if (deleting.live.busy && !deleting.processExited) killTree(deleting.pid);
    }
    this.#deleting.clear();
    for (const [botId, state] of live) {
      if (state.busy) killTree(state.session?.pid());
      this.#releaseSession(botId, state);
    }
  }

  #liveFor(botId: string): Live {
    let live = this.#live.get(botId);
    if (!live) {
      live = {
        session: null,
        fresh: false,
        sawSession: false,
        busy: false,
        sent: false,
        steering: false,
        steeringItem: null,
        item: null,
        text: null,
        lastText: null,
        activity: null,
        stopRequested: false,
        stopNotice: null,
        stopPending: null,
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

  #dest(botId: string, live: Live): string {
    return live.item?.conversationId ?? botId;
  }

  #launchTurn(bot: StoredBot, item: WorkItem, live: Live): void {
    this.#startTurn(bot, item).catch((error: unknown) => {
      if (this.#live.get(bot.id) !== live || live.item?.id !== item.id) return;
      this.#finishTurn(bot.id, false, error instanceof Error ? error.message : String(error));
    });
  }

  async #startTurn(bot: StoredBot, item: WorkItem): Promise<void> {
    const live = this.#liveFor(bot.id);
    if (this.#setupIssue(bot)) throw new Error("The bot needs setup before it can run.");
    if (live.stopRequested) {
      this.#finishTurn(bot.id, false, null);
      return;
    }
    let text = item.text;
    let session = live.session;
    if (!session) {
      const account = accountFor(this.#db, bot.provider);
      const saved = this.#db.prepare("SELECT provider, account_id, cli_session_id, total_cost_usd FROM sessions WHERE bot_id = ?").get(bot.id) as
        | { provider: string; account_id: string; cli_session_id: string; total_cost_usd: number | null }
        | undefined;
      const resumeId = saved && saved.provider === bot.provider && saved.account_id === account.id ? saved.cli_session_id : null;
      let created: ProviderSession;
      try {
        created = this.#createSession(bot, {
          cwd: bot.projectFolder ?? bot.homeFolder,
          extraDirs: bot.projectFolder ? [bot.homeFolder] : [],
          model: bot.model,
          instructions: botPrompt(bot),
          autonomy: bot.autonomy,
          codexSandboxReady: this.#codexSandboxReady(),
          allowRules: allowRules(this.#db, bot.id),
          resumeId,
          resumeCostUsd: resumeId === null ? null : saved?.total_cost_usd ?? null,
          env: accountEnv(account),
          mcpServers: this.#mcpServers(bot),
        });
      } catch (error) {
        // The MCP hook can issue a token even when no ProviderSession was constructed.
        this.#safely(() => this.#hooks.sessionClosed?.(bot.id));
        throw error;
      }
      live.session = created;
      live.fresh = resumeId === null;
      live.sawSession = false;
      try {
        created.onEvent((event) => this.#onEvent(bot.id, created, event));
        await created.start();
      } catch (error) {
        if (live.session !== created) {
          created.close();
          return;
        }
        this.#closeSession(bot.id, live);
        if (!live.busy || live.item?.id !== item.id) return; // an exit already settled this turn
        if (live.stopRequested) {
          this.#finishTurn(bot.id, false, null);
          return;
        }
        if (resumeId === null) throw error;
        this.#db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(bot.id); // couldn't resume: start fresh with a recap
        this.#safely(() => this.#hooks.turnRequeued?.(item));
        this.#safely(() => this.#hooks.turnStarted?.(item)); // this retry starts directly, without pump
        return this.#startTurn(bot, item);
      }
      if (live.session !== created) {
        created.close();
        return;
      }
      if (!live.busy || live.item?.id !== item.id) return;
      if (live.stopRequested) {
        this.#finishTurn(bot.id, false, null);
        return;
      }
      if (live.fresh) {
        const before = item.conversationId === bot.id ? item.messageId ?? undefined : undefined;
        const recap = buildRecap(history(this.#db, bot.id, before, RECAP_MESSAGES), bot.name);
        if (recap) text = `${recap}\n\n${text}`;
      }
      session = created;
    }
    try {
      await session.send(text);
    } catch (error) {
      if (live.session !== session) return; // this attempt exited and was replaced while send was pending
      throw error;
    }
    if (!live.busy || live.item?.id !== item.id || live.session !== session) return;
    live.sent = true;
    if (live.stopRequested) {
      let handled = false;
      if (live.stopPending) {
        try {
          handled = await live.stopPending === true;
        } catch {
          // stop() reports the rejection and clears Stop. The provider still owns this turn's outcome.
          return;
        }
      }
      if (this.#live.get(bot.id) !== live || !live.busy || live.item?.id !== item.id || live.session !== session) return;
      if (live.stopRequested && !handled) await this.stop(bot.id);
      return;
    }
    await this.#steerQueued(bot.id); // anything sent while the turn was starting goes in now, in order
  }

  // Only your messages and bot replies in this conversation may steer; other sources wait.
  async #steerQueued(botId: string): Promise<void> {
    const live = this.#liveFor(botId);
    if (live.steering) return;
    live.steering = true;
    try {
      const next = () => nextSteerable(this.#db, botId, this.#dest(botId, live));
      for (let item = next(); item && live.busy && live.sent; item = next()) {
        const session = live.session;
        if (!session) break;
        setStatus(this.#db, item.id, "done"); // claim it first, so a turn that ends meanwhile can't pick it up too
        live.steeringItem = item;
        let steered = false;
        try {
          steered = await session.steer(item.text);
        } catch (error) {
          if (this.#live.get(botId) !== live) return;
          // Expected inability to steer is false in the provider contract; rejected calls are failures.
          console.error("Tjommie: steering a work item failed", error);
        }
        if (this.#live.get(botId) !== live) return;
        live.steeringItem = null;
        if (!steered) {
          setStatus(this.#db, item.id, "queued"); // it runs as the next turn instead
          break;
        }
        this.#setChatMessageStatus(item, "sent");
      }
    } finally {
      live.steering = false;
    }
    if (this.#live.get(botId) !== live) return;
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
        live.sawSession = true;
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
         ON CONFLICT(bot_id) DO UPDATE SET
           total_cost_usd = CASE WHEN sessions.provider = excluded.provider AND sessions.account_id = excluded.account_id
             AND sessions.cli_session_id = excluded.cli_session_id THEN sessions.total_cost_usd ELSE NULL END,
           provider = excluded.provider, account_id = excluded.account_id,
           cli_session_id = excluded.cli_session_id, updated_at = excluded.updated_at`,
      )
      .run(botId, bot.provider, accountFor(this.#db, bot.provider).id, cliSessionId, nowIso());
  }

  #appendText(botId: string, live: Live, delta: string): void {
    const conversationId = this.#dest(botId, live);
    if (!live.text) {
      if (live.activity) this.#closeActivity(live);
      const message = addMessage(this.#db, { conversationId, author: botId, kind: "text", body: "", status: "streaming" });
      live.text = { id: message.id, body: "" };
      this.#emitMessage(message, "message.added");
    }
    live.text.body += delta;
    this.#broadcast("message.delta", { id: live.text.id, conversationId, delta });
  }

  #flushText(live: Live, status: Message["status"]): void {
    if (!live.text) return;
    this.#emitMessage(updateMessage(this.#db, live.text.id, { body: live.text.body, status }));
    if (status === "done" && live.text.body.trim()) live.lastText = live.text;
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
      conversationId: this.#dest(botId, live),
      author: botId,
      kind: "activity",
      body: activityBody([item]),
      data: { items: [item] } satisfies ActivityData,
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
        data: { items: live.activity.items } satisfies ActivityData,
        status,
      }),
    );
  }

  #closeActivity(live: Live, unfinished: "done" | "failed" = "done"): void {
    for (const item of live.activity?.items ?? []) {
      if (item.status === "running") item.status = unfinished;
    }
    this.#saveActivity(live, "done");
    live.activity = null;
  }

  #onApproval(botId: string, live: Live, event: { id: string; tool: string; summary: string; detail: string; rule: string | null }): void {
    if (event.rule && allowRules(this.#db, botId).includes(event.rule)
      && (!event.rule.startsWith("codex-edit:") || event.autoAllow === true)) {
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
      data: { approvalId, tool: event.tool, detail: event.detail, rule: event.rule, status: "pending" } satisfies ApprovalData,
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
    this.#broadcast("notify", { kind: "approval", botId, approvalId, title: `${getStoredBot(this.#db, botId).name} is waiting for you`, body: event.summary });
    this.#emitBot(botId);
  }

  #showApprovalStatus(approval: Approval): void {
    const message = getMessage(this.#db, approval.messageId);
    const parsed = ApprovalData.safeParse(message.data);
    // Preserve a damaged card's fields without letting it interrupt turn cleanup.
    const data = parsed.success
      ? { ...parsed.data, status: approval.status } satisfies ApprovalData
      : { ...message.data, status: approval.status };
    this.#emitMessage(updateMessage(this.#db, message.id, { data }));
    this.#broadcast("approval.resolved", approval);
  }

  #expireApprovals(live: Live): void {
    for (const approvalId of live.approvals.keys()) {
      try {
        this.#showApprovalStatus(resolveApproval(this.#db, approvalId, "deny"));
      } catch (error) {
        if (!(error instanceof RpcError && error.code === "already_answered")) throw error;
      }
    }
    live.approvals.clear();
  }

  #recordUsage(botId: string, event: Extract<ProviderEvent, { type: "usage" }>): void {
    const bot = getStoredBot(this.#db, botId);
    const accountId = accountFor(this.#db, bot.provider).id;
    transaction(this.#db, () => {
      this.#db
        .prepare(
          `INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        )
        .run(
          newId(),
          botId,
          accountId,
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
      if (event.totalCostUsd !== undefined) {
        this.#db
          .prepare("UPDATE sessions SET total_cost_usd = ? WHERE bot_id = ? AND provider = ? AND account_id = ?")
          .run(event.totalCostUsd, botId, bot.provider, accountId);
      }
    });
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
    this.#broadcast("notify", { kind: "limit", botId, title: `${name} hit its usage limit`, body: `It will pick up again at ${at}.` });
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
    if (live.activity) this.#closeActivity(live, outcome === "done" ? "done" : "failed");
    const item = live.item;
    const conversationId = this.#dest(botId, live);
    const requeued = live.limitHit && !ok && !live.stopRequested;
    if (item && requeued) {
      setStatus(this.#db, item.id, "queued"); // runs again once the limit resets
      this.#setChatMessageStatus(item, "queued");
    } else if (item) {
      setStatus(this.#db, item.id, outcome);
      if (outcome === "interrupted") this.#notice(conversationId, "notice", live.stopNotice ?? "Stopped.", null);
      if (outcome === "failed") this.#notice(conversationId, "error", error ?? "Something went wrong.", retryData(item));
    }
    this.#expireApprovals(live); // the turn that asked is over
    const result: TurnResult = { outcome, text: live.lastText?.body ?? null, textMessageId: live.lastText?.id ?? null, error };
    live.busy = false;
    live.sent = false;
    live.item = null;
    live.stopRequested = false;
    live.stopNotice = null;
    live.stopPending = null;
    live.limitHit = false;
    live.lastText = null;
    if (live.reload) {
      live.reload = false;
      this.#closeSession(botId, live);
    }
    if (item) this.#safely(() => requeued ? this.#hooks.turnRequeued?.(item) : this.#hooks.turnEnded?.(item, result));
    this.#emitBot(botId);
    this.pump(botId);
  }

  #onExit(botId: string, live: Live, error: string | null): void {
    const resumeFailed = !live.fresh && !live.sawSession;
    live.session = null;
    this.#safely(() => this.#hooks.sessionClosed?.(botId));
    if (live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    if (!live.busy) return;
    if (resumeFailed && live.item && !live.stopRequested) {
      // The CLI couldn't resume this session: start fresh with a recap instead.
      const item = live.item;
      this.#flushText(live, "interrupted");
      if (live.activity) this.#closeActivity(live, "failed");
      live.lastText = null;
      this.#db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(botId);
      setStatus(this.#db, item.id, "queued");
      live.busy = false;
      live.sent = false;
      live.item = null;
      this.#safely(() => this.#hooks.turnRequeued?.(item));
      this.pump(botId);
      return;
    }
    live.limitHit = false; // an exit is terminal even if a limit event arrived first
    this.#finishTurn(botId, false, error ?? "The CLI stopped unexpectedly.");
  }

  #closeSession(botId: string, live: Live): void {
    const session = live.session;
    live.session = null; // its exit event is ignored from now on
    if (!session) return;
    try {
      session.close();
    } finally {
      this.#safely(() => this.#hooks.sessionClosed?.(botId));
    }
  }

  #releaseSession(botId: string, live: Live): void {
    if (live.idleTimer) clearTimeout(live.idleTimer);
    if (live.limitTimer) clearTimeout(live.limitTimer);
    live.idleTimer = null;
    live.limitTimer = null;
    try {
      this.#closeSession(botId, live);
    } catch (error) {
      console.error("Tjommie: closing a CLI session failed", error);
    }
  }

  #armIdle(botId: string, live: Live): void {
    if (!live.session || live.idleTimer) return;
    live.idleTimer = setTimeout(() => {
      live.idleTimer = null;
      if (!live.busy) this.#closeSession(botId, live);
    }, IDLE_SHUTDOWN_MS);
    live.idleTimer.unref();
  }

  #setChatMessageStatus(item: WorkItem, status: Message["status"]): void {
    if (!item.messageId || item.conversationId !== item.botId) return;
    const message = getMessage(this.#db, item.messageId);
    if (message.conversationId !== item.botId || message.author !== "you" || message.kind !== "text") return;
    this.#emitMessage(updateMessage(this.#db, message.id, { status }));
  }

  #notice(conversationId: string, kind: "notice" | "error", body: string, data: ErrorData | null): void {
    this.#emitMessage(
      addMessage(this.#db, { conversationId, author: "system", kind, body: body.slice(0, 500), data, status: "done" }),
      "message.added",
    );
  }

  #mcpServers(bot: StoredBot): McpServer[] {
    try {
      return this.#hooks.mcpServers?.(bot) ?? [];
    } catch (error) {
      console.error("Tjommie: a runner hook failed", error);
      throw error; // Starting without the requested servers would hide a broken tool configuration.
    }
  }

  #safely(hook: () => void): void {
    try {
      hook();
    } catch (error) {
      console.error("Tjommie: a runner hook failed", error);
    }
  }

  #emitMessage(message: Message, event: "message.added" | "message.updated" = "message.updated"): void {
    this.#broadcast(event, message);
  }

  #emitBot(botId: string): void {
    let bot: StoredBot;
    try {
      bot = getStoredBot(this.#db, botId);
    } catch (error) {
      if (!(error instanceof RpcError && error.code === "not_found")) console.error("Tjommie: loading a bot failed", error);
      return;
    }
    try {
      this.#broadcast("bot.updated", this.toBot(bot));
    } catch (error) {
      console.error("Tjommie: broadcasting a bot failed", error);
    }
  }
}
