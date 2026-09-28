import { describe, expect, it, vi } from "vitest";
import { botHistory, loadConversation, request, retryConversation, roomHistory } from "./connection";
import { initialState } from "./state";
import { getState, update } from "./store";

describe("connection", () => {
  it("rejects a call made while Tjommie isn't connected, instead of throwing", async () => {
    await expect(request("bots.list", {})).rejects.toThrow("Tjommie isn't connected right now.");
  });

  it("rejects a chat's history while Tjommie isn't connected, as the room's does", async () => {
    await expect(botHistory("b1")()).rejects.toThrow("Tjommie isn't connected right now.");
    await expect(roomHistory()).rejects.toThrow("Tjommie isn't connected right now.");
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
});
