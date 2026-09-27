import type { Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { timeline } from "./timeline";

const at = (hour: number, minute: number, day = 27) => new Date(2026, 8, day, hour, minute).toISOString();
const message = (id: string, createdAt: string): Message => ({ id, conversationId: "b1", author: "b1", kind: "text", body: id, data: null, status: "done", createdAt });

describe("timeline", () => {
  it("labels the first message, gaps of 15 minutes or more, and each new day", () => {
    const now = new Date(2026, 8, 27, 12, 0);
    const items = timeline([message("a", at(9, 14, 26)), message("b", at(9, 14)), message("c", at(9, 20)), message("d", at(9, 40))], now);
    expect(items.map((item) => (item.type === "stamp" ? item.label : item.message.id))).toEqual(["Yesterday 9:14 AM", "a", "Today 9:14 AM", "b", "c", "Today 9:40 AM", "d"]);
  });
});
