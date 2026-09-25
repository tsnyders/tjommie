import { z } from "zod";

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

export const MessageKind = z.enum(["text", "activity", "approval", "notice", "error"]);
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

export const Approval = z.object({
  id: z.string(),
  botId: z.string(),
  tool: z.string(),
  summary: z.string(),
  detail: z.string(),
  rule: z.string().nullable(),
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
