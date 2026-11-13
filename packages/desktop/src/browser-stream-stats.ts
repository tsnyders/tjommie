// Statistics for the live browser stream acceptance test (spec 3.9). Arrival times are monotonic milliseconds,
// such as performance.now() readings taken as each streamed frame arrives.

export type StreamStats = { frames: number; seconds: number; medianFps: number; p95GapMs: number; maxGapMs: number };

export function summarizeStream(arrivalsMs: readonly number[]): StreamStats {
  const frames = arrivalsMs.length;
  if (frames < 2) return { frames, seconds: 0, medianFps: 0, p95GapMs: 0, maxGapMs: 0 };
  const first = arrivalsMs[0];
  const last = arrivalsMs[frames - 1];

  const gaps: number[] = [];
  for (let i = 1; i < frames; i++) gaps.push(arrivalsMs[i] - arrivalsMs[i - 1]);
  const sortedGaps = [...gaps].sort((a, b) => a - b);
  // Nearest rank: the ceil(0.95 n)-th smallest gap. Integer arithmetic keeps exact multiples exact.
  const p95GapMs = sortedGaps[Math.ceil((95 * sortedGaps.length) / 100) - 1];
  const maxGapMs = sortedGaps[sortedGaps.length - 1];

  // Only whole one-second windows that fit inside the run count. A run shorter than one second has none,
  // so its median is 0 and it fails the target.
  const seconds = (last - first) / 1000;
  const windows = Math.floor(seconds);
  const counts = new Array<number>(windows).fill(0);
  for (const time of arrivalsMs) {
    const bucket = Math.floor((time - first) / 1000);
    if (bucket < windows) counts[bucket]++;
  }
  counts.sort((a, b) => a - b);
  const middle = Math.floor(counts.length / 2);
  const medianFps = counts.length === 0 ? 0 : counts.length % 2 === 1 ? counts[middle] : (counts[middle - 1] + counts[middle]) / 2;

  return { frames, seconds, medianFps, p95GapMs, maxGapMs };
}

// The spec 3.9 target. Loopback is held to the same bar as LAN.
export function meetsStreamTarget(stats: StreamStats): boolean {
  return stats.medianFps >= 55 && stats.p95GapMs <= 34;
}
