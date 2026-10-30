import { HostToRelay, PushData, PushKind, RELAY, RelayClose, loginMessage, type PushData as PushDataValue } from "@tjommie/protocol";

export type Attachment =
  | { role: "control"; stage: "challenge"; nonce: string; relayHost: string; expires: number }
  | { role: "control"; stage: "ready"; since: number }
  | { role: "device"; channel: string; expires: number; paired: boolean }
  | { role: "channel"; channel: string };

export interface Sock {
  send(message: string | Uint8Array): void;
  close(code: number): void;
  getAttachment(): Attachment | null;
  setAttachment(value: Attachment | null): void;
}

type PushResult = { ok: boolean; unregistered: boolean };
type RoomOptions = {
  hostId: string;
  now: () => number;
  random: () => Uint8Array;
  registerKey: (key: string) => string;
  pushEnabled: boolean;
  push: (tokens: string[], data: PushDataValue) => Promise<PushResult[]>;
  sockets?: Sock[];
};

function base64url(bytes: Uint8Array): string {
  return btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""))
    .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function decodeBase64url(value: string): ArrayBuffer {
  return Uint8Array.from(atob(value.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - value.length % 4) % 4)), (char) => char.charCodeAt(0)).buffer as ArrayBuffer;
}

async function verifyLogin(publicKey: string, signature: string, message: Uint8Array): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey("raw", decodeBase64url(publicKey), "Ed25519", false, ["verify"]);
    return await crypto.subtle.verify("Ed25519", key, decodeBase64url(signature), Uint8Array.from(message).buffer as ArrayBuffer);
  } catch { return false; }
}

export class HostRoomCore {
  private readonly options: RoomOptions;
  private readonly states = new Map<Sock, Attachment>();
  private readonly pending = new Map<string, { device: Sock; ip?: string; expires: number; frames: Uint8Array[]; bytes: number }>();
  private readonly channels = new Map<string, { device: Sock; host: Sock }>();
  private readonly connects = new Map<string, { start: number; count: number }>();
  private host: Sock | null = null;

  constructor(options: RoomOptions) {
    this.options = options;
    const paired = new Map<string, { device?: Sock; host?: Sock }>();
    for (const sock of options.sockets ?? []) {
      const state = sock.getAttachment();
      if (!state) { sock.close(RelayClose.hostOffline); continue; }
      if (state.role === "device" && !state.paired) { sock.setAttachment(null); sock.close(RelayClose.hostOffline); continue; }
      this.states.set(sock, state);
      if (state.role === "control" && state.stage === "ready") {
        const current = this.host ? this.states.get(this.host) : null;
        if (current?.role === "control" && current.stage === "ready" && current.since >= state.since)
          this.close(sock, RelayClose.replaced);
        else {
          if (this.host) this.close(this.host, RelayClose.replaced);
          this.host = sock;
        }
      } else if (state.role === "device" || state.role === "channel") {
        const pair = paired.get(state.channel) ?? {};
        if (state.role === "device") pair.device = sock;
        else pair.host = sock;
        paired.set(state.channel, pair);
      }
    }
    for (const [channel, pair] of paired) {
      if (pair.device && pair.host && this.host) this.channels.set(channel, { device: pair.device, host: pair.host });
      else {
        if (pair.device) this.close(pair.device, RelayClose.hostOffline);
        if (pair.host) this.close(pair.host, RelayClose.hostOffline);
      }
    }
  }

  controlSocket(): Sock | null { return this.host; }

  private limited(ip?: string): boolean {
    // shortcut: this room-scoped counter resets on hibernation, use an edge rate-limit binding for global abuse control.
    if (!ip) return false;
    const now = this.options.now();
    const previous = this.connects.get(ip);
    const next = !previous || now - previous.start >= 60_000 ? { start: now, count: 1 } : { start: previous.start, count: previous.count + 1 };
    this.connects.set(ip, next);
    return next.count > RELAY.connectsPerIpPerMinute;
  }

  connectControl(sock: Sock, relayHost: string, ip?: string): void {
    if (this.limited(ip)) { sock.close(RelayClose.limited); return; }
    const nonce = base64url(this.options.random());
    const state: Attachment = { role: "control", stage: "challenge", nonce, relayHost, expires: this.options.now() + RELAY.acceptTimeoutMs };
    this.states.set(sock, state);
    sock.setAttachment(state);
    sock.send(JSON.stringify({ t: "challenge", nonce }));
  }

