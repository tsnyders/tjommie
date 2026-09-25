import { describe, expect, it } from "vitest";
import { Approval, BotInput, BotPatch, ClientMessage, PROTOCOL_VERSION, RoutineInput, RoutinePatch, Settings, TEAM_ROOM_ID, methods } from "./index.ts";

const bot = { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" } as const;

describe("protocol", () => {
  it("is version 1", () => {
    expect(PROTOCOL_VERSION).toBe(1);
  });

  it("parses hello and request messages and rejects anything else", () => {
    expect(ClientMessage.parse({ type: "hello", protocol: 1, token: "t" }).type).toBe("hello");
    expect(ClientMessage.parse({ type: "req", id: 1, method: "bots.list", params: {} }).type).toBe("req");
    expect(() => ClientMessage.parse({ type: "nope" })).toThrow();
  });

  it("trims bot names and rejects blank ones", () => {
    expect(BotInput.parse({ ...bot, name: "  Writer " }).name).toBe("Writer");
    expect(() => BotInput.parse({ ...bot, name: "   " })).toThrow();
  });

  it("leaves unspecified fields out of a patch so they stay unchanged", () => {
    expect(BotPatch.parse({ autonomy: "full" })).toEqual({ autonomy: "full" });
  });

  it("applies chat.send defaults and rejects blank text", () => {
    expect(methods["chat.send"].params.parse({ botId: "b", text: " hi " })).toEqual({ botId: "b", text: "hi", files: [] });
    expect(() => methods["chat.send"].params.parse({ botId: "b", text: "  " })).toThrow();
  });
});

describe("protocol for bots working together", () => {
  it("names the Team room", () => {
    expect(TEAM_ROOM_ID).toBe("team");
  });

  it("accepts a weekday routine and rejects bad days and times", () => {
    const routine = { botId: "b", name: "Morning check", prompt: "Check pnet.co.za", days: [1, 2, 3, 4, 5], times: ["08:48", "12:47"] };
    expect(RoutineInput.parse(routine)).toEqual(routine);
    expect(() => RoutineInput.parse({ ...routine, days: [7] })).toThrow();
    expect(() => RoutineInput.parse({ ...routine, times: ["8:48"] })).toThrow();
    expect(() => RoutineInput.parse({ ...routine, times: ["24:00"] })).toThrow();
  });

  it("accepts real IANA time zones and rejects unknown ones", () => {
    const routine = { botId: "b", name: "Morning check", prompt: "Check pnet.co.za", days: [1], times: ["08:48"] };
    expect(RoutineInput.parse({ ...routine, timeZone: "Africa/Johannesburg" }).timeZone).toBe("Africa/Johannesburg");
    expect(() => RoutineInput.parse({ ...routine, timeZone: "Mars/Olympus" })).toThrow();
    expect(RoutinePatch.parse({ timeZone: "Africa/Johannesburg" })).toEqual({ timeZone: "Africa/Johannesburg" });
    expect(() => RoutinePatch.parse({ timeZone: "Mars/Olympus" })).toThrow();
  });

  it("leaves unspecified routine fields out of a patch", () => {
    expect(RoutinePatch.parse({ enabled: false })).toEqual({ enabled: false });
  });

  it("describes routine proposals as well as tool approvals", () => {
    const approval = { id: "a", botId: "b", kind: "routine", tool: "Routine", summary: "s", detail: "d", rule: null, payload: { name: "x" }, status: "pending", messageId: "m", createdAt: "t" };
    expect(Approval.parse(approval).kind).toBe("routine");
  });

  it("bounds the chain limit between 1 and 20", () => {
    expect(Settings.parse({ chainLimit: 5 })).toEqual({ chainLimit: 5 });
    expect(() => Settings.parse({ chainLimit: 0 })).toThrow();
    expect(() => Settings.parse({ chainLimit: 21 })).toThrow();
  });
});
