import { z } from "zod";

// The conversation id of the Team room, the one room every bot is in.
export const TEAM_ROOM_ID = "team";

export const Provider = z.enum(["claude", "codex"]);
export type Provider = z.infer<typeof Provider>;

// ask: every change needs you. folder: free inside its own folders. full: never asks.
export const Autonomy = z.enum(["ask", "folder", "full"]);
export type Autonomy = z.infer<typeof Autonomy>;

export const BotColor = z.enum(["brown", "red", "orange", "amber", "green", "teal", "blue", "purple", "pink", "gray"]);
export type BotColor = z.infer<typeof BotColor>;

export const BotShape = z.enum(["circle", "blob", "rounded-square", "pill", "triangle", "hexagon", "cloud", "droplet"]);
export type BotShape = z.infer<typeof BotShape>;

export const BotState = z.enum(["idle", "working", "waiting", "paused", "limited", "needs_setup"]);
export type BotState = z.infer<typeof BotState>;

export const Bot = z.object({
  id: z.string(),
  name: z.string(),
  color: BotColor,
  shape: BotShape,
  instructions: z.string(),
  provider: Provider,
  model: z.string(),
  autonomy: Autonomy,
  homeFolder: z.string(),
  projectFolder: z.string().nullable(),
  browser: z.boolean(),
  paused: z.boolean(),
  canMessage: z.array(z.string()),
  state: BotState,
  limitedUntil: z.string().nullable(),
  createdAt: z.string(),
});
export type Bot = z.infer<typeof Bot>;

const botFields = {
  name: z.string().trim().min(1).max(40),
  color: BotColor,
  shape: BotShape,
  instructions: z.string().max(20_000),
  provider: Provider,
  model: z.string().min(1).max(100),
  autonomy: Autonomy,
  projectFolder: z.string().nullable(),
  browser: z.boolean(),
  canMessage: z.array(z.string()),
};
export const BotInput = z.object(botFields).partial({ autonomy: true, projectFolder: true, browser: true, canMessage: true });
export type BotInput = z.output<typeof BotInput>;
export const BotPatch = z.object(botFields).partial();
export type BotPatch = z.output<typeof BotPatch>;

// What `data` holds for the newer kinds:
// - handoff, for an exchange: { exchangeId, fromBotId, toBotId, messages, status: "waiting" | "replied" | "failed" };
//   the body is the request's first line. For an answer given in the Team room: { roomMessageId }.
// - routine: { routineId, runId, status, note }, the "Routine: <name>" label above a run.
// - file: { path, name, size }, a file a bot shared; the body is its caption.
// - text posts in the Team room that belong to an exchange: { exchangeId, toBotId, files?: { name, path }[] }.
export const MessageKind = z.enum(["text", "activity", "approval", "notice", "error", "handoff", "routine", "file"]);
export type MessageKind = z.infer<typeof MessageKind>;

export const MessageStatus = z.enum(["queued", "sent", "streaming", "done", "failed", "interrupted"]);
export type MessageStatus = z.infer<typeof MessageStatus>;

export const Message = z.object({
  id: z.string(),
  conversationId: z.string(),
  author: z.string(), // "you", "system" or a bot id
  kind: MessageKind,
  body: z.string(),
  data: z.record(z.string(), z.unknown()).nullable(),
  status: MessageStatus,
  createdAt: z.string(),
});
export type Message = z.infer<typeof Message>;

export const ApprovalDecision = z.enum(["once", "always", "deny"]);
export type ApprovalDecision = z.infer<typeof ApprovalDecision>;

// tool: a CLI asking to use a tool. routine: a bot proposing a routine (payload holds the RoutineInput).
export const Approval = z.object({
  id: z.string(),
  botId: z.string(),
  kind: z.enum(["tool", "routine"]),
  tool: z.string(),
  summary: z.string(),
  detail: z.string(),
  rule: z.string().nullable(),
  payload: z.record(z.string(), z.unknown()).nullable(),
  status: z.enum(["pending", "allowed", "denied"]),
  messageId: z.string(),
  createdAt: z.string(),
});
export type Approval = z.infer<typeof Approval>;

export const ProviderSetup = z.object({
  provider: Provider,
  installed: z.boolean(),
  version: z.string().nullable(),
  signedIn: z.boolean(),
  account: z.string().nullable(),
  plan: z.string().nullable(),
  error: z.string().nullable(),
});
export type ProviderSetup = z.infer<typeof ProviderSetup>;

export const ModelInfo = z.object({ id: z.string(), label: z.string(), isDefault: z.boolean() });
export type ModelInfo = z.infer<typeof ModelInfo>;

// Days are 0 (Sunday) to 6 (Saturday). Times are 24-hour "HH:MM" wall-clock times in the routine's time zone.
const routineFields = {
  name: z.string().trim().min(1).max(60),
  prompt: z.string().trim().min(1).max(10_000),
  days: z.array(z.number().int().min(0).max(6)).min(1).max(7),
  times: z.array(z.string().regex(/^([01]\d|2[0-3]):[0-5]\d$/)).min(1).max(12),
  timeZone: z.string().min(1).max(64),
  timeLimitMinutes: z.number().int().min(1).max(1440).nullable(),
  enabled: z.boolean(),
};
export const RoutineInput = z.object({ botId: z.string(), ...routineFields }).partial({ timeZone: true, timeLimitMinutes: true, enabled: true });
export type RoutineInput = z.output<typeof RoutineInput>;
export const RoutinePatch = z.object(routineFields).partial();
export type RoutinePatch = z.output<typeof RoutinePatch>;
export const Routine = z.object({ id: z.string(), botId: z.string(), ...routineFields, nextRunAt: z.string().nullable(), createdAt: z.string() });
export type Routine = z.infer<typeof Routine>;

export const RoutineRunStatus = z.enum(["queued", "running", "done", "stopped", "failed", "missed", "skipped"]);
export type RoutineRunStatus = z.infer<typeof RoutineRunStatus>;
export const RoutineRun = z.object({
  id: z.string(),
  routineId: z.string(),
  dueAt: z.string(),
  startedAt: z.string().nullable(),
  endedAt: z.string().nullable(),
  status: RoutineRunStatus,
  note: z.string().nullable(),
});
export type RoutineRun = z.infer<typeof RoutineRun>;

export const Exchange = z.object({
  id: z.string(),
  fromBotId: z.string(),
  toBotId: z.string(),
  request: z.string(),
  reply: z.string().nullable(),
  status: z.enum(["waiting", "replied", "failed"]),
  createdAt: z.string(),
  repliedAt: z.string().nullable(),
});
export type Exchange = z.infer<typeof Exchange>;

// chainLimit: how many bot-to-bot requests one chain (a message from you, a room post or a routine run) may make.
export const Settings = z.object({ chainLimit: z.number().int().min(1).max(20) });
export type Settings = z.infer<typeof Settings>;