  connectDevice(sock: Sock, ip?: string): void {
    // ponytail: IPv6 /64 rotation can fill all 8 room slots; bucket /64s in both caps if abuse warrants it.
    const maxPendingPerIp = 2;
    if (this.limited(ip) || this.channels.size + this.pending.size >= RELAY.maxChannels || this.pending.size >= RELAY.maxPendingChannels ||
      ip !== undefined && [...this.pending.values()].filter((entry) => entry.ip === ip).length >= maxPendingPerIp) {
      sock.close(RelayClose.limited); return;
    }
    if (!this.host) { sock.close(RelayClose.hostOffline); return; }
    let channel: string;
    do { channel = base64url(this.options.random()); } while (this.pending.has(channel) || this.channels.has(channel));
    const expires = this.options.now() + RELAY.acceptTimeoutMs;
    const state: Attachment = { role: "device", channel, expires, paired: false };
    this.states.set(sock, state);
    sock.setAttachment(state);
    this.pending.set(channel, { device: sock, ip, expires, frames: [], bytes: 0 });
    try { this.host.send(JSON.stringify({ t: "open", channel })); }
    catch { this.close(this.host, RelayClose.hostDead); }
  }

  connectChannel(sock: Sock, channel: string, _ip?: string): void {
    const pending = this.pending.get(channel);
    if (!this.host || !pending || pending.expires <= this.options.now()) { sock.close(RelayClose.hostOffline); return; }
    this.pending.delete(channel);
    const deviceState: Attachment = { role: "device", channel, expires: pending.expires, paired: true };
    const hostState: Attachment = { role: "channel", channel };
    this.states.set(pending.device, deviceState);
    pending.device.setAttachment(deviceState);
    this.states.set(sock, hostState);
    sock.setAttachment(hostState);
    this.channels.set(channel, { device: pending.device, host: sock });
    try { for (const frame of pending.frames) sock.send(frame); }
    catch { this.close(sock, RelayClose.hostOffline); }
  }

  async onMessage(sock: Sock, message: string | ArrayBuffer | Uint8Array): Promise<boolean> {
    const state = this.states.get(sock);
    if (!state) return false;
    if (state.role === "control") {
      const badControl = state.stage === "challenge" ? RelayClose.badLogin : 1008;
      if (typeof message !== "string") { this.close(sock, badControl); return true; }
      if (message.length > (state.stage === "challenge" ? 1_024 : 70_000)) { this.close(sock, RelayClose.tooLarge); return true; }
      let parsed: unknown;
      try { parsed = JSON.parse(message); } catch { this.close(sock, badControl); return true; }
      const unknownPush = state.stage === "ready" && typeof parsed === "object" && parsed !== null &&
        "t" in parsed && parsed.t === "push" && "kind" in parsed && typeof parsed.kind === "string" &&
        !PushKind.safeParse(parsed.kind).success ? parsed : null;
      const result = HostToRelay.safeParse(unknownPush ? { ...unknownPush, kind: "approval" } : parsed);
      if (!result.success) { this.close(sock, badControl); return true; }
      if (state.stage === "challenge") {
        if (state.expires <= this.options.now() || result.data.t !== "login" || !await verifyLogin(result.data.publicKey, result.data.signature,
          loginMessage(this.options.hostId, state.nonce, state.relayHost))) { this.close(sock, RelayClose.badLogin); return true; }
        if (this.states.get(sock) !== state || state.expires <= this.options.now() || this.options.registerKey(result.data.publicKey) !== result.data.publicKey) {
          this.close(sock, RelayClose.badLogin); return true;
        }
        if (this.host && this.host !== sock) this.close(this.host, RelayClose.replaced);
        const ready: Attachment = { role: "control", stage: "ready", since: this.options.now() };
        this.host = sock;
        this.states.set(sock, ready);
        sock.setAttachment(ready);
        sock.send(JSON.stringify({ t: "ready", push: this.options.pushEnabled, kinds: PushKind.options }));
        return true;
      }
      if (result.data.t !== "push") { this.close(sock, 1008); return true; }
      const failed = result.data.tokens.map(() => ({ ok: false, unregistered: false }));
      let results = failed;
      if (!unknownPush && this.options.pushEnabled) {
        try {
          const data = PushData.parse({ v: "1", hostId: this.options.hostId, kind: result.data.kind, botId: result.data.botId,
            ...(result.data.roomId === undefined ? {} : { roomId: result.data.roomId }) });
          results = await this.options.push(result.data.tokens, data);
        } catch { results = failed; }
      }
      if (this.host === sock) {
        try { sock.send(JSON.stringify({ t: "pushed", id: result.data.id, results })); }
        catch { this.close(sock, RelayClose.hostDead); }
      }
      return false;
    }
    if (typeof message === "string") { this.close(sock, 1003); return true; }
    const frame = message instanceof Uint8Array ? message : new Uint8Array(message);
    if (frame.byteLength < 1 || frame.byteLength > RELAY.maxFrame) { this.close(sock, RelayClose.tooLarge); return true; }
    if (state.role === "device" && !state.paired) {
      const pending = this.pending.get(state.channel);
      if (!pending) { this.close(sock, RelayClose.hostOffline); return true; }
      if (pending.frames.length >= 8 || pending.bytes + frame.byteLength > 65_536) { this.close(sock, RelayClose.tooLarge); return true; }
      pending.frames.push(new Uint8Array(frame));
      pending.bytes += frame.byteLength;
      return false;
    }
    const pair = this.channels.get(state.channel);
    if (!pair) { this.close(sock, RelayClose.hostOffline); return true; }
    try { (state.role === "device" ? pair.host : pair.device).send(frame); }
    catch { this.close(sock, RelayClose.hostOffline); return true; }
    return false;
  }

