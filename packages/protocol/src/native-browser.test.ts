import { describe, expect, it } from "vitest";
import { NativeBrowserCommand, NativeBrowserMount, NativeBrowserStreamMessage, NativeBrowserUiCommand } from "./native-browser.ts";

describe("native browser trust boundaries", () => {
  it("accepts internal automation and cleanup but excludes both from renderer commands", () => {
    for (const command of [{ kind: "close" }, { kind: "delete_profile" }, { kind: "cancel_tools" }, { kind: "tool", name: "browser_snapshot", args: {} }]) {
      expect(NativeBrowserCommand.safeParse(command).success).toBe(true);
      expect(NativeBrowserUiCommand.safeParse(command).success).toBe(false);
    }
    const remote = { kind: "remote", controllerId: "trusted-connection", action: { kind: "input", generation: "one", pageId: "page", controlEpoch: 2, viewportRevision: 3, input: { kind: "text", text: "synthetic PIN" } } };
    expect(NativeBrowserCommand.safeParse(remote).success).toBe(true);
    expect(NativeBrowserUiCommand.safeParse(remote).success).toBe(false);
    expect(NativeBrowserCommand.safeParse({ ...remote, action: { ...remote.action, controlEpoch: undefined } }).success).toBe(false);
  });
  it("lets a remote controller turn the live stream on or off, but not the renderer", () => {
    const stream = { kind: "remote", controllerId: "rpc:one", action: { kind: "stream", on: true, maxFps: 30 } };
    expect(NativeBrowserCommand.safeParse(stream).success).toBe(true);
    expect(NativeBrowserCommand.safeParse({ ...stream, action: { kind: "stream", on: false } }).success).toBe(true);
    expect(NativeBrowserUiCommand.safeParse(stream.action).success).toBe(false);
    for (const action of [{ kind: "stream" }, { kind: "stream", on: true, maxFps: 0 }, { kind: "stream", on: true, maxFps: 61 }, { kind: "stream", on: true, extra: 1 }]) {
      expect(NativeBrowserCommand.safeParse({ ...stream, action }).success).toBe(false);
    }
  });
  it("accepts only a streamed frame or an end from the desktop host's own posts", () => {
    const frame = { data: "image", mime: "image/jpeg", width: 1280, height: 800, sequence: 4, streamId: "stream" };
    expect(NativeBrowserStreamMessage.safeParse({ kind: "stream_frame", botId: "b1", frame }).success).toBe(true);
    for (const reason of ["private", "closed", "failed"]) expect(NativeBrowserStreamMessage.safeParse({ kind: "stream_end", botId: "b1", reason }).success).toBe(true);
    const { streamId: _streamId, ...unnamed } = frame;
    for (const message of [
      { kind: "stream_frame", botId: "b1", frame: unnamed }, { kind: "stream_frame", botId: "", frame }, { kind: "stream_frame", botId: "b1", frame, extra: 1 },
      { kind: "stream_end", botId: "b1", reason: "bored" }, { kind: "stream_end", botId: "b1" }, { kind: "frame", botId: "b1", frame },
    ]) expect(NativeBrowserStreamMessage.safeParse(message).success).toBe(false);
  });
  it("requires a session and page identity for page mutations", () => {
    expect(NativeBrowserUiCommand.safeParse({ kind: "navigate", url: "https://example.test" }).success).toBe(false);
    expect(NativeBrowserUiCommand.safeParse({ kind: "navigate", generation: "generation", pageId: "page", url: "https://example.test" }).success).toBe(true);
  });
  it("distinguishes a temporary overlay from leaving the browser", () => {
    const mount = { visible: true, bounds: { x: 0, y: 44, width: 640, height: 480 } };
    expect(NativeBrowserMount.safeParse({ ...mount, occluded: true }).success).toBe(true);
    expect(NativeBrowserMount.safeParse({ ...mount, occluded: "true" }).success).toBe(false);
  });
  it("rejects extra privileged keys, nonfinite bounds, negative bounds and oversized tools", () => {
    expect(NativeBrowserUiCommand.safeParse({ kind: "state", script: "process.exit()" }).success).toBe(false);
    for (const x of [NaN, Infinity, -1, 40000]) expect(NativeBrowserMount.safeParse({ visible: true, bounds: { x, y: 44, width: 100, height: 100 } }).success).toBe(false);
    expect(NativeBrowserCommand.safeParse({ kind: "tool", name: "browser_evaluate", args: { expression: "x".repeat(1_000_001) } }).success).toBe(false);
  });
});
