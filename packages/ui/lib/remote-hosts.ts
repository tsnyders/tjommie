"use client";

import { Capacitor } from "@capacitor/core";
import { z } from "zod";
import { LanEndpoint, RelayClose, RelayUrl, RemoteKey, RemoteName, parsePairingCode, type RemotePreface } from "@tjommie/protocol";
import { connectTransport, type Client, type RpcTransport } from "@tjommie/client";
import { authenticateRemote, RemoteRefusedError } from "../../client/src/remote-peer.ts";
import type { FrameTransport, WireEndpoint } from "../../client/src/frame-socket.ts";
import { generateKeypair, initNoise } from "@tjommie/transport";
import { fromBase64, toBase64 } from "@tjommie/mobile";
import { appearance, scanner, secureStore } from "../../mobile/src/plugins.ts";
import { desktop } from "./desktop";
import { decodeMobileIdentity, encodeMobileIdentity } from "./mobile-identity";
import { WireOpenError, frameSockets } from "./relay-wire";
import { LINK } from "./state";

// The PCs this device is paired with, saved by the Android app and by the desktop app (for another PC). The relay is the PC's, as it last said.
const RemoteHostSchema = z.object({
  id: z.uuid(), name: RemoteName, deviceId: z.uuid().nullable(), hostKey: RemoteKey, lan: z.array(LanEndpoint).min(1).max(16), relay: RelayUrl.nullable().default(null),
}).strict();
export type RemoteHost = z.infer<typeof RemoteHostSchema>;
const HOSTS = "tjommie.mobile.hosts.v1";
const PENDING = "tjommie.mobile.pending.v1";
let initializing: Promise<void> | undefined;

// The PC's relay said the PC isn't connected to it (close 4404), and nothing reached it on the LAN either.
export class HostOfflineError extends Error {
  constructor() {
    super("Your PC is offline. Check that it's awake with Tjommie running.");
    this.name = "HostOfflineError";
  }
}

