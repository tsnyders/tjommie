import type { ClientEvent } from "@tjommie/client";
import type { AccountInfo, Bot, Message, ProviderSetup, Room, Routine, RoutineRun, Settings } from "@tjommie/protocol";

export type MessageEvent = Extract<ClientEvent, { event: "message.added" | "message.updated" | "message.delta" }>;

export type Conversation = {
  status: "loading" | "ready" | "failed";
  messages: Message[];
  hasMore: boolean;
  pending: MessageEvent[]; // message events that arrived while the first page was loading
};

export type Connection =
  | { status: "connecting" }
  | { status: "ready" }
  | { status: "reconnecting" }
  | { status: "update" }
  | { status: "failed"; reason: string }
  | { status: "outside" };

export type AppState = {
  connection: Connection;
  bots: Bot[];
  setup: ProviderSetup[] | null;
  accounts: AccountInfo[] | null; // null until the first list arrives
  signInsEnded: string[]; // attempt ids of the latest setup.signInEnded events; a new connection keeps them
  settings: Settings | null;
  routines: Routine[];
  latestRuns: Record<string, RoutineRun>; // each routine's newest routine.run event, by routine id
  rooms: Room[]; // Everyone first, then the others oldest first
  conversations: Record<string, Conversation>;
};

export const initialState: AppState = {
  connection: { status: "connecting" },
  bots: [],
  setup: null,
  accounts: null,
  signInsEnded: [],
  settings: null,
  routines: [],
  latestRuns: {},
  rooms: [],
  conversations: {},
};

type Page = { messages: Message[]; hasMore: boolean };

// Everyone has no createdAt, so it comes first.
const byCreated = <T extends { createdAt: string | null }>(a: T, b: T): number => (a.createdAt ?? "").localeCompare(b.createdAt ?? "");

function upsert<T extends { id: string; createdAt: string | null }>(list: T[], item: T): T[] {
  return [...list.filter((existing) => existing.id !== item.id), item].sort(byCreated);
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  return Object.fromEntries(Object.entries(record).filter(([name]) => name !== key));
}

// An update or delta for a message outside the loaded pages (an old card being answered) changes nothing.
export function applyMessageEvent(messages: Message[], event: MessageEvent): Message[] {
  switch (event.event) {
    case "message.added":
      return messages.some((m) => m.id === event.data.id) ? messages : [...messages, event.data];
    case "message.updated":
      return messages.map((m) => (m.id === event.data.id ? event.data : m));
    case "message.delta":
      return messages.map((m) => (m.id === event.data.id ? { ...m, body: m.body + event.data.delta } : m));
  }
}

export function applyEvent(state: AppState, event: ClientEvent): AppState {
  switch (event.event) {
    case "bot.updated":
      // ponytail: Re-sorting each state tick is fine below 100 bots; above that, preserve positions for unchanged createdAt.
      return { ...state, bots: upsert(state.bots, event.data) };
    case "bot.deleted":
      return {
        ...state,
        bots: state.bots.filter((bot) => bot.id !== event.data.id),
        routines: state.routines.filter((routine) => routine.botId !== event.data.id),
        rooms: state.rooms.map((room) => ({ ...room, botIds: room.botIds.filter((id) => id !== event.data.id) })),
        conversations: without(state.conversations, event.data.id),
      };
    case "setup.changed":
      return { ...state, setup: event.data };
    case "accounts.changed":
      return { ...state, accounts: event.data };
    case "setup.signInEnded":
      return { ...state, signInsEnded: [...state.signInsEnded, event.data.attempt].slice(-20) };
    case "settings.changed":
      return { ...state, settings: event.data };
    case "routine.updated":
      return { ...state, routines: upsert(state.routines, event.data) };
    case "routine.deleted":
      return { ...state, routines: state.routines.filter((routine) => routine.id !== event.data.id) };
    case "room.updated":
      return { ...state, rooms: upsert(state.rooms, event.data) };
    case "room.deleted":
      return { ...state, rooms: state.rooms.filter((room) => room.id !== event.data.id), conversations: without(state.conversations, event.data.id) };
    case "routine.run":
      return { ...state, latestRuns: { ...state.latestRuns, [event.data.routineId]: event.data } };
    case "message.added":
    case "message.updated":
    case "message.delta": {
      const id = event.data.conversationId;
      const conversation = state.conversations[id];
      if (!conversation) return state; // not open: its history loads when it opens
      const next: Conversation =
        conversation.status === "loading"
          ? { ...conversation, pending: [...conversation.pending, event] }
          : { ...conversation, messages: applyMessageEvent(conversation.messages, event) };
      return { ...state, conversations: { ...state.conversations, [id]: next } };
    }
    default:
      return state; // approvals arrive as messages, and the desktop app shows notifications
  }
}

export function startLoading(state: AppState, conversationId: string): AppState {
  return { ...state, conversations: { ...state.conversations, [conversationId]: { status: "loading", messages: [], hasMore: false, pending: [] } } };
}

// The first page arrived. Events that came in meanwhile are replayed on top: the page already holds added
// and updated messages, but not text that is still streaming, which the daemon only saves when it ends.
export function finishLoading(state: AppState, conversationId: string, page: Page): AppState {
  const conversation = state.conversations[conversationId];
  if (!conversation || conversation.status !== "loading") return state;
  const messages = conversation.pending.reduce((list, event) => applyMessageEvent(list, event), page.messages);
  return { ...state, conversations: { ...state.conversations, [conversationId]: { status: "ready", messages, hasMore: page.hasMore, pending: [] } } };
}

export function addOlder(state: AppState, conversationId: string, page: Page): AppState {
  const conversation = state.conversations[conversationId];
  if (!conversation || conversation.status !== "ready") return state;
  const known = new Set(conversation.messages.map((m) => m.id));
  const older = page.messages.filter((m) => !known.has(m.id));
  return { ...state, conversations: { ...state.conversations, [conversationId]: { ...conversation, messages: [...older, ...conversation.messages], hasMore: page.hasMore } } };
}

export function forgetConversation(state: AppState, conversationId: string): AppState {
  return { ...state, conversations: without(state.conversations, conversationId) };
}

// A new connection: everything is fetched again, and open chats reload their history.
export function connected(state: AppState, snapshot: { bots: Bot[]; settings: Settings; routines: Routine[]; rooms: Room[]; accounts?: AccountInfo[] | null }): AppState {
  return {
    ...state,
    connection: { status: "ready" },
    bots: [...snapshot.bots].sort(byCreated),
    settings: snapshot.settings,
    routines: [...snapshot.routines].sort(byCreated),
    rooms: [...snapshot.rooms].sort(byCreated),
    accounts: snapshot.accounts ?? null,
    latestRuns: {},
    conversations: {},
  };
}

// A room's bots, in the bots' order. Everyone always holds every bot: the daemon sends no room.updated for it when a bot is made, so its
// botIds can be out of date.
export const roomBots = (room: Room, bots: Bot[]): Bot[] => (room.everyone ? bots : bots.filter((bot) => room.botIds.includes(bot.id)));
