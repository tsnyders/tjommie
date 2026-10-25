import { describe, expect, it, vi } from "vitest";
import { botHistory, closeConversation, loadConversation, request, retryConversation, roomHistory } from "./connection";
import { applyEvent, initialState } from "./state";
import { getState, update } from "./store";

describe("connection", () => {
  it("rejects a call made while Tjommie isn't connected, instead of throwing", async () => {
    await expect(request("bots.list", {})).rejects.toThrow("Tjommie isn't connected right now.");
  });

  it("rejects a chat's history while Tjommie isn't connected, as the room's does", async () => {
    await expect(botHistory("b1")()).rejects.toThrow("Tjommie isn't connected right now.");
    await expect(roomHistory("team")()).rejects.toThrow("Tjommie isn't connected right now.");
  });

  it("loads a chat whose history failed to load again on Retry", async () => {
    update(() => initialState);
    const fetchPage = vi.fn(async () => ({ messages: [], hasMore: true }));
    fetchPage.mockRejectedValueOnce(new Error("History unavailable"));

    await loadConversation("b1", fetchPage);
    expect(getState().conversations.b1.status).toBe("failed");
    await retryConversation("b1", fetchPage);
    expect(getState().conversations.b1).toMatchObject({ status: "ready", hasMore: true });
    expect(fetchPage).toHaveBeenCalledTimes(2);
    update(() => initialState);
  });

  it("forgets a closed exchange, so its messages stop arriving and a reopen loads it again", async () => {
    update(() => initialState);
    const fetchPage = vi.fn(async () => ({ messages: [], hasMore: false }));
    await loadConversation("x1", fetchPage);
    closeConversation("x1");
    expect(getState().conversations.x1).toBeUndefined();
    const reply = { id: "m1", conversationId: "x1", author: "b2", kind: "text" as const, body: "MANGO", data: null, status: "done" as const, createdAt: "2026-09-27T09:00:00.000Z" };
    update((state) => applyEvent(state, { type: "event", event: "message.added", data: reply }));
    expect(getState().conversations.x1).toBeUndefined();
    await loadConversation("x1", fetchPage);
    expect(fetchPage).toHaveBeenCalledTimes(2);
    update(() => initialState);
  });
});
