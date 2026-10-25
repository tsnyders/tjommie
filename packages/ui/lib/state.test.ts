import type { ClientEvent } from "@tjommie/client";
import { TEAM_ROOM_ID, type AccountInfo, type Bot, type Message, type Room, type Routine, type RoutineRun } from "@tjommie/protocol";
import { describe, expect, it, vi } from "vitest";
import { loadConversation } from "./connection";
import { LINK, addOlder, applyEvent, catchUp, connected, finishLoading, initialState, roomBots, startLoading, type AppState, type MessageEvent } from "./state";
import { getState, update } from "./store";

const bot = (id: string, createdAt: string): Bot => ({
  id,
  name: id,
  color: "blue",
  shape: "circle",
  instructions: "",
  provider: "claude",
  model: "sonnet",
  autonomy: "folder",
  homeFolder: `C:\\bots\\${id}`,
  projectFolder: null,
  browser: false,
  paused: false,
  canMessage: [],
  state: "idle",
  setupReason: null,
  limitedUntil: null,
  createdAt,
});
const message = (id: string, extra: Partial<Message> = {}): Message => ({
  id,
  conversationId: "b1",
  author: "b1",
  kind: "text",
  body: "",
  data: null,
  status: "done",
  createdAt: "2026-09-27T09:00:00.000Z",
  ...extra,
});
const routine = (id: string, botId: string): Routine => ({
  id,
  botId,
  name: "Morning check",
  prompt: "Check pnet.co.za",
  days: [1, 2, 3, 4, 5],
  times: ["08:48"],
  timeZone: "Africa/Johannesburg",
  timeLimitMinutes: null,
  enabled: true,
  nextRunAt: null,
  createdAt: "2026-09-27T08:00:00.000Z",
});
const run = (id: string, routineId: string, status: RoutineRun["status"]): ClientEvent => ({
  type: "event",
  event: "routine.run",
  data: { id, botId: "b1", routineId, dueAt: "2026-09-27T08:48:00.000Z", startedAt: null, endedAt: null, status, note: null },
});
const everyone = (botIds: string[]): Room => ({ id: TEAM_ROOM_ID, name: "Everyone", botIds, everyone: true, createdAt: null });
const room = (id: string, botIds: string[], createdAt: string): Room => ({ id, name: id, botIds, everyone: false, createdAt });
const added = (data: Message): MessageEvent => ({ type: "event", event: "message.added", data });
const updated = (data: Message): MessageEvent => ({ type: "event", event: "message.updated", data });
const delta = (id: string, text: string): MessageEvent => ({ type: "event", event: "message.delta", data: { id, conversationId: "b1", delta: text } });
const ready = (messages: Message[]): AppState => finishLoading(startLoading(initialState, "b1"), "b1", { messages, hasMore: false });

it("freezes the reconnect timing contract", () => {
  expect(LINK).toEqual({ pingForegroundMs: 20_000, pingBackgroundMs: 60_000, pongTimeoutMs: 10_000,
    lanWindowMs: 1_000, backoffMs: [0, 1_000, 2_000, 5_000, 10_000] });
});

