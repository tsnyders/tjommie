import { createHash, randomBytes } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { createServer, type Server as TcpServer } from "node:net";
import { hostname, networkInterfaces } from "node:os";
import { fileURLToPath } from "node:url";
import { ClientMessage, PROTOCOL_VERSION, RemoteConfirm, RemotePreface, encodePairingCode, remoteMethods, remotePrologue, type Device, type EventData, type EventName, type RemoteStatus } from "@tjommie/protocol";
import { ZodError } from "zod";
import { EncryptedMessages } from "../../client/src/encrypted-channel.ts";
import { generateKeypair, initNoise, responder, type NoiseSession } from "../../transport/src/index.ts";
import { addDevice, clearPushToken as clearStoredPushToken, deviceForKey, getDevice, listDevices, pushTargets as storedPushTargets, revokeDevice, setPushToken, touchDevice } from "./devices-store.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { tcpWire, type PeerWire } from "./peer-wire.ts";
import { loadRemoteIdentity, type HostIdentity } from "./remote-identity.ts";
import { RpcError } from "./server.ts";
import type { CredentialTransport } from "./account-credentials.ts";

type Invite = { id: string; secret: Uint8Array; expires: number };
type Peer = { wire: PeerWire; deviceId: string | null; welcomed: boolean; lastFrameAt: number; send: (value: unknown) => void; close: () => void };
type Options = {
  db: Db; root: string; dispatch: (method: string, params: unknown) => Promise<unknown>; onChanged?: () => void;
  credentials?: CredentialTransport; port?: number; bind?: string; addresses?: () => string[]; relayUrl?: (forPairing: boolean) => string | null;
};
export const REMOTE_PORT = 47101;
const empty = new Uint8Array();
const keyText = (key: Uint8Array) => Buffer.from(key).toString("base64url");

export function initializeHostNoise(): void {
  const source = new URL("../../transport/wasm/tjommie_noise_bg.wasm", import.meta.url);
  const asset = existsSync(source) ? source : new URL("./tjommie_noise_bg.wasm", import.meta.url);
  initNoise(new Uint8Array(readFileSync(fileURLToPath(asset))));
}

