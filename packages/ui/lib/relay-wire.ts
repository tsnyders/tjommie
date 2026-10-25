import { fromBase64, toBase64 } from "@tjommie/mobile";
import { MAX_NOISE_FRAME } from "../../client/src/encrypted-channel.ts";
import { relayConnectUrl, webSocketFrames, type FrameSocketApi, type FrameTransport, type WireEndpoint } from "../../client/src/frame-socket.ts";
import { socket } from "../../mobile/src/plugins.ts";
import { desktop, type DesktopRemote } from "./desktop";

// One wire on the native side: Android's TjommieSocket or the desktop app's bridge, each carrying one Noise frame per message.
type NativeWire = {
  listen(frame: (id: string, frame: Uint8Array) => void, closed: (id: string, code?: number) => void): Promise<() => void>;
  open(): Promise<string>;
  send(id: string, frame: Uint8Array): Promise<void>;
  close(id: string): void;
};

// A wire that couldn't open. When it closed before its open returned, the relay's close code comes along (4404: the PC is offline).
export class WireOpenError extends Error {
  readonly code: number | undefined;
  constructor(message: string, code?: number) {
    super(message);
    this.name = "WireOpenError";
    this.code = code;
  }
}

// A native wire as a FrameTransport. Frames go out one at a time, in order; a bad frame or more than 32 MiB waiting closes it. A close that
// arrives before the open says which wire is ours fails the open, with its code.
async function nativeFrames(native: NativeWire, failure: string): Promise<FrameTransport> {
  const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<(code?: number) => void>();
  const earlyClosed = new Map<string, number | undefined>();
  let id: string | null = null, closed = false, closeCode: number | undefined, pendingBytes = 0;
  let sending = Promise.resolve(), unlisten = (): void => {};
  const close = (code?: number): void => {
    if (closed) return;
    closed = true; closeCode = code;
    unlisten();
    if (id) native.close(id);
    for (const listener of closes) listener(code);
  };
  try {
    unlisten = await native.listen(
      (frameId, frame) => {
        if (frameId !== id || closed) return;
        try {
          if (frame.length === 0 || frame.length > MAX_NOISE_FRAME) throw new Error();
          for (const listener of frames) listener(frame);
        } catch { close(); }
      },
      (closedId, code) => {
        if (id === null) { if (earlyClosed.size < 8) earlyClosed.set(closedId, code); return; }
        if (closedId === id) close(code);
      },
    );
    id = await native.open();
    if (earlyClosed.has(id)) throw new WireOpenError(failure, earlyClosed.get(id));
    const wire = id;
    return {
      send: (frame) => {
        if (closed || frame.length === 0 || frame.length > MAX_NOISE_FRAME || pendingBytes + frame.length > 32 * 1024 * 1024) { close(); throw new Error("The PC connection is closed."); }
        pendingBytes += frame.length;
        sending = sending.then(async () => { if (!closed) await native.send(wire, frame); }).catch(() => close()).finally(() => { pendingBytes -= frame.length; });
      },
      close: () => close(),
      onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
      onClose: (listener) => { closes.add(listener); if (closed) queueMicrotask(() => listener(closeCode)); return () => { closes.delete(listener); }; },
    };
  } catch (error) {
    close();
    throw error instanceof WireOpenError ? error : new WireOpenError(failure);
  }
}

// A frame the bridge can't decode arrives empty, which closes its wire.
function decode(text: string): Uint8Array {
  try { return fromBase64(text); } catch { return new Uint8Array(); }
}

const tcp = (endpoint: { host: string; port: number }, timeoutMs: number): NativeWire => ({
  listen: async (frame, closed) => {
    const subscriptions: { remove: () => Promise<void> }[] = [];
    const unlisten = (): void => { for (const subscription of subscriptions.splice(0)) void subscription.remove().catch(() => {}); };
    try {
      subscriptions.push(await socket.addListener("frame", (event) => frame(event.connectionId, decode(event.dataBase64))));
      subscriptions.push(await socket.addListener("closed", (event) => closed(event.connectionId)));
    } catch (error) { unlisten(); throw error; }
    return unlisten;
  },
  open: async () => (await socket.open({ ...endpoint, timeoutMs })).connectionId,
  send: (id, frame) => socket.send({ connectionId: id, dataBase64: toBase64(frame) }),
  close: (id) => { void socket.close({ connectionId: id }).catch(() => {}); },
});

const bridged = (remote: DesktopRemote, endpoint: WireEndpoint, timeoutMs: number): NativeWire => ({
  listen: async (frame, closed) => {
    const off = [remote.onFrame(frame), remote.onClosed(closed)];
    return () => { for (const each of off) each(); };
  },
  open: async () => (await remote.open(endpoint, timeoutMs)).id,
  send: (id, frame) => remote.send(id, frame),
  close: (id) => remote.close(id),
});

// This device's wires to a PC. The desktop app opens every wire in its main process; on Android, LAN goes over TjommieSocket and the relay
// over the web view's WebSocket.
export function frameSockets(): FrameSocketApi {
  const remote = desktop()?.remote;
  return {
    open: (endpoint, timeoutMs) => {
      if (remote) return nativeFrames(bridged(remote, endpoint, timeoutMs), "This PC could not be reached.");
      if (endpoint.kind === "relay") return webSocketFrames(relayConnectUrl(endpoint.url, endpoint.hostId), timeoutMs);
      return nativeFrames(tcp(endpoint, timeoutMs), "This PC could not be reached on Wi-Fi.");
    },
  };
}

// A network change can leave an open TCP socket stale.
export function onNetworkChange(listener: () => void): void {
  void socket.addListener("network", () => listener()).catch(() => {});
}