  onClose(sock: Sock): void { this.close(sock, 1000, false); }

  private close(sock: Sock, code: number, notify = true): void {
    const state = this.states.get(sock);
    if (!state) return;
    this.states.delete(sock);
    if (notify) { sock.setAttachment(null); sock.close(code); }
    if (state.role === "control") {
      if (this.host !== sock) return;
      this.host = null;
      for (const pair of this.channels.values()) {
        this.states.delete(pair.device); this.states.delete(pair.host);
        pair.device.setAttachment(null); pair.host.setAttachment(null);
        pair.device.close(RelayClose.hostOffline); pair.host.close(RelayClose.hostOffline);
      }
      this.channels.clear();
      for (const pending of this.pending.values()) {
        this.states.delete(pending.device); pending.device.setAttachment(null); pending.device.close(RelayClose.hostOffline);
      }
      this.pending.clear();
    } else if (state.role === "device" && !state.paired) {
      this.pending.delete(state.channel);
    } else {
      const pair = this.channels.get(state.channel);
      if (!pair) return;
      this.channels.delete(state.channel);
      const other = pair.device === sock ? pair.host : pair.device;
      this.states.delete(other);
      other.setAttachment(null);
      other.close(code);
    }
  }

  sweep(lastPing: number | null): number | null {
    const now = this.options.now();
    for (const [sock, state] of this.states)
      if (state.role === "control" && state.stage === "challenge" && state.expires <= now) this.close(sock, RelayClose.badLogin);
    for (const pending of this.pending.values()) if (pending.expires <= now) this.close(pending.device, RelayClose.hostOffline);
    const state = this.host ? this.states.get(this.host) : null;
    if (state?.role === "control" && state.stage === "ready" && now - Math.max(state.since, lastPing ?? 0) >= RELAY.hostDeadMs)
      this.close(this.host!, RelayClose.hostDead);
    return this.nextDeadline(lastPing);
  }

  nextDeadline(lastPing: number | null): number | null {
    const state = this.host ? this.states.get(this.host) : null;
    let deadline = state?.role === "control" && state.stage === "ready" ? Math.max(state.since, lastPing ?? 0) + RELAY.hostDeadMs : null;
    for (const attachment of this.states.values()) if (attachment.role === "control" && attachment.stage === "challenge")
      deadline = deadline === null ? attachment.expires : Math.min(deadline, attachment.expires);
    for (const pending of this.pending.values()) deadline = deadline === null ? pending.expires : Math.min(deadline, pending.expires);
    return deadline;
  }
}
