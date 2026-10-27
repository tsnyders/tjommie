import { ClientConnectionError, connect, type Client, type ClientEvent } from "@tjommie/client";
import type { Message, MethodName, ParamsIn, PushData, Result } from "@tjommie/protocol";
import { z } from "zod";
import { RemoteRefusedError } from "../../client/src/remote-peer.ts";
import { desktop, type DesktopConnection } from "./desktop";
import { keepAliveRunning, syncKeepAlive } from "./keep-alive";
import { errorText } from "./labels";
import { registerPushIfAllowed, stopSendingPush, unregisterPush } from "./push";
import { onNetworkChange } from "./relay-wire";
import { HostOfflineError, forgetRemoteHost, isAndroidApp, openRemoteClient, remoteHosts } from "./remote-hosts";
import { failedSetup } from "./setup";
import { LINK, addOlder, applyEvent, catchUp, connected, finishLoading, forgetConversation, initialState, startLoading, type MessageEvent } from "./state";
import { getState, update } from "./store";
import { atMost } from "./utils";

const PAGE = 50;
const STARTING_TRIES = 3; // failed tries before "Starting Tjommie" says why it can't connect
const CALLS = 16; // calls in flight at once; the PC's host for phones closes a connection with 32
const FORGET_WAIT_MS = 3_000; // how long Forget waits on the PC, then on the phone's push service, to let go of push
type RemoteTarget = { remoteHostId: string };
type ConnectionTarget = DesktopConnection | RemoteTarget;
let client: Client | null = null; // the ready connection, for calls
let link: Client | null = null; // the same connection without the call limit, for a remote PC's pings
let latest: ConnectionTarget | null = null;
let retry: ReturnType<typeof setTimeout> | undefined;
let heartbeat: ReturnType<typeof setTimeout> | undefined;
let failures = 0; // the latest connection's failed tries in a row
let lastPong = 0; // when the remote PC last answered a ping
let started = false;
const fetchers = new Map<string, (before?: string) => Promise<Page>>(); // each loaded conversation's pages, to catch up after a reconnect
const arriving = new Map<string, MessageEvent[]>(); // message events for conversations whose newest page is being fetched again

type Page = { messages: Message[]; hasMore: boolean };

const isRemote = (target: ConnectionTarget | null): target is RemoteTarget => target !== null && "remoteHostId" in target;
const visible = (): boolean => typeof document === "undefined" || document.visibilityState !== "hidden";
// A hidden app without keep-alive is frozen soon after, so its link stops pinging and retrying until it is back in front.
const awake = (): boolean => visible() || keepAliveRunning();

// The live daemon client. Screens call it only while the connection is ready.
export function daemon(): Client {
  if (!client) throw new Error("Tjommie isn't connected right now.");
  return client;
}

// Calls the daemon, or rejects when Tjommie isn't connected, so a call made from an effect never throws.
export function request<M extends MethodName>(method: M, params: ParamsIn<M>): Promise<Result<M>> {
  return client ? client.call(method, params) : Promise.reject(new Error("Tjommie isn't connected right now."));
}

// Connects once for the life of the window, and again whenever the desktop app restarts the daemon.
export function startConnection(): void {
  if (started) return;
  started = true;
  if (typeof document !== "undefined") document.addEventListener("visibilitychange", onVisibility);
  if (isAndroidApp()) {
    onNetworkChange(reconnectNow);
    connectRemote();
    return;
  }
  const api = desktop();
  if (!api) {
    update((state) => ({ ...state, connection: { status: "outside" } }));
    return;
  }
  // The desktop's own daemon, unless the window is on another PC: then it waits for "This PC" (connectLocal).
  api.onConnection((next) => { if (!isRemote(latest)) open(next); });
  api.onDaemonFailed((reason) => { if (!isRemote(latest)) failed(reason); });
  openLocal();
}

function openLocal(): void {
  void desktop()?.connection().then((result) => {
    if (isRemote(latest)) return;
    if (result.ok) open({ port: result.port, token: result.token });
    else failed(result.error);
  });
}

