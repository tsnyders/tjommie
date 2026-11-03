import { z } from "zod";
import { VaultHostCommand, vaultMethods, type VaultHostAction } from "./vault.ts";

export const VAULT_HOST_BODY_BYTES = 4 * 1024 * 1024;
export const VaultHostJob = z.strictObject({ id: z.uuid(), sequence: z.number().int().positive().max(Number.MAX_SAFE_INTEGER), command: VaultHostCommand });
export type VaultHostJob = z.infer<typeof VaultHostJob>;
export const VaultHostReply = z.discriminatedUnion("ok", [
  z.strictObject({ id: z.uuid(), ok: z.literal(true), result: z.unknown() }),
  z.strictObject({ id: z.uuid(), ok: z.literal(false) }),
]);
export function vaultHostResult(action: VaultHostAction, value: unknown) {
  return action.kind === "disconnect" ? z.strictObject({}).parse(value) : vaultMethods[`vault.${action.kind}`].result.parse(value);
}
