import { RelayUrl } from "@tjommie/protocol";
import { MAX_NOISE_FRAME } from "./encrypted-channel.ts";

export interface FrameTransport {
  send(frame: Uint8Array): void;
  close(): void;
  onFrame(listener: (frame: Uint8Array) => void): () => void;
  onClose(listener: (code?: number) => void): () => void;
}

export type WireEndpoint = { kind: "lan"; host: string; port: number } | { kind: "relay"; url: string; hostId: string };
export interface FrameSocketApi { open(endpoint: WireEndpoint, timeoutMs: number): Promise<FrameTransport> }

export function relayWebSocketUrl(base: string, path: string): string {
  const url = new URL(RelayUrl.parse(base));
  url.protocol = "wss:";
  url.pathname = path;
  url.search = "";
  url.hash = "";
  return url.toString();
}

export function relayConnectUrl(base: string, hostId: string): string {
  return relayWebSocketUrl(base, `/v1/hosts/${encodeURIComponent(hostId)}/connect`);
}

export function webSocketFrames(url: string, timeoutMs: number): Promise<FrameTransport> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<(code?: number) => void>();
    let opened = false, closed = false, closeCode: number | undefined;
    const close = (reason: string, code?: number): void => {
      if (closed) return;
      closed = true; closeCode = code; clearTimeout(timer); socket.close();
      if (!opened) reject(new Error(reason));
      for (const listener of closes) listener(code);
    };
    const timer = setTimeout(() => close("Relay connection timed out."), timeoutMs);
    socket.addEventListener("open", () => {
      if (closed) return;
      opened = true; clearTimeout(timer);
      resolve({
        send: (frame) => {
          if (closed || socket.readyState !== WebSocket.OPEN || frame.length === 0 || frame.length > MAX_NOISE_FRAME || socket.bufferedAmount + frame.length > 32 * 1024 * 1024) {
            close("Relay connection closed."); throw new Error("Relay connection closed.");
          }
          try { socket.send(frame); } catch { close("Relay connection closed."); throw new Error("Relay connection closed."); }
        },
        close: () => close("Relay connection closed."),
        onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
        onClose: (listener) => { closes.add(listener); if (closed) queueMicrotask(() => listener(closeCode)); return () => { closes.delete(listener); }; },
      });
    });
    socket.addEventListener("message", (event) => {
      if (closed) return;
      if (!(event.data instanceof ArrayBuffer) || event.data.byteLength === 0 || event.data.byteLength > MAX_NOISE_FRAME) { close("Invalid relay frame."); return; }
      const frame = new Uint8Array(event.data);
      for (const listener of frames) listener(frame);
    });
    socket.addEventListener("error", () => close("Relay connection failed."));
    socket.addEventListener("close", (event) => close("Relay connection closed.", event.code));
  });
}
