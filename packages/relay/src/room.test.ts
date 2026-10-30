import { generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { afterEach, expect, it, vi } from "vitest";
import { HostToRelay, RELAY, RelayClose, RelayToHost, loginMessage, type PushData } from "@tjommie/protocol";
import { HostRoomCore, type Attachment, type Sock } from "./room.ts";

const hostId = "99cf98ba-2638-4b16-8a6e-1691c1673b45";
const relayHost = "relay.example";

class FakeSock implements Sock {
  messages: (string | Uint8Array)[] = [];
  closes: number[] = [];
  attachment: Attachment | null = null;
  send(message: string | Uint8Array): void { this.messages.push(message); }
  close(code: number): void { this.closes.push(code); }
  getAttachment(): Attachment | null { return this.attachment; }
  setAttachment(value: Attachment | null): void { this.attachment = value; }
}

function hostKey() {
  const keys = generateKeyPairSync("ed25519");
  return { publicKey: keys.publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64url"), privateKey: keys.privateKey };
}

function fixture() {
  let now = 0;
  let stored: string | null = null;
  let pushes = 0;
  const sent: PushData[] = [];
  const core = new HostRoomCore({
    hostId, now: () => now, random: () => randomBytes(32),
    registerKey: (key) => { stored ??= key; return stored; },
    pushEnabled: true,
    push: async (tokens, data) => { pushes++; sent.push(data); return tokens.map(() => ({ ok: true, unregistered: false })); },
  });
  return { core, advance: (ms: number) => { now += ms; }, stored: () => stored, pushes: () => pushes, sent: () => sent };
}

async function login(core: HostRoomCore, key: ReturnType<typeof hostKey>, ip?: string): Promise<FakeSock> {
  const socket = new FakeSock();
  core.connectControl(socket, relayHost, ip);
  const challenge = RelayToHost.parse(JSON.parse(socket.messages[0] as string));
  if (challenge.t !== "challenge") throw new Error("Expected login challenge");
  const signature = sign(null, loginMessage(hostId, challenge.nonce, relayHost), key.privateKey).toString("base64url");
  await core.onMessage(socket, JSON.stringify(HostToRelay.parse({ t: "login", publicKey: key.publicKey, signature })));
  return socket;
}

function openChannel(core: HostRoomCore, ip: string): { device: FakeSock; host: FakeSock; channel: string } {
  const device = new FakeSock();
  core.connectDevice(device, ip);
  const control = core.controlSocket();
  if (!control) throw new Error("Host is offline");
  const opened = RelayToHost.parse(JSON.parse((control as FakeSock).messages.at(-1) as string));
  if (opened.t !== "open") throw new Error("Expected open message");
  const host = new FakeSock();
  core.connectChannel(host, opened.channel);
  return { device, host, channel: opened.channel };
}

afterEach(() => vi.restoreAllMocks());

it("registers a verified key, rejects a different key without replacing the host, and replaces a valid old login", async () => {
  const f = fixture();
  const key = hostKey();
  const first = await login(f.core, key);
  expect(f.stored()).toBe(key.publicKey);
  expect(RelayToHost.parse(JSON.parse(first.messages.at(-1) as string))).toEqual({
    t: "ready", push: true, kinds: ["approval", "limit", "routine_failed", "done", "failed"],
  });
  const attacker = await login(f.core, hostKey());
  expect(attacker.closes).toContain(RelayClose.badLogin);
  expect(f.core.controlSocket()).toBe(first);
  const replacement = await login(f.core, key);
  expect(first.closes).toContain(RelayClose.replaced);
  expect(first.attachment).toBeNull();
  expect(f.core.controlSocket()).toBe(replacement);
});

it("rejects signatures bound to another host id, relay host, or nonce", async () => {
  for (const signed of [
    (nonce: string) => loginMessage("18f86a27-1d41-4910-aa9c-9cf82ef61dfc", nonce, relayHost),
    (nonce: string) => loginMessage(hostId, nonce, "other.example"),
    () => loginMessage(hostId, randomBytes(32).toString("base64url"), relayHost),
  ]) {
    const f = fixture();
    const key = hostKey();
    const control = new FakeSock();
    f.core.connectControl(control, relayHost);
    const challenge = RelayToHost.parse(JSON.parse(control.messages[0] as string));
    if (challenge.t !== "challenge") throw new Error("Expected challenge");
    const signature = sign(null, signed(challenge.nonce), key.privateKey).toString("base64url");
    await f.core.onMessage(control, JSON.stringify({ t: "login", publicKey: key.publicKey, signature }));
    expect(control.closes).toContain(RelayClose.badLogin);
    expect(f.stored()).toBeNull();
  }
});

it("rejects offline devices, pairs channels, and forwards binary frames unchanged in both directions", async () => {
  const f = fixture();
  const offline = new FakeSock();
  f.core.connectDevice(offline, "192.0.2.1");
  expect(offline.closes).toContain(RelayClose.hostOffline);
  await login(f.core, hostKey());
  const { device, host } = openChannel(f.core, "192.0.2.2");
  const toHost = Uint8Array.of(1, 2, 3), toDevice = Uint8Array.of(4, 5);
  expect(await f.core.onMessage(device, toHost)).toBe(false);
  expect(await f.core.onMessage(host, toDevice)).toBe(false);
  expect(host.messages).toEqual([toHost]);
  expect(device.messages).toEqual([toDevice]);
  f.core.onClose(device);
  expect(host.closes).toContain(1000);
});

it("does not count host channel legs against the host control IP", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey(), "203.0.113.10");
  for (let i = 0; i < RELAY.connectsPerIpPerMinute; i++) {
    const device = new FakeSock();
    f.core.connectDevice(device, "198.51.100.10");
    const opened = RelayToHost.parse(JSON.parse(control.messages.at(-1) as string));
    if (opened.t !== "open") throw new Error("Expected open message");
    const hostLeg = new FakeSock();
    f.core.connectChannel(hostLeg, opened.channel, "203.0.113.10");
    expect(hostLeg.closes).toEqual([]);
    expect(device.attachment).toMatchObject({ paired: true });
  }
});

