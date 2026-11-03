import { z } from "zod";

export const LoginOrigin = z.string().max(2048).refine((value) => {
  try { const url = new URL(value); return url.protocol === "https:" && !url.username && !url.password && url.origin === value; }
  catch { return false; }
}, "Use an exact HTTPS origin, for example https://example.com.");
const Id = z.string().uuid();
export const LoginEntry = z.object({
  id: Id, name: z.string().trim().min(1).max(100), origin: LoginOrigin,
  botId: z.string().min(1).max(256), revision: z.number().int().positive(),
  createdAt: z.string().datetime(), updatedAt: z.string().datetime(),
}).strict();
export type LoginEntry = z.infer<typeof LoginEntry>;
export const VaultStatus = z.object({
  state: z.enum(["locked", "unlocked", "unavailable"]),
  reason: z.string().max(500).nullable(), pendingDeletes: z.number().int().nonnegative(),
}).strict();
export type VaultStatus = z.infer<typeof VaultStatus>;
export const VaultFillTarget = z.object({
  botId: z.string().min(1).max(256), generation: z.string().min(1).max(100),
  pageId: z.string().min(1).max(100), controlEpoch: z.number().int().nonnegative(),
}).strict();
export type VaultFillTarget = z.infer<typeof VaultFillTarget>;
const EntryVersion = { id: Id, revision: z.number().int().positive() };

// No secret, username, selector, script, credential target or host path crosses this protocol.
export const vaultMethods = {
  "vault.status": { params: z.object({}).strict(), result: VaultStatus },
  "vault.list": { params: z.object({ botId: z.string().min(1).max(256).optional() }).strict(), result: z.array(LoginEntry).max(1000) },
  "vault.unlock": { params: z.object({}).strict(), result: VaultStatus },
  "vault.lock": { params: z.object({}).strict(), result: VaultStatus },
  "vault.cancel": { params: z.object({}).strict(), result: z.object({}).strict() },
  "vault.entry": { params: z.object({ botId: z.string().min(1).max(256), id: Id.optional(), revision: z.number().int().positive().optional() }).strict(), result: z.object({ status: z.enum(["saved", "cancelled"]), entry: LoginEntry.nullable() }).strict() },
  "vault.delete": { params: z.object(EntryVersion).strict(), result: z.object({ status: z.enum(["deleted", "pending", "cancelled"]) }).strict() },
  "vault.fill": { params: VaultFillTarget.extend(EntryVersion).strict(), result: z.object({ status: z.enum(["filled", "cancelled"]), privateEpoch: z.number().int().nonnegative().nullable() }).strict() },
  "vault.finish": { params: z.object({ botId: VaultFillTarget.shape.botId, generation: VaultFillTarget.shape.generation, privateEpoch: z.number().int().nonnegative(), mode: z.enum(["clear", "close"]) }).strict(), result: z.object({ status: z.enum(["cleared", "closed", "cancelled"]) }).strict() },
} as const;
export const VaultHostAction = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("status") }).strict(), z.object({ kind: z.literal("list"), botId: VaultFillTarget.shape.botId.optional() }).strict(),
  z.object({ kind: z.literal("unlock") }).strict(), z.object({ kind: z.literal("lock") }).strict(),
  vaultMethods["vault.entry"].params.extend({ kind: z.literal("entry") }),
  vaultMethods["vault.delete"].params.extend({ kind: z.literal("delete") }),
  vaultMethods["vault.fill"].params.extend({ kind: z.literal("fill") }),
  vaultMethods["vault.finish"].params.extend({ kind: z.literal("finish") }),
  z.object({ kind: z.literal("disconnect") }).strict(),
  z.object({ kind: z.literal("cancel") }).strict(),
]);
export type VaultHostAction = z.infer<typeof VaultHostAction>;
export const VaultHostCommand = z.object({ controllerId: z.string().min(1).max(256), action: VaultHostAction }).strict();
export type VaultHostCommand = z.infer<typeof VaultHostCommand>;
export type VaultNativeApi = { command(action: VaultHostAction): Promise<unknown> };