export class RemoteHost {
  #options: Options;
  #identity: HostIdentity | null = null;
  #listener: TcpServer | null = null;
  #opening: Promise<void> | null = null;
  #closed = false;
  #invites = new Map<string, Invite>();
  #peers = new Set<Peer>();
  #attempts = new Map<string, { at: number; count: number }>();
  #timer: ReturnType<typeof setInterval> | undefined;
  constructor(options: Options) {
    this.#options = { relayUrl: () => null, ...options };
    // A PSK exists only in the process that created it. An app restart cancels outstanding codes.
    options.db.prepare("DELETE FROM pairing_codes WHERE used_at IS NULL").run();
  }
  status(): RemoteStatus {
    return { hostId: this.#identity?.id ?? null, hostName: this.#identity?.name ?? hostname(), listening: this.#listener !== null,
      port: this.#listener ? (this.#listener.address() as { port: number }).port : null,
      connectedDeviceIds: [...new Set([...this.#peers].flatMap((peer) => peer.deviceId ? [peer.deviceId] : []))],
      relay: { url: this.#options.relayUrl?.(false) ?? null, state: "off", push: false } };
  }
  async start(): Promise<void> {
    if (this.#closed) throw new RpcError("bad_state", "The phone connection is closed.");
    if (this.#listener) return;
    if (this.#opening) return this.#opening;
    this.#opening = this.#start();
    try { await this.#opening; } finally { this.#opening = null; }
  }
  ensureListening(): Promise<void> { return this.start(); }
  async #start(): Promise<void> {
    initializeHostNoise();
    const identity = this.#identity ?? await loadRemoteIdentity({ db: this.#options.db, root: this.#options.root, name: hostname(), generate: generateKeypair, credentials: this.#options.credentials });
    if (this.#closed) { identity.privateKey.fill(0); throw new Error("The phone connection is closed."); }
    this.#identity = identity;
    if (!this.#timer) { this.#timer = setInterval(() => this.#expire(), 30_000); this.#timer.unref(); }
    const listener = createServer((socket) => this.acceptWire(tcpWire(socket)));
    await new Promise<void>((resolve, reject) => {
      listener.once("error", reject);
      listener.listen(this.#options.port ?? REMOTE_PORT, this.#options.bind ?? "0.0.0.0", () => { listener.off("error", reject); resolve(); });
    });
    if (this.#closed) { listener.close(); throw new Error("The phone connection is closed."); }
    listener.on("error", () => { /* Connection failure is reported by the next explicit pairing attempt. */ });
    this.#listener = listener;
  }
  async createPairing(): Promise<{ id: string; code: string; uri: string; expires: string }> {
    await this.start();
    const identity = this.#identity;
    if (!identity || !this.#listener || this.#closed) throw new RpcError("bad_state", "The phone connection is unavailable.");
    const addresses = this.#options.addresses?.() ?? Object.values(networkInterfaces()).flatMap((entries) => (entries ?? []).filter((entry) => !entry.internal && entry.family === "IPv4").map((entry) => entry.address));
    if (!addresses.length) throw new RpcError("bad_state", "Connect this PC to Wi-Fi or Ethernet before pairing a phone.");
    for (const id of [...this.#invites.keys()]) this.cancelPairing(id);
    const id = newId(), secret = new Uint8Array(randomBytes(32)), expires = Date.now() + 600_000;
    this.#options.db.prepare("INSERT INTO pairing_codes(id,secret_hash,expires_at,created_at) VALUES(?,?,?,?)").run(id, createHash("sha256").update(secret).digest("hex"), new Date(expires).toISOString(), nowIso());
    this.#invites.set(id, { id, secret, expires });
    const code = encodePairingCode({ v: 2, relay: this.#options.relayUrl?.(true) ?? null, pairingId: id, hostId: identity.id, hostName: identity.name, hostKey: keyText(identity.publicKey), secret: keyText(secret),
      lan: [...new Set(addresses)].slice(0, 16).map((host) => ({ host, port: (this.#listener!.address() as { port: number }).port })), expires: new Date(expires).toISOString() });
    return { id, code, uri: `tjommie://pair#${code}`, expires: new Date(expires).toISOString() };
  }
  cancelPairing(id: string): void {
    const invite = this.#invites.get(id); invite?.secret.fill(0); this.#invites.delete(id);
    this.#options.db.prepare("DELETE FROM pairing_codes WHERE id = ? AND used_at IS NULL").run(id);
  }
  #expire(): void {
    for (const [id, invite] of this.#invites) if (invite.expires <= Date.now()) this.cancelPairing(id);
    for (const [address, value] of this.#attempts) if (Date.now() - value.at > 60_000) this.#attempts.delete(address);
    for (const peer of this.#peers) if (Date.now() - peer.lastFrameAt >= 150_000) peer.close();
  }
  pushTargets(): { deviceId: string; token: string }[] { return storedPushTargets(this.#options.db); }
  clearPushToken(deviceId: string, token: string): void { clearStoredPushToken(this.#options.db, deviceId, token); }
  revoke(id: string): Device {
    const device = revokeDevice(this.#options.db, id);
    for (const peer of this.#peers) if (peer.deviceId === id) peer.close();
    this.#options.onChanged?.(); return device;
  }
  broadcast<E extends EventName>(event: E, data: EventData<E>): void {
    for (const peer of this.#peers) {
      if (!peer.deviceId || !peer.welcomed) continue;
      try {
        if (getDevice(this.#options.db, peer.deviceId).revokedAt) { peer.close(); continue; }
        peer.send({ type: "event", event, data });
      } catch { peer.close(); }
    }
  }
  acceptWire(wire: PeerWire): void {
    this.#expire();
    const address = wire.address, previous = this.#attempts.get(address);
    const attempt = previous ?? { at: Date.now(), count: 0 };
    if (wire.kind === "lan") {
      attempt.count++;
      if (this.#attempts.size < 256 || previous) this.#attempts.set(address, attempt);
    }
    const pending = [...this.#peers].filter((peer) => !peer.deviceId && peer.wire.kind === wire.kind);
    const active = [...this.#peers].filter((peer) => peer.deviceId).length;
    if (this.#closed || active >= 32 || wire.kind === "lan" && (pending.filter((peer) => peer.wire.address === address).length >= 4 || attempt.count > 20) || !this.#identity) { wire.destroy(); return; }
    // Rotate only within this wire kind so relay attempts cannot evict LAN handshakes.
    if (pending.length >= 8) { pending[0].close(); this.#peers.delete(pending[0]); }
    const identity = this.#identity;
    let phase: "preface" | "handshake" | "confirm" | "refused" | "hello" | "rpc" = "preface";
    let preface: RemotePreface | null = null, noise: NoiseSession | null = null, messages: EncryptedMessages | null = null;
    let inFlight = 0, done = false, refusalTimer: ReturnType<typeof setTimeout> | undefined;
    const pendingIds = new Set<number>();
    const close = (): void => {
      if (done) return; done = true; clearTimeout(timer); clearTimeout(refusalTimer); messages?.close(); noise?.close(); wire.destroy();
    };
    const write = (frame: Uint8Array): void => {
      if (done || wire.buffered + frame.length > 32 * 1024 * 1024) throw new Error("Connection closed.");
      wire.write(frame);
    };
    const send = (value: unknown): void => {
      if (!messages) throw new Error();
      try { for (const frame of messages.encode(JSON.stringify(value))) write(frame); } catch { close(); }
    };
    const peer: Peer = { wire, deviceId: null, welcomed: false, lastFrameAt: Date.now(), send, close };
    const timer = setTimeout(close, 15_000); timer.unref();
    this.#peers.add(peer);
    wire.onClose(() => {
      close(); this.#peers.delete(peer);
      if (peer.welcomed && !this.#closed) {
        if (peer.deviceId) { try { touchDevice(this.#options.db, peer.deviceId); } catch { /* Revoked devices stay revoked. */ } }
        this.#options.onChanged?.();
      }
    });
    wire.onFrame((frame) => {
      try {
        if (done) return;
        peer.lastFrameAt = Date.now();
        if (phase === "refused") return;
        if (phase === "preface") {
          if (frame.length > 2048) throw new Error();
          preface = RemotePreface.parse(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(frame)));
          if (preface.hostId !== identity.id) throw new Error();
          const invite = preface.mode === "pair" ? this.#invites.get(preface.pairingId) : undefined;
          if (preface.mode === "pair" && (!invite || invite.expires <= Date.now())) throw new Error();
          noise = responder({ privateKey: identity.privateKey, ...(invite ? { psk: invite.secret } : {}), prologue: remotePrologue(preface) });
          phase = "handshake"; return;
        }
        if (phase === "handshake") {
          if (!noise || !preface || noise.readHandshake(frame).length !== 0) throw new Error();
          write(noise.writeHandshake(empty));
          if (!noise.handshakeFinished) throw new Error();
          messages = new EncryptedMessages(noise); phase = "confirm"; return;
        }
        if (!messages || !noise || !preface) throw new Error();
        const text = messages.receive(frame); if (text === null) return;
        if (phase === "confirm") {
          const confirm = RemoteConfirm.parse(JSON.parse(text));
          const remote = noise.remotePublicKey; if (!remote) throw new Error();
          let device: Device | null = null;
          if (preface.mode === "pair") {
            const invite = this.#invites.get(preface.pairingId);
            if (!invite || invite.expires <= Date.now()) throw new Error();
            transaction(this.#options.db, () => {
              const used = this.#options.db.prepare("UPDATE pairing_codes SET used_at = ? WHERE id = ? AND used_at IS NULL AND expires_at > ?").run(nowIso(), invite.id, nowIso());
              if (used.changes !== 1) throw new Error();
              device = addDevice(this.#options.db, { publicKey: keyText(remote), name: confirm.name });
            });
            invite.secret.fill(0); this.#invites.delete(invite.id);
          } else {
            device = deviceForKey(this.#options.db, keyText(remote));
            if (!device || device.revokedAt) {
              send({ type: "refused", reason: device ? "revoked" : "unknown" });
              // Give the encrypted frame time to leave TCP before destroying the wire.
              if (!done) { phase = "refused"; refusalTimer = setTimeout(close, 1_000); refusalTimer.unref(); }
              return;
            }
          }
          if (!device || [...this.#peers].filter((entry) => entry.deviceId).length >= 32) throw new Error();
          peer.deviceId = device.id; touchDevice(this.#options.db, peer.deviceId);
          send({ type: "ready", deviceId: peer.deviceId, hostId: identity.id, hostName: identity.name, relay: this.#options.relayUrl?.(false) ?? null }); phase = "hello";
          return;
        }
        if (!peer.deviceId || getDevice(this.#options.db, peer.deviceId).revokedAt) throw new Error();
        const request = ClientMessage.parse(JSON.parse(text));
        if (phase === "hello") {
          if (request.type !== "hello" || request.protocol !== PROTOCOL_VERSION || request.token !== "device") throw new Error();
          send({ type: "welcome", protocol: PROTOCOL_VERSION }); phase = "rpc"; peer.welcomed = true; clearTimeout(timer); this.#options.onChanged?.(); return;
        }
        if (request.type !== "req" || inFlight >= 32 || pendingIds.has(request.id)) throw new Error();
        inFlight++; pendingIds.add(request.id);
        void Promise.resolve().then(async () => {
          if (done || !peer.deviceId || getDevice(this.#options.db, peer.deviceId).revokedAt) return;
          let result: unknown;
          if (request.method === "remote.ping") { remoteMethods["remote.ping"].params.parse(request.params); result = {}; }
          else if (request.method === "device.registerPush") {
            setPushToken(this.#options.db, peer.deviceId, remoteMethods["device.registerPush"].params.parse(request.params).token);
            result = {};
          } else result = await this.#options.dispatch(request.method, request.params);
          if (!done && peer.deviceId && !getDevice(this.#options.db, peer.deviceId).revokedAt) send({ type: "res", id: request.id, ok: true, result });
        }).catch((error: unknown) => {
          if (done) return;
          const code = error instanceof RpcError ? error.code : error instanceof ZodError ? "bad_params" : "internal";
          send({ type: "res", id: request.id, ok: false, error: { code, message: error instanceof RpcError ? error.message : "This request could not be completed on the PC." } });
        }).finally(() => { inFlight--; pendingIds.delete(request.id); });
      } catch { close(); }
    });
  }
  async close(): Promise<void> {
    if (this.#closed) return; this.#closed = true; clearInterval(this.#timer);
    for (const invite of this.#invites.values()) invite.secret.fill(0); this.#invites.clear();
    for (const peer of this.#peers) peer.close();
    const listener = this.#listener; this.#listener = null;
    if (listener) await new Promise<void>((resolve) => listener.close(() => resolve()));
    if (this.#opening) await this.#opening.catch(() => {});
    this.#identity?.privateKey.fill(0); this.#identity = null;
  }
  hasDevices(): boolean { return listDevices(this.#options.db).length > 0; }
}
