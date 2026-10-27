import { HostToRelay, LEGACY_PUSH_KINDS, PushKind, RELAY, RelayClose, RelayToHost, loginMessage, type RelayState } from "@tjommie/protocol";
import { relayWebSocketUrl } from "../../client/src/frame-socket.ts";
import { webSocketWire, type PeerWire } from "./peer-wire.ts";
import type { RelayIdentity } from "./relay-identity.ts";

type PushResult = { ok: boolean; unregistered: boolean };
type PendingPush = { count: number; resolve: (results: PushResult[]) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> };
type OnlineWaiter = { resolve: () => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> };
type Options = {
  url: () => string | null; hostId: string; identity: RelayIdentity; onChannel: (wire: PeerWire) => void; onState: () => void;
  socket?: (url: string) => WebSocket; wire?: (socket: WebSocket) => PeerWire;
};
const retryMs = [1_000, 2_000, 5_000, 10_000, 30_000] as const;
const connectionError = () => new Error("The relay connection is unavailable.");

export class RelayClient {
  #options: Options;
  #socket: WebSocket | null = null;
  #channels = new Set<WebSocket>();
  #pending = new Map<number, PendingPush>();
  #onlineWaiters = new Set<OnlineWaiter>();
  #watch: ReturnType<typeof setInterval> | undefined;
  #retry: ReturnType<typeof setTimeout> | undefined;
  #loginTimer: ReturnType<typeof setTimeout> | undefined;
  #active = false;
  #authenticated = false;
  #challenged = false;
  #push = false;
  #kinds: readonly PushKind[] = LEGACY_PUSH_KINDS;
  #state: RelayState = "off";
  #attempt = 0;
  #nextPushId = 0;
  #lastTick = 0;
  #lastPing = 0;
  #lastPong = 0;

