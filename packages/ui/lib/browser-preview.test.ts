import { afterEach, describe, expect, it, vi } from "vitest";
import type { BrowserFrame, BrowserState } from "@tjommie/protocol";
import { BrowserPreview } from "./browser-preview";

const state: BrowserState = { botId: "bot", generation: "one", control: "bot", activePageId: "page", tabs: [], dialog: null };
const frame: BrowserFrame = { data: "AAAA", mime: "image/jpeg", width: 640, height: 480, sequence: 1 };
afterEach(() => vi.useRealTimers());
describe("visible browser card preview", () => {
  it("polls once a second only while visible and clears retained pixels when hidden/private", async () => {
    vi.useFakeTimers();
    const pull = vi.fn(async () => ({ state, frame })), show = vi.fn(), preview = new BrowserPreview(pull, show);
    await vi.advanceTimersByTimeAsync(5000); expect(pull).not.toHaveBeenCalled();
    preview.setVisible(true); await vi.advanceTimersByTimeAsync(1000); expect(pull).toHaveBeenCalledTimes(2);
    preview.setVisible(false); expect(show).toHaveBeenLastCalledWith(null);
    await vi.advanceTimersByTimeAsync(5000); expect(pull).toHaveBeenCalledTimes(2);
    pull.mockResolvedValue({ state: { ...state, privateActive: true }, frame });
    preview.setVisible(true); await vi.advanceTimersByTimeAsync(0); expect(show).toHaveBeenLastCalledWith(null);
    preview.setVisible(false);
  });
  it("drops late hidden replies and never has a second request queued", async () => {
    vi.useFakeTimers();
    let finish!: (value: { state: BrowserState; frame: BrowserFrame }) => void;
    const pull = vi.fn(() => new Promise<{ state: BrowserState; frame: BrowserFrame }>((resolve) => { finish = resolve; })), show = vi.fn();
    const preview = new BrowserPreview(pull, show); preview.setVisible(true);
    await vi.advanceTimersByTimeAsync(10_000); expect(pull).toHaveBeenCalledTimes(1);
    preview.setVisible(false); finish({ state, frame }); await vi.advanceTimersByTimeAsync(0);
    expect(show).toHaveBeenLastCalledWith(null); expect(show).not.toHaveBeenCalledWith(frame);
  });
});
