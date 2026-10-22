import type { Message } from "@tjommie/protocol";
import { stamp } from "./format";

export type TimelineItem = { type: "stamp"; key: string; label: string } | { type: "message"; key: string; message: Message };

const GAP_MS = 15 * 60_000;

// A chat's messages with a time label before the first one, after a gap of 15 minutes or more, and at each new day.
export function timeline(messages: Message[], now: Date = new Date()): TimelineItem[] {
  const items: TimelineItem[] = [];
  let previous: Date | null = null;
  for (const message of messages) {
    const at = new Date(message.createdAt);
    if (!previous || at.getTime() - previous.getTime() >= GAP_MS || at.toDateString() !== previous.toDateString()) {
      items.push({ type: "stamp", key: `stamp-${message.id}`, label: stamp(at, now) });
    }
    items.push({ type: "message", key: message.id, message });
    previous = at;
  }
  return items;
}
