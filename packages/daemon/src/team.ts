import { closeSync, constants, existsSync, fstatSync, lstatSync, mkdirSync, mkdtempSync, openSync, readdirSync, readSync, realpathSync, renameSync, rmSync, statSync, writeSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import {
  ExchangeHandoffData, FileData, RoutineApprovalData, RoutineInput, TEAM_ROOM_ID,
  type Approval, type ApprovalDecision, type BotState, type EventData, type EventName,
  type Exchange, type ExchangePostData, type Message, type NoticeData, type RoomHandoffData,
} from "@tjommie/protocol";
import { z } from "zod";
import { createApproval, getApproval, resolveApproval } from "./approvals.ts";
import { getStoredBot, listStoredBots, type StoredBot } from "./bots.ts";
import { addMessage, getMessage, history, updateMessage } from "./chat.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { defineTool, type McpTool } from "./mcp.ts";
import { enqueue, getWorkItem, newChain, setStatus, useChainRequest, type WorkItem } from "./queue.ts";
import { createRoutine, hostTimeZone } from "./routines.ts";
import { copyIntoInbox, type TurnResult } from "./runner.ts";
import { duration } from "./scheduler.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { getSettings } from "./settings.ts";

const ROOM_CONTEXT = 20;
const MAX_SHARE_BYTES = 200 * 1024 * 1024;
const SNAPSHOT_MAX_AGE_MS = 24 * 60 * 60 * 1000;
const MAX_PENDING_ROUTINE_PROPOSALS = 5;
const DAYS = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"] as const;
const DAY_LABELS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const STATE_WORDS: Record<BotState, string> = {
  idle: "idle",
  working: "working",
  waiting: "waiting for the person",
  paused: "paused",
  limited: "at its usage limit",
  needs_setup: "not set up yet",
};
const WORD = /[\p{L}\p{N}\p{M}_]/u;
const EMAIL_END = /[.\-+%]/u;

const within = (root: string, target: string): boolean => {
  const rel = relative(root, target);
  return rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
};

type ExchangeRow = {
  id: string;
  from_bot_id: string;
  to_bot_id: string;
  request: string;
  reply: string | null;
  status: Exchange["status"];
  chain_id: string;
  reply_to: string;
  created_at: string;
  replied_at: string | null;
};

export type BotDeletionHandle = Readonly<{
  bot: StoredBot;
  exchanges: readonly Readonly<{ row: ExchangeRow; asker: StoredBot | null; replyTo: string; fromName: string; toName: string }>[];
  snapshotFolders: readonly string[];
}>;

const toExchange = (row: ExchangeRow): Exchange => ({
  id: row.id,
  fromBotId: row.from_bot_id,
  toBotId: row.to_bot_id,
  request: row.request,
  reply: row.reply,
  status: row.status,
  createdAt: row.created_at,
  repliedAt: row.replied_at,
});

const firstLine = (text: string, max = 200): string => text.trim().split(/\r?\n/, 1)[0].slice(0, max);

const requestPrompt = (from: string, request: string) =>
  `[Message from ${from}, another bot. Your final message in this turn goes back to ${from} as your reply.]\n${request}`;

const replyPrompt = (to: string, request: string, reply: string, files: string[], first: boolean) =>
  [
    `[${to} ${first ? "replied" : "followed up"} on your request "${firstLine(request, 120)}":]`,
    reply,
    ...(files.length ? [`[Files ${to} shared, now in your inbox folder: ${files.join(", ")}]`] : []),
  ].join("\n");

const failedPrompt = (to: string, request: string, reason: string) =>
  `[${to} couldn't finish your request "${firstLine(request, 120)}": ${reason}. Tell the person if it matters.]`;

const roomPrompt = (context: string[], text: string) =>
  [
    "[Team room: the person mentioned you. Your reply is posted in the room for everyone.",
    ...(context.length ? ["Recent room messages, oldest first:", ...context] : []),
    "]",
    `The person wrote: ${text}`,
  ].join("\n");

function describeDays(days: number[]): string {
  const sorted = [...new Set(days)].sort((a, b) => a - b);
  const key = sorted.join("");
  if (key === "0123456") return "every day";
  if (key === "12345") return "weekdays";
  if (key === "06") return "weekends";
  return sorted.map((day) => DAY_LABELS[day]).join(", ");
}

export function getExchange(db: Db, id: string): Exchange {
  const row = db.prepare("SELECT * FROM exchanges WHERE id = ?").get(id) as ExchangeRow | undefined;
  if (!row) throw new RpcError("not_found", "No such exchange");
  return toExchange(row);
}

// The bots a Team room post mentions: "@Name" for whole names, longest names first, or "@everyone".
export function findMentions(text: string, bots: { id: string; name: string }[]): string[] {
  let rest = text.toLowerCase();
  const take = (name: string): boolean => {
    const tag = `@${name.toLowerCase()}`;
    let hit = false;
    for (let at = rest.indexOf(tag); at !== -1; at = rest.indexOf(tag, at + tag.length)) {
      const before = rest[at - 1] ?? "";
      if (WORD.test(before) || EMAIL_END.test(before) || WORD.test(rest[at + tag.length] ?? "")) continue;
      rest = rest.slice(0, at) + " ".repeat(tag.length) + rest.slice(at + tag.length);
      hit = true;
    }
    return hit;
  };
  if (take("everyone")) return bots.map((bot) => bot.id);
  const found = new Set<string>();
  for (const bot of [...bots].sort((a, b) => b.name.length - a.name.length)) if (take(bot.name)) found.add(bot.id);
  return bots.filter((bot) => found.has(bot.id)).map((bot) => bot.id);
}

// The real path of `target` when it's an existing file inside one of `roots`, with links followed.
// Anything else is an error the bot can read.
export function fileInside(roots: string[], target: string): string {
  let real: string;
  try {
    real = realpathSync.native(target);
  } catch {
    throw new Error(`There's no file at ${target}.`);
  }
  const inside = roots.some((root) => {
    let base: string;
    try {
      base = realpathSync.native(root);
    } catch {
      return false;
    }
    const rel = relative(base, real);
    return rel !== "" && rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
  });
  if (!inside) throw new Error("You can only share files from your own folders.");
  const stats = statSync(real);
  if (!stats.isFile()) throw new Error(`${target} is a folder, not a file.`);
  if (stats.size > MAX_SHARE_BYTES) throw new Error("That file is too big to share. The limit is 200 MB.");
  return real;
}

// Open once and read a bounded snapshot from that handle. A later rename, link swap,
// deletion or growth of the source cannot change the file card or the delivered bytes.
function snapshotFile(roots: string[], target: string, staging: string, botId: string): FileData {
  const file = fileInside(roots, target);
  const source = openSync(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  let folder: string | null = null;
  try {
    const opened = fstatSync(source);
    const checked = statSync(fileInside(roots, file));
    if (!opened.isFile() || opened.dev !== checked.dev || opened.ino !== checked.ino) throw new Error("The file changed while it was being shared. Please try again.");
    if (opened.size > MAX_SHARE_BYTES) throw new Error("That file is too big to share. The limit is 200 MB.");
    mkdirSync(staging, { recursive: true, mode: 0o700 });
    folder = mkdtempSync(join(staging, `share-${botId}-`));
    const path = join(folder, basename(file));
    const destination = openSync(path, "wx", 0o600);
    let size = 0;
    try {
      const buffer = Buffer.alloc(64 * 1024);
      for (;;) {
        const count = readSync(source, buffer, 0, buffer.length, null);
        if (count === 0) break;
        size += count;
        if (size > MAX_SHARE_BYTES) throw new Error("That file is too big to share. The limit is 200 MB.");
        let offset = 0;
        while (offset < count) offset += writeSync(destination, buffer, offset, count - offset);
      }
    } finally {
      closeSync(destination);
    }
    const finished = fstatSync(source);
    if (size !== opened.size || finished.size !== opened.size || finished.mtimeMs !== opened.mtimeMs) throw new Error("The file changed while it was being shared. Please try again.");
    return { path, name: basename(file), size };
  } catch (error) {
    if (folder) rmSync(folder, { recursive: true, force: true });
    throw error;
  } finally {
    closeSync(source);
  }
}

export type TeamDeps = {
  db: Db;
  broadcast: Broadcast;
  // Work is already committed to the queue. Runner.deliver accepts it without inserting again.
  deliver: (work: WorkItem) => void;
  stopItem: (itemId: string, notice: string) => void | Promise<void>;
  currentWork: (botId: string) => WorkItem | null;
  stateOf: (bot: StoredBot) => BotState;
};

// Bots working together: the tools on Tjommie's MCP server, exchanges between bots, and the Team room.
export class Team {
  #db: Db;
  #broadcast: Broadcast;
  #deliver: (work: WorkItem) => void;
  #stopItem: TeamDeps["stopItem"];
  #currentWork: (botId: string) => WorkItem | null;
  #stateOf: (bot: StoredBot) => BotState;
  #shared = new Map<string, string[]>(); // work item id -> files the bot shared during that turn
  #sessionChains = new Map<string, string>(); // autonomous CLI work shares one chain until sessionClosed
  #pending: (() => void)[] | null = null;
  #deletionHandles = new WeakSet<BotDeletionHandle>();
  #deletedBots = new Set<string>();

  constructor(deps: TeamDeps) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#deliver = deps.deliver;
    this.#stopItem = deps.stopItem;
    this.#currentWork = deps.currentWork;
    this.#stateOf = deps.stateOf;
  }

  // Call once at daemon startup, before starting work. Old folders survived a crash.
  start(): void {
    const staging = this.#staging();
    if (!existsSync(staging)) return;
    const cutoff = Date.now() - SNAPSHOT_MAX_AGE_MS;
    for (const entry of readdirSync(staging, { withFileTypes: true })) {
      if (!entry.isDirectory() || !/^(share|cleanup)-/.test(entry.name)) continue;
      const folder = join(staging, entry.name);
      if (lstatSync(folder).mtimeMs < cutoff) rmSync(folder, { recursive: true, force: true });
    }
  }

  // The tools on Tjommie's MCP server. `botId` is the calling bot, known from its session's token.
  tools(): McpTool[] {
    return [
      defineTool({
        name: "list_bots",
        description: "Lists the bots you may ask for help with message_bot: their names, what they do, and whether they're busy.",
        input: z.object({}),
        run: (botId) => this.#listBots(botId),
      }),
      defineTool({
        name: "message_bot",
        description:
          "Sends a request to another bot and returns at once. The bot works on it when it's free, and its answer arrives later as a new message to you. Don't wait or check for it.",
        input: z.object({
          bot: z.string().trim().min(1).describe("The bot's name, as list_bots shows it"),
          message: z.string().trim().min(1).max(20_000).describe("What you want, with everything the bot needs to know"),
        }),
        run: (botId, { bot, message }) => this.#messageBot(botId, bot, message),
      }),
      defineTool({
        name: "share_file",
        description: "Shows a file from your own folders to the person, as a file card. When you're answering another bot, the file goes to that bot too.",
        input: z.object({
          path: z.string().min(1).describe("An absolute path, or one relative to your working folder"),
          caption: z.string().max(500).optional().describe("A short note shown on the card"),
        }),
        run: (botId, { path, caption }) => this.#shareFile(botId, path, caption),
      }),
      defineTool({
        name: "add_routine",
        description: "Proposes a routine: work you do on a schedule. The person sees a card and decides. Nothing is saved until they approve it.",
        input: z.object({
          name: z.string().trim().min(1).max(60),
          days: z.array(z.enum(DAYS)).min(1).max(7).describe("Days of the week"),
          times: z
            .array(z.string().regex(/^([01]\d|2[0-3]):[0-5]\d$/))
            .min(1)
            .max(12)
            .describe(`24-hour times in the person's time zone, like "08:30"`),
          prompt: z.string().trim().min(1).max(10_000).describe("What to do on each run"),
          time_limit_minutes: z.number().int().min(1).max(1440).optional().describe("Stop each run after this many minutes"),
        }),
        run: (botId, args) =>
          this.#proposeRoutine(botId, {
            name: args.name,
            days: args.days.map((day) => DAYS.indexOf(day)),
            times: args.times,
            prompt: args.prompt,
            timeLimitMinutes: args.time_limit_minutes ?? null,
          }),
      }),
    ];
  }

  // A post from the person in the Team room. Every bot it mentions answers in the room.
  postToRoom(text: string): Message {
    const context = this.#describe(history(this.#db, TEAM_ROOM_ID, undefined, ROOM_CONTEXT));
    const mentioned = findMentions(text, listStoredBots(this.#db));
    const queued: WorkItem[] = [];
    const post = this.#atomic(() => {
      const message = this.#add({ conversationId: TEAM_ROOM_ID, author: "you", kind: "text", body: text, status: "done" });
      if (mentioned.length) {
        const chainId = newChain(this.#db);
        for (const botId of mentioned) queued.push(enqueue(this.#db, { botId, source: "room", text: roomPrompt(context, text), messageId: message.id, conversationId: TEAM_ROOM_ID, chainId }));
      }
      return message;
    });
    for (const item of queued) this.#wake(item);
    return post;
  }

  // Compose with the runner's sessionClosed hook, alongside revoking its MCP token.
  sessionClosed(botId: string): void {
    this.#sessionChains.delete(botId);
  }

  // The runner calls this after each turn it started.
  turnEnded(item: WorkItem, result: TurnResult): void {
    this.#sessionChains.delete(item.botId);
    const files = this.#shared.get(item.id) ?? [];
    this.#shared.delete(item.id);
    const exchange = this.#exchange(item.conversationId);
    if (exchange && exchange.to_bot_id === item.botId) this.#answer(exchange, result, files);
    if (item.conversationId === TEAM_ROOM_ID && result.outcome === "done" && result.textMessageId) {
      this.#add({ conversationId: item.botId, author: item.botId, kind: "handoff", body: "Answered in the Team room", data: { roomMessageId: result.textMessageId } satisfies RoomHandoffData, status: "done" });
    }
  }

  // Capture after the session closes and before attempting the fallible folder-first delete.
  // This does not change the queue, exchanges, snapshots or published state.
  botDeleting(botId: string): BotDeletionHandle {
    const bot = getStoredBot(this.#db, botId);
    const open = this.#db.prepare("SELECT * FROM exchanges WHERE status = 'waiting' AND (from_bot_id = ? OR to_bot_id = ?)").all(botId, botId) as ExchangeRow[];
    const folders = new Set<string>();
    const staging = this.#staging();
    if (existsSync(staging)) {
      for (const entry of readdirSync(staging, { withFileTypes: true })) {
        if (entry.isDirectory() && entry.name.startsWith(`share-${botId}-`)) folders.add(join(staging, entry.name));
      }
    }
    // Also capture folders made by the earlier version, before names included the owner.
    const cards = this.#db.prepare("SELECT json_extract(data, '$.path') AS path FROM messages WHERE kind = 'file' AND author = ?").all(botId) as { path: string | null }[];
    for (const card of cards) {
      if (card.path) {
        const folder = this.#snapshotFolder(card.path);
        if (folder) folders.add(folder);
      }
    }
    const handle: BotDeletionHandle = Object.freeze({
      bot: Object.freeze(bot),
      exchanges: Object.freeze(open.map((row) => Object.freeze({
        row: Object.freeze(row),
        asker: this.#botOrNull(row.from_bot_id),
        replyTo: row.reply_to,
        fromName: this.#name(row.from_bot_id),
        toName: this.#name(row.to_bot_id),
      }))),
      snapshotFolders: Object.freeze([...folders]),
    });
    this.#deletionHandles.add(handle);
    return handle;
  }

  // Call only after deleteBot succeeds. The handle survives cascaded rows and repeated calls.
  botDeleted(handle: BotDeletionHandle): void {
    if (!this.#deletionHandles.has(handle)) throw new RpcError("bad_params", "Unknown bot deletion handle");
    const botId = handle.bot.id;
    if (this.#deletedBots.has(botId)) return;
    if (this.#botOrNull(botId)) throw new RpcError("busy", "The bot must be deleted before finishing its exchanges");
    const queued: WorkItem[] = [];
    const stopping: string[] = [];
    const moved: { from: string; to: string }[] = [];
    try {
      this.#atomic(() => {
        for (const captured of handle.exchanges) {
          const { row } = captured;
          const existing = this.#exchange(row.id);
          if (existing && existing.status !== "waiting") continue;
          if (!existing) {
            this.#db.prepare("INSERT INTO exchanges (id, from_bot_id, to_bot_id, request, reply, status, chain_id, reply_to, created_at, replied_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
              .run(row.id, row.from_bot_id, row.to_bot_id, row.request, row.reply, row.status, row.chain_id, row.reply_to, row.created_at, row.replied_at);
          }
          const running = this.#db.prepare("SELECT id FROM work_items WHERE exchange_id = ? AND source = 'bot_request' AND status = 'running'").all(row.id) as { id: string }[];
          stopping.push(...running.map((item) => item.id));
          this.#db.prepare("UPDATE work_items SET status = 'failed', ended_at = ? WHERE exchange_id = ? AND source = 'bot_request' AND status = 'queued'").run(nowIso(), row.id);
          this.#fail(row, `${handle.bot.name} was deleted`, captured);
          if (row.to_bot_id === botId && captured.asker && this.#botOrNull(captured.asker.id)) {
            const replyTo = this.#db.prepare("SELECT 1 FROM conversations WHERE id = ?").get(captured.replyTo) ? captured.replyTo : captured.asker.id;
            queued.push(enqueue(this.#db, { botId: captured.asker.id, source: "bot_reply", text: failedPrompt(handle.bot.name, row.request, `${handle.bot.name} was deleted`), conversationId: replyTo, chainId: row.chain_id, exchangeId: row.id }));
          }
        }
        // Renames are reversible if the transaction cannot commit. Delete the moved
        // directories only after commit, before publishing any events.
        for (const from of handle.snapshotFolders) {
          if (!existsSync(from)) continue;
          const to = join(this.#staging(), `cleanup-${botId}-${newId()}`);
          renameSync(from, to);
          moved.push({ from, to });
        }
      }, () => {
        this.#deletedBots.add(botId);
        this.sessionClosed(botId);
        for (const [id, files] of this.#shared) {
          const kept = files.filter((file) => !handle.snapshotFolders.includes(dirname(file)));
          if (kept.length) this.#shared.set(id, kept);
          else this.#shared.delete(id);
        }
        for (const move of moved) this.#removeSnapshotFolder(move.to);
      });
    } catch (error) {
      for (const move of moved.reverse()) renameSync(move.to, move.from);
      throw error;
    }
    for (const itemId of stopping) {
      const failed = (error: unknown) => console.error("Tjommie: could not stop a deleted bot's request", error);
      try {
        void Promise.resolve(this.#stopItem(itemId, `${handle.bot.name} was deleted.`)).catch(failed);
      } catch (error) {
        failed(error);
      }
    }
    for (const item of queued) this.#wake(item);
  }

  // The person answered a routine proposal: "Add routine" (once or always) or "Dismiss" (deny).
  answerRoutineProposal(approvalId: string, decision: ApprovalDecision): Approval {
    return this.#atomic(() => {
      const pending = getApproval(this.#db, approvalId);
      if (pending.kind !== "routine") throw new RpcError("bad_params", "This is not a routine proposal");
      const approval = resolveApproval(this.#db, approvalId, decision);
      if (approval.status === "allowed") {
        const input = RoutineInput.parse(approval.payload);
        if (input.botId !== approval.botId) throw new RpcError("bad_params", "The routine belongs to a different bot");
        this.#emit("routine.updated", createRoutine(this.#db, input));
      }
      const message = getMessage(this.#db, approval.messageId);
      const data = { ...RoutineApprovalData.parse(message.data), status: approval.status } satisfies RoutineApprovalData;
      this.#emit("message.updated", updateMessage(this.#db, message.id, { data }));
      this.#emit("approval.resolved", approval);
      const bot = getStoredBot(this.#db, approval.botId);
      this.#emit("bot.updated", { ...bot, state: this.#stateOf(bot) });
      return approval;
    });
  }

  #listBots(botId: string): string {
    const targets = getStoredBot(this.#db, botId).canMessage.map((id) => getStoredBot(this.#db, id));
    if (targets.length === 0) return "You can't message any bots yet. The person can add bots to your Can message list in your settings.";
    return targets.map((bot) => `- ${bot.name} (${STATE_WORDS[this.#stateOf(bot)]}): ${firstLine(bot.instructions, 120) || "No description."}`).join("\n");
  }

  #messageBot(fromId: string, name: string, request: string): string {
    const from = getStoredBot(this.#db, fromId);
    const wanted = name.replace(/^@/, "").toLowerCase();
    const to = listStoredBots(this.#db).find((bot) => bot.name.toLowerCase() === wanted);
    if (!to) throw new Error(`There's no bot called ${name}. Use list_bots to see who you can ask.`);
    if (to.id === from.id) throw new Error("You can't message yourself.");
    if (!from.canMessage.includes(to.id)) throw new Error(`You're not allowed to message ${to.name}. The person can add ${to.name} to your Can message list.`);
    const work = this.#currentWork(fromId);
    const id = newId();
    const queued = this.#atomic(() => {
      const chainId = work
        ? work.chainId ?? getWorkItem(this.#db, work.id).chainId ?? newChain(this.#db)
        : this.#sessionChains.get(fromId) ?? newChain(this.#db);
      if (!useChainRequest(this.#db, chainId, getSettings(this.#db).chainLimit)) {
        throw new Error("Limit reached, ask the user. This chain of requests between bots has used all its requests, so ask the person before asking another bot.");
      }
      if (work && !work.chainId) this.#db.prepare("UPDATE work_items SET chain_id = ? WHERE id = ?").run(chainId, work.id);
      this.#db
        .prepare("INSERT INTO exchanges (id, from_bot_id, to_bot_id, request, status, chain_id, reply_to, created_at) VALUES (?, ?, ?, ?, 'waiting', ?, ?, ?)")
        .run(id, from.id, to.id, request, chainId, work?.conversationId ?? from.id, nowIso());
      this.#db.prepare("INSERT INTO conversations (id, kind, bot_id) VALUES (?, 'exchange', NULL)").run(id);
      const message = this.#add({ conversationId: id, author: from.id, kind: "text", body: request, status: "done" });
      this.#add({ conversationId: TEAM_ROOM_ID, author: from.id, kind: "text", body: request, data: { exchangeId: id, toBotId: to.id } satisfies ExchangePostData, status: "done" });
      const row = { exchangeId: id, fromBotId: from.id, toBotId: to.id, messages: 1, status: "waiting" } satisfies ExchangeHandoffData;
      for (const botId of [from.id, to.id]) this.#add({ conversationId: botId, author: from.id, kind: "handoff", body: firstLine(request), data: row, status: "done" });
      return enqueue(this.#db, { botId: to.id, source: "bot_request", text: requestPrompt(from.name, request), messageId: message.id, conversationId: id, chainId, exchangeId: id });
    });
    if (!work) this.#sessionChains.set(fromId, queued.chainId!);
    try {
      this.#wake(queued);
    } catch (error) {
      this.#atomic(() => {
        this.#db.prepare("UPDATE chains SET requests_made = requests_made - 1 WHERE id = ? AND requests_made > 0").run(queued.chainId);
        this.#fail(this.#exchange(id)!, "the request could not be delivered");
      });
      throw error;
    }
    return `Sent. ${to.name} will reply.`;
  }

  #shareFile(botId: string, path: string, caption: string | undefined): string {
    const bot = getStoredBot(this.#db, botId);
    const roots = bot.projectFolder ? [bot.projectFolder, bot.homeFolder] : [bot.homeFolder];
    const file = fileInside(roots, resolve(roots[0], path));
    // Legacy project folders must not expose the daemon's data or another bot's files.
    const managed = realpathSync.native(dirname(this.#staging()));
    if (within(managed, file) && !within(realpathSync.native(bot.homeFolder), file)) throw new Error("You can only share files from your own folders.");
    const work = this.#currentWork(botId);
    const exchange = work ? this.#exchange(work.conversationId) : undefined;
    const needsReply = exchange?.to_bot_id === botId && this.#botOrNull(exchange.from_bot_id) !== null;
    const snapshot = snapshotFile(roots, file, this.#staging(), botId);
    let durable: string | null = null;
    try {
      // Without a bot recipient the person receives the file through this bot's inbox.
      if (!needsReply) durable = join(bot.homeFolder, "inbox", copyIntoInbox(bot.homeFolder, snapshot.path));
      this.#atomic(() => this.#add({ conversationId: work?.conversationId ?? botId, author: botId, kind: "file", body: caption?.trim() || snapshot.name, data: { ...snapshot, path: durable ?? snapshot.path } satisfies FileData, status: "done" }));
    } catch (error) {
      if (durable) rmSync(durable, { force: true });
      rmSync(dirname(snapshot.path), { recursive: true, force: true });
      throw error;
    }
    if (needsReply && work) this.#shared.set(work.id, [...(this.#shared.get(work.id) ?? []), snapshot.path]);
    else this.#releaseSnapshots([snapshot.path]);
    return `Shared ${snapshot.name}.`;
  }

  #proposeRoutine(botId: string, r: { name: string; days: number[]; times: string[]; prompt: string; timeLimitMinutes: number | null }): string {
    const bot = getStoredBot(this.#db, botId);
    const times = [...new Set(r.times)].sort();
    const summary = `Add a routine: ${r.name}, ${describeDays(r.days)} at ${times.join(", ")}${r.timeLimitMinutes ? `, stop after ${duration(r.timeLimitMinutes)}` : ""}.`;
    const approvalId = newId();
    const payload = RoutineInput.parse({ botId, ...r, timeZone: hostTimeZone() });
    this.#atomic(() => {
      const pending = this.#db.prepare("SELECT COUNT(*) AS n FROM approvals WHERE bot_id = ? AND kind = 'routine' AND status = 'pending'").get(botId) as { n: number };
      if (pending.n >= MAX_PENDING_ROUTINE_PROPOSALS) throw new Error(`You already have ${MAX_PENDING_ROUTINE_PROPOSALS} routine proposals waiting for the person. Wait for them to answer before proposing another.`);
      const message = this.#add({
        conversationId: botId,
        author: botId,
        kind: "approval",
        body: summary,
        data: { approvalId, kind: "routine", tool: "Routine", detail: r.prompt, status: "pending" } satisfies RoutineApprovalData,
        status: "done",
      });
      const approval = createApproval(this.#db, {
        id: approvalId,
        botId,
        kind: "routine",
        tool: "Routine",
        summary,
        detail: r.prompt,
        rule: null,
        payload,
        messageId: message.id,
      });
      this.#emit("approval.added", approval);
      this.#emit("notify", { kind: "approval", botId, approvalId, title: `${bot.name} is waiting for you`, body: summary });
      this.#emit("bot.updated", { ...bot, state: this.#stateOf(bot) });
    });
    return "Proposed. The person sees a card and decides; nothing is saved until they approve it.";
  }

  // What the bot that was asked said at the end of a turn in the exchange, or why it couldn't answer.
  #answer(row: ExchangeRow, result: TurnResult, files: string[]): void {
    const first = row.status !== "replied"; // a failed request may have been retried
    const to = this.#botOrNull(row.to_bot_id);
    if (!to) return;
    const from = this.#botOrNull(row.from_bot_id);
    // Deletion already settled this exchange. Late output must not revive it.
    if (!from && row.status === "failed") {
      if (files.length) this.#saveFilesForPerson(to, files);
      return;
    }
    if (result.outcome !== "done") {
      const reason = result.outcome === "interrupted" ? "it was stopped" : (result.error ?? "something went wrong");
      if (first) this.#atomic(() => this.#fail(row, reason));
      try {
        if (files.length) this.#saveFilesForPerson(to, files);
      } catch (error) {
        console.error("Tjommie: could not save files from a failed exchange turn", error);
      }
      this.#tell(row, failedPrompt(to.name, row.request, reason));
      return;
    }
    const reply = result.text ?? "(No written reply.)";
    const recipient = from ?? to;
    const inbox: string[] = [];
    try {
      for (const path of files) inbox.push(copyIntoInbox(recipient.homeFolder, path));
    } catch {
      for (const name of inbox) rmSync(join(recipient.homeFolder, "inbox", name), { force: true });
      const reason = "the shared files could not be copied into your inbox";
      if (first) this.#atomic(() => this.#fail(row, reason));
      this.#tell(row, failedPrompt(to.name, row.request, reason));
      return;
    }
    let queued: WorkItem | null;
    try {
      queued = this.#atomic(() => {
        if (first) this.#db.prepare("UPDATE exchanges SET status = 'replied', reply = ?, replied_at = ? WHERE id = ?").run(reply, nowIso(), row.id);
        const shared = files.map((path, index) => {
          const durable = join(recipient.homeFolder, "inbox", inbox[index]);
          this.#pointFileCards(path, durable);
          return { name: basename(path), path: durable };
        });
        this.#add({ conversationId: TEAM_ROOM_ID, author: to.id, kind: "text", body: reply, data: { exchangeId: row.id, toBotId: row.from_bot_id, files: shared } satisfies ExchangePostData, status: "done" });
        this.#refreshRows(row.id);
        return this.#queueReply(row, replyPrompt(to.name, row.request, reply, inbox, first));
      });
    } catch (error) {
      for (const name of inbox) rmSync(join(recipient.homeFolder, "inbox", name), { force: true });
      if (first) this.#atomic(() => this.#fail(row, "the reply could not be saved"));
      throw error;
    }
    if (queued) {
      try {
        this.#wake(queued);
      } catch (error) {
        if (first) this.#atomic(() => this.#fail(row, "the reply could not be delivered"));
        throw error;
      }
    }
    this.#releaseSnapshots(files);
  }

  // Ends an exchange that won't get an answer, in its handoff rows and in the room.
  #fail(row: ExchangeRow, reason: string, names?: { fromName: string; toName: string }): void {
    this.#db.prepare("UPDATE exchanges SET status = 'failed', reply = ?, replied_at = ? WHERE id = ?").run(reason, nowIso(), row.id);
    this.#refreshRows(row.id);
    this.#add({
      conversationId: TEAM_ROOM_ID,
      author: "system",
      kind: "notice",
      body: `${names?.toName ?? this.#name(row.to_bot_id)} couldn't answer ${names?.fromName ?? this.#name(row.from_bot_id)}: ${reason}.`,
      data: { exchangeId: row.id } satisfies NoticeData,
      status: "done",
    });
  }

  // Sends a reply or a failure to the bot that asked, into the conversation its asking turn was writing to.
  #tell(row: ExchangeRow, text: string): void {
    const queued = this.#queueReply(row, text);
    if (queued) this.#wake(queued);
  }

  #queueReply(row: ExchangeRow, text: string): WorkItem | null {
    if (!this.#botOrNull(row.from_bot_id)) return null;
    const replyTo = this.#db.prepare("SELECT 1 FROM conversations WHERE id = ?").get(row.reply_to) ? row.reply_to : row.from_bot_id;
    return enqueue(this.#db, { botId: row.from_bot_id, source: "bot_reply", text, conversationId: replyTo, chainId: row.chain_id, exchangeId: row.id });
  }

  // Brings the handoff rows in both chats up to date: message count and status.
  #refreshRows(exchangeId: string): void {
    const row = this.#exchange(exchangeId)!;
    const { n } = this.#db.prepare("SELECT COUNT(*) AS n FROM messages WHERE conversation_id = ? AND kind = 'text'").get(exchangeId) as { n: number };
    const ids = this.#db.prepare("SELECT id FROM messages WHERE kind = 'handoff' AND json_extract(data, '$.exchangeId') = ?").all(exchangeId) as { id: string }[];
    for (const { id } of ids) {
      const message = getMessage(this.#db, id);
      const data = { ...ExchangeHandoffData.parse(message.data), messages: n, status: row.status } satisfies ExchangeHandoffData;
      this.#emit("message.updated", updateMessage(this.#db, id, { data }));
    }
  }

  #exchange(id: string): ExchangeRow | undefined {
    return this.#db.prepare("SELECT * FROM exchanges WHERE id = ?").get(id) as ExchangeRow | undefined;
  }

  #botOrNull(id: string): StoredBot | null {
    try {
      return getStoredBot(this.#db, id);
    } catch (error) {
      if (error instanceof RpcError && error.code === "not_found") return null;
      throw error;
    }
  }

  #name(id: string): string {
    return this.#botOrNull(id)?.name ?? "A deleted bot";
  }

  #staging(): string {
    const database = this.#db.location();
    if (!database) throw new Error("File sharing needs a saved database.");
    return join(dirname(database), "shared-files");
  }

  #snapshotFolder(file: string): string | null {
    const folder = dirname(file);
    return dirname(folder) === this.#staging() && basename(folder).startsWith("share-") ? folder : null;
  }

  #removeSnapshotFolder(folder: string): void {
    try {
      rmSync(folder, { recursive: true, force: true });
    } catch (error) {
      // A locked orphan is retried by the next startup sweep.
      console.error("Tjommie: shared file cleanup failed", error);
    }
  }

  #releaseSnapshots(files: string[]): void {
    for (const file of files) {
      const folder = this.#snapshotFolder(file);
      if (folder) this.#removeSnapshotFolder(folder);
    }
  }

  #pointFileCards(snapshot: string, durable: string): void {
    const cards = this.#db.prepare("SELECT id FROM messages WHERE kind = 'file' AND json_extract(data, '$.path') = ?").all(snapshot) as { id: string }[];
    for (const { id } of cards) {
      const message = getMessage(this.#db, id);
      const data = { ...FileData.parse(message.data), path: durable } satisfies FileData;
      this.#emit("message.updated", updateMessage(this.#db, id, { data }));
    }
  }

  #saveFilesForPerson(bot: StoredBot, files: string[]): void {
    const copies: string[] = [];
    try {
      for (const file of files) copies.push(join(bot.homeFolder, "inbox", copyIntoInbox(bot.homeFolder, file)));
      this.#atomic(() => {
        for (const [index, file] of files.entries()) this.#pointFileCards(file, copies[index]);
      });
    } catch (error) {
      for (const copy of copies) rmSync(copy, { force: true });
      throw error;
    }
    this.#releaseSnapshots(files);
  }

  // Room messages as lines of text for a bot's prompt.
  #describe(messages: Message[]): string[] {
    const name = (id: string) => (id === "you" ? "Person" : id === "system" ? "Tjommie" : this.#name(id));
    return messages
      .filter((m) => (m.kind === "text" || m.kind === "notice") && m.body.trim() !== "")
      .map((m) => {
        const to = m.data?.toBotId;
        return `${name(m.author)}${typeof to === "string" ? ` to ${name(to)}` : ""}: ${m.body.trim().slice(0, 2000)}`;
      });
  }

  #add(m: { conversationId: string; author: string; kind: Message["kind"]; body: string; data?: Record<string, unknown> | null; status: Message["status"] }): Message {
    const message = addMessage(this.#db, m);
    this.#emit("message.added", message);
    return message;
  }

  #wake(item: WorkItem): void {
    try {
      this.#deliver(item);
    } catch (error) {
      setStatus(this.#db, item.id, "failed");
      throw error;
    }
  }

  #emit<E extends EventName>(event: E, data: EventData<E>): void {
    const send = () => {
      try {
        this.#broadcast(event, data);
      } catch (error) {
        console.error("Tjommie: a Team broadcast failed", error);
      }
    };
    if (this.#pending) this.#pending.push(send);
    else send();
  }

  #atomic<T>(work: () => T, afterCommit?: () => void): T {
    if (this.#pending) {
      if (afterCommit) throw new Error("An after-commit action cannot run inside another Team transaction.");
      return work();
    }
    const events: (() => void)[] = [];
    this.#pending = events;
    let result!: T;
    try {
      transaction(this.#db, () => { result = work(); });
    } finally {
      this.#pending = null;
    }
    afterCommit?.();
    for (const send of events) send();
    return result;
  }
}