export function isAndroidApp(): boolean { return Capacitor.isNativePlatform() && Capacitor.getPlatform() === "android"; }
// Appearance is best-effort: an older installed bridge must still be usable.
export function setNativeCanvas(canvas: { color: string; dark: boolean }): void {
  if (!isAndroidApp() || !/^#[0-9a-f]{6}$/i.test(canvas.color)) return;
  void appearance.setCanvas(canvas).catch(() => {});
  void appearance.getInsets().catch(() => {});
}
function readHosts(key: string): RemoteHost[] {
  if (typeof localStorage === "undefined") return [];
  try { return z.array(RemoteHostSchema).max(16).parse(JSON.parse(localStorage.getItem(key) ?? "[]")); }
  catch { return []; }
}
export function remoteHosts(): RemoteHost[] {
  const current = readHosts(HOSTS);
  return [...current, ...readHosts(PENDING).filter((host) => !current.some((entry) => entry.id === host.id))];
}
function saveHost(host: RemoteHost, key = HOSTS): void {
  const current = readHosts(key).filter((entry) => entry.id !== host.id);
  if (current.length >= 16) throw new Error("Forget a PC before pairing another one.");
  localStorage.setItem(key, JSON.stringify([RemoteHostSchema.parse(host), ...current]));
}
const identityName = (id: string, pending = false) => `identity-${id}${pending ? ".pending" : ""}`;

// Where this device's identity for each PC is sealed: the Android Keystore, or the desktop app's credential store.
type Vault = { seal(name: string, value: Uint8Array): Promise<void>; read(name: string): Promise<Uint8Array | null>; delete(name: string): Promise<void> };
function vault(): Vault {
  const remote = desktop()?.remote;
  if (remote) return remote;
  return {
    seal: (name, value) => secureStore.seal({ name, valueBase64: toBase64(value) }),
    read: async (name) => {
      const { valueBase64 } = await secureStore.read({ name });
      return valueBase64 ? fromBase64(valueBase64) : null;
    },
    delete: (name) => secureStore.delete({ name }),
  };
}
async function clearPending(id: string): Promise<void> {
  await vault().delete(identityName(id, true));
  localStorage.setItem(PENDING, JSON.stringify(readHosts(PENDING).filter((host) => host.id !== id)));
}
async function promote(host: RemoteHost): Promise<void> {
  const sealed = await vault().read(identityName(host.id, true));
  if (!sealed) throw new Error("The pending connection key is missing. Pair again.");
  try {
    const key = decodeMobileIdentity(sealed, host.id, host.hostKey); key.fill(0);
    await vault().seal(identityName(host.id), sealed);
  } finally { sealed.fill(0); }
  saveHost(host);
  await clearPending(host.id);
}
async function initialize(): Promise<void> {
  if (!isAndroidApp() && !desktop()?.remote) throw new Error("Open this from the Tjommie app.");
  if (!initializing) initializing = (async () => {
    const response = await fetch("/tjommie_noise_bg.wasm");
    if (!response.ok) throw new Error("The secure connection component is missing. Reinstall Tjommie.");
    initNoise(new Uint8Array(await response.arrayBuffer()));
  })().catch((error: unknown) => { initializing = undefined; throw error; });
  return initializing;
}
function keyBytes(value: string): Uint8Array { return fromBase64(value.replace(/-/g, "+").replace(/_/g, "/") + "="); }
const unreachable = (host: RemoteHost): Error =>
  new Error(host.relay ? "Can't reach your PC. Check that it's awake with Tjommie open." : "Can't reach your PC. Keep both devices on the same Wi-Fi, and keep Tjommie open on the PC.");

type Peer = Awaited<ReturnType<typeof authenticateRemote>>;

// Every LAN endpoint is opened at once; the first to finish the Noise handshake wins and the others close. The relay joins once the LAN window
// passes, or sooner when every LAN endpoint has failed. A refusal is the PC's own authenticated answer, so it ends the race. All within 30 s.
function authenticate(host: RemoteHost, privateKey: Uint8Array, preface: RemotePreface, psk?: Uint8Array, name = "My Android phone", onConfirmSent?: () => void): Promise<Peer> {
  const sockets = frameSockets(), hostPublicKey = keyBytes(host.hostKey), deadlineAt = Date.now() + 30_000;
  return new Promise((resolve, reject) => {
    const wires = new Set<FrameTransport>();
    let settled = false, running = 0, relayStarted = host.relay === null, offline = false;
    const end = (): void => {
      settled = true; clearTimeout(lanWindow); clearTimeout(deadline);
      for (const wire of wires) wire.close();
    };
    const fail = (error: Error): void => { if (!settled) { end(); reject(error); } };
    const leg = (endpoint: WireEndpoint, timeoutMs: number): void => {
      running += 1;
      void (async () => {
        let wire: FrameTransport | undefined;
        try {
          wire = await sockets.open(endpoint, timeoutMs);
          if (settled) { wire.close(); return; }
          wires.add(wire);
          wire.onClose((code) => { if (code === RelayClose.hostOffline) offline = true; });
          const peer = await authenticateRemote({ wire, preface, privateKey, hostPublicKey, name, psk, onConfirmSent });
          if (settled) { peer.transport.close(); return; }
          wires.delete(wire);
          end();
          resolve(peer);
        } catch (error) {
          wire?.close();
          if (error instanceof RemoteRefusedError) fail(error);
          else if (error instanceof WireOpenError && error.code === RelayClose.hostOffline) offline = true; // closed before its open returned
        } finally {
          running -= 1;
          if (!settled && running === 0) {
            if (relayStarted) fail(offline ? new HostOfflineError() : unreachable(host));
            else relay();
          }
        }
      })();
    };
    const relay = (): void => {
      if (relayStarted || settled || host.relay === null) return;
      relayStarted = true;
      leg({ kind: "relay", url: host.relay, hostId: host.id }, Math.max(1, deadlineAt - Date.now()));
    };
    const lanWindow = setTimeout(relay, LINK.lanWindowMs);
    const deadline = setTimeout(() => fail(unreachable(host)), 30_000);
    for (const endpoint of host.lan) leg({ kind: "lan", ...endpoint }, LINK.lanWindowMs);
  });
}

export async function scanMobilePairing(): Promise<string | null> {
  if (!isAndroidApp()) return null;
  return (await scanner.scan()).text;
}

export async function pairRemote(input: string, name: string): Promise<RemoteHost> {
  const code = parsePairingCode(input);
  RemoteName.parse(name);
  const existing = readHosts(HOSTS).find((host) => host.id === code.hostId);
  if (existing && existing.hostKey !== code.hostKey) throw new Error("This PC's connection key changed. Check the PC before forgetting it and pairing again.");
  await initialize();
  const keys = generateKeypair(), psk = keyBytes(code.secret);
  const host: RemoteHost = { id: code.hostId, name: code.hostName, deviceId: null, hostKey: code.hostKey, lan: code.lan, relay: code.relay };
  let confirmationSent = false;
  try {
    // Save the candidate identity before sending confirmation. If the acknowledgement is lost after the
    // PC commits, reconnect can recover using this key without reusing the consumed invitation.
    const envelope = encodeMobileIdentity(host.id, host.hostKey, keys.privateKey);
    try { await vault().seal(identityName(host.id, true), envelope); }
    finally { envelope.fill(0); }
    const peer = await authenticate(host, keys.privateKey, { v: 1, hostId: host.id, mode: "pair", pairingId: code.pairingId }, psk, name, () => { confirmationSent = true; });
    peer.transport.close();
    host.deviceId = peer.deviceId; host.relay = peer.relay; await promote(host);
    return host;
  } catch (error) {
    if (confirmationSent) saveHost(host, PENDING); // recovery may resume a committed pairing whose acknowledgement was lost
    else await clearPending(host.id);
    throw error;
  } finally { keys.privateKey.fill(0); psk.fill(0); }
}

export async function openRemoteClient(id: string): Promise<Client> {
  await initialize();
  const candidates = [readHosts(HOSTS).find((host) => host.id === id), readHosts(PENDING).find((host) => host.id === id)];
  let failure: unknown = new Error("Pair this device with your PC first.");
  for (const [index, host] of candidates.entries()) {
    if (!host) continue;
    const sealed = await vault().read(identityName(id, index === 1));
    if (!sealed) continue;
    let key: Uint8Array | undefined;
    let transport: RpcTransport | undefined;
    try {
      try { key = decodeMobileIdentity(sealed, id, host.hostKey); } finally { sealed.fill(0); }
      const peer = await authenticate(host, key, { v: 1, mode: "resume", hostId: host.id });
      transport = peer.transport;
      host.deviceId = peer.deviceId; host.name = peer.hostName; host.relay = peer.relay;
      if (index === 1) await promote(host); else saveHost(host);
      try { return await connectTransport(peer.transport, { recordEvents: false }); } catch (error) { peer.transport.close(); throw error; }
    } catch (error) {
      transport?.close();
      if (!(failure instanceof RemoteRefusedError)) failure = error; // the PC refusing one identity outweighs the other one not getting through
    } finally { key?.fill(0); }
  }
  throw failure;
}

export async function forgetRemoteHost(id: string): Promise<void> {
  await vault().delete(identityName(id));
  await clearPending(id);
  localStorage.setItem(HOSTS, JSON.stringify(readHosts(HOSTS).filter((host) => host.id !== id)));
}
