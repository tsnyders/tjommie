import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vitest";
import { captureNativeBrowserFrame } from "./native-browser-frame.ts";
import { NativeBrowserAccess } from "./native-browser-access.ts";

describe("hidden native frame lifecycle", () => {
  it("starts painting, captures one bounded image and always stops", async () => {
    const debuggerApi = Object.assign(new EventEmitter(), { sendCommand: vi.fn(async (method: string) => method === "Page.captureScreenshot" ? { data: Buffer.alloc(100).toString("base64") } : {}) });
    expect((await captureNativeBrowserFrame(debuggerApi, () => {})).length).toBeGreaterThan(100);
    expect(debuggerApi.sendCommand.mock.calls.map(([method]) => method)).toEqual(["Page.startScreencast", "Page.captureScreenshot", "Page.stopScreencast"]);
    expect(debuggerApi.listenerCount("message")).toBe(0);
  });
  it("discards an in-flight image when privacy starts and never returns a queued image", async () => {
    const access = new NativeBrowserAccess(); access.take("person"); const ticket = access.ticket();
    const debuggerApi = Object.assign(new EventEmitter(), { sendCommand: vi.fn(async (method: string) => {
      if (method === "Page.captureScreenshot") { access.beginPrivate("person", access.controlEpoch); return { data: Buffer.alloc(100).toString("base64") }; }
      return {};
    }) });
    await expect(captureNativeBrowserFrame(debuggerApi, () => access.check(ticket))).rejects.toThrow("discarded");
    expect(debuggerApi.sendCommand).toHaveBeenLastCalledWith("Page.stopScreencast");
    await expect(captureNativeBrowserFrame(debuggerApi, () => access.check(ticket))).rejects.toThrow();
    expect(debuggerApi.sendCommand).toHaveBeenCalledTimes(3);
  });
});
