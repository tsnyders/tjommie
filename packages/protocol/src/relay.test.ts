import { expect, it } from "vitest";
import { RelayUrl } from "./remote.ts";
import { DEFAULT_RELAY_URL, HostToRelay, LEGACY_PUSH_KINDS, PushData, PushKind, RELAY, RelayClose, RelayToHost, loginMessage } from "./relay.ts";

const hostId = "99cf98ba-2638-4b16-8a6e-1691c1673b45";
const key = "AQ".padEnd(43, "A");

it("freezes relay limits and control messages", () => {
  expect(DEFAULT_RELAY_URL).toBe("https://tjommie-relay.tyrellsnyders30.workers.dev");
  if (DEFAULT_RELAY_URL === null) throw new Error("The public relay URL must be set");
  expect(RelayUrl.parse(DEFAULT_RELAY_URL)).toBe(DEFAULT_RELAY_URL);
  expect(RELAY).toEqual({ maxFrame: 65_535, hostPingMs: 30_000, hostDeadMs: 75_000, acceptTimeoutMs: 10_000,
    maxChannels: 32, maxPendingChannels: 8, connectsPerIpPerMinute: 20, maxPushTokens: 16 });
  expect(RelayClose).toEqual({ hostOffline: 4404, badLogin: 4401, replaced: 4409, hostDead: 4408, tooLarge: 1009, limited: 4429 });
  expect(RelayToHost.safeParse({ t: "challenge", nonce: key }).success).toBe(true);
  expect(RelayToHost.parse({ t: "ready", push: true })).toEqual({ t: "ready", push: true });
  expect(RelayToHost.parse({ t: "ready", push: true, kinds: ["done"] })).toEqual({ t: "ready", push: true, kinds: ["done"] });
  expect(HostToRelay.safeParse({ t: "login", publicKey: key, signature: "A".repeat(86) }).success).toBe(true);
});

it("bounds push tokens and kinds and excludes content from push data", () => {
  expect(PushKind.options).toEqual(["approval", "limit", "routine_failed", "done", "failed"]);
  expect(LEGACY_PUSH_KINDS).toEqual(["approval", "limit", "routine_failed"]);
  const push = { t: "push", id: 1, tokens: ["token"], kind: "approval", botId: "bot" };
  expect(HostToRelay.safeParse(push).success).toBe(true);
  expect(HostToRelay.parse({ ...push, kind: "done", roomId: "room" })).toEqual({ ...push, kind: "done", roomId: "room" });
  expect(HostToRelay.safeParse({ ...push, tokens: Array(17).fill("token") }).success).toBe(false);
  expect(HostToRelay.safeParse({ ...push, kind: "chat" }).success).toBe(false);
  const data = { v: "1", hostId, kind: "approval", botId: "bot" };
  expect(PushData.safeParse(data).success).toBe(true);
  expect(PushData.parse({ ...data, kind: "failed", roomId: "room" })).toEqual({ ...data, kind: "failed", roomId: "room" });
  expect(PushData.safeParse({ ...data, roomId: "x".repeat(65) }).success).toBe(false);
  expect(PushData.safeParse({ ...data, title: "secret" }).success).toBe(false);
});

it("signs the domain-separated login message bytes", () => {
  expect(loginMessage("h", "n", "r.example")).toEqual(new TextEncoder().encode("tjommie-relay-login/v1\nh\nn\nr.example"));
});