// The desktop app couldn't start the daemon. Stop retrying the old connection; "Try again" asks for a new one.
function failed(reason: string): void {
  clearTimeout(retry);
  stopSendingPush();
  latest = null;
  update((state) => ({ ...state, connection: { status: "failed", reason } }));
}

// Asks the desktop app to start the daemon again after it failed. On another PC, it tries that PC again.
export function restartDaemon(): void {
  if (isAndroidApp() || isRemote(latest)) {
    connectRemote(isRemote(latest) ? latest.remoteHostId : undefined);
    return;
  }
  update((state) => ({ ...state, connection: { status: "connecting" } }));
  void desktop()?.restartDaemon();
}

function open(connection: DesktopConnection): void {
  if (latest && "token" in latest && latest.token === connection.token) return; // already connecting with this one
  latest = connection;
  failures = 0;
  void attempt(connection);
}

// Connects to a saved PC now: on start, from the connect screen, the host switcher or a push. Another PC's cached screens go first; the same
// PC's stay up while it reconnects. With no PC (Switch PC), this device goes to the connect screen.
export function connectRemote(id = remoteHosts()[0]?.id): void {
  if (!isRemote(latest) || latest.remoteHostId !== id) leave();
  if (!id) {
    if (isAndroidApp() && remoteHosts().length === 0) void unregisterPush();
    update((state) => ({ ...state, connection: { status: "outside" } }));
    return;
  }
  update((state) => (state.connection.status === "failed" ? { ...state, connection: { status: "connecting" } } : state)); // Connect was pressed
  begin({ remoteHostId: id });
}

// The desktop app's own PC again, after another PC.
export function connectLocal(): void {
  if (!isRemote(latest)) return;
  leave();
  openLocal();
}

// The PC's keys go first: if deleting them fails, nothing has moved (the removed screen stays and says why). Only then does the window leave
// that PC, for the next saved one on a phone and this PC's own Tjommie on the desktop. Letting go of push is tidying up, so a PC or push service
// that fails or doesn't answer in time never holds Forget up.
export async function forgetRemote(id: string): Promise<void> {
  const current = link;
  if (isAndroidApp() && isRemote(latest) && latest.remoteHostId === id) {
    stopSendingPush(); // before its token is cleared there, so nothing reaches it after
    if (current) await atMost(FORGET_WAIT_MS, current.call("device.registerPush", { token: null }));
  }
  try {
    await forgetRemoteHost(id);
  } catch (error) {
    if (isAndroidApp() && isRemote(latest) && latest.remoteHostId === id && current && link === current) void registerPushIfAllowed(current).catch(() => {});
    throw error;
  }
  if (isAndroidApp() && remoteHosts().length === 0) await atMost(FORGET_WAIT_MS, unregisterPush());
  if (!isRemote(latest) || latest.remoteHostId !== id) return;
  leave();
  if (isAndroidApp()) connectRemote();
  else openLocal();
}

// A tapped push: its PC becomes the current one if it isn't (and is saved here), and the room it came from, or else the bot's chat, is where to go.
export function routePush(data: PushData): string | null {
  if (!remoteHosts().some((host) => host.id === data.hostId) || !z.uuid().safeParse(data.botId).success) return null;
  const current = isRemote(latest) && latest.remoteHostId === data.hostId;
  if (!current) connectRemote(data.hostId);
  const room = z.string().min(1).max(64).safeParse(data.roomId);
  return room.success ? `/room/?id=${encodeURIComponent(room.data)}` : `/chat/?bot=${encodeURIComponent(data.botId)}`;
}

// Drops the current connection and everything cached from its PC.
function leave(): void {
  clearTimeout(retry);
  clearTimeout(heartbeat);
  latest = null;
  const previous = client;
  client = null;
  link = null;
  void previous?.close().catch(() => {});
  stopSendingPush();
  fetchers.clear();
  update(() => initialState);
  syncKeepAlive("outside");
}

