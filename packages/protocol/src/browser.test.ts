import { describe, expect, it } from "vitest";
import { browserMethods } from "./browser.ts";

const target = { botId: "b1", generation: "generation", pageId: "page" };
describe("browser wire bounds", () => {
  it("requires a page and session generation for input", () => {
    expect(browserMethods["browser.input"].params.safeParse({ botId: "b1", input: { kind: "text", text: "hello" } }).success).toBe(false);
    expect(browserMethods["browser.input"].params.parse({ ...target, input: { kind: "key", action: "down", key: "Control" } })).toMatchObject(target);
  });
  it("rejects nonfinite and oversized coordinate and viewport payloads", () => {
    for (const x of [-1, Infinity, NaN, 5000]) expect(browserMethods["browser.input"].params.safeParse({ ...target, input: { kind: "mouse", action: "down", x, y: 0 } }).success).toBe(false);
    expect(browserMethods["browser.resize"].params.safeParse({ ...target, width: 100_000, height: 900 }).success).toBe(false);
    expect(browserMethods["browser.input"].params.safeParse({ ...target, input: { kind: "text", text: "x".repeat(100_001) } }).success).toBe(false);
  });
  it("keeps frame cursors, CSS viewport dimensions and input layout fences on the wire", () => {
    expect(browserMethods["browser.frame"].params.parse({ botId: "b1", after: { streamId: "stream", sequence: 3 } })).toHaveProperty("after", { streamId: "stream", sequence: 3 });
    expect(browserMethods["browser.input"].params.parse({ ...target, viewportRevision: 4, input: { kind: "text", text: "hello" } })).toHaveProperty("viewportRevision", 4);
    expect(browserMethods["browser.input"].params.safeParse({ ...target, viewportRevision: -1, input: { kind: "text", text: "hello" } }).success).toBe(false);
    const result = browserMethods["browser.frame"].result.parse({ state: { botId: "b1", generation: "generation", control: "bot", activePageId: "page", tabs: [], dialog: null, revision: 7, viewportRevision: 4, busy: true }, frame: { data: "image", mime: "image/jpeg", width: 1280, height: 800, viewportWidth: 2560, viewportHeight: 1600, pageId: "page", viewportRevision: 4, streamId: "stream", sequence: 3 } });
    expect(result.frame).toMatchObject({ width: 1280, viewportWidth: 2560, viewportRevision: 4 });
    expect(result.state).toMatchObject({ revision: 7, busy: true });
  });
});
