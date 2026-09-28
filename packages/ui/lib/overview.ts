import { ActivityData, ApprovalData, ExchangeHandoffData, ExchangePostData, type Bot, type BotState, type Message, type Routine } from "@tjommie/protocol";
import { clock, dayName, timeAgo } from "./format";
import { botName, stateLabel } from "./labels";

export type OverviewFilter = "all" | "working" | "waiting" | "paused";

export const FILTERS = [
  { value: "all", label: "All" },
  { value: "working", label: "Working" },
  { value: "waiting", label: "Waiting" },
  { value: "paused", label: "Paused" },
] as const satisfies readonly { value: OverviewFilter; label: string }[];

// "6 bots, 2 working, 1 waiting for you".
export function overviewSummary(bots: Pick<Bot, "state">[]): string {
  const count = (state: BotState): number => bots.filter((bot) => bot.state === state).length;
  const parts = [`${bots.length} ${bots.length === 1 ? "bot" : "bots"}`];
  if (count("working")) parts.push(`${count("working")} working`);
  if (count("waiting")) parts.push(`${count("waiting")} waiting for you`);
  return parts.join(", ");
}

export function matches(bot: Pick<Bot, "name" | "state">, filter: OverviewFilter, query: string): boolean {
  const typed = query.trim().toLowerCase();
  if (typed && !bot.name.toLowerCase().includes(typed)) return false;
  return filter === "all" || bot.state === filter;
}

const firstLine = (text: string): string =>
  text
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean) ?? "";

// What the bot is doing, in one line, from its newest messages (WF2's task line).
export function taskLine(bot: Pick<Bot, "id" | "state" | "limitedUntil" | "setupReason">, messages: Message[]): string {
  if (bot.state === "paused") return "Paused by you";
  if (bot.state === "limited") return stateLabel(bot);
  if (bot.state === "needs_setup") return bot.setupReason ?? "Needs setup";
  const newest = [...messages].reverse();
  if (bot.state === "waiting") {
    for (const message of newest) {
      const card = message.kind === "approval" ? ApprovalData.safeParse(message.data).data : undefined;
      // Claude describes a Write by its file name alone, so a body without spaces gets the tool: "Write: hello.txt".
      if (card?.status === "pending") return /\s/.test(message.body.trim()) ? message.body : `${card.tool}: ${message.body}`;
    }
    return "Waiting for you";
  }
  if (bot.state === "working") {
    const row = newest.find((message) => message.kind === "activity");
    const step = row ? ActivityData.safeParse(row.data).data?.items.at(-1) : undefined;
    return step?.status === "running" ? step.summary : "Working";
  }
  const reply = newest.find((message) => message.author === bot.id && message.kind === "text" && message.body.trim());
  return reply ? firstLine(reply.body) : "No messages yet";
}

// "Next: Morning check, 8:48 AM", "Next: Morning check, Tomorrow 8:48 AM", "No routine" or "Paused".
export function nextRoutineLine(bot: Pick<Bot, "id" | "paused">, routines: Routine[], now: Date = new Date()): string {
  if (bot.paused) return "Paused";
  const next = routines
    .filter((routine) => routine.botId === bot.id && routine.enabled && routine.nextRunAt)
    .sort((a, b) => (a.nextRunAt ?? "").localeCompare(b.nextRunAt ?? ""))[0];
  if (!next?.nextRunAt) return "No routine";
  const day = dayName(next.nextRunAt, now);
  return `Next: ${next.name}, ${day === "Today" ? "" : `${day} `}${clock(next.nextRunAt)}`;
}

// The bots this bot had exchanges with today, from the handoff rows among its newest messages.
export function talkingWith(botId: string, messages: Message[], now: Date = new Date()): string[] {
  const today = now.toDateString();
  const partners = new Set<string>();
  for (const message of messages) {
    if (message.kind !== "handoff" || new Date(message.createdAt).toDateString() !== today) continue;
    const row = ExchangeHandoffData.safeParse(message.data);
    if (row.success) partners.add(row.data.fromBotId === botId ? row.data.toBotId : row.data.fromBotId);
  }
  return [...partners];
}

// The Team room card's line: which bots talked last, and when.
export function roomLine(messages: Message[], bots: Pick<Bot, "id" | "name">[], now: Date = new Date()): string {
  const post = [...messages].reverse().find((message) => message.kind === "text" && ExchangePostData.safeParse(message.data).success);
  if (!post) return "No bot chats yet";
  const { toBotId } = ExchangePostData.parse(post.data);
  return `${botName(bots, post.author)} and ${botName(bots, toBotId)} talked ${timeAgo(post.createdAt, now)}`;
}
