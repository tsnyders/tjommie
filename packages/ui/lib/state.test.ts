import type { ClientEvent } from "@tjommie/client";
import type { Bot, Message, Routine } from "@tjommie/protocol";
import { describe, expect, it, vi } from "vitest";
import { loadConversation } from "./connection";
import { addOlder, applyEvent, connected, finishLoading, initialState, startLoading, type AppState } from "./state";
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
const added = (data: Message): ClientEvent => ({ type: "event", event: "message.added", data });
const updated = (data: Message): ClientEvent => ({ type: "event", event: "message.updated", data });
const delta = (id: string, text: string): ClientEvent => ({ type: "event", event: "message.delta", data: { id, conversationId: "b1", delta: text } });
const ready = (messages: Message[]): AppState => finishLoading(startLoading(initialState, "b1"), "b1", { messages, hasMore: false });

describe("app state", () => {
  it("keeps bots in creation order and forgets a deleted bot's chat and routines", () => {
    let state = connected(initialState, { bots: [bot("b2", "2026-09-27T10:00:00.000Z"), bot("b1", "2026-09-27T09:00:00.000Z")], settings: { chainLimit: 5 }, routines: [routine("r1", "b1")] });
    expect(state.bots.map((b) => b.id)).toEqual(["b1", "b2"]);
    state = applyEvent(state, { type: "event", event: "bot.updated", data: { ...bot("b1", "2026-09-27T09:00:00.000Z"), state: "working" } });
    expect(state.bots[0].state).toBe("working");
    state = startLoading(state, "b1");
    state = applyEvent(state, { type: "event", event: "bot.deleted", data: { id: "b1" } });
    expect(state.bots.map((b) => b.id)).toEqual(["b2"]);
    expect(state.routines).toEqual([]);
    expect(state.conversations.b1).toBeUndefined();
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

  it("adds older pages above without duplicates, and starts over on a new connection", () => {
    let state = ready([message("m3"), message("m4")]);
    state = addOlder(state, "b1", { messages: [message("m1"), message("m2"), message("m3")], hasMore: false });
    expect(state.conversations.b1.messages.map((m) => m.id)).toEqual(["m1", "m2", "m3", "m4"]);
    state = connected(state, { bots: [], settings: { chainLimit: 5 }, routines: [] });
    expect(state.conversations).toEqual({});
    expect(state.connection).toEqual({ status: "ready" });
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

    update((state) => connected(state, { bots: [], settings: { chainLimit: 5 }, routines: [] }));
    expect(getState().conversations.b1).toBeUndefined();
    await loadConversation("b1", fetchPage);
    expect(fetchPage).toHaveBeenCalledTimes(2);
    update(() => initialState);
  });
});
