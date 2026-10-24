import { randomUUID } from "node:crypto";
import type { AccountInfo, AccountProviderUsage, Provider, ProviderSetup } from "@tjommie/protocol";
import { emptyProviderUsage, type AccountUsageRead } from "./account-quota.ts";
import { WindowsAccountCredentials, type CredentialStore } from "./account-credentials.ts";
import { hasPricing } from "./account-pricing.ts";
import { accountBudget, claimCapNotification, getAccount, getDefaultAccount, getFallbackAccount, listAccountLimits, listAccounts, replaceAccountLimits, updateAccount, type AccountKind, type StoredAccount, type StoredAccountLimit } from "./accounts-store.ts";
import { getStoredBot, type StoredBot } from "./bots.ts";
import { resolveCli } from "./cli.ts";
import type { Db } from "./db.ts";
import type { Paths } from "./paths.ts";
import type { ProviderAuthentication, ProviderEvent } from "./providers/types.ts";
import { accountEnv, cancelAccountSignIn, checkAccountLimits, checkAccountSetup, checkAccountUsage, logoutAccount, prepareAccountCodexSandbox, prepareAccountFolder, signInAccount, type Account } from "./setup.ts";
import { RpcError } from "./server.ts";

export type AccountRunContext = Readonly<{
  runId: string; botId: string; provider: Provider; pinnedAccountId: string; effectiveAccountId: string;
  kind: AccountKind; accountRevision: number; usingFallback: boolean; model: string;
}>;
export type AccountSelection = { ready: true; context: AccountRunContext } | { ready: false; reason: string; accountId: string; limitedUntil?: string | null };
export type PreparedAccount = { env: Record<string, string>; authentication?: ProviderAuthentication; codexSandboxReady: boolean; maxBudgetUsd?: number };
export type AccountRuntimeOptions = {
  db: Db; paths: Paths; credentials?: CredentialStore;
  onChanged?: (accountId: string) => void;
  onSignInEnded?: (accountId: string, attempt: string) => void;
  onCap?: (accountId: string, threshold: 80 | 100) => void;
  // Read-only probes and owned login lifecycle are injectable for synthetic tests.
  probe?: typeof checkAccountSetup; probeLimit?: typeof checkAccountLimits; login?: typeof signInAccount; cancelLogin?: typeof cancelAccountSignIn;
  probeUsage?: (account: Account) => Promise<AccountUsageRead>;
  sandbox?: typeof prepareAccountCodexSandbox; logout?: typeof logoutAccount;
};
function versionAtLeast(version: string | null, expected: [number, number, number]): boolean {
  const parts = version?.split(".").map(Number); if (!parts || parts.length < 3 || parts.slice(0, 3).some((part) => !Number.isSafeInteger(part) || part < 0)) return false;
  for (let i = 0; i < 3; i++) { if (parts[i] !== expected[i]) return parts[i] > expected[i]; } return true;
}
const fail = (message: string): never => { throw new RpcError("bad_state", message); };

