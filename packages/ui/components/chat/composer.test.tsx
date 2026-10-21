import type { Bot, Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { canStop } from "./composer";

const card = (id: string, data: Record<string, unknown>): Message => ({
  id,
  conversationId: "b1",
  author: "b1",
  kind: "approval",
  body: "Write hello.txt",
  data: { approvalId: `a-${id}`, tool: "Write", detail: "hello.txt", ...data },
  status: "done",
  createdAt: "2026-09-27T07:14:00.000Z",
});
const tool = card("m1", { status: "pending", rule: null });
const routine = card("m2", { status: "pending", kind: "routine" });
const answered = card("m3", { status: "allowed", rule: null });
const bot = (state: Bot["state"]): Pick<Bot, "state"> => ({ state });

describe("canStop", () => {
  it("offers Stop while the bot works, or waits on a tool card that holds its turn", () => {
    expect(canStop(bot("working"), [])).toBe(true);
    expect(canStop(bot("waiting"), [answered, tool])).toBe(true);
    expect(canStop(bot("waiting"), [{ ...tool, data: { ...tool.data, kind: "tool" } }])).toBe(true);
  });

  it("offers no Stop for a bot that only waits on a routine it proposed, or that isn't running", () => {
    expect(canStop(bot("waiting"), [routine, answered])).toBe(false);
    expect(canStop(bot("waiting"), [])).toBe(false);
    expect(canStop(bot("idle"), [tool])).toBe(false);
    expect(canStop(bot("paused"), [tool])).toBe(false);
  });
});
