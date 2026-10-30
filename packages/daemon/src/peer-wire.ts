import type { Socket } from "node:net";
import { FrameDecoder, lengthPrefixed, MAX_NOISE_FRAME } from "../../client/src/encrypted-channel.ts";

export interface PeerWire {
  readonly kind: "lan" | "relay";
  readonly address: string;
  readonly buffered: number;
  write(frame: Uint8Array): void;
  destroy(): void;
  onFrame(listener: (frame: Uint8Array) => void): void;
  onClose(listener: () => void): void;
}

export function tcpWire(socket: Socket): PeerWire {
  const decoder = new FrameDecoder();
  const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<() => void>();
  let closed = false;
  socket.setNoDelay(true); socket.setKeepAlive(true, 30_000);
  socket.on("data", (chunk) => {
    if (socket.destroyed) return;
    try { for (const frame of decoder.feed(chunk)) {
      if (socket.destroyed) break;
      for (const listener of frames) { if (socket.destroyed) break; listener(frame); }
    } }
    catch { socket.destroy(); }
  });
  socket.on("error", () => socket.destroy());
  socket.on("close", () => { closed = true; for (const listener of closes) listener(); });
  return {
    kind: "lan", address: socket.remoteAddress ?? "unknown",
    get buffered() { return socket.writableLength; },
    write: (frame) => { if (socket.destroyed) throw new Error("Connection closed."); socket.write(lengthPrefixed(frame)); },
    destroy: () => socket.destroy(),
    onFrame: (listener) => { frames.add(listener); },
    onClose: (listener) => { closes.add(listener); if (closed) queueMicrotask(listener); },
  };
}

export function webSocketWire(socket: WebSocket): PeerWire {
  const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<() => void>();
  let closed = false;
  socket.binaryType = "arraybuffer";
  const destroy = (): void => { if (!closed) { closed = true; socket.close(); for (const listener of closes) listener(); } };
  socket.addEventListener("message", (event) => {
    if (closed) return;
    if (!(event.data instanceof ArrayBuffer) || event.data.byteLength === 0 || event.data.byteLength > MAX_NOISE_FRAME) { destroy(); return; }
    const frame = new Uint8Array(event.data);
    for (const listener of frames) { if (closed) break; listener(frame); }
  });
  socket.addEventListener("error", destroy);
  socket.addEventListener("close", destroy);
  return {
    kind: "relay", address: "relay", get buffered() { return socket.bufferedAmount; },
    write: (frame) => {
      if (closed || socket.readyState !== WebSocket.OPEN || frame.length === 0 || frame.length > MAX_NOISE_FRAME) { destroy(); throw new Error("Connection closed."); }
      try { socket.send(frame); } catch { destroy(); throw new Error("Connection closed."); }
    },
    destroy, onFrame: (listener) => { frames.add(listener); },
    onClose: (listener) => { closes.add(listener); if (closed) queueMicrotask(listener); },
  };
}
