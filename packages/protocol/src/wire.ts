import { z } from "zod";
import { browserMethods } from "./browser.ts";
import {
  Approval,
  ApprovalDecision,
  Bot,
  BotInput,
  BotPatch,
  Exchange,
  Message,
  ModelInfo,
  Provider,
  ProviderSetup,
  Routine,
  RoutineInput,
  RoutinePatch,
  RoutineRun,
  Settings,
} from "./model.ts";

export const PROTOCOL_VERSION = 1;

export const ErrorCode = z.enum([
  "bad_params", "not_found", "busy", "name_taken", "already_answered", "bad_state",
  "not_installed", "no_account", "unknown_method", "internal",
]);
export type ErrorCode = z.infer<typeof ErrorCode>;

const Empty = z.object({});
const Id = z.object({ id: z.string() });
const Page = { before: z.string().optional(), limit: z.number().int().min(1).max(200).default(50) };
const MessagePage = z.object({ messages: z.array(Message), hasMore: z.boolean() });
const PendingRoutineNotification = z.object({
  kind: z.literal("routine_failed"), botId: z.string(), routineId: z.string(), runId: z.string(), title: z.string(), body: z.string(),
});

// Every request the daemon answers: params are validated on arrival, results are what handlers return.
export const methods = {
  ...browserMethods,
  "setup.status": { params: Empty, result: z.array(ProviderSetup) },
  "setup.signIn": { params: z.object({ provider: Provider }), result: z.object({ url: z.string().nullable(), attempt: z.string() }) },
  "setup.prepareCodexSandbox": { params: Empty, result: ProviderSetup.options[1] },
  "models.list": { params: z.object({ provider: Provider }), result: z.array(ModelInfo) },
  "bots.list": { params: Empty, result: z.array(Bot) },
  "bots.create": { params: BotInput, result: Bot },
  "bots.update": { params: z.object({ id: z.string(), patch: BotPatch }), result: Bot },
  "bots.delete": { params: Id, result: Empty },
  "bots.pause": { params: z.object({ id: z.string(), paused: z.boolean() }), result: Bot },
  "bots.showBrowser": { params: Id, result: Empty },
  "bots.notes": { params: Id, result: z.object({ text: z.string() }) },
  "chat.history": { params: z.object({ botId: z.string(), ...Page }), result: MessagePage },
  "chat.send": {
    params: z.object({ botId: z.string(), text: z.string().trim().min(1).max(100_000), files: z.array(z.string()).max(20).default([]) }),
    result: Message,
  },
  "chat.stop": { params: z.object({ botId: z.string() }), result: Empty },
  "chat.retry": { params: z.object({ messageId: z.string(), botId: z.string().optional() }), result: Message },
  "approvals.respond": { params: z.object({ approvalId: z.string(), decision: ApprovalDecision }), result: Approval },
  "room.history": { params: z.object(Page), result: MessagePage },
  "room.post": { params: z.object({ text: z.string().trim().min(1).max(100_000) }), result: Message },
  "exchanges.get": { params: Id, result: MessagePage.extend({ exchange: Exchange }) },
  "routines.list": { params: z.object({ botId: z.string().optional() }), result: z.array(Routine) },
  "routines.create": { params: RoutineInput, result: Routine },
  "routines.update": { params: z.object({ id: z.string(), patch: RoutinePatch }), result: Routine },
  "routines.delete": { params: Id, result: Empty },
  "routines.runs": { params: z.object({ routineId: z.string() }), result: z.array(RoutineRun) },
  "notifications.pending": { params: Empty, result: z.array(PendingRoutineNotification) },
  "notifications.ack": { params: z.object({ runId: z.string().min(1) }), result: Empty },
  "settings.get": { params: Empty, result: Settings },
  "settings.set": { params: Settings.partial(), result: Settings },
} as const;

const Notification = z.object({ botId: z.string(), title: z.string(), body: z.string() });

// Everything the daemon pushes to connected clients.
export const events = {
  "message.added": Message,
  "message.updated": Message,
  "message.delta": z.object({ id: z.string(), conversationId: z.string(), delta: z.string() }),
  "bot.updated": Bot,
  "bot.deleted": Id,
  "approval.added": Approval,
  "approval.resolved": Approval,
  "setup.changed": z.array(ProviderSetup),
  // A provider's sign-in ended, sent after the fresh setup.changed. Also when it changed nothing: the window was
  // closed, the time ran out or the login failed.
  "setup.signInEnded": z.object({ provider: Provider, attempt: z.string() }),
  notify: z.discriminatedUnion("kind", [
    Notification.extend({ kind: z.literal("approval"), approvalId: z.string().optional() }),
    Notification.extend({ kind: z.literal("limit") }),
    Notification.extend({ kind: z.literal("routine_failed"), routineId: z.string().optional(), runId: z.string().optional() }),
  ]),
  "routine.updated": Routine,
  "routine.deleted": Id,
  "routine.run": RoutineRun,
  "settings.changed": Settings,
} as const;

export type MethodName = keyof typeof methods;
export type ParamsIn<M extends MethodName> = z.input<(typeof methods)[M]["params"]>;
export type ParamsOut<M extends MethodName> = z.output<(typeof methods)[M]["params"]>;
export type Result<M extends MethodName> = z.output<(typeof methods)[M]["result"]>;
export type EventName = keyof typeof events;
export type EventData<E extends EventName> = z.output<(typeof events)[E]>;

export const ClientMessage = z.discriminatedUnion("type", [
  z.object({ type: z.literal("hello"), protocol: z.number().int(), token: z.string() }),
  z.object({ type: z.literal("req"), id: z.number().int(), method: z.string(), params: z.unknown() }),
]);
export type ClientMessage = z.infer<typeof ClientMessage>;

export type ServerMessage =
  | { type: "welcome"; protocol: number }
  | { type: "res"; id: number; ok: true; result: unknown }
  | { type: "res"; id: number; ok: false; error: { code: ErrorCode; message: string } }
  | { type: "event"; event: EventName; data: unknown };
