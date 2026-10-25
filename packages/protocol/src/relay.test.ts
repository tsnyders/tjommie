import { expect, it } from "vitest";
import { RelayUrl } from "./remote.ts";
import { DEFAULT_RELAY_URL, HostToRelay, PushData, RELAY, RelayClose, RelayToHost, loginMessage } from "./relay.ts";

const hostId = "99cf98ba-2638-4b16-8a6e-1691c1673b45";
const key = "AQ".padEnd(43, "A");

it("freezes relay limits and control messages", () => {
  expect(RelayUrl.nullable().safeParse(DEFAULT_RELAY_URL).success).toBe(true);
  expect(RELAY).toEqual({ maxFrame: 65_535, hostPingMs: 30_000, hostDeadMs: 75_000, acceptTimeoutMs: 10_000,
    maxChannels: 32, maxPendingChannels: 8, connectsPerIpPerMinute: 20, maxPushTokens: 16 });
  expect(RelayClose).toEqual({ hostOffline: 4404, badLogin: 4401, replaced: 4409, hostDead: 4408, tooLarge: 1009, limited: 4429 });
  expect(RelayToHost.safeParse({ t: "challenge", nonce: key }).success).toBe(true);
  expect(HostToRelay.safeParse({ t: "login", publicKey: key, signature: "A".repeat(86) }).success).toBe(true);
});

it("bounds push tokens and kinds and excludes content from push data", () => {
  const push = { t: "push", id: 1, tokens: ["token"], kind: "approval", botId: "bot" };
  expect(HostToRelay.safeParse(push).success).toBe(true);
  expect(HostToRelay.safeParse({ ...push, tokens: Array(17).fill("token") }).success).toBe(false);
  expect(HostToRelay.safeParse({ ...push, kind: "chat" }).success).toBe(false);
  const data = { v: "1", hostId, kind: "approval", botId: "bot" };
  expect(PushData.safeParse(data).success).toBe(true);
  expect(PushData.safeParse({ ...data, title: "secret" }).success).toBe(false);
});

it("signs the domain-separated login message bytes", () => {
  expect(loginMessage("h", "n", "r.example")).toEqual(new TextEncoder().encode("tjommie-relay-login/v1\nh\nn\nr.example"));
});
