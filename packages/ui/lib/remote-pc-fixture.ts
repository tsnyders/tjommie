// A synthetic PC used only by the connection tests; never imported by the product. It answers over a fake FrameSocketApi the way the PC's
// RemoteHost does: the device's preface, a real Noise IK handshake with this PC's key, confirm → ready (or refused), then RPC inside the
// encrypted channel. Each endpoint behaves as its leg says, and any wire can die silently or be closed with a code, the way a relay closes one.
import { RemotePreface, remotePrologue, type Bot, type Message } from "@tjommie/protocol";
import { EncryptedMessages } from "../../client/src/encrypted-channel.ts";
import type { FrameSocketApi, FrameTransport, WireEndpoint } from "../../client/src/frame-socket.ts";
import { generateKeypair, responder, type NoiseSession } from "../../transport/src/index.ts";

// How the PC answers on one endpoint: at once, after a delay, never (the connect times out), refused at once, or, for a relay, "PC offline".
export type Leg = "answer" | "unreachable" | "refused" | "offline" | { delayMs: number };
export type PcWire = { endpoint: WireEndpoint; closed: boolean; die(): void; drop(code?: number): void };
export type FakePc = {
  hostKey: string;
  legs: Map<string, Leg>; // by endpointKey; "answer" when missing
  refuse: "revoked" | "unknown" | null;
  bots: Bot[];
  messages: Record<string, Message[]>; // by conversation, oldest first
  calls: string[]; // every method asked, in order
  hold: Set<string>; // methods the PC never answers
  opens: WireEndpoint[];
  wires: PcWire[];
  sockets: FrameSocketApi;
  add(message: Message): void; // saved, and sent to every live connection
};

export const endpointKey = (endpoint: WireEndpoint): string => (endpoint.kind === "relay" ? "relay" : `${endpoint.host}:${endpoint.port}`);

export function fakePc(options: { hostId: string; deviceId: string; hostName: string; relay: string | null }): FakePc {
  const keys = generateKeypair();
  const live = new Set<(text: string) => void>();
  const pc: FakePc = {
    hostKey: Buffer.from(keys.publicKey).toString("base64url"),
    legs: new Map(),
    refuse: null,
    bots: [],
    messages: {},
    calls: [],
    hold: new Set(),
    opens: [],
    wires: [],
    sockets: {
      open: (endpoint, timeoutMs) => {
        pc.opens.push(endpoint);
        const leg = pc.legs.get(endpointKey(endpoint)) ?? "answer";
        if (leg === "unreachable") return new Promise((_, reject) => setTimeout(() => reject(new Error("Synthetic connect timeout")), timeoutMs));
        if (leg === "refused") return Promise.reject(new Error("Synthetic connection refused"));
        return Promise.resolve(wire(endpoint, leg));
      },
    },
    add: (message) => {
      (pc.messages[message.conversationId] ??= []).push(message);
      for (const send of live) send(JSON.stringify({ type: "event", event: "message.added", data: message }));
    },
  };

  function answer(method: string, params: Record<string, unknown>): unknown {
    switch (method) {
      case "bots.list":
        return pc.bots;
      case "settings.get":
        return { chainLimit: 5, relayUrl: null };
      case "chat.history":
      case "room.history": {
        const all = pc.messages[String(params.botId ?? params.roomId)] ?? [];
        const end = typeof params.before === "string" ? all.findIndex((message) => message.id === params.before) : all.length;
        const limit = Number(params.limit ?? 50);
        return { messages: all.slice(Math.max(0, end - limit), end), hasMore: end > limit };
      }
      case "routines.list":
      case "rooms.list":
      case "accounts.list":
      case "notifications.pending":
      case "setup.status":
        return [];
      default:
        return {};
    }
  }

  function wire(endpoint: WireEndpoint, leg: Leg): FrameTransport {
    const frames = new Set<(frame: Uint8Array) => void>(), closes = new Set<(code?: number) => void>();
    let phase: "preface" | "handshake" | "confirm" | "rpc" = "preface", noise: NoiseSession | undefined, messages: EncryptedMessages | undefined;
    let dead = false, closedCode: number | undefined;
    const state: PcWire = { endpoint, closed: false, die: () => { dead = true; live.delete(sendText); }, drop: (code) => end(code) };
    pc.wires.push(state);
    const toDevice = (frame: Uint8Array, delayMs = 0): void => {
      const deliver = (): void => { if (!state.closed && !dead) for (const listener of frames) listener(frame); };
      if (delayMs > 0) setTimeout(deliver, delayMs);
      else queueMicrotask(deliver);
    };
    function sendText(text: string): void {
      if (messages && !state.closed && !dead) for (const frame of messages.encode(text)) toDevice(frame);
    }
    function end(code?: number): void {
      if (state.closed) return;
      state.closed = true; closedCode = code; live.delete(sendText); messages?.close(); noise?.close();
      for (const listener of closes) listener(code);
    }
    function receive(frame: Uint8Array): void {
      if (phase === "preface") {
        noise = responder({ privateKey: keys.privateKey, prologue: remotePrologue(RemotePreface.parse(JSON.parse(new TextDecoder().decode(frame)))) });
        phase = "handshake";
        return;
      }
      if (phase === "handshake") {
        noise?.readHandshake(frame);
        const reply = noise?.writeHandshake(new Uint8Array());
        if (!noise || !reply) return;
        messages = new EncryptedMessages(noise);
        phase = "confirm";
        toDevice(reply, typeof leg === "object" ? leg.delayMs : 0);
        return;
      }
      const text = messages?.receive(frame);
      if (!text) return;
      if (phase === "confirm") {
        if (pc.refuse) { sendText(JSON.stringify({ type: "refused", reason: pc.refuse })); return; }
        sendText(JSON.stringify({ type: "ready", deviceId: options.deviceId, hostId: options.hostId, hostName: options.hostName, relay: options.relay }));
        phase = "rpc";
        live.add(sendText);
        return;
      }
      const request = JSON.parse(text) as { type: string; id: number; method: string; params: Record<string, unknown> };
      if (request.type === "hello") { sendText(JSON.stringify({ type: "welcome", protocol: 1 })); return; }
      pc.calls.push(request.method);
      if (!pc.hold.has(request.method)) sendText(JSON.stringify({ type: "res", id: request.id, ok: true, result: answer(request.method, request.params) }));
    }
    if (leg === "offline") queueMicrotask(() => end(4404)); // the relay has no PC to pair this leg with
    return {
      send: (frame) => {
        if (state.closed) throw new Error("Synthetic wire closed");
        if (!dead && leg !== "offline") receive(frame);
      },
      close: () => end(),
      onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
      onClose: (listener) => { closes.add(listener); if (state.closed) queueMicrotask(() => listener(closedCode)); return () => { closes.delete(listener); }; },
    };
  }

  return pc;
}
