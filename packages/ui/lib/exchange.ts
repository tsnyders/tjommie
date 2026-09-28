import type { Bot, Exchange, Message } from "@tjommie/protocol";
import { stamp } from "./format";
import { botName } from "./labels";

// How long the answer took: "less than a minute", "2 minutes", "1 hour".
function waited(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "less than a minute";
  if (minutes < 60) return `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
  const hours = Math.round(minutes / 60);
  return `${hours} ${hours === 1 ? "hour" : "hours"}`;
}

// The exchange view's meta line (WF6): "Today 9:31 AM · Replied after 2 minutes".
export function exchangeMeta(exchange: Exchange, now?: Date): string {
  const asked = stamp(exchange.createdAt, now);
  if (exchange.status === "waiting") return `${asked} · Waiting for a reply`;
  if (exchange.status === "failed") return `${asked} · No reply: ${exchange.reply ?? "something went wrong"}`;
  const took = exchange.repliedAt ? Date.parse(exchange.repliedAt) - Date.parse(exchange.createdAt) : 0;
  return `${asked} · Replied after ${waited(took)}`;
}

// The line under a replied exchange: where the answer, and any files, went.
export function exchangeCaption(exchange: Exchange, messages: Message[], bots: Pick<Bot, "id" | "name">[]): string | null {
  if (exchange.status !== "replied") return null;
  const from = botName(bots, exchange.fromBotId);
  const files = messages.filter((message) => message.kind === "file").length;
  if (files === 0) return `Sent back to ${from}.`;
  return `Sent back to ${from}. ${files === 1 ? "The file is" : "The files are"} in ${from}'s inbox.`;
}
