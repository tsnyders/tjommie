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
});
