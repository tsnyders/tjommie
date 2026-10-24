import { constants, promises as fs } from "node:fs";
import { join, resolve } from "node:path";
import type { AccountProviderUsage, AccountUsageWindow } from "@tjommie/protocol";
import type { AccountLimitSnapshot, Account } from "./setup.ts";

export type AccountUsageData = Pick<AccountProviderUsage, "planLabel" | "windows" | "credits" | "extraUsageEnabled">;
export type AccountUsageRead = { status: "available"; usage: AccountUsageData; limits: AccountLimitSnapshot[]; recoveryAllowed: boolean }
  | { status: "unavailable" | "error"; error: string | null };

const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const label = (value: unknown, maximum = 120): string | null => typeof value === "string" ? value.replace(/[\x00-\x1f\x7f]/g, "").trim().slice(0, maximum) || null : null;
const percent = (value: unknown): number | null => {
  const number = typeof value === "string" && value.length <= 32 && /^\d+(?:\.\d+)?$/.test(value) ? Number(value) : value;
  return typeof number === "number" && Number.isFinite(number) && number >= 0 && number <= 100 ? number : null;
};
const instant = (value: unknown): string | null => typeof value === "string" && Number.isFinite(Date.parse(value)) ? new Date(value).toISOString() : null;
const failed = (): AccountUsageRead => ({ status: "error", error: "Provider usage could not be read. Try again later." });

export function emptyProviderUsage(): AccountProviderUsage {
  return { status: "unavailable", checkedAt: null, lastSuccessfulAt: null, error: null,
    planLabel: null, windows: [], credits: null, extraUsageEnabled: null };
}

export function normalizeCodexUsage(value: unknown, snapshots: AccountLimitSnapshot[] | null): AccountUsageRead {
  if (!record(value) || !record(value.rateLimits)) return failed();
  const buckets = record(value.rateLimitsByLimitId) ? Object.values(value.rateLimitsByLimitId) : [value.rateLimits];
  if (!buckets.length || buckets.length > 32 || !buckets.every(record)) return failed();
  // A credits-only response is displayable, but never proves that a previous quota block recovered.
  if (!snapshots && buckets.some((bucket) => bucket.primary != null || bucket.secondary != null || bucket.rateLimitReachedType != null)) return failed();
  const limits = (snapshots ?? []).map((snapshot) => ({ ...snapshot, windows: [...snapshot.windows] }));
  const windows: AccountUsageWindow[] = [];
  for (const snapshot of snapshots ?? []) {
    const bucket = (record(value.rateLimitsByLimitId) ? value.rateLimitsByLimitId[snapshot.limitId] : undefined)
      ?? buckets.find((entry) => entry.limitId === snapshot.limitId) ?? (snapshot.limitId === "codex" ? value.rateLimits : undefined);
    const model = record(bucket) ? label(bucket.normalModelSlug, 256) : null;
    const name = record(bucket) ? label(bucket.limitName) : null;
    for (const window of snapshot.windows) {
      if (window.windowKey === "provider") continue;
      const title = window.durationMinutes === 300 ? "Session" : window.durationMinutes === 10080 ? "Weekly"
        : window.durationMinutes === null ? `${window.windowKey === "primary" ? "Primary" : "Secondary"} limit` : `${window.durationMinutes} minute limit`;
      windows.push({ id: `${snapshot.limitId}:${window.windowKey}`, label: `${title}${name ? ` · ${name}` : ""}`.slice(0, 120),
        scope: model ? { kind: "model", id: model, label: name ?? model.slice(0, 120) } : null,
        durationMinutes: window.durationMinutes, usedPercent: window.usedPercent, resetsAt: window.resetsAt });
    }
  }
  const rawCredits = record(value.rateLimits.credits) ? value.rateLimits.credits : buckets.find((bucket) => record(bucket.credits))?.credits;
  const balance = record(rawCredits) && typeof rawCredits.balance === "string" && /^-?\d+(?:\.\d+)?$/.test(rawCredits.balance) && rawCredits.balance.length <= 100 ? rawCredits.balance : null;
  const credits = record(rawCredits) ? { balance, hasCredits: typeof rawCredits.hasCredits === "boolean" ? rawCredits.hasCredits : null,
    unlimited: typeof rawCredits.unlimited === "boolean" ? rawCredits.unlimited : null } : null;
  const permissionBuckets = [value.rateLimits, ...buckets];
  const blocked = value.ordinaryUsageAllowed === false || permissionBuckets.some((bucket) => bucket.spendControlReached === true);
  if (blocked) {
    let provider = limits.find((snapshot) => snapshot.limitId === "provider");
    if (!provider) { provider = { limitId: "provider", windows: [] }; limits.push(provider); }
    provider.windows.push({ windowKey: "permission", durationMinutes: null, usedPercent: null, resetsAt: null, state: "reached" });
  }
  return { status: "available", usage: { planLabel: label(value.rateLimits.planType) ?? label(buckets.find((bucket) => typeof bucket.planType === "string")?.planType),
    windows: windows.slice(0, 64), credits, extraUsageEnabled: null }, limits,
    // Null/absent spend-control state is unavailable, not evidence that an old spend block cleared.
    recoveryAllowed: snapshots !== null && value.ordinaryUsageAllowed === true && permissionBuckets.every((bucket) => bucket.spendControlReached === false) };
}

