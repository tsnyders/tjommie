import { encodePairingCode } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { countdown, pairingProblem, pairingReady } from "./pairing";

const NOW = Date.parse("2026-10-22T10:00:00.000Z");
const key = "A".repeat(43);
const code = (expires: string): string =>
  encodePairingCode({ v: 1, relay: null, pairingId: "11111111-1111-4111-8111-111111111111", hostId: "22222222-2222-4222-8222-222222222222", hostName: "Home PC", hostKey: key, secret: key, lan: [{ host: "192.168.1.20", port: 4310 }], expires });

describe("pairingProblem", () => {
  it("accepts a good code, in a link or alone, and says nothing for an empty box", () => {
    const good = code(new Date(NOW + 9 * 60_000).toISOString());
    expect(pairingProblem(good, NOW)).toBeNull();
    expect(pairingProblem(`  tjommie://pair#${good}\n`, NOW)).toBeNull();
    expect(pairingProblem("   ", NOW)).toBeNull();
    expect(pairingReady(good, NOW)).toBe(true);
    expect(pairingReady("", NOW)).toBe(false);
  });

  it("refuses junk and an expired code, without repeating what was typed", () => {
    const expired = code(new Date(NOW - 1000).toISOString());
    for (const bad of ["not a code", "tjommie://pair#short", expired]) {
      const problem = pairingProblem(bad, NOW);
      expect(problem).toBe("This pairing code is invalid or expired.");
      expect(problem).not.toContain(bad);
      expect(pairingReady(bad, NOW)).toBe(false);
    }
  });
});

describe("countdown", () => {
  it("counts down in minutes and seconds, and says Expired at zero", () => {
    expect(countdown("2026-10-22T10:10:00.000Z", NOW)).toEqual({ text: "10:00", expired: false });
    expect(countdown("2026-10-22T10:09:59.001Z", NOW)).toEqual({ text: "10:00", expired: false });
    expect(countdown("2026-10-22T10:00:05.000Z", NOW)).toEqual({ text: "0:05", expired: false });
    expect(countdown("2026-10-22T10:00:00.000Z", NOW)).toEqual({ text: "Expired", expired: true });
    expect(countdown("2026-10-22T09:00:00.000Z", NOW)).toEqual({ text: "Expired", expired: true });
  });
});