describe("app state", () => {
  it("holds the accounts: unknown until listed, replaced whole by each change, and listed again on a new connection", () => {
    const list = [{ id: "a1", label: "Claude (Personal)" }] as AccountInfo[];
    expect(initialState.accounts).toBeNull();
    let state = connected(initialState, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [], accounts: list });
    expect(state.accounts).toBe(list);
    const next = [{ id: "a2", label: "Work" }] as AccountInfo[];
    state = applyEvent(state, { type: "event", event: "accounts.changed", data: next });
    expect(state.accounts).toBe(next);
    expect(connected(state, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [] }).accounts).toBeNull(); // an older daemon has none
  });

  it("keeps bots in creation order and forgets a deleted bot's chat and routines", () => {
    let state = connected(initialState, { bots: [bot("b2", "2026-09-27T10:00:00.000Z"), bot("b1", "2026-09-27T09:00:00.000Z")], settings: { chainLimit: 5, relayUrl: null }, routines: [routine("r1", "b1")], rooms: [] });
    expect(state.bots.map((b) => b.id)).toEqual(["b1", "b2"]);
    state = applyEvent(state, { type: "event", event: "bot.updated", data: { ...bot("b1", "2026-09-27T09:00:00.000Z"), state: "working" } });
    expect(state.bots[0].state).toBe("working");
    state = startLoading(state, "b1");
    state = applyEvent(state, { type: "event", event: "bot.deleted", data: { id: "b1" } });
    expect(state.bots.map((b) => b.id)).toEqual(["b2"]);
    expect(state.routines).toEqual([]);
    expect(state.conversations.b1).toBeUndefined();
  });

  it("keeps Everyone first and the other rooms oldest first, updates and removes them, and lists them again on a new connection", () => {
    const growth = room("r2", ["b1"], "2026-10-25T10:00:00.000Z");
    let state = connected(initialState, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [growth, everyone(["b1"])] });
    expect(state.rooms.map((each) => each.id)).toEqual([TEAM_ROOM_ID, "r2"]);
    state = applyEvent(state, { type: "event", event: "room.updated", data: room("r1", ["b1"], "2026-10-25T09:00:00.000Z") });
    state = applyEvent(state, { type: "event", event: "room.updated", data: { ...growth, name: "Growth" } });
    expect(state.rooms.map((each) => each.name)).toEqual(["Everyone", "r1", "Growth"]);
    state = startLoading(state, "r1");
    state = applyEvent(state, { type: "event", event: "room.deleted", data: { id: "r1" } });
    expect(state.rooms.map((each) => each.id)).toEqual([TEAM_ROOM_ID, "r2"]);
    expect(state.conversations.r1).toBeUndefined();
    expect(connected(state, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [everyone([])] }).rooms.map((each) => each.id)).toEqual([TEAM_ROOM_ID]);
  });

  it("takes a deleted bot out of every room", () => {
    const bots = [bot("b1", "2026-09-27T09:00:00.000Z"), bot("b2", "2026-09-27T10:00:00.000Z")];
    const rooms = [everyone(["b1", "b2"]), room("r1", ["b1", "b2"], "2026-10-25T09:00:00.000Z"), room("r2", ["b2"], "2026-10-25T10:00:00.000Z")];
    let state = connected(initialState, { bots, settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms });
    state = applyEvent(state, { type: "event", event: "bot.deleted", data: { id: "b1" } });
    expect(state.rooms.map((each) => each.botIds)).toEqual([["b2"], ["b2"], ["b2"]]);
  });

  it("has every bot in Everyone, even one made after the rooms were listed, and only its members in another room", () => {
    let state = connected(initialState, { bots: [bot("b1", "2026-09-27T09:00:00.000Z")], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [everyone(["b1"]), room("r1", ["b1"], "2026-10-25T09:00:00.000Z")] });
    // The new bot comes with no room.updated: the daemon sends those for the other rooms only.
    state = applyEvent(state, { type: "event", event: "bot.updated", data: bot("b2", "2026-09-27T10:00:00.000Z") });
    const [all, growth] = state.rooms;
    expect(roomBots(all, state.bots).map((each) => each.id)).toEqual(["b1", "b2"]);
    expect(roomBots(growth, state.bots).map((each) => each.id)).toEqual(["b1"]);
  });

  it("applies new, updated and streaming messages to an open chat, and ignores chats that aren't open", () => {
    let state = ready([message("m1", { body: "Hi" })]);
    state = applyEvent(state, added(message("m2", { status: "streaming" })));
    state = applyEvent(state, delta("m2", "Wor"));
    state = applyEvent(state, delta("m2", "king"));
    expect(state.conversations.b1.messages.map((m) => m.body)).toEqual(["Hi", "Working"]);
    state = applyEvent(state, updated(message("m2", { body: "Working on it.", status: "done" })));
    expect(state.conversations.b1.messages[1]).toMatchObject({ body: "Working on it.", status: "done" });
    const other = applyEvent(state, added(message("x1", { conversationId: "b9" })));
    expect(other).toBe(state);
  });

  it("rebuilds a reply that was streaming while its chat opened (opened mid-stream)", () => {
    let state = startLoading(initialState, "b1");
    state = applyEvent(state, delta("m1", "Hel"));
    state = applyEvent(state, delta("m1", "lo"));
    state = finishLoading(state, "b1", { messages: [message("m1", { status: "streaming", body: "" })], hasMore: false });
    expect(state.conversations.b1.messages[0].body).toBe("Hello");
    state = applyEvent(state, delta("m1", " there"));
    expect(state.conversations.b1.messages[0].body).toBe("Hello there");
    state = applyEvent(state, updated(message("m1", { status: "done", body: "Hello there!" })));
    expect(state.conversations.b1.messages[0]).toMatchObject({ status: "done", body: "Hello there!" });
  });

  it("shows a message that arrived while its chat was loading exactly once", () => {
    let state = startLoading(initialState, "b1");
    state = applyEvent(state, added(message("m2", { body: "New" })));
    state = finishLoading(state, "b1", { messages: [message("m1"), message("m2", { body: "New" })], hasMore: true });
    expect(state.conversations.b1.messages.map((m) => m.id)).toEqual(["m1", "m2"]);
    expect(state.conversations.b1.hasMore).toBe(true);
  });

  it("adds older pages above without duplicates", () => {
    let state = ready([message("m3"), message("m4")]);
    state = addOlder(state, "b1", { messages: [message("m1"), message("m2"), message("m3")], hasMore: false });
    expect(state.conversations.b1.messages.map((m) => m.id)).toEqual(["m1", "m2", "m3", "m4"]);
  });

  it("keeps loaded chats on a new connection, so nothing blanks, and forgets ones still loading so their screens load them again", () => {
    let state = startLoading(ready([message("m1"), message("m2")]), "b2");
    state = { ...state, connection: { status: "reconnecting" } };
    state = connected(state, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [] });
    expect(state.connection).toEqual({ status: "ready" });
    expect(state.conversations.b1.messages.map((m) => m.id)).toEqual(["m1", "m2"]);
    expect(state.conversations.b2).toBeUndefined();
  });

  it("lists the routine failures waiting to be shown on each connection", () => {
    const failure = { kind: "routine_failed" as const, botId: "b1", routineId: "r1", runId: "u1", title: "Morning check failed", body: "It ran out of time." };
    expect(initialState.notifications).toEqual([]);
    const state = connected(initialState, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [], notifications: [failure] });
    expect(state.notifications).toEqual([failure]);
    expect(connected(state, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [] }).notifications).toEqual([]);
  });

  it("keeps each routine's newest run event, so an open routine editor can fetch its runs again", () => {
    let state = applyEvent(initialState, run("u1", "r1", "running"));
    const running = state.latestRuns.r1;
    expect(running).toMatchObject({ id: "u1", status: "running" });
    state = applyEvent(state, run("u2", "r2", "queued"));
    expect(state.latestRuns.r1).toBe(running); // another routine's run leaves this one alone
    state = applyEvent(state, run("u1", "r1", "done"));
    expect(state.latestRuns.r1).toMatchObject({ id: "u1", status: "done" });
    expect(connected(state, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [] }).latestRuns).toEqual({});
  });

  it("records ended sign-in attempts, so an old attempt ending never reads as the new one ending", () => {
    const ended = (provider: "claude" | "codex", attempt: string): ClientEvent => ({ type: "event", event: "setup.signInEnded", data: { provider, attempt } });
    expect(initialState.signInsEnded).toEqual([]);
    let state = applyEvent(initialState, ended("codex", "a"));
    expect(state.signInsEnded.includes("b")).toBe(false); // a card signing in as "b" keeps waiting
    state = applyEvent(state, ended("codex", "b"));
    expect(state.signInsEnded).toEqual(["a", "b"]);
    // A new connection keeps them, so an attempt that ended before it still reads as ended.
    expect(connected(state, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [] }).signInsEnded).toEqual(["a", "b"]);
    for (let i = 0; i < 30; i++) state = applyEvent(state, ended("claude", `n${i}`));
    expect(state.signInsEnded).toHaveLength(20);
    expect(state.signInsEnded.at(-1)).toBe("n29");
  });

  describe("catching up after a reconnect", () => {
    it("replaces from the newest page's oldest message onward when that message is loaded, keeping the older pages", () => {
      let state = ready([message("m1"), message("m2"), message("m3", { body: "Working" })]);
      state = catchUp(state, "b1", { messages: [message("m2"), message("m3", { body: "Done." }), message("m4")], hasMore: true });
      expect(state.conversations.b1.messages.map((m) => [m.id, m.body])).toEqual([["m1", ""], ["m2", ""], ["m3", "Done."], ["m4", ""]]);
      expect(state.conversations.b1.hasMore).toBe(false); // the older pages were already loaded
    });

    it("replaces the chat with the newest page, with more to load, when more than a page arrived while away", () => {
      let state = ready([message("m1"), message("m2")]);
      state = catchUp(state, "b1", { messages: [message("m5"), message("m6")], hasMore: true });
      expect(state.conversations.b1.messages.map((m) => m.id)).toEqual(["m5", "m6"]);
      expect(state.conversations.b1.hasMore).toBe(true);
    });

    it("replays what arrived while the page was being fetched, so a new message and its streamed text survive", () => {
      const during = [added(message("m2", { status: "streaming" })), delta("m2", "Hel"), delta("m2", "lo")];
      let state = during.reduce(applyEvent, ready([message("m1")])); // shown live meanwhile
      state = catchUp(state, "b1", { messages: [message("m1")], hasMore: false }, during); // the page was read before m2 started
      expect(state.conversations.b1.messages.map((m) => [m.id, m.body])).toEqual([["m1", ""], ["m2", "Hello"]]);
    });

    it("leaves a chat that isn't loaded alone", () => {
      const state = startLoading(initialState, "b1");
      expect(catchUp(state, "b1", { messages: [message("m1")], hasMore: false })).toBe(state);
      expect(catchUp(initialState, "b9", { messages: [], hasMore: false })).toBe(initialState);
    });
  });

  it("keeps a failed history load until a new connection clears it", async () => {
    update(() => initialState);
    const fetchPage = vi.fn(async () => {
      throw new Error("History unavailable");
    });

    await loadConversation("b1", fetchPage);
    expect(getState().conversations.b1.status).toBe("failed");
    await loadConversation("b1", fetchPage);
    expect(fetchPage).toHaveBeenCalledTimes(1);

    update((state) => connected(state, { bots: [], settings: { chainLimit: 5, relayUrl: null }, routines: [], rooms: [] }));
    expect(getState().conversations.b1).toBeUndefined();
    await loadConversation("b1", fetchPage);
    expect(fetchPage).toHaveBeenCalledTimes(2);
    update(() => initialState);
  });
});