function claudeWindow(value: unknown, id: string, title: string, duration: number, scope: AccountUsageWindow["scope"] = null): AccountUsageWindow | null {
  if (!record(value)) return null;
  const used = percent(value.utilization ?? value.percent);
  if (used === null) return null;
  return { id, label: title.slice(0, 120), scope, durationMinutes: duration, usedPercent: used, resetsAt: instant(value.resets_at) };
}

export function normalizeClaudeUsage(value: unknown, planLabel: string | null): AccountUsageRead {
  if (!record(value)) return failed();
  const windows: AccountUsageWindow[] = [];
  for (const [key, title, duration] of [["five_hour", "Session", 300], ["seven_day", "Weekly", 10080]] as const) {
    const window = claudeWindow(value[key], key, title, duration); if (window) windows.push(window);
  }
  const accountWindows = [...windows];
  let scopedComplete = true, scopedExhausted = false;
  for (const [key, name] of [["seven_day_opus", "Opus"], ["seven_day_omelette", "Omelette"]] as const) {
    const window = claudeWindow(value[key], `weekly:model:${name.toLowerCase()}`, `Weekly · ${name}`, 10080, { kind: "model", id: null, label: name });
    if (window) { windows.push(window); scopedExhausted ||= window.usedPercent === 100; }
    else if (value[key] != null) scopedComplete = false;
  }
  if (value.limits != null && !Array.isArray(value.limits)) scopedComplete = false;
  for (const entry of Array.isArray(value.limits) ? value.limits.slice(0, 128) : []) {
    if (!record(entry) || entry.kind !== "weekly_scoped" || !record(entry.scope)) { scopedComplete = false; continue; }
    const kind = record(entry.scope.model) ? "model" : record(entry.scope.surface) ? "surface" : null;
    const scope = kind ? entry.scope[kind] : null;
    if (!kind || !record(scope)) { scopedComplete = false; continue; }
    const id = label(scope.id, 256), name = label(scope.display_name) ?? id?.slice(0, 120) ?? null;
    if (!name) { scopedComplete = false; continue; }
    const windowId = `weekly:${kind}:${id ?? name.toLowerCase().replace(/[^a-z0-9]+/g, "_")}`.slice(0, 256);
    const window = claudeWindow(entry, windowId, `Weekly · ${name}`, 10080, { kind, id, label: name });
    if (!window) { scopedComplete = false; continue; }
    scopedExhausted ||= window.usedPercent === 100;
    const twin = windows.findIndex((candidate) => candidate.scope?.kind === kind && (id && candidate.scope.id ? id === candidate.scope.id : candidate.scope.label.toLowerCase() === name.toLowerCase()));
    if (twin >= 0) windows[twin] = window; else windows.push(window);
  }
  if (Array.isArray(value.limits) && value.limits.length > 128 || windows.length > 64) scopedComplete = false;
  const extraUsageEnabled = record(value.extra_usage) && typeof value.extra_usage.is_enabled === "boolean" ? value.extra_usage.is_enabled : null;
  if (!windows.length && extraUsageEnabled === null) return failed();
  const usedIds = new Set<string>();
  for (const window of windows) {
    const base = window.id; let suffix = 1;
    while (usedIds.has(window.id)) window.id = `${base.slice(0, 248)}:${++suffix}`;
    usedIds.add(window.id);
  }
  return { status: "available", usage: { planLabel, windows: windows.slice(0, 64), credits: null, extraUsageEnabled },
    limits: accountWindows.length ? [{ limitId: "claude", windows: accountWindows.map((window) => ({ windowKey: window.id, durationMinutes: window.durationMinutes,
      usedPercent: window.usedPercent, resetsAt: window.resetsAt, state: window.usedPercent! >= 100 ? "reached" : window.usedPercent! >= 90 ? "near" : "ok" })) }] : [],
    recoveryAllowed: accountWindows.length === 2 && scopedComplete && !scopedExhausted };
}

