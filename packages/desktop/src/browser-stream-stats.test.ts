import { describe, expect, it } from "vitest";
import { meetsStreamTarget, summarizeStream } from "./browser-stream-stats.ts";

// Frame i arrives at i * 1000 / fps milliseconds.
const steady = (fps: number, seconds: number): number[] => Array.from({ length: fps * seconds }, (_, i) => (i * 1000) / fps);

describe("summarizeStream", () => {
  it("reads a steady 60 fps run as 60 fps with a 16.7 ms gap", () => {
    const stats = summarizeStream(steady(60, 60));
    expect(stats.frames).toBe(3600);
    expect(stats.medianFps).toBe(60);
    expect(stats.p95GapMs).toBeCloseTo(1000 / 60, 6);
    expect(stats.maxGapMs).toBeCloseTo(1000 / 60, 6);
    expect(meetsStreamTarget(stats)).toBe(true);
  });

  it("fails a 200 ms stall every second on the median, while p95 still passes", () => {
    // Each second: 41 frames 20 ms apart, then the 200 ms gap to the next second's first frame.
    const arrivals = Array.from({ length: 4 * 41 }, (_, i) => Math.floor(i / 41) * 1000 + (i % 41) * 20);
    const stats = summarizeStream(arrivals);
    // 164 frames over 3.8 s, so 163 gaps: 160 of 20 ms and 3 of 200 ms.
    // p95: ceil(0.95 * 163) = ceil(154.85) = 155, the 155th smallest gap, which is 20 ms.
    // Windows: floor(3.8) = 3. Each [k s, k+1 s) holds offsets 0..800 ms, which is 41 frames. Median 41.
    // p95 needs more than 5% of gaps over 34 ms to fail, and one stall per second is about 2%.
    expect(stats).toEqual({ frames: 164, seconds: 3.8, medianFps: 41, p95GapMs: 20, maxGapMs: 200 });
    expect(meetsStreamTarget(stats)).toBe(false);
  });

  it("fails a steady 30 fps run on the median", () => {
    const stats = summarizeStream(steady(30, 60));
    expect(stats.medianFps).toBe(30);
    expect(stats.p95GapMs).toBeCloseTo(1000 / 30, 6);
    expect(meetsStreamTarget(stats)).toBe(false);
  });

  it("returns zeros for fewer than two arrivals", () => {
    expect(summarizeStream([])).toEqual({ frames: 0, seconds: 0, medianFps: 0, p95GapMs: 0, maxGapMs: 0 });
    expect(summarizeStream([1234])).toEqual({ frames: 1, seconds: 0, medianFps: 0, p95GapMs: 0, maxGapMs: 0 });
  });

  it("matches a hand-computed short run", () => {
    // Arrivals 0, 500, 1000, 1200, 1400, 1600, 2000 ms.
    // Gaps 500, 500, 200, 200, 200, 400, sorted 200, 200, 200, 400, 500, 500.
    // p95: ceil(0.95 * 6) = ceil(5.7) = 6, so the 6th smallest gap, 500. maxGap = 500.
    // seconds = (2000 - 0) / 1000 = 2, so the whole windows are [0, 1000) and [1000, 2000).
    // [0, 1000) holds 0 and 500, which is 2 frames. [1000, 2000) holds 1000, 1200, 1400 and 1600, which is 4 frames.
    // The arrival at 2000 ends the run and sits in no whole window.
    // Median of [2, 4] = (2 + 4) / 2 = 3.
    expect(summarizeStream([0, 500, 1000, 1200, 1400, 1600, 2000])).toEqual({ frames: 7, seconds: 2, medianFps: 3, p95GapMs: 500, maxGapMs: 500 });
  });

  it("holds the target at exactly 55 fps and 34 ms", () => {
    expect(meetsStreamTarget({ frames: 3300, seconds: 60, medianFps: 55, p95GapMs: 34, maxGapMs: 40 })).toBe(true);
    expect(meetsStreamTarget({ frames: 3300, seconds: 60, medianFps: 54.9, p95GapMs: 34, maxGapMs: 40 })).toBe(false);
    expect(meetsStreamTarget({ frames: 3300, seconds: 60, medianFps: 55, p95GapMs: 34.1, maxGapMs: 40 })).toBe(false);
  });
});
