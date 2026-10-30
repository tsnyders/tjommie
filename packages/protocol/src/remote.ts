import { z } from "zod";

export const RemoteName = z.string().trim().min(1).max(80).refine((value) => !/[\x00-\x1f\x7f]/.test(value), "Enter a device name without control characters.");
export const RemoteKey = z.string().regex(/^[A-Za-z0-9_-]{43}$/).refine((value) => {
  try { return btoa(atob(value.replace(/-/g, "+").replace(/_/g, "/") + "=")).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "") === value; }
  catch { return false; }
}, "Invalid public key.");
export const LanEndpoint = z.object({ host: z.union([z.ipv4(), z.ipv6()]), port: z.number().int().min(1).max(65535) }).strict();
export const RelayUrl = z.url({ protocol: /^https$/ }).max(512);
export const PairingCode = z.object({
  v: z.literal(2), relay: RelayUrl.nullable(), pairingId: z.uuid(), hostId: z.uuid(), hostName: RemoteName,
  hostKey: RemoteKey, secret: RemoteKey, lan: z.array(LanEndpoint).min(1).max(16), expires: z.iso.datetime(),
}).strict();
export type PairingCode = z.infer<typeof PairingCode>;
export const Device = z.object({
  id: z.uuid(), name: RemoteName, publicKey: RemoteKey, pairedAt: z.iso.datetime(),
  lastSeenAt: z.iso.datetime().nullable(), revokedAt: z.iso.datetime().nullable(),
});
export type Device = z.infer<typeof Device>;
export const RelayState = z.enum(["off", "connecting", "online", "error"]);
export type RelayState = z.infer<typeof RelayState>;
export const RemoteStatus = z.object({
  hostId: z.uuid().nullable(), hostName: RemoteName, listening: z.boolean(), port: z.number().int().min(1).max(65535).nullable(),
  connectedDeviceIds: z.array(z.uuid()).max(64), relay: z.object({ url: RelayUrl.nullable(), state: RelayState, push: z.boolean() }),
});
export type RemoteStatus = z.infer<typeof RemoteStatus>;
export const RemotePreface = z.discriminatedUnion("mode", [
  z.object({ v: z.literal(1), hostId: z.uuid(), mode: z.literal("pair"), pairingId: z.uuid() }).strict(),
  z.object({ v: z.literal(1), hostId: z.uuid(), mode: z.literal("resume") }).strict(),
]);
export type RemotePreface = z.infer<typeof RemotePreface>;
export function remotePrologue(preface: RemotePreface): Uint8Array {
  const valid = RemotePreface.parse(preface);
  return new TextEncoder().encode(`Tjommie remote v1\n${JSON.stringify(valid)}`);
}
export const RemoteConfirm = z.object({ type: z.literal("confirm"), protocol: z.literal(1), name: RemoteName }).strict();
export const RemoteReady = z.object({ type: z.literal("ready"), deviceId: z.uuid(), hostId: z.uuid(), hostName: RemoteName, relay: RelayUrl.nullable() }).strict();
export const RemoteRefused = z.object({ type: z.literal("refused"), reason: z.enum(["revoked", "unknown"]) }).strict();

export const PushKind = z.enum(["approval", "limit", "routine_failed", "done", "failed"]);
export type PushKind = z.infer<typeof PushKind>;

export function encodePairingCode(input: PairingCode): string {
  const bytes = new TextEncoder().encode(JSON.stringify(PairingCode.parse(input)));
  return btoa(Array.from(bytes, (value) => String.fromCharCode(value)).join("")).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function parsePairingCode(input: string, now = Date.now()): PairingCode {
  try {
    const code = input.startsWith("tjommie://pair#") ? input.slice(15) : input;
    if (!/^[A-Za-z0-9_-]{16,8192}$/.test(code)) throw new Error();
    const raw = atob(code.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - code.length % 4) % 4));
    const result = PairingCode.parse(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(raw, (value) => value.charCodeAt(0)))));
    if (encodePairingCode(result) !== code) throw new Error();
    const remaining = Date.parse(result.expires) - now;
    if (remaining <= 0 || remaining > 660_000) throw new Error();
    return result;
  } catch { throw new Error("This pairing code is invalid or expired."); }
}

export const remoteMethods = {
  "remote.status": { params: z.object({}), result: RemoteStatus },
  "remote.ping": { params: z.object({}), result: z.object({}) },
  "devices.list": { params: z.object({}), result: z.array(Device).max(64) },
  "devices.rename": { params: z.object({ id: z.uuid(), name: RemoteName }), result: Device },
  "devices.revoke": { params: z.object({ id: z.uuid() }), result: Device },
  "device.registerPush": { params: z.object({ token: z.string().min(1).max(4096).nullable(), kinds: z.array(z.string().max(32)).max(16).optional() }), result: z.object({}) },
  "pairing.create": { params: z.object({}), result: z.object({ id: z.uuid(), code: z.string().max(8192), uri: z.string().max(8210), expires: z.iso.datetime() }) },
  "pairing.cancel": { params: z.object({ id: z.uuid() }), result: z.object({}) },
} as const;
export const remoteEvents = { "devices.changed": z.object({}) } as const;