it("buffers early device frames in order, bounds the buffer, and expires an unaccepted channel", async () => {
  const f = fixture();
  await login(f.core, hostKey());
  const device = new FakeSock();
  f.core.connectDevice(device, "192.0.2.3");
  const control = f.core.controlSocket() as FakeSock;
  const opened = RelayToHost.parse(JSON.parse(control.messages.at(-1) as string));
  if (opened.t !== "open") throw new Error("Expected open message");
  await f.core.onMessage(device, Uint8Array.of(1));
  await f.core.onMessage(device, Uint8Array.of(2));
  const host = new FakeSock();
  f.core.connectChannel(host, opened.channel);
  expect(host.messages).toEqual([Uint8Array.of(1), Uint8Array.of(2)]);
  const overflow = new FakeSock();
  f.core.connectDevice(overflow, "192.0.2.4");
  for (let i = 0; i < 9; i++) await f.core.onMessage(overflow, Uint8Array.of(i));
  expect(overflow.closes).toContain(RelayClose.tooLarge);
  const timeout = new FakeSock();
  f.core.connectDevice(timeout, "192.0.2.5");
  f.advance(10_001);
  f.core.sweep(null);
  expect(timeout.closes).toContain(RelayClose.hostOffline);
});

it("restores paired attachments and closes pending devices whose frame buffer was evicted", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey());
  const { device, host } = openChannel(f.core, "192.0.2.8");
  const pending = new FakeSock();
  f.core.connectDevice(pending, "192.0.2.9");
  await f.core.onMessage(pending, Uint8Array.of(9));
  const restored = new HostRoomCore({ hostId, now: () => 0, random: () => randomBytes(32),
    registerKey: (key) => key, pushEnabled: false, push: async () => [], sockets: [control, device, host, pending] });
  expect(pending.closes).toContain(RelayClose.hostOffline);
  await restored.onMessage(device, Uint8Array.of(7));
  expect(host.messages.at(-1)).toEqual(Uint8Array.of(7));
});

it("closes oversized frames and limits the 21st device connect and 33rd channel", async () => {
  const f = fixture();
  await login(f.core, hostKey());
  const first = openChannel(f.core, "198.51.100.1").host;
  for (let i = 1; i < 20; i++) openChannel(f.core, "198.51.100.1");
  const limited = new FakeSock();
  f.core.connectDevice(limited, "198.51.100.1");
  expect(limited.closes).toContain(RelayClose.limited);
  for (let i = 20; i < 32; i++) openChannel(f.core, `198.51.100.${i + 2}`);
  const full = new FakeSock();
  f.core.connectDevice(full, "203.0.113.1");
  expect(full.closes).toContain(RelayClose.limited);
  await f.core.onMessage(first, new Uint8Array(65_536));
  expect(first.closes).toContain(RelayClose.tooLarge);
});

