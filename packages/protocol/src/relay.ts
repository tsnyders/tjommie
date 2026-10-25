import { z } from "zod";
import { RemoteKey } from "./remote.ts";

export const DEFAULT_RELAY_URL: string | null = null;
export const RELAY = { maxFrame: 65_535, hostPingMs: 30_000, hostDeadMs: 75_000, acceptTimeoutMs: 10_000,
  maxChannels: 32, maxPendingChannels: 8, connectsPerIpPerMinute: 20, maxPushTokens: 16 } as const;
export const PushKind = z.enum(["approval", "limit", "routine_failed"]);
export type PushKind = z.infer<typeof PushKind>;
export const loginMessage = (hostId: string, nonce: string, relayHost: string) =>
  new TextEncoder().encode(`tjommie-relay-login/v1\n${hostId}\n${nonce}\n${relayHost}`);
export const RelayToHost = z.discriminatedUnion("t", [
  z.object({ t: z.literal("challenge"), nonce: RemoteKey }),
  z.object({ t: z.literal("ready"), push: z.boolean() }),
  z.object({ t: z.literal("open"), channel: RemoteKey }),
  z.object({ t: z.literal("pushed"), id: z.number().int(), results: z.array(z.object({ ok: z.boolean(), unregistered: z.boolean() })).max(16) }),
]);
export type RelayToHost = z.infer<typeof RelayToHost>;
export const HostToRelay = z.discriminatedUnion("t", [
  z.object({ t: z.literal("login"), publicKey: RemoteKey, signature: z.string().regex(/^[A-Za-z0-9_-]{86}$/) }),
  z.object({ t: z.literal("push"), id: z.number().int(), tokens: z.array(z.string().min(1).max(4096)).min(1).max(16), kind: PushKind, botId: z.string().max(64) }),
]);
export type HostToRelay = z.infer<typeof HostToRelay>;
export const PushData = z.object({ v: z.literal("1"), hostId: z.uuid(), kind: PushKind, botId: z.string().max(64) }).strict();
export type PushData = z.infer<typeof PushData>;
export const RelayClose = { hostOffline: 4404, badLogin: 4401, replaced: 4409, hostDead: 4408, tooLarge: 1009, limited: 4429 } as const;