  constructor(options: Options) { this.#options = options; }
  state(): RelayState { return this.#state; }
  pushEnabled(): boolean { return this.#state === "online" && this.#push; }
  kinds(): readonly PushKind[] { return this.#kinds; }

  waitUntilOnline(): Promise<void> {
    if (this.#state === "online") return Promise.resolve();
    if (!this.#active || this.#options.url() === null) return Promise.reject(connectionError());
    return new Promise((resolve, reject) => {
      const waiter: OnlineWaiter = { resolve, reject, timer: setTimeout(() => { this.#onlineWaiters.delete(waiter); reject(connectionError()); }, RELAY.acceptTimeoutMs) };
      waiter.timer.unref();
      this.#onlineWaiters.add(waiter);
    });
  }

  start(): void {
    if (this.#active) return;
    this.#active = true;
    this.#lastTick = Date.now();
    this.#watch = setInterval(() => this.#tick(), 5_000);
    this.#watch.unref();
    this.#connect();
  }

  stop(): void {
    this.#active = false;
    clearInterval(this.#watch); this.#watch = undefined;
    this.#attempt = 0;
    this.#disconnect();
    this.#setState("off");
  }

  push(tokens: string[], kind: PushKind, botId: string, roomId?: string): Promise<PushResult[]> {
    const socket = this.#socket;
    if (!socket || !this.pushEnabled()) return Promise.reject(connectionError());
    const id = ++this.#nextPushId;
    const message = HostToRelay.safeParse({ t: "push", id, tokens, kind, botId, roomId });
    if (!message.success) return Promise.reject(new Error("Invalid relay push request."));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.#pending.delete(id); reject(connectionError()); }, RELAY.acceptTimeoutMs);
      timer.unref();
      this.#pending.set(id, { count: tokens.length, resolve, reject, timer });
      try { socket.send(JSON.stringify(message.data)); }
      catch { this.#fail(socket); }
    });
  }

  #setState(state: RelayState): void {
    if (this.#state === state) return;
    this.#state = state;
    if (state === "online" || state === "off") {
      for (const waiter of this.#onlineWaiters) {
        clearTimeout(waiter.timer);
        if (state === "online") waiter.resolve(); else waiter.reject(connectionError());
      }
      this.#onlineWaiters.clear();
    }
    try { this.#options.onState(); } catch { /* A UI broadcast cannot escape a WebSocket listener. */ }
  }

  #disconnect(): void {
    clearTimeout(this.#retry); this.#retry = undefined;
    clearTimeout(this.#loginTimer); this.#loginTimer = undefined;
    const socket = this.#socket; this.#socket = null;
    this.#authenticated = false; this.#challenged = false; this.#push = false; this.#kinds = LEGACY_PUSH_KINDS;
    for (const [id, pending] of this.#pending) { clearTimeout(pending.timer); pending.reject(connectionError()); this.#pending.delete(id); }
    for (const channel of this.#channels) channel.close();
    this.#channels.clear();
    socket?.close();
  }

  #connect(): void {
    if (!this.#active) return;
    this.#disconnect();
    const base = this.#options.url();
    if (base === null) { this.#setState("off"); return; }
    this.#setState("connecting");
    try {
      const url = relayWebSocketUrl(base, `/v1/hosts/${encodeURIComponent(this.#options.hostId)}/control`);
      const socket = this.#options.socket?.(url) ?? new WebSocket(url);
      this.#socket = socket;
      this.#loginTimer = setTimeout(() => this.#fail(socket), RELAY.acceptTimeoutMs);
      this.#loginTimer.unref();
      socket.addEventListener("message", (event) => this.#message(socket, base, event));
      socket.addEventListener("error", () => this.#fail(socket));
      socket.addEventListener("close", (event) => this.#fail(socket, event.code));
    } catch { this.#fail(); }
  }

  #fail(socket?: WebSocket, code?: number): void {
    if (!this.#active || socket && socket !== this.#socket) return;
    this.#disconnect();
    if (this.#options.url() === null) { this.#setState("off"); return; }
    this.#setState("error");
    if (code === RelayClose.badLogin) this.#attempt = retryMs.length - 1;
    const delay = retryMs[Math.min(this.#attempt++, retryMs.length - 1)];
    this.#retry = setTimeout(() => this.#connect(), delay);
    this.#retry.unref();
  }

  #message(socket: WebSocket, base: string, event: MessageEvent): void {
    if (socket !== this.#socket || !this.#active) return;
    if (event.data === "pong") { this.#lastPong = Date.now(); return; }
    if (typeof event.data !== "string") { this.#fail(socket); return; }
    let raw: unknown;
    try { raw = JSON.parse(event.data); } catch { this.#fail(socket); return; }
    const parsed = RelayToHost.safeParse(raw);
    if (!parsed.success) { this.#fail(socket); return; }
    const message = parsed.data;
    if (message.t === "challenge") {
      if (this.#challenged || this.#authenticated) { this.#fail(socket); return; }
      this.#challenged = true;
      try {
        socket.send(JSON.stringify(HostToRelay.parse({ t: "login", publicKey: Buffer.from(this.#options.identity.publicKey).toString("base64url"),
          signature: Buffer.from(this.#options.identity.sign(loginMessage(this.#options.hostId, message.nonce, new URL(base).host))).toString("base64url") })));
      } catch { this.#fail(socket); }
    } else if (message.t === "ready") {
      if (!this.#challenged || this.#authenticated) { this.#fail(socket); return; }
      clearTimeout(this.#loginTimer); this.#loginTimer = undefined;
      this.#authenticated = true; this.#push = message.push;
      this.#kinds = message.kinds?.flatMap((kind) => { const parsed = PushKind.safeParse(kind); return parsed.success ? [parsed.data] : []; }) ?? LEGACY_PUSH_KINDS;
      this.#attempt = 0;
      this.#lastPing = this.#lastPong = Date.now();
      this.#setState("online");
    } else if (message.t === "open") {
      if (!this.#authenticated) { this.#fail(socket); return; }
      this.#openChannel(socket, base, message.channel);
    } else {
      if (!this.#authenticated) { this.#fail(socket); return; }
      const pending = this.#pending.get(message.id);
      if (!pending) return;
      this.#pending.delete(message.id); clearTimeout(pending.timer);
      if (message.results.length !== pending.count) pending.reject(connectionError());
      else pending.resolve(message.results);
    }
  }

  #openChannel(control: WebSocket, base: string, channelId: string): void {
    if (this.#channels.size >= RELAY.maxChannels) return;
    try {
      const url = relayWebSocketUrl(base, `/v1/hosts/${encodeURIComponent(this.#options.hostId)}/channels/${encodeURIComponent(channelId)}`);
      const socket = this.#options.socket?.(url) ?? new WebSocket(url);
      socket.binaryType = "arraybuffer";
      this.#channels.add(socket);
      const timer = setTimeout(() => socket.close(), RELAY.acceptTimeoutMs); timer.unref();
      socket.addEventListener("open", () => {
        clearTimeout(timer);
        if (control !== this.#socket || !this.#active) { socket.close(); return; }
        try {
          this.#options.onChannel((this.#options.wire ?? webSocketWire)(socket));
        } catch { socket.close(); }
      });
      socket.addEventListener("error", () => socket.close());
      socket.addEventListener("close", () => { clearTimeout(timer); this.#channels.delete(socket); });
    } catch { /* The relay expires an unaccepted channel without exposing its id. */ }
  }

  #tick(): void {
    const now = Date.now(), drift = now - this.#lastTick;
    this.#lastTick = now;
    if (drift > 15_000 || drift < 0) { this.#connect(); return; }
    const socket = this.#socket;
    if (!socket || !this.#authenticated) return;
    if (now - this.#lastPong > RELAY.hostDeadMs) { this.#fail(socket); return; }
    if (now - this.#lastPing < RELAY.hostPingMs) return;
    try { socket.send("ping"); this.#lastPing = now; }
    catch { this.#fail(socket); }
  }
}
