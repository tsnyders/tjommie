"use client";

import { Capacitor } from "@capacitor/core";
import { z } from "zod";
import { LanEndpoint, RemoteKey, RemoteName, parsePairingCode, type RemotePreface } from "@tjommie/protocol";
import { connectTransport, type Client, type RpcTransport } from "@tjommie/client";
import { authenticateRemote } from "../../client/src/remote-peer.ts";
import type { FrameTransport } from "../../client/src/frame-socket.ts";
import { generateKeypair, initNoise } from "@tjommie/transport";
import { fromBase64, toBase64 } from "@tjommie/mobile";
import { appearance, scanner, secureStore, socket } from "../../mobile/src/plugins.ts";
import { decodeMobileIdentity, encodeMobileIdentity } from "./mobile-identity";

const MobileHostSchema = z.object({ id: z.uuid(), name: RemoteName, deviceId: z.uuid().nullable(), hostKey: RemoteKey, lan: z.array(LanEndpoint).min(1).max(16) }).strict();
export type MobileHost = z.infer<typeof MobileHostSchema>;
const HOSTS = "tjommie.mobile.hosts.v1";
const PENDING = "tjommie.mobile.pending.v1";
let initializing: Promise<void> | undefined;
export function isAndroidApp(): boolean { return Capacitor.isNativePlatform() && Capacitor.getPlatform() === "android"; }
// Appearance is best-effort: an older installed bridge must still be usable.
export function setNativeCanvas(canvas: { color: string; dark: boolean }): void {
  if (!isAndroidApp() || !/^#[0-9a-f]{6}$/i.test(canvas.color)) return;
  void appearance.setCanvas(canvas).catch(() => {});
  void appearance.getInsets().catch(() => {});
}
function readHosts(key: string): MobileHost[] {
  if (typeof localStorage === "undefined") return [];
  try { return z.array(MobileHostSchema).max(16).parse(JSON.parse(localStorage.getItem(key) ?? "[]")); }
  catch { return []; }
}
export function mobileHosts(): MobileHost[] {
  const current = readHosts(HOSTS);
  return [...current, ...readHosts(PENDING).filter((host) => !current.some((entry) => entry.id === host.id))];
}
function saveHost(host: MobileHost, key = HOSTS): void {
  const current = readHosts(key).filter((entry) => entry.id !== host.id);
  if (current.length >= 16) throw new Error("Forget a PC before pairing another one.");
  localStorage.setItem(key, JSON.stringify([MobileHostSchema.parse(host), ...current]));
}
const identityName = (id: string, pending = false) => `identity-${id}${pending ? ".pending" : ""}`;
async function clearPending(id: string): Promise<void> {
  await secureStore.delete({ name: identityName(id, true) });
  localStorage.setItem(PENDING, JSON.stringify(readHosts(PENDING).filter((host) => host.id !== id)));
}
async function promote(host: MobileHost): Promise<void> {
  const sealed = await secureStore.read({ name: identityName(host.id, true) });
  if (!sealed.valueBase64) throw new Error("The pending connection key is missing. Pair again.");
  const bytes = fromBase64(sealed.valueBase64);
  try { const key = decodeMobileIdentity(bytes, host.id, host.hostKey); key.fill(0); } finally { bytes.fill(0); }
  await secureStore.seal({ name: identityName(host.id), valueBase64: sealed.valueBase64 });
  saveHost(host);
  await clearPending(host.id);
}
async function initialize(): Promise<void> {
  if (!isAndroidApp()) throw new Error("Open this from the Android app.");
  if (!initializing) initializing = (async () => {
    const response = await fetch("/tjommie_noise_bg.wasm");
    if (!response.ok) throw new Error("The secure connection component is missing. Reinstall Tjommie.");
    initNoise(new Uint8Array(await response.arrayBuffer()));
  })().catch((error: unknown) => { initializing = undefined; throw error; });
  return initializing;
}
function keyBytes(value: string): Uint8Array { return fromBase64(value.replace(/-/g, "+").replace(/_/g, "/") + "="); }

async function openWire(endpoint: { host: string; port: number }): Promise<FrameTransport> {
  const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<() => void>();
  const subscriptions: { remove: () => Promise<void> }[] = [];
  let connectionId: string | null = null, closed = false, pendingBytes = 0;
  let sending = Promise.resolve();
  const earlyClosed = new Set<string>();
  const close = (): void => {
    if (closed) return; closed = true;
    for (const subscription of subscriptions.splice(0)) void subscription.remove().catch(() => {});
    if (connectionId) void socket.close({ connectionId }).catch(() => {});
    for (const listener of closes) listener();
  };
  try {
    subscriptions.push(await socket.addListener("frame", (event) => {
      if (event.connectionId !== connectionId || closed) return;
      try {
        const frame = fromBase64(event.dataBase64);
        if (frame.length === 0 || frame.length > 65535) throw new Error();
        for (const listener of frames) listener(frame);
      } catch { close(); }
    }));
    subscriptions.push(await socket.addListener("closed", (event) => {
      if (connectionId === null) { if (earlyClosed.size < 8) earlyClosed.add(event.connectionId); return; }
      if (event.connectionId === connectionId) close();
    }));
    connectionId = (await socket.open({ ...endpoint, timeoutMs: 1000 })).connectionId;
    if (earlyClosed.has(connectionId)) throw new Error();
    return {
      send: (frame) => {
        if (closed || !connectionId || frame.length === 0 || frame.length > 65535 || pendingBytes + frame.length > 32 * 1024 * 1024) { close(); throw new Error("The PC connection is closed."); }
        const id = connectionId; pendingBytes += frame.length;
        sending = sending.then(async () => { if (!closed) await socket.send({ connectionId: id, dataBase64: toBase64(frame) }); }).catch(close).finally(() => { pendingBytes -= frame.length; });
      },
      close,
      onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
      onClose: (listener) => { closes.add(listener); if (closed) queueMicrotask(listener); return () => { closes.delete(listener); }; },
    };
  } catch { close(); throw new Error("This PC could not be reached on Wi-Fi."); }
}

async function authenticate(host: MobileHost, privateKey: Uint8Array, preface: RemotePreface, psk?: Uint8Array, name = "My Android phone", onConfirmSent?: () => void) {
  const deadline = Date.now() + 30_000;
  for (const endpoint of host.lan) {
    if (Date.now() > deadline) break;
    let wire: FrameTransport | undefined;
    try {
      wire = await openWire(endpoint);
      return await authenticateRemote({ wire, preface, privateKey, hostPublicKey: keyBytes(host.hostKey), name, psk, onConfirmSent });
    } catch { wire?.close(); }
  }
  throw new Error("Can't reach or authenticate this PC. Keep both devices on the same Wi-Fi, keep Tjommie open on the PC, and check that this phone is still paired.");
}

export async function scanMobilePairing(): Promise<string | null> {
  if (!isAndroidApp()) return null;
  return (await scanner.scan()).text;
}

export async function pairMobile(input: string, name: string): Promise<MobileHost> {
  const code = parsePairingCode(input);
  RemoteName.parse(name);
  const existing = readHosts(HOSTS).find((host) => host.id === code.hostId);
  if (existing && existing.hostKey !== code.hostKey) throw new Error("This PC's connection key changed. Check the PC before forgetting it and pairing again.");
  await initialize();
  const keys = generateKeypair(), psk = keyBytes(code.secret);
  const host: MobileHost = { id: code.hostId, name: code.hostName, deviceId: null, hostKey: code.hostKey, lan: code.lan };
  let confirmationSent = false;
  try {
    // Save the candidate identity before sending confirmation. If the acknowledgement is lost after the
    // PC commits, reconnect can recover using this key without reusing the consumed invitation.
    const envelope = encodeMobileIdentity(host.id, host.hostKey, keys.privateKey);
    try { await secureStore.seal({ name: identityName(host.id, true), valueBase64: toBase64(envelope) }); }
    finally { envelope.fill(0); }
    const peer = await authenticate(host, keys.privateKey, { v: 1, hostId: host.id, mode: "pair", pairingId: code.pairingId }, psk, name, () => { confirmationSent = true; });
    peer.transport.close();
    host.deviceId = peer.deviceId; await promote(host);
    return host;
  } catch (error) {
    if (confirmationSent) saveHost(host, PENDING); // recovery may resume a committed pairing whose acknowledgement was lost
    else await clearPending(host.id);
    throw error;
  } finally { keys.privateKey.fill(0); psk.fill(0); }
}

export async function openMobileClient(id: string): Promise<Client> {
  await initialize();
  const candidates = [readHosts(HOSTS).find((host) => host.id === id), readHosts(PENDING).find((host) => host.id === id)];
  let failure: unknown = new Error("Pair this phone with your PC first.");
  for (const [index, host] of candidates.entries()) {
    if (!host) continue;
    const sealed = await secureStore.read({ name: identityName(id, index === 1) });
    if (!sealed.valueBase64) continue;
    let key: Uint8Array | undefined;
    let transport: RpcTransport | undefined;
    try {
      const envelope = fromBase64(sealed.valueBase64);
      try { key = decodeMobileIdentity(envelope, id, host.hostKey); } finally { envelope.fill(0); }
      const peer = await authenticate(host, key, { v: 1, mode: "resume", hostId: host.id });
      transport = peer.transport;
      host.deviceId = peer.deviceId; host.name = peer.hostName;
      if (index === 1) await promote(host); else saveHost(host);
      try { return await connectTransport(peer.transport, { recordEvents: false }); } catch (error) { peer.transport.close(); throw error; }
    } catch (error) { transport?.close(); failure = error; }
    finally { key?.fill(0); }
  }
  throw failure;
}

export async function forgetMobileHost(id: string): Promise<void> {
  await secureStore.delete({ name: identityName(id) });
  await clearPending(id);
  localStorage.setItem(HOSTS, JSON.stringify(readHosts(HOSTS).filter((host) => host.id !== id)));
}
