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
  setupReason: z.string().nullable(),
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

const ProviderSetupFields = {
  installed: z.boolean(),
  version: z.string().nullable(),
  signedIn: z.boolean(),
  account: z.string().nullable(),
  plan: z.string().nullable(),
  error: z.string().nullable(),
};
export const CodexSandboxReadiness = z.enum(["ready", "notConfigured", "updateRequired", "unavailable"]);
export type CodexSandboxReadiness = z.infer<typeof CodexSandboxReadiness>;
export const ProviderSetup = z.discriminatedUnion("provider", [
  z.object({ provider: z.literal("claude"), ...ProviderSetupFields }),
  z.object({ provider: z.literal("codex"), ...ProviderSetupFields, sandbox: CodexSandboxReadiness }),
]);
export type ProviderSetup = z.infer<typeof ProviderSetup>;

export const ModelInfo = z.object({ id: z.string(), label: z.string(), isDefault: z.boolean() });
export type ModelInfo = z.infer<typeof ModelInfo>;

const isTimeZone = (timeZone: string): boolean => {
  try {
    new Intl.DateTimeFormat("en", { timeZone });
    return true;
  } catch {
    return false;
  }
};

// Days are 0 (Sunday) to 6 (Saturday). Times are 24-hour "HH:MM" wall-clock times in the routine's time zone.
const routineFields = {
  name: z.string().trim().min(1).max(60),
  prompt: z.string().trim().min(1).max(10_000),
  days: z.array(z.number().int().min(0).max(6)).min(1).max(7),
  times: z.array(z.string().regex(/^([01]\d|2[0-3]):[0-5]\d$/)).min(1).max(12),
  timeZone: z.string().min(1).max(64).refine(isTimeZone, "not an IANA time zone"),
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
  botId: z.string(),
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

// Payload schemas are separate from Message.data so existing wire readers keep
// accepting records. messageData validates a payload before a typed reader uses it.
export const ExchangeHandoffData = z.strictObject({
  exchangeId: z.string(),
  fromBotId: z.string(),
  toBotId: z.string(),
  messages: z.number().int().nonnegative(),
  status: Exchange.shape.status,
});
export type ExchangeHandoffData = z.infer<typeof ExchangeHandoffData>;
export const RoomHandoffData = z.strictObject({ roomMessageId: z.string() });
export type RoomHandoffData = z.infer<typeof RoomHandoffData>;
export const HandoffData = z.union([ExchangeHandoffData, RoomHandoffData]);
export type HandoffData = z.infer<typeof HandoffData>;

export const RoutineLabelData = z.strictObject({
  routineId: z.string(),
  runId: z.string(),
  status: RoutineRunStatus,
  note: z.string().nullable(),
});
export type RoutineLabelData = z.infer<typeof RoutineLabelData>;

export const FileData = z.strictObject({ path: z.string(), name: z.string(), size: z.number().int().nonnegative() });
export type FileData = z.infer<typeof FileData>;

const approvalDataFields = {
  approvalId: z.string(),
  tool: z.string(),
  detail: z.string(),
  status: Approval.shape.status,
};
export const ToolApprovalData = z.strictObject({
  ...approvalDataFields,
  kind: z.literal("tool").optional(), // Older tool cards omit kind.
  rule: z.string().nullable(),
});
export type ToolApprovalData = z.infer<typeof ToolApprovalData>;
export const RoutineApprovalData = z.strictObject({ ...approvalDataFields, kind: z.literal("routine") });
export type RoutineApprovalData = z.infer<typeof RoutineApprovalData>;
export const ApprovalData = z.union([ToolApprovalData, RoutineApprovalData]);
export type ApprovalData = z.infer<typeof ApprovalData>;

export const ActivityItem = z.strictObject({
  id: z.string(),
  name: z.string(),
  summary: z.string(),
  status: z.enum(["running", "done", "failed"]),
});
export type ActivityItem = z.infer<typeof ActivityItem>;
export const ActivityData = z.strictObject({ items: z.array(ActivityItem) });
export type ActivityData = z.infer<typeof ActivityData>;

export const ErrorData = z.strictObject({ retryMessageId: z.string(), retryBotId: z.string().optional() });
export type ErrorData = z.infer<typeof ErrorData>;
export const NoticeData = z.strictObject({ exchangeId: z.string() });
export type NoticeData = z.infer<typeof NoticeData>;

// Your attachments store inbox names. Exchange posts store the recipient's paths.
export const UserMessageData = z.strictObject({ files: z.array(z.string()) });
export type UserMessageData = z.infer<typeof UserMessageData>;
export const ExchangePostData = z.strictObject({
  exchangeId: z.string(),
  toBotId: z.string(),
  files: z.array(z.strictObject({ name: z.string(), path: z.string() })).optional(),
});
export type ExchangePostData = z.infer<typeof ExchangePostData>;
export const TextData = z.union([UserMessageData, ExchangePostData]);
export type TextData = z.infer<typeof TextData>;

export const MessageData = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("text"), data: TextData.nullable() }),
  z.object({ kind: z.literal("activity"), data: ActivityData }),
  z.object({ kind: z.literal("approval"), data: ApprovalData }),
  z.object({ kind: z.literal("notice"), data: NoticeData.nullable() }),
  z.object({ kind: z.literal("error"), data: ErrorData.nullable() }),
  z.object({ kind: z.literal("handoff"), data: HandoffData }),
  z.object({ kind: z.literal("routine"), data: RoutineLabelData }),
  z.object({ kind: z.literal("file"), data: FileData }),
]);
export type MessageData = z.infer<typeof MessageData>;

// Check the returned kind to narrow data. Invalid payloads throw a ZodError.
export function messageData(message: Pick<Message, "kind" | "data">): MessageData {
  return MessageData.parse(message);
}

// chainLimit: how many bot-to-bot requests one chain (a message from you, a room post or a routine run) may make.
export const Settings = z.object({ chainLimit: z.number().int().min(1).max(20) });
export type Settings = z.infer<typeof Settings>;
