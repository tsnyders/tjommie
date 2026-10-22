import { describe, expect, it, vi } from "vitest";
import { nativeBrowserHandlers } from "./native-browser-handlers.ts";

const state = { botId: "one", generation: "g", control: "bot", activePageId: "p", tabs: [{ id: "p", title: "Page", url: "https://example.com/" }], dialog: null };
describe("native browser compatibility handlers", () => {
  it("uses the native host for state, controls and navigation without a frame stream", async () => {
    const request = vi.fn(async () => state);
    const handlers = nativeBrowserHandlers(request);
    expect(await handlers["browser.frame"]?.({ botId: "one" })).toEqual({ state, frame: null });
    await handlers["browser.navigate"]?.({ botId: "one", generation: "g", pageId: "p", url: "https://example.com/next" });
    expect(request).toHaveBeenLastCalledWith("one", { kind: "navigate", generation: "g", pageId: "p", url: "https://example.com/next" });
    expect(() => handlers["browser.input"]?.({ botId: "one", generation: "g", pageId: "p", input: { kind: "text", text: "no JPEG input" } })).toThrow("directly");
  });
  it("rejects malformed native state instead of passing it to old clients", async () => {
    const handlers = nativeBrowserHandlers(async () => ({ bad: "state" }));
    await expect(handlers["browser.open"]?.({ botId: "one" })).rejects.toThrow();
  });
});
