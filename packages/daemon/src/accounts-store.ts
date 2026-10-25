import type { Provider } from "@tjommie/protocol";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";
import { accountBudget } from "./usage.ts";
export { accountBudget } from "./usage.ts";

export type AccountKind = "subscription" | "api_key";
export type StoredAccount = {
  id: string; provider: Provider; label: string; kind: AccountKind; folder: string;
  credentialRef: string | null; isDefault: boolean; isFallback: boolean; spendingCapUsd: number | null;
  limitedUntil: string | null; createdAt: string; deletedAt: string | null; capRevision: number;
};
export type AccountInput = {
  id?: string; provider: Provider; label: string; kind: AccountKind; folder: string;
  credentialRef?: string | null; isDefault?: boolean; isFallback?: boolean; spendingCapUsd?: number | null;
};
export type AccountPatch = Partial<Pick<StoredAccount, "label" | "credentialRef" | "isDefault" | "isFallback" | "spendingCapUsd">>;
export type AccountLimitWindow = { windowKey: string; durationMinutes: number | null; usedPercent: number | null; resetsAt: string | null; state: "ok" | "near" | "reached" };
export type StoredAccountLimit = AccountLimitWindow & { accountId: string; limitId: string; observedAt: string };
const columns = `id, provider, label, kind, folder, credential_ref AS credentialRef, is_default AS isDefault,
  is_fallback AS isFallback, spending_cap_usd AS spendingCapUsd, limited_until AS limitedUntil,
  created_at AS createdAt, deleted_at AS deletedAt, cap_revision AS capRevision`;
type AccountRow = Omit<StoredAccount, "isDefault" | "isFallback"> & { isDefault: number; isFallback: number };
const account = (row: AccountRow): StoredAccount => ({ ...row, isDefault: row.isDefault === 1, isFallback: row.isFallback === 1 });
const bad = (message: string): never => { throw new RpcError("bad_params", message); };