// Connects to this PC now, from the first backoff step. A ready link is closed (its calls in flight reject and are never sent again), and the
// cached screens stay up while it reconnects. A failed first connect keeps its screen, so pairing there isn't interrupted.
function begin(target: RemoteTarget): void {
  clearTimeout(retry);
  failures = 0;
  latest = target;
  update((state) => {
    const { status } = state.connection;
    const next = state.host === target.remoteHostId ? state : { ...state, host: target.remoteHostId };
    if (status === "failed" || status === "connecting" || status === "reconnecting") return next;
    return { ...next, connection: next.settings ? { status: "reconnecting" } : { status: "connecting" } };
  });
  void attempt(target);
}

// The remote link may be dead: the network changed, a ping went unanswered, the app is back after a sleep, or the link closed. It is replaced at
// once; a hidden app without keep-alive only lets it go, and its next return starts again.
function reconnectNow(): void {
  const target = latest;
  if (!isRemote(target) || getState().connection.status === "removed") return;
  if (awake()) {
    begin({ remoteHostId: target.remoteHostId });
    return;
  }
  clearTimeout(heartbeat);
  const previous = client;
  client = null;
  link = null;
  stopSendingPush();
  void previous?.close().catch(() => {});
  update((state) => (state.connection.status === "ready" ? { ...state, connection: { status: "reconnecting" } } : state));
}

function onVisibility(): void {
  if (!isRemote(latest)) return;
  if (!visible()) {
    beat(); // slower with keep-alive, or none
    return;
  }
  const status = getState().connection.status;
  if (status === "removed") return;
  syncKeepAlive(status);
  // A link that answered within the last minute gets a ping now; an older one (the app slept, and its socket may be a zombie) is replaced at once.
  if (link && Date.now() - lastPong <= LINK.pingBackgroundMs) beat(0);
  else reconnectNow();
}

// The next ping on the remote link: every 20 s in front, every 60 s hidden while keep-alive runs, otherwise none.
function beat(wait: number | null = visible() ? LINK.pingForegroundMs : keepAliveRunning() ? LINK.pingBackgroundMs : null): void {
  clearTimeout(heartbeat);
  const current = link;
  if (!current || wait === null) return;
  heartbeat = setTimeout(() => void ping(current), wait);
}

async function ping(current: Client): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const answered = await Promise.race([
    // Any answer counts, an error too (a PC from before remote.ping says unknown_method); only silence or a closed link means it's gone.
    current.call("remote.ping", {}).then(() => true, (error: unknown) => !(error instanceof ClientConnectionError)),
    new Promise<false>((resolve) => { timer = setTimeout(() => resolve(false), LINK.pongTimeoutMs); }),
  ]);
  clearTimeout(timer);
  if (link !== current) return; // replaced meanwhile
  if (!answered) {
    reconnectNow();
    return;
  }
  lastPong = Date.now();
  beat();
}