it("marks a silent host dead after 75 seconds and closes its channels", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey());
  const { device, host } = openChannel(f.core, "192.0.2.6");
  f.advance(75_001);
  f.core.sweep(null);
  expect(control.closes).toContain(RelayClose.hostDead);
  expect(device.closes).toContain(RelayClose.hostOffline);
  expect(host.closes).toContain(RelayClose.hostOffline);
});

it("keeps a recently pinged host alive across the 75 second sweep", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey());
  f.advance(RELAY.hostDeadMs + 1);
  expect(f.core.sweep(RELAY.hostDeadMs - 1)).toBe(RELAY.hostDeadMs * 2 - 1);
  expect(control.closes).toEqual([]);
});

it("expires unauthenticated control sockets and restores the expiry from their attachment", () => {
  const f = fixture();
  const control = new FakeSock();
  f.core.connectControl(control, relayHost);
  expect(f.core.nextDeadline(null)).toBe(RELAY.acceptTimeoutMs);
  const restored = new HostRoomCore({ hostId, now: () => RELAY.acceptTimeoutMs, random: () => randomBytes(32),
    registerKey: (key) => key, pushEnabled: false, push: async () => [], sockets: [control] });
  restored.sweep(null);
  expect(control.closes).toContain(RelayClose.badLogin);
  expect(control.attachment).toBeNull();
  expect(restored.nextDeadline(null)).toBeNull();
});

it("closes overlarge control text before parsing in both login and ready stages", async () => {
  const f = fixture();
  const challenge = new FakeSock();
  f.core.connectControl(challenge, relayHost);
  expect(await f.core.onMessage(challenge, "x".repeat(1_025))).toBe(true);
  expect(challenge.closes).toContain(RelayClose.tooLarge);
  const ready = await login(f.core, hostKey());
  expect(await f.core.onMessage(ready, "x".repeat(70_001))).toBe(true);
  expect(ready.closes).toContain(RelayClose.tooLarge);
});

it("limits pending channels, rejects reused and expired channel ids, and resets the IP window", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey());
  for (let i = 0; i < RELAY.maxPendingChannels; i++) f.core.connectDevice(new FakeSock(), `192.0.2.${i + 1}`);
  const ninth = new FakeSock();
  f.core.connectDevice(ninth, "192.0.2.20");
  expect(ninth.closes).toContain(RelayClose.limited);
  const opened = RelayToHost.parse(JSON.parse(control.messages.at(-1) as string));
  if (opened.t !== "open") throw new Error("Expected open message");
  f.core.connectChannel(new FakeSock(), opened.channel);
  const reused = new FakeSock();
  f.core.connectChannel(reused, opened.channel);
  expect(reused.closes).toContain(RelayClose.hostOffline);
  const later = new FakeSock();
  f.core.connectDevice(later, "192.0.2.21");
  const expired = RelayToHost.parse(JSON.parse(control.messages.at(-1) as string));
  if (expired.t !== "open") throw new Error("Expected open message");
  f.advance(RELAY.acceptTimeoutMs);
  const lateLeg = new FakeSock();
  f.core.connectChannel(lateLeg, expired.channel);
  expect(lateLeg.closes).toContain(RelayClose.hostOffline);

  const rate = fixture();
  for (let i = 0; i < RELAY.connectsPerIpPerMinute; i++) rate.core.connectControl(new FakeSock(), relayHost, "203.0.113.7");
  const blocked = new FakeSock();
  rate.core.connectControl(blocked, relayHost, "203.0.113.7");
  expect(blocked.closes).toContain(RelayClose.limited);
  rate.advance(60_000);
  const allowed = new FakeSock();
  rate.core.connectControl(allowed, relayHost, "203.0.113.7");
  expect(allowed.closes).toEqual([]);
});

