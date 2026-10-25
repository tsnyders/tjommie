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

// The phone's filter chips, with how many bots each would show. A state no bot is in has no chip (unless it's the one picked, so it can be left),
// and when no chip would narrow the list there are none at all.
export function phoneFilters(bots: Pick<Bot, "state">[], current: OverviewFilter): { value: OverviewFilter; label: string; count: number }[] {
  const chips = FILTERS.map((filter) => ({ ...filter, count: filter.value === "all" ? bots.length : bots.filter((bot) => bot.state === filter.value).length }));
  const narrowing = chips.filter((chip) => chip.value !== "all" && chip.count > 0 && chip.count < bots.length);
  if (narrowing.length === 0 && current === "all") return [];
  return chips.filter((chip) => chip.value === "all" || chip.count > 0 || chip.value === current);
}

// Folio's one-sentence lede under the Agents title: "Two bots are working; Nova is waiting for you."
export function overviewLede(bots: Pick<Bot, "name" | "state">[]): string {
  if (bots.length === 0) return "No bots yet.";
  const working = bots.filter((bot) => bot.state === "working");
  const waiting = bots.filter((bot) => bot.state === "waiting");
  const parts: string[] = [];
  if (working.length) parts.push(working.length === 1 ? `${working[0].name} is working` : `${working.length} bots are working`);
  if (waiting.length) parts.push(waiting.length === 1 ? `${waiting[0].name} is waiting for you` : `${waiting.length} bots are waiting for you`);
  if (parts.length === 0) return "All quiet.";
  const sentence = parts.join("; ");
  return `${sentence[0].toUpperCase()}${sentence.slice(1)}.`;
}

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

// A room's line (its card on Agents, its row in the rooms list), from its newest post of any kind: yours, a bot's answer, or a bot-to-bot
// exchange, which names both bots. Notices, activity and a reply still on its way aren't posts. A room whose posts are still being read
// (undefined) gets a blank line that keeps its height, not "No posts yet".
export function roomLine(messages: Message[] | undefined, bots: Pick<Bot, "id" | "name">[], now: Date = new Date()): string {
  if (!messages) return "\u00a0";
  const post = [...messages].reverse().find((message) => message.kind === "text" && message.body.trim() !== "");
  if (!post) return "No posts yet";
  const ago = timeAgo(post.createdAt, now);
  const exchange = ExchangePostData.safeParse(post.data);
  if (exchange.success) return `${botName(bots, post.author)} and ${botName(bots, exchange.data.toBotId)} talked ${ago}`;
  return post.author === "you" ? `You posted ${ago}` : `${botName(bots, post.author)} answered ${ago}`;
}
