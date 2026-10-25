import { describe, expect, it } from "vitest";
import { Approval, Bot, BotInput, BotPatch, ClientMessage, ErrorCode, PROTOCOL_VERSION, ProviderSetup, RoomInput, RoomPatch, RoutineInput, RoutinePatch, Settings, TEAM_ROOM_ID, events, methods } from "./index.ts";

const bot = { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" } as const;

describe("protocol", () => {
  it("requires a typed Codex sandbox status and a setup reason on bots", () => {
    const base = { installed: true, version: "0.155.1", signedIn: true, account: null, plan: null, error: null };
    expect(ProviderSetup.parse({ ...base, provider: "codex", sandbox: "ready" })).toMatchObject({ sandbox: "ready" });
    for (const sandbox of ["notConfigured", "updateRequired", "unavailable"]) {
      expect(ProviderSetup.parse({ ...base, provider: "codex", sandbox })).toMatchObject({ sandbox });
    }
    expect(ProviderSetup.safeParse({ ...base, provider: "codex" }).success).toBe(false);
    expect(ProviderSetup.safeParse({ ...base, provider: "codex", sandbox: "unknown" }).success).toBe(false);
    const minimalBot = { id: "b", ...bot, autonomy: "folder", homeFolder: "C:\\b", projectFolder: null, browser: false,
      paused: false, canMessage: [], state: "needs_setup", setupReason: "Set up Codex's sandbox", limitedUntil: null, createdAt: "t" };
    expect(Bot.parse(minimalBot).setupReason).toBe("Set up Codex's sandbox");
    expect(Bot.safeParse({ ...minimalBot, setupReason: undefined }).success).toBe(false);
    expect(events["bot.updated"].parse(minimalBot).setupReason).toBe("Set up Codex's sandbox");
    expect(events["setup.changed"].parse([{ ...base, provider: "codex", sandbox: "ready" }])).toHaveLength(1);
  });

  it("types the one-time Codex sandbox setup call", () => {
    expect(methods["setup.prepareCodexSandbox"].params.parse({})).toEqual({});
    const status = { provider: "codex", installed: true, version: "0.155.1", signedIn: true, account: null, plan: null, error: null, sandbox: "ready" };
    expect(methods["setup.prepareCodexSandbox"].result.parse(status)).toEqual(status);
  });
  it("is version 1", () => {
    expect(PROTOCOL_VERSION).toBe(1);
  });

  it("lists every daemon error code and rejects unknown codes", () => {
    const codes = ["bad_params", "not_found", "busy", "name_taken", "already_answered", "bad_state", "not_installed", "no_account", "unknown_method", "internal"];
    expect(ErrorCode.options).toEqual(codes);
    for (const code of codes) expect(ErrorCode.parse(code)).toBe(code);
    expect(ErrorCode.safeParse("unexpected").success).toBe(false);
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

  it("defaults room calls to the Team room for older callers", () => {
    expect(methods["room.history"].params.parse({})).toEqual({ limit: 50, roomId: TEAM_ROOM_ID });
    expect(methods["room.post"].params.parse({ text: " hello " })).toEqual({ roomId: TEAM_ROOM_ID, text: "hello" });
  });

  it("preserves an optional bot id when selecting a failed room turn to retry", () => {
    expect(methods["chat.retry"].params.parse({ messageId: "m" })).toEqual({ messageId: "m" });
    expect(methods["chat.retry"].params.parse({ messageId: "m", botId: "b" })).toEqual({ messageId: "m", botId: "b" });
  });

  it("requires an explicit hasMore flag on history responses", () => {
    for (const method of ["chat.history", "room.history"] as const) {
      expect(methods[method].result.parse({ messages: [], hasMore: false })).toEqual({ messages: [], hasMore: false });
      expect(methods[method].result.safeParse([]).success).toBe(false);
      expect(methods[method].result.safeParse({ messages: [] }).success).toBe(false);
    }
    const exchange = { id: "e", fromBotId: "a", toBotId: "b", status: "waiting", request: "Help", reply: null, createdAt: "t", repliedAt: null };
    expect(methods["exchanges.get"].result.parse({ exchange, messages: [], hasMore: false }).hasMore).toBe(false);
    expect(methods["exchanges.get"].result.safeParse({ exchange, messages: [] }).success).toBe(false);
  });
});

describe("protocol for bots working together", () => {
  it("types durable routine notifications and their explicit acknowledgement", () => {
    const notification = { kind: "routine_failed", botId: "b", routineId: "r", runId: "run", title: "Failed", body: "Restarted" };
    expect(methods["notifications.pending"].params.parse({})).toEqual({});
    expect(methods["notifications.pending"].result.parse([notification])).toEqual([notification]);
    expect(methods["notifications.pending"].result.safeParse([{ ...notification, runId: undefined }]).success).toBe(false);
    expect(methods["notifications.ack"].params.parse({ runId: "run" })).toEqual({ runId: "run" });
    expect(methods["notifications.ack"].params.safeParse({ runId: "" }).success).toBe(false);
    expect(methods["notifications.ack"].result.parse({})).toEqual({});
  });

  it("names the Team room", () => {
    expect(TEAM_ROOM_ID).toBe("team");
  });

  it("validates room inputs and permits an empty patch", () => {
    expect(RoomInput.parse({ name: "  Research  ", botIds: ["bot"] })).toEqual({ name: "Research", botIds: ["bot"] });
    expect(() => RoomInput.parse({ name: " ", botIds: ["bot"] })).toThrow();
    expect(() => RoomInput.parse({ name: "x".repeat(61), botIds: ["bot"] })).toThrow();
    expect(() => RoomInput.parse({ name: "Research", botIds: [] })).toThrow();
    expect(RoomPatch.parse({})).toEqual({});
  });

  it("describes room methods and events", () => {
    const room = { id: "research", name: "Research", botIds: ["writer"], everyone: false, createdAt: "created" };
    const input = { name: room.name, botIds: room.botIds };
    expect(methods["rooms.list"].params.parse({})).toEqual({});
    expect(methods["rooms.list"].result.parse([room])).toEqual([room]);
    expect(methods["rooms.create"].params.parse(input)).toEqual(input);
    expect(methods["rooms.create"].result.parse(room)).toEqual(room);
    expect(methods["rooms.update"].params.parse({ id: room.id, patch: {} })).toEqual({ id: room.id, patch: {} });
    expect(methods["rooms.update"].result.parse(room)).toEqual(room);
    expect(methods["rooms.delete"].params.parse({ id: room.id })).toEqual({ id: room.id });
    expect(methods["rooms.delete"].result.parse({})).toEqual({});
    expect(events["room.updated"].parse(room)).toEqual(room);
    expect(events["room.deleted"].parse({ id: room.id })).toEqual({ id: room.id });
  });

  it("lets callers list all routines or one bot's routines", () => {
    expect(methods["routines.list"].params.parse({})).toEqual({});
    expect(methods["routines.list"].params.parse({ botId: "b" })).toEqual({ botId: "b" });
  });

  it("describes notification kinds and their optional targets", () => {
    const notification = { botId: "b", title: "Needs attention", body: "Open this bot" };
    expect(events.notify.safeParse(notification).success).toBe(false);
    for (const target of [
      { kind: "approval", approvalId: "a" },
      { kind: "limit" },
      { kind: "routine_failed", routineId: "r", runId: "run" },
    ]) expect(events.notify.parse({ ...notification, ...target })).toEqual({ ...notification, ...target });
    for (const kind of ["approval", "limit", "routine_failed"]) {
      expect(events.notify.parse({ ...notification, kind })).toEqual({ ...notification, kind });
    }
    expect(events.notify.safeParse({ ...notification, kind: "other" }).success).toBe(false);
    expect(events.notify.safeParse({ ...notification, kind: "approval", approvalId: 42 }).success).toBe(false);
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