const USAGE_URL = "https://api.anthropic.com/api/oauth/usage";
const FILE_LIMIT = 64 * 1024, RESPONSE_LIMIT = 128 * 1024;
const cooldowns = new Map<string, number>();
type ClaudeUsageOptions = { accountsRoot: string; fetch?: typeof fetch; now?: () => number };
const samePath = (a: string, b: string): boolean => process.platform === "win32" ? resolve(a).toLowerCase() === resolve(b).toLowerCase() : resolve(a) === resolve(b);

async function readClaudeCredential(account: Account, root: string): Promise<{ token: string; expiresAt: number | null; plan: string | null }> {
  if (!/^[a-zA-Z0-9_-]{1,256}$/.test(account.id) || !samePath(account.folder, join(root, account.id))) throw new Error("Account identity mismatch");
  const anchors = [resolve(root), resolve(account.folder)];
  const before = await Promise.all(anchors.map((path) => fs.lstat(path)));
  if (before.some((stat) => !stat.isDirectory() || stat.isSymbolicLink())) throw new Error("Invalid account directory");
  const path = join(account.folder, ".credentials.json"), stat = await fs.lstat(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.nlink !== 1 || stat.size > FILE_LIMIT) throw new Error("Invalid credential file");
  const file = await fs.open(path, constants.O_RDONLY | (process.platform === "win32" ? 0 : constants.O_NOFOLLOW));
  let text: string;
  try {
    const opened = await file.stat();
    if (opened.dev !== stat.dev || opened.ino !== stat.ino || opened.nlink !== 1 || opened.size > FILE_LIMIT) throw new Error("Credential file changed");
    const bytes = Buffer.alloc(FILE_LIMIT + 1); let size = 0;
    while (size < bytes.length) { const read = await file.read(bytes, size, bytes.length - size, size); if (!read.bytesRead) break; size += read.bytesRead; }
    if (size > FILE_LIMIT) throw new Error("Credential file too large");
    text = bytes.toString("utf8", 0, size);
  } finally { await file.close(); }
  const after = await Promise.all([...anchors, path].map((entry) => fs.lstat(entry)));
  if (after.some((entry) => entry.isSymbolicLink()) || before.some((entry, index) => entry.dev !== after[index].dev || entry.ino !== after[index].ino)
    || stat.dev !== after[2].dev || stat.ino !== after[2].ino || after[2].nlink !== 1) throw new Error("Credential path changed");
  const parsed: unknown = JSON.parse(text);
  const oauth = record(parsed) && record(parsed.claudeAiOauth) ? parsed.claudeAiOauth : null;
  if (!oauth || typeof oauth.accessToken !== "string" || !/^[A-Za-z0-9._~+/-]{1,16384}=*$/.test(oauth.accessToken) || oauth.accessToken.length > 16384) throw new Error("No subscription credential");
  const token = oauth.accessToken;
  if ([oauth.subscriptionType, oauth.rateLimitTier].some((value) => typeof value === "string" && value.includes(token))) throw new Error("Invalid plan metadata");
  const subscription = label(oauth.subscriptionType), tier = label(oauth.rateLimitTier)?.split("_").at(-1);
  return { token, expiresAt: typeof oauth.expiresAt === "number" && Number.isFinite(oauth.expiresAt) ? oauth.expiresAt : null,
    plan: subscription ? `${subscription[0].toUpperCase()}${subscription.slice(1)}${tier ? ` ${tier}` : ""}`.slice(0, 120) : null };
}

