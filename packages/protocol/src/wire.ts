import { z } from "zod";
import { Approval, ApprovalDecision, Bot, BotInput, BotPatch, Message, ModelInfo, Provider, ProviderSetup } from "./model.ts";

export const PROTOCOL_VERSION = 1;

const Empty = z.object({});
const Id = z.object({ id: z.string() });

// Every request the daemon answers: params are validated on arrival, results are what handlers return.
export const methods = {
  "setup.status": { params: Empty, result: z.array(ProviderSetup) },
  "setup.signIn": { params: z.object({ provider: Provider }), result: z.object({ url: z.string().nullable() }) },
  "models.list": { params: z.object({ provider: Provider }), result: z.array(ModelInfo) },
  "bots.list": { params: Empty, result: z.array(Bot) },
  "bots.create": { params: BotInput, result: Bot },
  "bots.update": { params: z.object({ id: z.string(), patch: BotPatch }), result: Bot },
  "bots.delete": { params: Id, result: Empty },
  "bots.pause": { params: z.object({ id: z.string(), paused: z.boolean() }), result: Bot },
  "chat.history": {
    params: z.object({ botId: z.string(), before: z.string().optional(), limit: z.number().int().min(1).max(200).default(50) }),
    result: z.array(Message),
  },
  "chat.send": {
    params: z.object({ botId: z.string(), text: z.string().trim().min(1).max(100_000), files: z.array(z.string()).max(20).default([]) }),
    result: Message,
  },
  "chat.stop": { params: z.object({ botId: z.string() }), result: Empty },
  "chat.retry": { params: z.object({ messageId: z.string() }), result: Message },
  "approvals.respond": { params: z.object({ approvalId: z.string(), decision: ApprovalDecision }), result: Approval },
} as const;

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
  notify: z.object({ botId: z.string(), title: z.string(), body: z.string() }),
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
  | { type: "res"; id: number; ok: false; error: { code: string; message: string } }
  | { type: "event"; event: EventName; data: unknown };