async function attempt(connection: ConnectionTarget): Promise<void> {
  clearTimeout(retry);
  clearTimeout(heartbeat);
  const previous = client;
  client = null;
  link = null;
  stopSendingPush();
  // A new connection is replacing one that still looked up (the daemon restarted, or the remote link went quiet): until it opens, calls fail as
  // they do while reconnecting, so the pill says so.
  if (previous) update((state) => (state.connection.status === "ready" ? { ...state, connection: { status: "reconnecting" } } : state));
  await previous?.close().catch(() => {});
  if (latest !== connection) return; // replaced while the old one closed
  let raw: Client;
  try {
    raw = isRemote(connection) ? await openRemoteClient(connection.remoteHostId) : await connect(connection.port, connection.token, { recordEvents: false });
  } catch (error) {
    lost(connection, error);
    return;
  }
  if (latest !== connection) {
    void raw.close();
    return;
  }
  const next = limitCalls(raw);
  // Events that arrive while the lists load are applied on top of them, in order. The daemon sends events and
  // replies in the order things happen, so the last word on each bot is always the newest.
  let early: ClientEvent[] | null = [];
  next.subscribe((event) => {
    if (early) {
      early.push(event);
      return;
    }
    update((state) => applyEvent(state, event));
    if (event.event === "message.added" || event.event === "message.updated" || event.event === "message.delta") arriving.get(event.data.conversationId)?.push(event);
  });
  try {
    // An older daemon has no accounts: the list stays unknown and the screens that need it say so.
    const [bots, settings, routines, rooms, accounts, notifications] = await Promise.all([
      next.call("bots.list", {}), next.call("settings.get", {}), next.call("routines.list", {}), next.call("rooms.list", {}), next.call("accounts.list", {}).catch(() => null), next.call("notifications.pending", {}),
    ]);
    if (latest !== connection) {
      void next.close();
      return;
    }
    const pending = early;
    early = null;
    client = next;
    link = raw;
    failures = 0;
    lastPong = Date.now();
    update((state) => pending.reduce((current, event) => applyEvent(current, event), connected(state, { bots, settings, routines, rooms, accounts, notifications })));
  } catch (error) {
    void next.close();
    lost(connection, error);
    return;
  }
  // setup.status runs both CLIs' checks, so it's asked once per connection and never polled. If the check
  // fails, both setup cards say why and offer Check again.
  next.call("setup.status", {}).then(
    (setup) => {
      if (client === next) update((state) => ({ ...state, setup }));
    },
    (error: unknown) => {
      if (client === next) update((state) => ({ ...state, setup: failedSetup(errorText(error)) }));
    },
  );
  catchUpAll(next);
  if (isRemote(connection)) {
    beat();
    void registerPushIfAllowed(raw).catch(() => {}); // raw sends immediately, without waiting in the screen RPC queue
    syncKeepAlive("ready", remoteHosts().find((host) => host.id === connection.remoteHostId)?.name ?? null);
  }
  void next.disconnected.then((error) => {
    if (client !== next) return;
    if (isRemote(connection)) reconnectNow();
    else lost(connection, error);
  });
}

// After a reconnect, each loaded chat fetches its newest page again and merges it, so what happened meanwhile shows up without blanking.
function catchUpAll(next: Client): void {
  for (const [id, conversation] of Object.entries(getState().conversations)) {
    const fetchPage = fetchers.get(id);
    if (conversation.status !== "ready" || !fetchPage) continue;
    const events: MessageEvent[] = [];
    arriving.set(id, events);
    fetchPage()
      .then(
        (page) => { if (client === next) update((state) => catchUp(state, id, page, events)); },
        () => {}, // the link went again; the next connection catches up
      )
      .finally(() => { if (arriving.get(id) === events) arriving.delete(id); });
  }
}

// The client with at most CALLS calls in flight; the rest wait their turn, in order. Every call goes through it (the startup lists, request and
// daemon()), so no burst, such as each room's or bot's history being read again at once, can trip the phone host's limit and drop the connection.
// A call that ends hands its slot straight to the next one waiting.
function limitCalls(client: Client): Client {
  let active = 0;
  const waiting: (() => void)[] = [];
  async function call<M extends MethodName>(method: M, params: ParamsIn<M>): Promise<Result<M>> {
    if (active < CALLS) active += 1;
    else await new Promise<void>((resolve) => waiting.push(resolve));
    try {
      return await client.call(method, params);
    } finally {
      const next = waiting.shift();
      if (next) next();
      else active -= 1;
    }
  }
  return { ...client, call };
}

// A connection attempt failed, or the desktop's daemon connection dropped.
function lost(connection: ConnectionTarget, error: unknown): void {
  if (latest !== connection) return;
  client = null;
  link = null;
  stopSendingPush();
  if (typeof error === "object" && error !== null && "code" in error && error.code === 4003) {
    update((state) => ({ ...state, connection: { status: "update" } }));
    return;
  }
  if (isRemote(connection)) {
    remoteFailed(connection, error);
    return;
  }
  failures += 1;
  update((state) => {
    if (state.connection.status === "ready") return { ...state, connection: { status: "reconnecting" } };
    if (state.connection.status === "reconnecting" || failures < STARTING_TRIES) return state;
    // Still starting (or after Try again): say why instead of spinning. The tries go on, so a daemon that recovers
    // connects on its own.
    const reason = error instanceof ClientConnectionError ? "Tjommie couldn't reach its background service." : errorText(error);
    return { ...state, connection: { status: "failed", reason } };
  });
  // If the daemon restarts, the desktop app sends a new connection. Until then, keep trying this one in
  // case only the socket dropped.
  retry = setTimeout(() => {
    if (latest === connection) void attempt(connection);
  }, 2_000);
}

