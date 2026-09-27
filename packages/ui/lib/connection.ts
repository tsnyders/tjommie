import { connect, type Client, type ClientEvent } from "@tjommie/client";
import type { Message, MethodName, ParamsIn, Result } from "@tjommie/protocol";
import { desktop, type DesktopConnection } from "./desktop";
import { addOlder, applyEvent, connected, finishLoading, startLoading } from "./state";
import { getState, update } from "./store";

const PAGE = 50;
let client: Client | null = null;
let latest: DesktopConnection | null = null;
let retry: ReturnType<typeof setTimeout> | undefined;
let started = false;

type Page = { messages: Message[]; hasMore: boolean };

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
  const api = desktop();
  if (!api) {
    update((state) => ({ ...state, connection: { status: "outside" } }));
    return;
  }
  api.onConnection((next) => open(next));
  api.onDaemonFailed(failed);
  void api.connection().then((result) => {
    if (result.ok) open({ port: result.port, token: result.token });
    else failed(result.error);
  });
}

// The desktop app couldn't start the daemon. Stop retrying the old connection; "Try again" asks for a new one.
function failed(reason: string): void {
  clearTimeout(retry);
  latest = null;
  update((state) => ({ ...state, connection: { status: "failed", reason } }));
}

// Asks the desktop app to start the daemon again after it failed.
export function restartDaemon(): void {
  update((state) => ({ ...state, connection: { status: "connecting" } }));
  void desktop()?.restartDaemon();
}

function open(connection: DesktopConnection): void {
  if (latest?.token === connection.token) return; // already connecting with this one
  latest = connection;
  void attempt(connection);
}

async function attempt(connection: DesktopConnection): Promise<void> {
  clearTimeout(retry);
  const previous = client;
  client = null;
  await previous?.close().catch(() => {});
  let next: Client;
  try {
    next = await connect(connection.port, connection.token, { recordEvents: false });
  } catch (error) {
    lost(connection, error);
    return;
  }
  if (latest !== connection) {
    void next.close();
    return;
  }
  // Events that arrive while the lists load are applied on top of them, in order. The daemon sends events and
  // replies in the order things happen, so the last word on each bot is always the newest.
  let early: ClientEvent[] | null = [];
  next.subscribe((event) => {
    if (early) early.push(event);
    else update((state) => applyEvent(state, event));
  });
  try {
    const [bots, settings, routines] = await Promise.all([next.call("bots.list", {}), next.call("settings.get", {}), next.call("routines.list", {})]);
    if (latest !== connection) {
      void next.close();
      return;
    }
    const pending = early;
    early = null;
    client = next;
    update((state) => pending.reduce((current, event) => applyEvent(current, event), connected(state, { bots, settings, routines })));
  } catch (error) {
    void next.close();
    lost(connection, error);
    return;
  }
  // setup.status runs both CLIs' checks, so it's asked once per connection and never polled.
  next.call("setup.status", {}).then(
    (setup) => {
      if (client === next) update((state) => ({ ...state, setup }));
    },
    () => {},
  );
  void next.disconnected.then((error) => {
    if (client === next) lost(connection, error);
  });
}

function lost(connection: DesktopConnection, error: unknown): void {
  if (latest !== connection) return;
  client = null;
  if (typeof error === "object" && error !== null && "code" in error && error.code === 4003) {
    update((state) => ({ ...state, connection: { status: "update" } }));
    return;
  }
  update((state) => (state.connection.status === "connecting" ? state : { ...state, connection: { status: "reconnecting" } }));
  // If the daemon restarts, the desktop app sends a new connection. Until then, keep trying this one in
  // case only the socket dropped.
  retry = setTimeout(() => {
    if (latest === connection) void attempt(connection);
  }, 2_000);
}

// A bot chat's pages, newest first.
export const botHistory =
  (botId: string) =>
  (before?: string): Promise<Page> =>
    daemon().call("chat.history", { botId, before, limit: PAGE });

// The Team room's pages, newest first.
export const roomHistory = (before?: string): Promise<Page> => request("room.history", { before, limit: PAGE });

// An exchange's messages: the newest 200, with no older pages to load.
export const exchangePage =
  (exchangeId: string) =>
  async (): Promise<Page> => {
    const { messages } = await request("exchanges.get", { id: exchangeId });
    return { messages, hasMore: false };
  };

// Loads a conversation's newest page once; events keep it current from then on.
export async function loadConversation(conversationId: string, fetchPage: (before?: string) => Promise<Page>): Promise<void> {
  if (getState().conversations[conversationId]) return;
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

// Loads the page before the oldest loaded message.
export async function loadOlder(conversationId: string, fetchPage: (before?: string) => Promise<Page>): Promise<void> {
  const conversation = getState().conversations[conversationId];
  if (!conversation || conversation.status !== "ready" || !conversation.hasMore || conversation.messages.length === 0) return;
  const page = await fetchPage(conversation.messages[0].id);
  update((state) => addOlder(state, conversationId, page));
}