export function listAccounts(db: Db, options: { includeDeleted?: boolean } = {}): StoredAccount[] {
  return (db.prepare(`SELECT ${columns} FROM accounts ${options.includeDeleted ? "" : "WHERE deleted_at IS NULL"} ORDER BY created_at, id`).all() as AccountRow[]).map(account);
}
export function getAccount(db: Db, id: string, options: { includeDeleted?: boolean } = {}): StoredAccount {
  const row = db.prepare(`SELECT ${columns} FROM accounts WHERE id = ? ${options.includeDeleted ? "" : "AND deleted_at IS NULL"}`).get(id) as AccountRow | undefined;
  if (!row) throw new RpcError("not_found", "Account not found");
  return account(row);
}
export function getDefaultAccount(db: Db, provider: Provider): StoredAccount {
  const found = listAccounts(db).filter((entry) => entry.provider === provider);
  const selected = found.find((entry) => entry.isDefault) ?? found[0];
  if (!selected) throw new RpcError("no_account", `No ${provider} account`);
  return selected;
}
export function getFallbackAccount(db: Db, provider: Provider): StoredAccount | null {
  return listAccounts(db).find((entry) => entry.provider === provider && entry.isFallback) ?? null;
}
function validate(value: Pick<StoredAccount, "label" | "kind" | "spendingCapUsd" | "isFallback" | "credentialRef">): void {
  if (!value.label.trim() || value.label.trim().length > 60) bad("Choose an account name of 1 to 60 characters");
  if (value.kind !== "api_key" && value.kind !== "subscription") bad("Unknown account kind");
  if (value.spendingCapUsd !== null && (!Number.isFinite(value.spendingCapUsd) || value.spendingCapUsd <= 0 || value.spendingCapUsd > 1_000_000)) bad("Spending cap must be a positive finite amount up to 1,000,000 USD");
  if (value.kind === "api_key" && value.spendingCapUsd === null) bad("API keys require a spending cap");
  if (value.kind === "subscription" && (value.isFallback || value.credentialRef !== null || value.spendingCapUsd !== null)) bad("Subscription accounts cannot use API-key settings");
  if (value.credentialRef !== null && (!value.credentialRef.trim() || value.credentialRef.length > 512)) bad("Invalid credential reference");
}
export function createAccount(db: Db, input: AccountInput): StoredAccount {
  if (input.provider !== "claude" && input.provider !== "codex") bad("Unknown provider");
  if (!input.folder.trim()) bad("Account folder is required");
  const value = { ...input, id: input.id ?? newId(), label: input.label.trim(), credentialRef: input.credentialRef ?? null,
    spendingCapUsd: input.spendingCapUsd ?? null, isFallback: input.isFallback ?? false };
  if (!value.id.trim()) bad("Account id is required");
  validate(value);
  transaction(db, () => {
    const isDefault = input.isDefault === true || !listAccounts(db).some((entry) => entry.provider === input.provider);
    if (isDefault) db.prepare("UPDATE accounts SET is_default = 0 WHERE provider = ?").run(input.provider);
    if (value.isFallback) db.prepare("UPDATE accounts SET is_fallback = 0 WHERE provider = ?").run(input.provider);
    db.prepare(`INSERT INTO accounts (id, provider, label, kind, folder, created_at, credential_ref, is_default, is_fallback, spending_cap_usd)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(value.id, value.provider, value.label, value.kind, value.folder, nowIso(), value.credentialRef, Number(isDefault), Number(value.isFallback), value.spendingCapUsd);
  });
  return getAccount(db, value.id);
}
export function updateAccount(db: Db, id: string, patch: AccountPatch): StoredAccount {
  const current = getAccount(db, id);
  if (current.isDefault && patch.isDefault === false) bad("Choose another default account before changing this one");
  const next = { ...current, ...patch, label: patch.label?.trim() ?? current.label };
  validate(next);
  transaction(db, () => {
    if (next.isDefault) db.prepare("UPDATE accounts SET is_default = 0 WHERE provider = ?").run(current.provider);
    if (next.isFallback) db.prepare("UPDATE accounts SET is_fallback = 0 WHERE provider = ?").run(current.provider);
    db.prepare(`UPDATE accounts SET label = ?, credential_ref = ?, is_default = ?, is_fallback = ?, spending_cap_usd = ?, cap_revision = ? WHERE id = ?`)
      .run(next.label, next.credentialRef, Number(next.isDefault), Number(next.isFallback), next.spendingCapUsd, current.capRevision + Number(next.spendingCapUsd !== current.spendingCapUsd), id);
  });
  return getAccount(db, id);
}
export function removeAccount(db: Db, id: string): void {
  const current = getAccount(db, id, { includeDeleted: true });
  if (current.deletedAt) return;
  if (db.prepare("SELECT 1 FROM bots WHERE account_id = ? LIMIT 1").get(id)) throw new RpcError("busy", "Move this account's bots before removing it");
  transaction(db, () => {
    db.prepare("UPDATE accounts SET deleted_at = ?, is_default = 0, is_fallback = 0 WHERE id = ?").run(nowIso(), id);
    if (current.isDefault) {
      const replacement = listAccounts(db).find((entry) => entry.provider === current.provider);
      if (replacement) db.prepare("UPDATE accounts SET is_default = 1 WHERE id = ?").run(replacement.id);
    }
  });
}
export function bindLegacyBots(db: Db): void {
  // Fresh installations seed the legacy accounts after migration. Repair only missing defaults,
  // leaving explicit choices and retired account identities untouched.
  db.prepare(`UPDATE accounts SET is_default = 1 WHERE deleted_at IS NULL
    AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.provider = accounts.provider AND a.deleted_at IS NULL AND a.is_default = 1)
    AND id = (SELECT a.id FROM accounts a WHERE a.provider = accounts.provider AND a.deleted_at IS NULL ORDER BY a.created_at, a.id LIMIT 1)`).run();
  db.prepare(`UPDATE bots SET account_id = COALESCE(
    (SELECT a.id FROM sessions s JOIN accounts a ON a.id = s.account_id WHERE s.bot_id = bots.id AND s.provider = bots.provider AND a.provider = bots.provider AND a.deleted_at IS NULL ORDER BY s.updated_at DESC, a.id LIMIT 1),
    (SELECT a.id FROM accounts a WHERE a.provider = bots.provider AND a.deleted_at IS NULL ORDER BY a.is_default DESC, a.created_at, a.id LIMIT 1))
    WHERE account_id IS NULL`).run();
}
export function listAccountLimits(db: Db, accountId: string): StoredAccountLimit[] {
  getAccount(db, accountId);
  return db.prepare(`SELECT account_id AS accountId, limit_id AS limitId, window_key AS windowKey, duration_minutes AS durationMinutes,
    used_percent AS usedPercent, resets_at AS resetsAt, state, observed_at AS observedAt FROM account_limit_windows
    WHERE account_id = ? ORDER BY limit_id, window_key`).all(accountId) as StoredAccountLimit[];
}
export function replaceAccountLimits(db: Db, input: { accountId: string; limitId: string; observedAt: string; windows: AccountLimitWindow[] }): void {
  getAccount(db, input.accountId);
  const stamp = new Date(input.observedAt);
  if (!Number.isFinite(stamp.getTime()) || !input.limitId.trim()) bad("Invalid limit snapshot");
  const observedAt = stamp.toISOString();
  const keys = new Set<string>();
  const windows = input.windows.map((window) => {
    if (!window.windowKey.trim() || keys.has(window.windowKey)) bad("Quota window identities must be unique");
    keys.add(window.windowKey);
    if (!(["ok", "near", "reached"] as const).includes(window.state)) bad("Invalid quota state");
    if (window.usedPercent !== null && (!Number.isFinite(window.usedPercent) || window.usedPercent < 0 || window.usedPercent > 100)) bad("Invalid quota percentage");
    if (window.durationMinutes !== null && (!Number.isFinite(window.durationMinutes) || window.durationMinutes <= 0)) bad("Invalid quota duration");
    if (window.resetsAt !== null && !Number.isFinite(Date.parse(window.resetsAt))) bad("Invalid quota reset");
    return { ...window, resetsAt: window.resetsAt === null ? null : new Date(window.resetsAt).toISOString() };
  });
  transaction(db, () => {
    const previous = db.prepare("SELECT observed_at FROM account_limit_snapshots WHERE account_id = ? AND limit_id = ?").get(input.accountId, input.limitId) as { observed_at: string } | undefined;
    if (previous && previous.observed_at >= observedAt) return;
    db.prepare(`INSERT INTO account_limit_snapshots VALUES (?, ?, ?) ON CONFLICT(account_id, limit_id) DO UPDATE SET observed_at = excluded.observed_at`).run(input.accountId, input.limitId, observedAt);
    db.prepare("DELETE FROM account_limit_windows WHERE account_id = ? AND limit_id = ?").run(input.accountId, input.limitId);
    const insert = db.prepare("INSERT INTO account_limit_windows VALUES (?, ?, ?, ?, ?, ?, ?, ?)");
    for (const window of windows) insert.run(input.accountId, input.limitId, window.windowKey, window.durationMinutes, window.usedPercent, window.resetsAt, window.state, observedAt);
    const reached = listAccountLimits(db, input.accountId).filter((window) => window.state === "reached");
    const resets = reached.map((window) => window.resetsAt);
    const until = resets.length === 0 || resets.includes(null) ? null : (resets as string[]).sort().at(-1)!;
    db.prepare("UPDATE accounts SET limited_until = ? WHERE id = ?").run(until, input.accountId);
  });
}
export function claimCapNotification(db: Db, id: string, threshold: 80 | 100): boolean {
  if (threshold !== 80 && threshold !== 100) bad("Invalid spending threshold");
  const current = getAccount(db, id);
  if (current.kind !== "api_key" || current.spendingCapUsd === null) return false;
  const budget = accountBudget(db, id);
  if (!(budget.knownUsd >= current.spendingCapUsd * threshold / 100 || (threshold === 100 && budget.unknownRows > 0))) return false;
  return db.prepare("INSERT OR IGNORE INTO account_cap_notifications VALUES (?, ?, ?, ?)").run(id, current.capRevision, threshold, nowIso()).changes === 1;
}