// A saved PC couldn't be reached. With its screens cached they stay, marked offline (its relay says it's off, or it can't be reached), and the
// tries go on with backoff while the app can run. Only a first connect with nothing cached says why in full. The PC refusing this device is final.
function remoteFailed(target: RemoteTarget, error: unknown): void {
  if (error instanceof RemoteRefusedError) {
    clearTimeout(retry);
    update((state) => ({ ...state, connection: { status: "removed" } }));
    syncKeepAlive("removed");
    return;
  }
  failures += 1;
  update((state) => {
    if (state.settings) return { ...state, connection: { status: "offline", reason: error instanceof HostOfflineError ? "host_offline" : "unreachable" } };
    return failures < STARTING_TRIES ? state : { ...state, connection: { status: "failed", reason: errorText(error) } };
  });
  syncKeepAlive(getState().connection.status);
  if (!awake()) return;
  retry = setTimeout(() => {
    if (latest === target) void attempt(target);
  }, LINK.backoffMs[Math.min(failures, LINK.backoffMs.length - 1)]);
}

// A bot chat's pages, newest first.
export const botHistory =
  (botId: string) =>
  (before?: string): Promise<Page> =>
    request("chat.history", { botId, before, limit: PAGE });

// A room's pages, newest first.
export const roomHistory =
  (roomId: string) =>
  (before?: string): Promise<Page> =>
    request("room.history", { roomId, before, limit: PAGE });

// An exchange's messages: the newest 200, with no older pages to load.
export const exchangePage =
  (exchangeId: string) =>
  async (): Promise<Page> => {
    const { messages } = await request("exchanges.get", { id: exchangeId });
    return { messages, hasMore: false };
  };

// Loads a conversation's newest page once; events keep it current from then on, and a reconnect catches it up.
export async function loadConversation(conversationId: string, fetchPage: (before?: string) => Promise<Page>): Promise<void> {
  if (getState().conversations[conversationId]) return;
  fetchers.set(conversationId, fetchPage);
  update((state) => startLoading(state, conversationId));
  try {
    const page = await fetchPage();
    update((state) => finishLoading(state, conversationId, page));
  } catch {
    update((state) => {
      const conversation = state.conversations[conversationId];
      if (!conversation || conversation.status !== "loading") return state;
      return {
        ...state,
        conversations: {
          ...state.conversations,
          [conversationId]: { ...conversation, status: "failed" as const, pending: [] },
        },
      };
    });
  }
}

// Retry after a failed load: loadConversation keeps a failed chat until it's forgotten.
export function retryConversation(conversationId: string, fetchPage: (before?: string) => Promise<Page>): Promise<void> {
  update((state) => forgetConversation(state, conversationId));
  return loadConversation(conversationId, fetchPage);
}

// A closed view's conversation (an exchange) is forgotten, so it stops following events. It loads again if reopened.
export function closeConversation(conversationId: string): void {
  fetchers.delete(conversationId);
  update((state) => forgetConversation(state, conversationId));
}

// Loads the page before the oldest loaded message.
export async function loadOlder(conversationId: string, fetchPage: (before?: string) => Promise<Page>): Promise<void> {
  const conversation = getState().conversations[conversationId];
  if (!conversation || conversation.status !== "ready" || !conversation.hasMore || conversation.messages.length === 0) return;
  const page = await fetchPage(conversation.messages[0].id);
  update((state) => addOlder(state, conversationId, page));
}
