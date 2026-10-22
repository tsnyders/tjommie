import { describe, expect, it } from "vitest";
import { NativeBrowserCommand, NativeBrowserMount, NativeBrowserUiCommand } from "./native-browser.ts";

describe("native browser trust boundaries", () => {
  it("accepts internal automation and cleanup but excludes both from renderer commands", () => {
    for (const command of [{ kind: "close" }, { kind: "delete_profile" }, { kind: "cancel_tools" }, { kind: "tool", name: "browser_snapshot", args: {} }]) {
      expect(NativeBrowserCommand.safeParse(command).success).toBe(true);
      expect(NativeBrowserUiCommand.safeParse(command).success).toBe(false);
    }
  });
  it("requires a session and page identity for page mutations", () => {
    expect(NativeBrowserUiCommand.safeParse({ kind: "navigate", url: "https://example.test" }).success).toBe(false);
    expect(NativeBrowserUiCommand.safeParse({ kind: "navigate", generation: "generation", pageId: "page", url: "https://example.test" }).success).toBe(true);
  });
  it("rejects extra privileged keys, nonfinite bounds, negative bounds and oversized tools", () => {
    expect(NativeBrowserUiCommand.safeParse({ kind: "state", script: "process.exit()" }).success).toBe(false);
    for (const x of [NaN, Infinity, -1, 40000]) expect(NativeBrowserMount.safeParse({ visible: true, bounds: { x, y: 44, width: 100, height: 100 } }).success).toBe(false);
    expect(NativeBrowserCommand.safeParse({ kind: "tool", name: "browser_evaluate", args: { expression: "x".repeat(1_000_001) } }).success).toBe(false);
  });
});
