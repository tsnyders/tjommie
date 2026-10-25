import { expect, it } from "vitest";
import { PairingCode, encodePairingCode, parsePairingCode } from "./remote.ts";

const now = Date.parse("2026-10-22T12:00:00.000Z");
const sample = () => ({ v: 1 as const, relay: null, pairingId: "04364399-a4af-4ba6-a1e0-58ee51a40822", hostId: "99cf98ba-2638-4b16-8a6e-1691c1673b45", hostName: "My PC", hostKey: "AQ".padEnd(43, "A"), secret: "Ag".padEnd(43, "A"), lan: [{ host: "192.168.1.2", port: 45782 }], expires: "2026-10-22T12:10:00.000Z" });

it("round trips a bounded one-use pairing code and its app URI", () => {
  const input = sample();
  const code = encodePairingCode(input);
  expect(parsePairingCode(code, now)).toEqual(input);
  expect(parsePairingCode(`tjommie://pair#${code}`, now)).toEqual(input);
  expect(code).not.toContain(input.secret);
});

it("rejects expired, overlong, malformed and unsupported pairing inputs without echoing them", () => {
  for (const value of ["secret-token", "x".repeat(9000), "https://example.com/#abc", encodePairingCode(sample()) + "="]) {
    expect(() => parsePairingCode(value, now)).toThrow("This pairing code is invalid or expired.");
  }
  expect(() => parsePairingCode(encodePairingCode(sample()), now + 600_000)).toThrow();
  expect(() => parsePairingCode(encodePairingCode(sample()), now - 120_000)).toThrow();
  expect(PairingCode.safeParse({ ...sample(), lan: [{ host: "example.com", port: 1234 }] }).success).toBe(false);
  expect(PairingCode.safeParse({ ...sample(), secret: "A".repeat(42) }).success).toBe(false);
  expect(PairingCode.safeParse({ ...sample(), relay: "http://relay.example.com" }).success).toBe(false);
});