async function responseJson(response: Response): Promise<unknown> {
  if (!response.body || Number(response.headers.get("content-length")) > RESPONSE_LIMIT) throw new Error("Invalid usage response");
  const reader = response.body.getReader(), chunks: Uint8Array[] = []; let size = 0;
  try {
    for (;;) { const next = await reader.read(); if (next.done) break; size += next.value.length;
      if (size > RESPONSE_LIMIT) throw new Error("Usage response too large"); chunks.push(next.value); }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

/** The selected CLI owns this credential and all refresh/write operations. Only its access token is consumed here. */
export async function readClaudeAccountUsage(account: Account, options: ClaudeUsageOptions): Promise<AccountUsageRead> {
  if (account.provider !== "claude") return { status: "unavailable", error: null };
  const now = options.now ?? Date.now;
  for (const [key, until] of cooldowns) if (until <= now()) cooldowns.delete(key);
  if ((cooldowns.get(account.folder) ?? 0) > now()) return { status: "error", error: "Claude usage is rate limited. Try again later." };
  let credential: Awaited<ReturnType<typeof readClaudeCredential>>;
  try { credential = await readClaudeCredential(account, options.accountsRoot); }
  catch { return { status: "unavailable", error: "Claude usage is unavailable for this sign-in. Check the account in Settings." }; }
  if (credential.expiresAt !== null && credential.expiresAt <= now()) return { status: "unavailable", error: "Claude sign-in has expired. Sign in again to refresh usage." };
  let response: Response | undefined;
  try {
    response = await (options.fetch ?? fetch)(USAGE_URL, { method: "GET", redirect: "error", signal: AbortSignal.timeout(15_000),
      headers: { Authorization: `Bearer ${credential.token}`, Accept: "application/json", "anthropic-beta": "oauth-2025-04-20" } });
    if (response.url && response.url !== USAGE_URL) return failed();
    if (response.status === 401 || response.status === 403) return { status: "unavailable", error: "Claude sign-in was rejected. Check the account in Settings." };
    if (response.status === 429) {
      const retry = response.headers.get("retry-after"), seconds = retry === null ? NaN : Number(retry);
      const until = Number.isFinite(seconds) ? now() + Math.max(0, seconds) * 1000 : retry === null ? NaN : Date.parse(retry);
      if (cooldowns.size >= 128) cooldowns.delete(cooldowns.keys().next().value!);
      cooldowns.set(account.folder, Number.isFinite(until) ? Math.max(now() + 1000, until) : now() + 60_000);
      return { status: "error", error: "Claude usage is rate limited. Try again later." };
    }
    if (!response.ok) return failed();
    const raw = await responseJson(response);
    if (JSON.stringify(raw).includes(credential.token)) return failed();
    const result = normalizeClaudeUsage(raw, credential.plan);
    return JSON.stringify(result).includes(credential.token) ? failed() : result;
  } catch { return failed(); }
  finally { if (response?.body && !response.body.locked) await response.body.cancel().catch(() => {}); }
}
