import { randomUUID } from "node:crypto";
import { createConnection } from "node:net";
import type { WebContents } from "electron";
import { LanEndpoint, RelayUrl } from "@tjommie/protocol";
import { FrameDecoder, lengthPrefixed, MAX_NOISE_FRAME } from "../../client/src/encrypted-channel.ts";
import { relayConnectUrl, webSocketFrames, type FrameTransport, type WireEndpoint } from "../../client/src/frame-socket.ts";

const MAX_WIRES = 8;
const MAX_BACKLOG = 32 * 1024 * 1024;
const failed = () => new Error("The PC connection is unavailable.");

function endpointOf(value: unknown): WireEndpoint {
  if (!value || typeof value !== "object" || !("kind" in value)) throw failed();
  if (value.kind === "lan") {
    // The wire endpoint also has a kind, so validate only the two LAN fields.
    const lan = LanEndpoint.safeParse({ host: "host" in value ? value.host : undefined, port: "port" in value ? value.port : undefined });
    if (!lan.success) throw failed();
    return { kind: "lan", ...lan.data };
  }
  if (value.kind === "relay" && "url" in value && "hostId" in value && typeof value.hostId === "string" &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.hostId)) {
    const url = RelayUrl.safeParse(value.url);
    if (url.success) return { kind: "relay", url: url.data, hostId: value.hostId };
  }
  throw failed();
}

function tcpFrames(endpoint: { host: string; port: number }, timeoutMs: number, signal: AbortSignal): Promise<FrameTransport> {
  return new Promise((resolve, reject) => {
    const socket = createConnection(endpoint);
    socket.setNoDelay(true);
    const decoder = new FrameDecoder();
    const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<(code?: number) => void>();
    let opened = false, closed = false;
    const close = (): void => {
      if (closed) return;
      closed = true;
      clearTimeout(timer);
      signal.removeEventListener("abort", close);
      socket.destroy();
      if (!opened) reject(failed());
      for (const listener of closes) listener();
    };
    const timer = setTimeout(close, timeoutMs);
    timer.unref();
    signal.addEventListener("abort", close, { once: true });
    socket.once("connect", () => {
      if (closed) return;
      opened = true;
      clearTimeout(timer);
      resolve({
        send: (frame) => {
          if (closed || frame.length === 0 || frame.length > MAX_NOISE_FRAME || socket.writableLength + frame.length + 4 > MAX_BACKLOG) {
            close(); throw failed();
          }
          try { socket.write(lengthPrefixed(frame)); } catch { close(); throw failed(); }
        },
        close,
        onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
        onClose: (listener) => { closes.add(listener); if (closed) queueMicrotask(() => listener()); return () => { closes.delete(listener); }; },
      });
    });
    socket.on("data", (chunk: Buffer) => {
      if (closed) return;
      try { for (const frame of decoder.feed(chunk)) for (const listener of frames) listener(frame); }
      catch { close(); }
    });
    socket.once("error", close);
    socket.once("close", close);
  });
}

type Active = { kind: WireEndpoint["kind"]; transport: FrameTransport; finish(code?: number): void };
type Pending = { kind: WireEndpoint["kind"]; canceled: boolean; cancel(): void };

export class RemoteWires {
  #active = new Map<string, Active>();
  #pending = new Set<Pending>();
  #owner: WebContents;
  #relay: typeof webSocketFrames;
  #lan: typeof tcpFrames;

  constructor(owner: WebContents, relay: typeof webSocketFrames = webSocketFrames, lan: typeof tcpFrames = tcpFrames) {
    this.#owner = owner;
    this.#relay = relay;
    this.#lan = lan;
    owner.on("did-navigate", () => this.closeAll());
    owner.on("render-process-gone", () => this.closeAll());
    owner.on("destroyed", () => this.closeAll());
  }

  async open(rawEndpoint: unknown, rawTimeoutMs: unknown): Promise<{ id: string }> {
    const endpoint = endpointOf(rawEndpoint);
    if (typeof rawTimeoutMs !== "number" || !Number.isInteger(rawTimeoutMs) || rawTimeoutMs < 1 || rawTimeoutMs > 60_000 ||
        [...this.#active.values(), ...this.#pending].filter((wire) => wire.kind === endpoint.kind).length >= MAX_WIRES) throw failed();
    const controller = new AbortController();
    const pending: Pending = { kind: endpoint.kind, canceled: false, cancel: () => { if (endpoint.kind === "lan") controller.abort(); } };
    this.#pending.add(pending);
    let transport: FrameTransport;
    try {
      transport = endpoint.kind === "lan" ? await this.#lan(endpoint, rawTimeoutMs, controller.signal) :
        await this.#relay(relayConnectUrl(endpoint.url, endpoint.hostId), rawTimeoutMs);
    } catch {
      this.#pending.delete(pending);
      throw failed();
    }
    this.#pending.delete(pending);
    if (pending.canceled || this.#owner.isDestroyed()) { transport.close(); throw failed(); }
    const id = randomUUID();
    let offFrame = () => {}, offClose = () => {};
    const finish = (code?: number): void => {
      if (!this.#active.delete(id)) return;
      offFrame(); offClose(); transport.close();
      this.#emit("remote:closed", id, code);
    };
    this.#active.set(id, { kind: endpoint.kind, transport, finish });
    offFrame = transport.onFrame((frame) => { if (!this.#emit("remote:frame", id, frame)) finish(); });
    offClose = transport.onClose(finish);
    return { id };
  }

  async send(rawId: unknown, rawFrame: unknown): Promise<void> {
    const active = typeof rawId === "string" ? this.#active.get(rawId) : undefined;
    if (!active || !(rawFrame instanceof Uint8Array)) throw failed();
    try { active.transport.send(rawFrame); } catch { active.finish(); throw failed(); }
  }

  close(rawId: unknown): void {
    if (typeof rawId === "string") this.#active.get(rawId)?.finish();
  }

  closeAll(): void {
    // shortcut: relay dials stay counted until open or timeout, upgrade when FrameSocket supports cancellation.
    for (const pending of this.#pending) { pending.canceled = true; pending.cancel(); }
    for (const active of this.#active.values()) active.finish();
  }

  #emit(channel: string, id: string, value: Uint8Array | number | undefined): boolean {
    if (this.#owner.isDestroyed()) return false;
    try { this.#owner.send(channel, id, value); return true; } catch { return false; }
  }
}