it("caps pending device channels per source IP without blocking a different IP", async () => {
  const f = fixture();
  await login(f.core, hostKey());
  const first = new FakeSock(), second = new FakeSock(), third = new FakeSock(), other = new FakeSock();
  f.core.connectDevice(first, "192.0.2.1");
  f.core.connectDevice(second, "192.0.2.1");
  f.core.connectDevice(third, "192.0.2.1");
  f.core.connectDevice(other, "192.0.2.2");
  expect(first.closes).toEqual([]);
  expect(second.closes).toEqual([]);
  expect(third.closes).toContain(RelayClose.limited);
  expect(other.closes).toEqual([]);
  f.core.onClose(first);
  const replacement = new FakeSock();
  f.core.connectDevice(replacement, "192.0.2.1");
  expect(replacement.closes).toEqual([]);
});

it("restores the newest ready host even if a stale ready attachment is listed last", () => {
  const newest = new FakeSock();
  newest.attachment = { role: "control", stage: "ready", since: 20 };
  const stale = new FakeSock();
  stale.attachment = { role: "control", stage: "ready", since: 10 };
  const core = new HostRoomCore({ hostId, now: () => 20, random: () => randomBytes(32),
    registerKey: (key) => key, pushEnabled: false, push: async () => [], sockets: [newest, stale] });
  expect(core.controlSocket()).toBe(newest);
  expect(stale.closes).toContain(RelayClose.replaced);
  expect(stale.attachment).toBeNull();
});

it("closes a pending device when the host control socket cannot receive open", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey());
  control.send = () => { throw new Error("socket closed"); };
  const device = new FakeSock();
  f.core.connectDevice(device, "192.0.2.7");
  expect(control.closes).toContain(RelayClose.hostDead);
  expect(device.closes).toContain(RelayClose.hostOffline);
});

it("rejects invalid push requests and never logs a token or IP", async () => {
  const log = vi.spyOn(console, "log").mockImplementation(() => {});
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  const info = vi.spyOn(console, "info").mockImplementation(() => {});
  const debug = vi.spyOn(console, "debug").mockImplementation(() => {});
  const f = fixture();
  const control = await login(f.core, hostKey());
  f.core.connectDevice(new FakeSock(), "203.0.113.77");
  await f.core.onMessage(control, JSON.stringify({ t: "push", id: 1, tokens: Array(17).fill("secret-token"), kind: "approval", botId: "bot" }));
  expect(f.pushes()).toBe(0);
  expect(control.closes).toContain(1008);
  expect(JSON.stringify([...log.mock.calls, ...error.mock.calls, ...warn.mock.calls, ...info.mock.calls, ...debug.mock.calls])).not.toMatch(/secret-token|203\.0\.113\.77/);
});

it("answers an unknown push kind with failed results and keeps the host online", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey());
  await f.core.onMessage(control, JSON.stringify({ t: "push", id: 1, tokens: ["token-1", "token-2"], kind: "chat", botId: "bot" }));
  expect(RelayToHost.parse(JSON.parse(control.messages.at(-1) as string))).toEqual({ t: "pushed", id: 1, results: [
    { ok: false, unregistered: false }, { ok: false, unregistered: false },
  ] });
  expect(control.closes).toEqual([]);
  expect(f.core.controlSocket()).toBe(control);
  expect(f.pushes()).toBe(0);
  await f.core.onMessage(control, JSON.stringify({ t: "push", id: 2, tokens: ["token-3"], kind: "done", botId: "bot", roomId: "room" }));
  expect(RelayToHost.parse(JSON.parse(control.messages.at(-1) as string))).toEqual({ t: "pushed", id: 2, results: [{ ok: true, unregistered: false }] });
  expect(f.sent()).toEqual([{ v: "1", hostId, kind: "done", botId: "bot", roomId: "room" }]);
});

it("closes an unknown-kind push before login with 4401", async () => {
  const f = fixture();
  const control = new FakeSock();
  f.core.connectControl(control, relayHost);
  await f.core.onMessage(control, JSON.stringify({ t: "push", id: 1, tokens: ["token"], kind: "chat", botId: "bot" }));
  expect(control.closes).toContain(RelayClose.badLogin);
  expect(f.pushes()).toBe(0);
});

it("closes malformed pushes even when their kind is unknown", async () => {
  const f = fixture();
  const control = await login(f.core, hostKey());
  await f.core.onMessage(control, JSON.stringify({ t: "push", id: 1, tokens: Array(17).fill("token"), kind: "chat", botId: "bot" }));
  expect(control.closes).toContain(1008);
  expect(f.pushes()).toBe(0);
});
