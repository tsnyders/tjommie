import { RemoteReady, remotePrologue, type RemotePreface } from "@tjommie/protocol";
import { initiator } from "../../transport/src/index.ts";
import { EncryptedMessages } from "./encrypted-channel.ts";
import type { RpcTransport } from "./transport.ts";

export interface FrameTransport {
  send(frame: Uint8Array): void;
  close(): void;
  onFrame(listener: (frame: Uint8Array) => void): () => void;
  onClose(listener: () => void): () => void;
}

// Noise authenticates the PC key from the out-of-band code. The first encrypted confirmation proves
// PSK possession; neither peer treats an early IK message as permission to execute RPC.
export function authenticateRemote(options: {
  wire: FrameTransport; preface: RemotePreface; privateKey: Uint8Array; hostPublicKey: Uint8Array; name: string; psk?: Uint8Array; onConfirmSent?: () => void;
}): Promise<{ transport: RpcTransport; deviceId: string; hostId: string; hostName: string }> {
  return new Promise((resolve, reject) => {
    const { wire } = options;
    let phase: "handshake" | "ready" | "rpc" = "handshake", closed = false;
    const listeners = new Set<(text: string) => void>(), closedListeners = new Set<(event: { code: number; reason: string }) => void>();
    const queued: string[] = [];
    let queuedBytes = 0;
    const off: (() => void)[] = [];
    const noise = initiator({ privateKey: options.privateKey, remotePublicKey: options.hostPublicKey, psk: options.psk, prologue: remotePrologue(options.preface) });
    let messages: EncryptedMessages | null = null;
    const close = (): void => {
      if (closed) return; closed = true; clearTimeout(timer);
      for (const unsubscribe of off.splice(0)) unsubscribe();
      messages?.close(); noise.close(); wire.close(); queued.splice(0);
      const event = { code: 1006, reason: "The PC connection closed. Check Wi-Fi and whether this device is still paired." };
      for (const listener of closedListeners) listener(event);
      reject(new Error("The PC could not be authenticated. Check the pairing code and that the PC is reachable."));
    };
    const send = (text: string): void => {
      if (closed || !messages) throw new Error("The PC connection is closed.");
      try { for (const frame of messages.encode(text)) wire.send(frame); } catch { close(); throw new Error("The PC connection is closed."); }
    };
    const timer = setTimeout(close, 15_000);
    off.push(wire.onClose(close));
    off.push(wire.onFrame((frame) => {
      try {
        if (closed) return;
        if (phase === "handshake") {
          if (noise.readHandshake(frame).length !== 0 || !noise.handshakeFinished) throw new Error();
          messages = new EncryptedMessages(noise); phase = "ready";
          send(JSON.stringify({ type: "confirm", protocol: 1, name: options.name })); options.onConfirmSent?.(); return;
        }
        if (!messages) throw new Error();
        const text = messages.receive(frame); if (text === null) return;
        if (phase === "ready") {
          const ready = RemoteReady.parse(JSON.parse(text));
          if (ready.hostId !== options.preface.hostId) throw new Error();
          phase = "rpc"; clearTimeout(timer);
          resolve({ hostId: ready.hostId, hostName: ready.hostName, deviceId: ready.deviceId, transport: {
            send,
            close,
            onMessage: (listener) => {
              listeners.add(listener);
              for (const buffered of queued.splice(0)) listener(buffered);
              queuedBytes = 0;
              return () => { listeners.delete(listener); };
            },
            onClose: (listener) => {
              closedListeners.add(listener);
              if (closed) queueMicrotask(() => listener({ code: 1006, reason: "The PC connection is closed." }));
              return () => { closedListeners.delete(listener); };
            },
          } }); return;
        }
        if (listeners.size) for (const listener of listeners) listener(text);
        else { queuedBytes += text.length; if (queuedBytes > 16 * 1024 * 1024) throw new Error(); queued.push(text); }
      } catch { close(); }
    }));
    try {
      wire.send(new TextEncoder().encode(JSON.stringify(options.preface)));
      wire.send(noise.writeHandshake(new Uint8Array()));
    } catch { close(); }
  });
}