export class AccountRuntime {
  #options: AccountRuntimeOptions;
  #credentials: CredentialStore;
  #setups = new Map<string, ProviderSetup>();
  #providerUsage = new Map<string, AccountProviderUsage>();
  #attempts = new Map<string, NonNullable<AccountInfo["signIn"]>>();
  #revisions = new Map<string, number>();
  #holds = new Map<string, number>();
  #runs = new Map<string, AccountRunContext>();
  #checking = new Map<string, Promise<AccountInfo>>();
  #startingSignIns = new Set<Promise<{ attempt: string; url: string | null }>>();
  #closed = false;
  #loginEpochs = new Map<string, number>();
  #quotaVersions = new Map<string, number>();
  #limitRetries = new Map<string, { revision: number; quotaVersion: number; expiresAt: number; runId: string | null; windows: StoredAccountLimit[] }>();
  constructor(options: AccountRuntimeOptions) { this.#options = options; this.#credentials = options.credentials ?? new WindowsAccountCredentials(options.paths.root); }
  #revision(id: string): number { return this.#revisions.get(id) ?? 0; }
  #notify(id: string): void { if (!this.#closed && !this.#holds.has(id)) this.#options.onChanged?.(id); }
  changed(id: string): void { this.#revisions.set(id, this.#revision(id) + 1); this.#providerUsage.delete(id); this.#notify(id); }
  metadataChanged(id: string): void { this.#notify(id); }
  hold(id: string): () => void {
    this.#holds.set(id, (this.#holds.get(id) ?? 0) + 1);
    this.#revisions.set(id, this.#revision(id) + 1);
    let released = false;
    return () => { if (released) return; released = true; const count = this.#holds.get(id)! - 1;
      if (count) this.#holds.set(id, count); else { this.#holds.delete(id); this.#notify(id); } };
  }
  inUse(id: string): boolean { return [...this.#runs.values()].some((run) => run.effectiveAccountId === id); }
  info(id: string): AccountInfo {
    const a = getAccount(this.#options.db, id);
    return { id: a.id, provider: a.provider, label: a.label, kind: a.kind, isDefault: a.isDefault, isFallback: a.isFallback,
      spendingCapUsd: a.spendingCapUsd, createdAt: a.createdAt, hasCredential: a.kind === "api_key" && a.credentialRef !== null,
      setup: this.#setups.get(id) ?? null, limits: listAccountLimits(this.#options.db, id), signIn: this.#attempts.get(id) ?? null,
      spending: accountBudget(this.#options.db, id), providerUsage: this.#providerUsage.get(id) ?? emptyProviderUsage() };
  }
  list(): AccountInfo[] { return listAccounts(this.#options.db).map((account) => this.info(account.id)); }
  check(id: string): Promise<AccountInfo> {
    if (this.#closed) return Promise.reject(new Error("Accounts are closed."));
    const existing = this.#checking.get(id); if (existing) return existing;
    const checking = this.#check(id).finally(() => { if (this.#checking.get(id) === checking) this.#checking.delete(id); });
    this.#checking.set(id, checking); return checking;
  }
  async #check(id: string): Promise<AccountInfo> {
    const account = getAccount(this.#options.db, id), revision = this.#revision(id);
    let setup: ProviderSetup;
    try {
      prepareAccountFolder(account, this.#options.paths.root);
      setup = await (this.#options.probe ?? checkAccountSetup)(account);
      if (account.kind === "api_key") {
        const configured = account.credentialRef !== null && await this.#credentials.read(id) !== null;
        // This checks local installation/configuration only. No paid API request validates the key.
        setup = { ...setup, signedIn: configured && setup.installed, account: null, plan: "API key (configured locally)", error: configured ? setup.installed ? null : setup.error : "Add an API key in Accounts." };
      }
    } catch {
      const base = { installed: resolveCli(account.provider) !== null, version: null, signedIn: false, account: null, plan: null, error: "This account could not be checked. Try again in Accounts." };
      setup = account.provider === "codex" ? { provider: "codex", ...base, sandbox: "unavailable" } : { provider: "claude", ...base };
    }
    if (!this.#closed && revision === this.#revision(id)) {
      this.#setups.set(id, setup);
      if (account.kind === "subscription" && setup.signedIn) {
        const before = this.#quotaMark(id);
        // Preserve the explicitly injected legacy seam for older synthetic fixtures only.
        // Production always uses the full probe, whose permission gates govern recovery.
        const legacyProbe = !this.#options.probeUsage && this.#options.probeLimit;
        try {
          if (legacyProbe) {
            const snapshots = account.provider === "codex" && this.#limit(account).blocked ? await legacyProbe(account) : null;
            if (snapshots?.length && !this.#closed && revision === this.#revision(id) && before === this.#quotaMark(id)) {
              this.#applyUsageLimits(id, { status: "available", usage: emptyProviderUsage(), limits: snapshots, recoveryAllowed: true });
            }
          } else {
            const result = await (this.#options.probeUsage?.(account) ?? checkAccountUsage(account, this.#options.paths.accounts));
            if (!this.#closed && revision === this.#revision(id) && before === this.#quotaMark(id)) this.#acceptUsage(id, result);
          }
        } catch {
          if (!legacyProbe && !this.#closed && revision === this.#revision(id) && before === this.#quotaMark(id)) {
            this.#acceptUsage(id, { status: "error", error: "Provider usage could not be read. Try again later." });
          }
        }
      } else this.#providerUsage.delete(id);
      this.#notify(id);
    }
    return this.info(id);
  }
  #acceptUsage(id: string, result: AccountUsageRead): void {
    const checkedAt = new Date().toISOString(), previous = this.#providerUsage.get(id) ?? emptyProviderUsage();
    if (result.status === "available") {
      this.#applyUsageLimits(id, result);
      this.#providerUsage.set(id, { ...result.usage, status: "available", checkedAt, lastSuccessfulAt: checkedAt, error: null });
    } else this.#providerUsage.set(id, { ...previous, status: result.status, checkedAt, error: result.error });
  }
  #applyUsageLimits(id: string, result: Extract<AccountUsageRead, { status: "available" }>): void {
    const observedAt = this.#nextQuotaStamp(id), previous = listAccountLimits(this.#options.db, id);
    for (const snapshot of result.limits) {
      const windows = [...snapshot.windows];
      // Missing/unknown permission metadata cannot erase a previously reported block,
      // even when the currently displayed utilization is below 100 percent.
      if (!result.recoveryAllowed) for (const old of previous.filter((window) => window.limitId === snapshot.limitId && window.state === "reached")) {
        const index = windows.findIndex((window) => window.windowKey === old.windowKey);
        if (index < 0) windows.push(old); else if (windows[index].state !== "reached") windows[index] = old;
      }
      replaceAccountLimits(this.#options.db, { accountId: id, limitId: snapshot.limitId, observedAt, windows });
    }
    if (result.recoveryAllowed && !result.limits.some((snapshot) => snapshot.limitId === "provider")) {
      replaceAccountLimits(this.#options.db, { accountId: id, limitId: "provider", observedAt, windows: [] });
    }
  }
  async checkAll(): Promise<AccountInfo[]> {
    const pending = listAccounts(this.#options.db);
    await Promise.all(Array.from({ length: Math.min(3, pending.length) }, async () => {
      while (!this.#closed) { const next = pending.shift(); if (!next) return; await this.check(next.id); }
    }));
    return this.list();
  }
  async setApiKey(id: string, key: string): Promise<void> {
    const account = getAccount(this.#options.db, id);
    if (account.kind !== "api_key") fail("Only API-key accounts can store an API key.");
    if (this.inUse(id)) fail("Stop the bots using this account before replacing its key.");
    this.changed(id);
    await this.#credentials.write(id, key);
    updateAccount(this.#options.db, id, { credentialRef: this.#credentials.reference(id) });
    const previous = this.#setups.get(id);
    if (previous) this.#setups.set(id, { ...previous, signedIn: previous.installed, account: null, plan: "API key (configured locally)", error: previous.installed ? null : previous.error });
    this.#notify(id);
  }
  async deleteCredential(id: string): Promise<void> {
    const account = getAccount(this.#options.db, id);
    if (this.inUse(id)) fail("Stop the bots using this account before removing its key.");
    this.changed(id);
    if (account.kind === "api_key") { await this.#credentials.delete(id); updateAccount(this.#options.db, id, { credentialRef: null }); }
    else {
      if (this.#attempts.get(id)?.state === "waiting") fail("Cancel this account's sign-in before removing it.");
      try { await (this.#options.logout ?? logoutAccount)(account); }
      catch { fail("This account could not sign out. It has been kept so you can retry removal."); }
    }
    this.#setups.delete(id); this.#notify(id);
  }
  signIn(id: string): Promise<{ attempt: string; url: string | null }> {
    if (this.#closed) return Promise.reject(new Error("Accounts are closed."));
    const pending = this.#signIn(id).finally(() => this.#startingSignIns.delete(pending));
    this.#startingSignIns.add(pending); return pending;
  }
  async #signIn(id: string): Promise<{ attempt: string; url: string | null }> {
    const account = getAccount(this.#options.db, id);
    if (account.kind !== "subscription") fail("API-key accounts do not use subscription sign-in.");
    if (this.inUse(id)) fail("Stop the bots using this account before signing in.");
    if (this.#attempts.get(id)?.state === "waiting") fail("Sign-in is already waiting for this account.");
    this.changed(id);
    const epoch = (this.#loginEpochs.get(id) ?? 0) + 1;
    this.#loginEpochs.set(id, epoch);
    let ended = false;
    try {
      prepareAccountFolder(account, this.#options.paths.root);
      const result = await (this.#options.login ?? signInAccount)(account, (ok, attempt) => {
        ended = true;
        if (this.#closed || this.#loginEpochs.get(id) !== epoch) return;
        if (ok) this.#attempts.delete(id); else this.#attempts.set(id, { attempt, state: "failed", message: account.provider === "claude" ? "Claude sign-in did not finish. Allow the browser to return to this computer, then retry. Code-paste sign-in is not supported here." : "Sign-in did not finish. You can try again." });
        this.#notify(id); this.#options.onSignInEnded?.(id, attempt);
        if (ok) void this.check(id).catch(() => {});
      });
      if (this.#closed && !ended) { await (this.#options.cancelLogin ?? cancelAccountSignIn)(account, result.attempt); return fail("Accounts closed while sign-in was starting."); }
      if (!ended) this.#attempts.set(id, { attempt: result.attempt, state: "waiting" });
      this.#notify(id); return result;
    } catch { return fail("Sign-in could not start. Check the provider installation and try again."); }
  }
  async cancelSignIn(id: string, attempt: string): Promise<void> {
    const account = getAccount(this.#options.db, id);
    if (this.#attempts.get(id)?.attempt !== attempt || this.#attempts.get(id)?.state !== "waiting") fail("That sign-in attempt is no longer active.");
    this.#loginEpochs.set(id, (this.#loginEpochs.get(id) ?? 0) + 1);
    try { await (this.#options.cancelLogin ?? cancelAccountSignIn)(account, attempt); }
    catch { fail("The sign-in process could not be closed. Try cancelling again."); }
    this.#attempts.delete(id); this.changed(id); this.#options.onSignInEnded?.(id, attempt);
  }
  async prepareSandbox(id: string): Promise<void> {
    const account = getAccount(this.#options.db, id);
    if (account.provider !== "codex" || this.inUse(id)) fail("Stop this Codex account's bots before setting up its sandbox.");
    this.changed(id); prepareAccountFolder(account, this.#options.paths.root);
    try { await (this.#options.sandbox ?? prepareAccountCodexSandbox)(account); }
    catch { fail("Codex sandbox setup did not finish. Try again in Accounts."); }
    await this.check(id);
  }
  #limit(account: StoredAccount): { blocked: boolean; until: string | null } {
    const reached = listAccountLimits(this.#options.db, account.id).filter((w) => w.state === "reached" && (w.resetsAt === null || Date.parse(w.resetsAt) > Date.now()));
    return { blocked: reached.length > 0, until: reached.some((w) => w.resetsAt === null) ? null : reached.map((w) => w.resetsAt!).sort().at(-1) ?? null };
  }
  #quotaMark(id: string): string {
    const rows = this.#options.db.prepare("SELECT limit_id, observed_at FROM account_limit_snapshots WHERE account_id = ? ORDER BY limit_id").all(id);
    return JSON.stringify([this.#quotaVersions.get(id) ?? 0, rows]);
  }
  #nextQuotaStamp(id: string): string {
    const latest = this.#options.db.prepare("SELECT MAX(observed_at) AS stamp FROM account_limit_snapshots WHERE account_id = ?").get(id) as { stamp: string | null };
    return new Date(Math.max(Date.now(), latest.stamp === null ? 0 : Date.parse(latest.stamp) + 1)).toISOString();
  }
  #canRetry(account: StoredAccount, runId?: string): boolean {
    const permit = this.#limitRetries.get(account.id);
    if (!permit || permit.revision !== this.#revision(account.id) || permit.quotaVersion !== (this.#quotaVersions.get(account.id) ?? 0)
      || permit.runId === null && permit.expiresAt <= Date.now() || permit.runId !== null && permit.runId !== runId) return false;
    const reached = listAccountLimits(this.#options.db, account.id).filter((window) => window.state === "reached" && (window.resetsAt === null || Date.parse(window.resetsAt) > Date.now()));
    return reached.length > 0 && reached.every((window) => window.resetsAt === null && permit.windows.some((old) => old.limitId === window.limitId && old.windowKey === window.windowKey && old.observedAt === window.observedAt));
  }
  retryLimit(id: string): AccountInfo {
    if (this.#closed) fail("Accounts are closed.");
    const account = getAccount(this.#options.db, id), setup = this.#setups.get(id);
    if (this.inUse(id)) fail("Stop the bots using this account before retrying its limit.");
    if (!setup?.installed || !setup.signedIn || this.#attempts.get(id)?.state === "waiting") fail("Check or sign in to this account before retrying work.");
    if (account.kind === "api_key" && (!account.credentialRef || accountBudget(this.#options.db, id).blocked)) fail("This API account's key or local spending cap prevents a retry.");
    const reached = listAccountLimits(this.#options.db, id).filter((window) => window.state === "reached" && (window.resetsAt === null || Date.parse(window.resetsAt) > Date.now()));
    if (!reached.length || reached.some((window) => window.resetsAt !== null)) fail("Retry is available only when the provider has not supplied a reset time. Wait for known resets.");
    this.#limitRetries.set(id, { revision: this.#revision(id), quotaVersion: this.#quotaVersions.get(id) ?? 0, expiresAt: Date.now() + 5 * 60_000, runId: null, windows: reached });
    this.#notify(id); return this.info(id);
  }
  completeTurn(context: AccountRunContext, successful: boolean): boolean {
    const id = context.effectiveAccountId, permit = this.#limitRetries.get(id);
    if (!permit || permit.runId !== context.runId) return false;
    this.#limitRetries.delete(id);
    if (successful && !this.#closed && permit.revision === this.#revision(id) && permit.quotaVersion === (this.#quotaVersions.get(id) ?? 0)) {
      const current = listAccountLimits(this.#options.db, id), observedAt = this.#nextQuotaStamp(id);
      for (const limitId of new Set(permit.windows.map((window) => window.limitId))) {
        const windows = current.filter((window) => window.limitId === limitId);
        const remaining = windows.filter((window) => !permit.windows.some((old) => old.limitId === window.limitId && old.windowKey === window.windowKey && old.observedAt === window.observedAt));
        if (remaining.length !== windows.length) replaceAccountLimits(this.#options.db, { accountId: id, limitId, observedAt, windows: remaining });
      }
    }
    // Runner is still completing the old turn; wake other bots after its retirement is installed.
    queueMicrotask(() => this.#notify(id));
    return true;
  }
  #reason(account: StoredAccount, bot: Pick<StoredBot, "id" | "model" | "autonomy">): string | null {
    if (this.#closed || this.#holds.has(account.id)) return "This account is changing. Work will continue when it is ready.";
    if (this.#attempts.get(account.id)?.state === "waiting") return "Finish signing in to this account.";
    const setup = this.#setups.get(account.id);
    if (!setup?.installed || !setup.signedIn) return "Check or sign in to this account in Accounts.";
    if (account.provider === "codex" && bot.autonomy === "folder" && (setup.provider !== "codex" || setup.sandbox !== "ready")) return "Set up this account's Codex sandbox in Accounts.";
    if (account.kind === "api_key") {
      if (account.provider === "claude" && !versionAtLeast(setup.version, [2, 1, 251])) return "Update Claude Code to 2.1.251 or later before using an API key with subprocess scrubbing.";
      if (account.provider === "codex" && !versionAtLeast(setup.version, [0, 160, 0])) return "Update Codex to 0.160.0 or later before using ephemeral API-key authentication.";
      if (!account.credentialRef) return "Add an API key to this account.";
      if (accountBudget(this.#options.db, account.id).blocked) return "This account's local spending cap is reached or its recorded cost is unknown.";
      if (account.provider === "codex" && !hasPricing(account.provider, bot.model)) return "This model has no supported local price. Choose a priced model before using an API key.";
      if ([...this.#runs.values()].some((run) => run.effectiveAccountId === account.id && run.botId !== bot.id)) return "Another bot is using this API account. Its queued work will wait.";
    }
    return null;
  }
  inspect(bot: StoredBot): AccountSelection {
    let pinned: StoredAccount;
    try { pinned = bot.accountId ? getAccount(this.#options.db, bot.accountId) : getDefaultAccount(this.#options.db, bot.provider); }
    catch { return { ready: false, reason: "Choose an account in this bot's settings.", accountId: bot.accountId ?? "" }; }
    if (pinned.provider !== bot.provider) return { ready: false, reason: "Choose an account for this bot's provider.", accountId: pinned.id };
    if (this.#holds.has(pinned.id)) return { ready: false, accountId: pinned.id, reason: "This account is changing. Work will continue when it is ready." };
    let selected = pinned;
    const limit = this.#limit(pinned);
    if (limit.blocked && !this.#canRetry(pinned)) {
      const fallback = pinned.kind === "subscription" ? getFallbackAccount(this.#options.db, bot.provider) : null;
      if (!fallback || this.#reason(fallback, bot) || this.#limit(fallback).blocked && !this.#canRetry(fallback)) return { ready: false, accountId: pinned.id, limitedUntil: limit.until, reason: "This account has reached its provider limit. Wait for reset or choose another account." };
      selected = fallback;
    }
    const reason = this.#reason(selected, bot);
    if (reason) return { ready: false, accountId: selected.id, reason };
    return { ready: true, context: Object.freeze({ runId: randomUUID(), botId: bot.id, provider: selected.provider, pinnedAccountId: pinned.id,
      effectiveAccountId: selected.id, kind: selected.kind, accountRevision: this.#revision(selected.id), usingFallback: selected.id !== pinned.id, model: bot.model }) };
  }
  assertCurrent(context: AccountRunContext): void {
    if (this.#closed) fail("Accounts are closed.");
    const account = getAccount(this.#options.db, context.effectiveAccountId);
    const bot = getStoredBot(this.#options.db, context.botId);
    if (bot.provider !== context.provider || bot.accountId && bot.accountId !== context.pinnedAccountId || bot.model !== context.model) fail("This bot's account or model changed before work could start.");
    if (this.#closed || this.#holds.has(account.id) || this.#revision(account.id) !== context.accountRevision || account.provider !== context.provider || account.kind !== context.kind) fail("This account changed before work could start. Retry when it is ready.");
    if (this.#limit(account).blocked && !this.#canRetry(account, context.runId) || account.kind === "api_key" && accountBudget(this.#options.db, account.id).blocked) fail("This account's limit is reached. Queued work will wait.");
  }
  async prepare(context: AccountRunContext): Promise<PreparedAccount> {
    this.assertCurrent(context);
    const account = getAccount(this.#options.db, context.effectiveAccountId);
    const retrying = this.#limit(account).blocked && this.#canRetry(account, context.runId);
    if ((account.kind === "api_key" || retrying) && [...this.#runs.values()].some((run) => run.effectiveAccountId === account.id && run.runId !== context.runId)) fail("Another turn is using this API account. Queued work will wait.");
    this.#runs.set(context.runId, context);
    if (retrying) this.#limitRetries.get(account.id)!.runId = context.runId;
    try {
      prepareAccountFolder(account, this.#options.paths.root);
      const key = account.kind === "api_key" ? await this.#credentials.read(account.id) : null;
      this.assertCurrent(context);
      if (account.kind === "api_key" && key === null) fail("This API key is unavailable. Add it again in Accounts.");
      const setup = this.#setups.get(account.id);
      const budget = accountBudget(this.#options.db, account.id);
      return { env: accountEnv(account), codexSandboxReady: setup?.provider === "codex" && setup.sandbox === "ready",
        ...(key === null ? {} : { authentication: { kind: "api_key", key } as const }),
        ...(account.kind === "api_key" && account.provider === "claude" && budget.capUsd !== null ? { maxBudgetUsd: budget.capUsd - budget.knownUsd } : {}) };
    } catch (error) { this.release(context); throw error; }
  }
  release(context: AccountRunContext): void {
    if (this.#limitRetries.get(context.effectiveAccountId)?.runId === context.runId) this.#limitRetries.delete(context.effectiveAccountId);
    if (this.#runs.delete(context.runId)) this.#notify(context.effectiveAccountId);
  }
  observeLimit(context: AccountRunContext, event: Extract<ProviderEvent, { type: "limit" }>): void {
    try { getAccount(this.#options.db, context.effectiveAccountId); } catch { return; }
    // Even a generic repeated exhaustion invalidates an older read or retry permit,
    // while preserving the more informative persisted window durations/reset times.
    this.#quotaVersions.set(context.effectiveAccountId, (this.#quotaVersions.get(context.effectiveAccountId) ?? 0) + 1);
    if (!event.windows && event.state === "reached" && this.#limit(getAccount(this.#options.db, context.effectiveAccountId)).blocked) { this.#notify(context.effectiveAccountId); return; }
    const observedAt = this.#nextQuotaStamp(context.effectiveAccountId);
    if (event.windows && event.limitId && event.limitId !== "provider") replaceAccountLimits(this.#options.db, { accountId: context.effectiveAccountId, limitId: "provider", observedAt, windows: [] });
    replaceAccountLimits(this.#options.db, { accountId: context.effectiveAccountId, limitId: event.limitId ?? "provider", observedAt,
      windows: event.windows ?? [{ windowKey: "primary", durationMinutes: null, state: event.state, usedPercent: event.usedPercent, resetsAt: event.resetsAt }] });
    this.#notify(context.effectiveAccountId);
  }
  usageChanged(id: string): void {
    for (const threshold of [80, 100] as const) if (claimCapNotification(this.#options.db, id, threshold)) this.#options.onCap?.(id, threshold);
    this.#notify(id);
  }
  async close(): Promise<void> {
    this.#closed = true;
    this.#limitRetries.clear();
    await Promise.all([...this.#attempts].filter(([, attempt]) => attempt.state === "waiting").map(async ([id, attempt]) => {
      try { await (this.#options.cancelLogin ?? cancelAccountSignIn)(getAccount(this.#options.db, id), attempt.attempt); } catch { /* No credentials in shutdown diagnostics. */ }
    }));
    this.#attempts.clear();
    await Promise.allSettled([...this.#checking.values(), ...this.#startingSignIns]);
  }
}
