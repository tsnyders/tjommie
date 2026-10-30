import { join } from "node:path";
import type { ProviderSetup } from "@tjommie/protocol";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AccountRuntime, type AccountRunContext, type AccountRuntimeOptions } from "./account-runtime.ts";
import { normalizeClaudeUsage, normalizeCodexUsage, type AccountUsageRead } from "./account-quota.ts";
import type { CredentialStore } from "./account-credentials.ts";
import { createAccount, getAccount, replaceAccountLimits, updateAccount } from "./accounts-store.ts";
import { createBot, getStoredBot } from "./bots.ts";
import { openDb } from "./db.ts";
import { dataPaths } from "./paths.ts";
import { ensureAccounts, parseCodexAccountLimits } from "./setup.ts";
import { tempRoot, waitFor } from "./testing.ts";
import { recordUsage } from "./usage.ts";

const openDatabases: ReturnType<typeof openDb>[] = [];

function fixture(extra: Partial<AccountRuntimeOptions> = {}) {
  const paths = dataPaths(tempRoot()), db = openDb(paths.db);
  openDatabases.push(db);
  ensureAccounts(db, paths.accounts);
  const keys = new Map<string, string>();
  const credentials: CredentialStore = { reference: (id) => `fixture/${id}`, read: vi.fn(async (id) => keys.get(id) ?? null),
    write: vi.fn(async (id, key) => { keys.set(id, key); }), delete: vi.fn(async (id) => { keys.delete(id); }) };
  const probe = vi.fn(async (account: { provider: "claude" | "codex" }): Promise<ProviderSetup> => {
    const base = { installed: true, version: "2.1.304", signedIn: true, account: "synthetic@example.invalid", plan: "fixture", error: null };
    return account.provider === "codex" ? { ...base, provider: "codex", sandbox: "ready" } : { ...base, provider: "claude" };
  });
  const onChanged = vi.fn(), onCap = vi.fn();
  const runtime = new AccountRuntime({ db, paths, credentials, probe, probeLimit: async () => null, onChanged, onCap, ...extra });
  const bot = createBot(db, paths.bots, { name: "Test", color: "blue", shape: "circle", instructions: "Synthetic", provider: "codex", model: "gpt-5.3-codex" });
  const api = (id: string, fallback = false) => createAccount(db, { id, provider: "codex", label: id, kind: "api_key", folder: join(paths.accounts, id), spendingCapUsd: 1, isFallback: fallback });
  const context = (): AccountRunContext => { const selected = runtime.inspect(getStoredBot(db, bot.id)); if (!selected.ready) throw new Error(selected.reason); return selected.context; };
  return { paths, db, runtime, keys, credentials, probe, onChanged, onCap, bot, api, context };
}

afterEach(() => { for (const db of openDatabases.splice(0)) if (db.isOpen) db.close(); });

describe("account runtime", () => {
  const codexReading = (ordinaryUsageAllowed: boolean | null = true, spendControlReached: boolean | null = false): AccountUsageRead => {
    const value = { ordinaryUsageAllowed, rateLimits: { limitId: "codex", planType: "pro", spendControlReached,
      primary: { usedPercent: 31, windowDurationMins: 10080, resetsAt: null }, credits: { balance: "38917.83", unlimited: false, hasCredits: true } } };
    return normalizeCodexUsage(value, parseCodexAccountLimits(value));
  };
  it("refreshes healthy signed-in subscriptions, exposes initial unavailable state, and skips API keys", async () => {
    const probeUsage = vi.fn<NonNullable<AccountRuntimeOptions["probeUsage"]>>().mockResolvedValue(codexReading());
    const h = fixture({ probeUsage }); h.api("api"); await h.runtime.setApiKey("api", "synthetic");
    expect(h.runtime.info("codex-main").providerUsage).toMatchObject({ status: "unavailable", checkedAt: null, windows: [] });
    await h.runtime.checkAll();
    expect(probeUsage.mock.calls.map(([account]) => account.id).sort()).toEqual(["claude-main", "codex-main"]);
    expect(h.runtime.info("codex-main").providerUsage).toMatchObject({ status: "available", checkedAt: expect.any(String), lastSuccessfulAt: expect.any(String),
      credits: { balance: "38917.83" }, windows: [{ label: "Weekly", usedPercent: 31 }] });
    expect(h.runtime.info("api").providerUsage).toMatchObject({ status: "unavailable", checkedAt: null });
    h.onChanged.mockClear(); h.runtime.list(); expect(h.onChanged).not.toHaveBeenCalled();
  });
  it("retains successful metadata and limit blocks on refresh failure, with generic errors only", async () => {
    const probeUsage = vi.fn<NonNullable<AccountRuntimeOptions["probeUsage"]>>().mockResolvedValue(codexReading());
    const h = fixture({ probeUsage }); await h.runtime.check("codex-main");
    const previous = h.runtime.info("codex-main").providerUsage;
    h.runtime.observeLimit(h.context(), { type: "limit", state: "reached", resetsAt: null, usedPercent: 100 });
    probeUsage.mockRejectedValueOnce(new Error("private synthetic token and response body"));
    await h.runtime.check("codex-main");
    expect(h.runtime.info("codex-main").providerUsage).toMatchObject({ ...previous, status: "error", checkedAt: expect.any(String), error: "Provider usage could not be read. Try again later." });
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    expect(JSON.stringify(h.runtime.info("codex-main"))).not.toContain("private synthetic");
    h.runtime.metadataChanged("codex-main"); expect(h.runtime.info("codex-main").providerUsage?.lastSuccessfulAt).toBe(previous?.lastSuccessfulAt);
    h.runtime.changed("codex-main"); expect(h.runtime.info("codex-main").providerUsage).toMatchObject({ status: "unavailable", lastSuccessfulAt: null, windows: [] });
  });
  it("blocks explicit Codex permission denial despite healthy percentages and requires affirmative permission recovery", async () => {
    const probeUsage = vi.fn<NonNullable<AccountRuntimeOptions["probeUsage"]>>().mockResolvedValue(codexReading(false));
    const h = fixture({ probeUsage }); await h.runtime.check("codex-main");
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    expect(h.runtime.info("codex-main").limits).toEqual(expect.arrayContaining([expect.objectContaining({ limitId: "provider", windowKey: "permission", state: "reached" })]));
    probeUsage.mockResolvedValueOnce(codexReading(null)).mockResolvedValueOnce(codexReading(true, null)).mockResolvedValueOnce(codexReading());
    await h.runtime.check("codex-main"); expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    await h.runtime.check("codex-main"); expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    await h.runtime.check("codex-main"); expect(h.context().effectiveAccountId).toBe("codex-main");
  });
  it("missing Codex gates cannot clear a specific exhausted window with a healthy replacement percentage", async () => {
    const probeUsage = vi.fn<NonNullable<AccountRuntimeOptions["probeUsage"]>>().mockResolvedValue(codexReading());
    const h = fixture({ probeUsage }); await h.runtime.check("codex-main"); const context = h.context();
    h.runtime.observeLimit(context, { type: "limit", limitId: "codex", state: "reached", usedPercent: 100, resetsAt: null,
      windows: [{ windowKey: "primary", durationMinutes: 10080, usedPercent: 100, resetsAt: null, state: "reached" }] });
    probeUsage.mockResolvedValueOnce(codexReading(null)); await h.runtime.check("codex-main");
    expect(h.runtime.info("codex-main")).toMatchObject({ providerUsage: { windows: [{ usedPercent: 31 }] }, limits: [expect.objectContaining({ state: "reached", usedPercent: 100 })] });
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    await h.runtime.check("codex-main"); expect(h.context().effectiveAccountId).toBe("codex-main");
  });
  it("Claude model exhaustion is display-only for healthy accounts but preserves an existing generic rejection", async () => {
    const value = { five_hour: { utilization: 8 }, seven_day: { utilization: 5 }, limits: [{ kind: "weekly_scoped", percent: 100, scope: { model: { display_name: "Opus" } } }] };
    const probeUsage = vi.fn<NonNullable<AccountRuntimeOptions["probeUsage"]>>().mockResolvedValue(normalizeClaudeUsage(value, "Max"));
    const h = fixture({ probeUsage }); await h.runtime.check("claude-main");
    const bot = createBot(h.db, h.paths.bots, { name: "Claude", color: "blue", shape: "circle", instructions: "Synthetic", provider: "claude", model: "sonnet" });
    const selection = h.runtime.inspect(getStoredBot(h.db, bot.id)); expect(selection.ready).toBe(true); if (!selection.ready) throw new Error("Expected ready");
    h.runtime.observeLimit(selection.context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    await h.runtime.check("claude-main"); expect(h.runtime.inspect(getStoredBot(h.db, bot.id)).ready).toBe(false);
    probeUsage.mockResolvedValueOnce(normalizeClaudeUsage({ ...value, limits: [] }, "Max"));
    await h.runtime.check("claude-main"); expect(h.runtime.inspect(getStoredBot(h.db, bot.id)).ready).toBe(true);
  });
  it("discards display metadata and recovery from a probe overtaken by a streamed quota event or auth change", async () => {
    const probeUsage = vi.fn<NonNullable<AccountRuntimeOptions["probeUsage"]>>().mockResolvedValue(codexReading());
    const h = fixture({ probeUsage }); await h.runtime.check("codex-main"); const previous = h.runtime.info("codex-main").providerUsage, context = h.context();
    for (const authChange of [false, true]) {
      let finish: (value: AccountUsageRead) => void = () => {};
      probeUsage.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
      const checking = h.runtime.check("codex-main"); await vi.waitFor(() => expect(probeUsage).toHaveBeenCalledTimes(authChange ? 3 : 2));
      if (authChange) h.runtime.changed("codex-main");
      else h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
      finish(codexReading()); await checking;
      expect(h.runtime.info("codex-main").providerUsage).toEqual(authChange ? expect.objectContaining({ status: "unavailable", windows: [] }) : previous);
      expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    }
  });
  it("adopts existing subscription folders without credentials and has side-effect-free metadata reads", async () => {
    const h = fixture();
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false });
    await h.runtime.checkAll(); h.onChanged.mockClear();
    const context = h.context(); expect(context.effectiveAccountId).toBe("codex-main");
    expect(Object.isFrozen(context)).toBe(true);
    const prepared = await h.runtime.prepare(context);
    expect(prepared.env.CODEX_HOME).toBe(join(h.paths.accounts, "codex-main"));
    expect(prepared.authentication).toBeUndefined();
    expect(h.credentials.read).not.toHaveBeenCalled();
    h.runtime.list(); h.runtime.info("codex-main"); expect(h.onChanged).not.toHaveBeenCalled();
    expect(h.runtime.inUse("codex-main")).toBe(true); h.runtime.release(context);
    expect(h.runtime.inUse("codex-main")).toBe(false);
  });
  it("stores an API key without probing or starting a request, never returns it in metadata", async () => {
    const h = fixture(); h.api("api"); await h.runtime.setApiKey("api", "synthetic-secret");
    expect(h.probe).not.toHaveBeenCalled();
    expect(h.runtime.info("api")).toMatchObject({ hasCredential: true, setup: null });
    expect(JSON.stringify(h.runtime.list())).not.toContain("synthetic-secret");
    expect(JSON.stringify(h.runtime.list())).not.toContain("fixture/api");
    await expect(h.runtime.setApiKey("codex-main", "synthetic-secret")).rejects.toThrow("Only API-key");
  });
  it("blocks unknown-reset limits for every pinned bot and only uses an opted-in API fallback", async () => {
    const h = fixture(); await h.runtime.checkAll(); const context = h.context();
    h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false, limitedUntil: null });
    h.api("api"); await h.runtime.setApiKey("api", "synthetic-secret"); await h.runtime.check("api");
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    updateAccount(h.db, "api", { isFallback: true });
    expect(h.context()).toMatchObject({ pinnedAccountId: "codex-main", effectiveAccountId: "api", usingFallback: true });
    h.runtime.observeLimit(context, { type: "limit", state: "ok", usedPercent: 1, resetsAt: null });
    expect(h.context()).toMatchObject({ effectiveAccountId: "codex-main", usingFallback: false });
  });
  it("uses all quota windows and does not invent a reset or replace known exhaustion with an unknown turn error", async () => {
    const h = fixture(); await h.runtime.checkAll(); const context = h.context();
    const later = new Date(Date.now() + 60_000).toISOString();
    h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: later, limitId: "codex", windows: [
      { windowKey: "primary", durationMinutes: 300, state: "reached", usedPercent: 100, resetsAt: later },
      { windowKey: "secondary", durationMinutes: 10080, state: "reached", usedPercent: 100, resetsAt: null },
    ] });
    h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    expect(h.runtime.info("codex-main").limits).toHaveLength(2);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false, limitedUntil: null });
  });
  it("requires known Codex model pricing and serializes API account turns", async () => {
    const h = fixture(); h.api("api"); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.checkAll();
    h.db.prepare("UPDATE bots SET account_id = ?, model = ? WHERE id = ?").run("api", "unknown-alias", h.bot.id);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false, reason: expect.stringContaining("price") });
    h.db.prepare("UPDATE bots SET model = ? WHERE id = ?").run("gpt-5.3-codex", h.bot.id);
    const first = h.context(), second = h.context(); await h.runtime.prepare(first);
    await expect(h.runtime.prepare(second)).rejects.toThrow("Another turn");
    await expect(h.runtime.setApiKey("api", "replacement")).rejects.toThrow("Stop");
    h.runtime.release(first); await expect(h.runtime.prepare(second)).resolves.toMatchObject({ authentication: { kind: "api_key", key: "synthetic-key" } });
    h.runtime.release(second);
  });
  it("rejects a prepared account changed while its credential read was pending and releases its lease", async () => {
    const h = fixture(); h.api("api"); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.checkAll();
    h.db.prepare("UPDATE bots SET account_id = ? WHERE id = ?").run("api", h.bot.id);
    let finish: (key: string) => void = () => {};
    vi.mocked(h.credentials.read).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const running = h.runtime.prepare(h.context()); const release = h.runtime.hold("api");
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    finish("synthetic-key"); await expect(running).rejects.toThrow("changed");
    expect(h.runtime.inUse("api")).toBe(false); release();
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(true);
  });
  it("nested holds invalidate stale contexts and notify once after the final release", async () => {
    const h = fixture(); await h.runtime.checkAll(); const old = h.context(); h.onChanged.mockClear();
    const a = h.runtime.hold("codex-main"), b = h.runtime.hold("codex-main");
    h.runtime.changed("codex-main"); a(); a(); expect(h.onChanged).not.toHaveBeenCalled();
    b(); expect(h.onChanged).toHaveBeenCalledExactlyOnceWith("codex-main");
    await expect(h.runtime.prepare(old)).rejects.toThrow("changed");
  });
  it("caps locally recorded API spend and deduplicates threshold notifications", async () => {
    const h = fixture(); h.api("api"); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.checkAll();
    h.db.prepare("UPDATE bots SET account_id = ? WHERE id = ?").run("api", h.bot.id);
    recordUsage(h.db, { id: "one", accountId: "api", botId: h.bot.id, provider: "codex", model: "gpt-5.3-codex", input: 1, cachedInput: 0, output: 0, reasoning: 0, costUsd: 0.8, costSource: "estimated" });
    h.runtime.usageChanged("api"); h.runtime.usageChanged("api"); expect(h.onCap).toHaveBeenCalledExactlyOnceWith("api", 80);
    recordUsage(h.db, { id: "two", accountId: "api", botId: h.bot.id, provider: "codex", model: "unknown", input: 1, cachedInput: 0, output: 0, reasoning: 0, costUsd: null, costSource: "unknown" });
    h.runtime.usageChanged("api"); expect(h.onCap).toHaveBeenLastCalledWith("api", 100);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false, reason: expect.stringContaining("unknown") });
  });
  it("never falls back to the same limited API account", async () => {
    const h = fixture(); h.api("api", true); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.checkAll();
    h.db.prepare("UPDATE bots SET account_id = ? WHERE id = ?").run("api", h.bot.id);
    const context = h.context(); h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false, accountId: "api" });
  });
  it("keeps per-account sandbox readiness separate and stops new probes after close", async () => {
    const h = fixture(); h.api("api"); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.checkAll();
    h.probe.mockResolvedValueOnce({ provider: "codex", installed: true, version: "1", signedIn: true, account: null, plan: null, error: null, sandbox: "notConfigured" });
    await h.runtime.check("api");
    h.db.prepare("UPDATE bots SET autonomy = ? WHERE id = ?").run("folder", h.bot.id);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(true);
    h.db.prepare("UPDATE bots SET account_id = ? WHERE id = ?").run("api", h.bot.id);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false, reason: expect.stringContaining("sandbox") });
    await h.runtime.close(); const calls = h.probe.mock.calls.length;
    await expect(h.runtime.check("api")).rejects.toThrow("closed"); expect(h.probe).toHaveBeenCalledTimes(calls);
  });
  it("correlates sign-in cancellation to the account and attempt, sanitizing failure", async () => {
    let complete: (ok: boolean, attempt: string) => void = () => {};
    const login = vi.fn<NonNullable<AccountRuntimeOptions["login"]>>(async (_account, callback) => { complete = callback; return { attempt: "attempt-1", url: "https://example.invalid/consent" }; });
    const cancelLogin = vi.fn(async () => {}), onSignInEnded = vi.fn();
    const h = fixture({ login, cancelLogin, onSignInEnded });
    await h.runtime.signIn("codex-main");
    expect(h.runtime.info("codex-main").signIn).toEqual({ attempt: "attempt-1", state: "waiting" });
    expect(JSON.stringify(h.runtime.list())).not.toContain("consent");
    await expect(h.runtime.cancelSignIn("codex-main", "stale")).rejects.toThrow("no longer active");
    await h.runtime.cancelSignIn("codex-main", "attempt-1");
    expect(cancelLogin).toHaveBeenCalledWith(expect.objectContaining({ id: "codex-main" }), "attempt-1");
    complete(true, "attempt-1"); expect(h.runtime.info("codex-main").signIn).toBeNull();
    expect(onSignInEnded).toHaveBeenCalledExactlyOnceWith("codex-main", "attempt-1");
  });
  it("explicit subscription removal signs out only the selected account and preserves metadata on failure", async () => {
    const logout = vi.fn<NonNullable<AccountRuntimeOptions["logout"]>>().mockRejectedValueOnce(new Error("private provider payload"));
    const h = fixture({ logout }); await h.runtime.checkAll();
    await expect(h.runtime.deleteCredential("codex-main")).rejects.toThrow("kept");
    expect(getAccount(h.db, "codex-main").deletedAt).toBeNull();
    expect(h.credentials.delete).not.toHaveBeenCalled();
    logout.mockResolvedValueOnce(); await h.runtime.deleteCredential("codex-main");
    expect(logout).toHaveBeenLastCalledWith(expect.objectContaining({ id: "codex-main", folder: join(h.paths.accounts, "codex-main") }));
    expect(h.runtime.info("codex-main").setup).toBeNull();
    expect(getAccount(h.db, "claude-main").deletedAt).toBeNull();
  });
  it("bounds startup checks to three concurrent probes and does not start queued checks after close", async () => {
    let active = 0, maximum = 0;
    const pending: (() => void)[] = [];
    const h = fixture({ probe: async (account) => {
      active++; maximum = Math.max(maximum, active);
      await new Promise<void>((resolve) => pending.push(resolve)); active--;
      const base = { installed: true, signedIn: false, version: "1", account: null, plan: null, error: null };
      return account.provider === "claude" ? { provider: "claude", ...base } : { provider: "codex", ...base, sandbox: "ready" };
    } });
    for (let index = 0; index < 6; index++) h.api(`a${index}`);
    const checking = h.runtime.checkAll(); expect(pending).toHaveLength(3);
    const closing = h.runtime.close(); for (const finish of pending) finish();
    await closing; await checking; expect(maximum).toBe(3); expect(pending).toHaveLength(3);
  });

  it("suppresses late lifecycle notifications and rejects stale preparation after database teardown", async () => {
    const h = fixture(); await h.runtime.checkAll();
    const context = h.context(); await h.runtime.prepare(context);
    const releaseHold = h.runtime.hold("codex-main");
    await h.runtime.close(); h.db.close(); h.onChanged.mockClear();
    h.runtime.release(context); releaseHold(); h.runtime.metadataChanged("codex-main"); h.runtime.changed("codex-main");
    expect(h.onChanged).not.toHaveBeenCalled();
    await expect(h.runtime.prepare(context)).rejects.toThrow("Accounts are closed");
  });

  it("recovers unknown-reset subscription limits only from a successful authoritative free read", async () => {
    const probeLimit = vi.fn<NonNullable<AccountRuntimeOptions["probeLimit"]>>().mockResolvedValue(null);
    const h = fixture({ probeLimit }); await h.runtime.checkAll(); const context = h.context();
    h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    await h.runtime.check("codex-main"); expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    probeLimit.mockRejectedValueOnce(new Error("private metadata failure"));
    await h.runtime.check("codex-main"); expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    probeLimit.mockResolvedValueOnce([
      { limitId: "codex", windows: [{ windowKey: "primary", durationMinutes: 300, usedPercent: 20, resetsAt: null, state: "ok" }] },
      { limitId: "other", windows: [{ windowKey: "secondary", durationMinutes: 10080, usedPercent: 100, resetsAt: null, state: "reached" }] },
    ]);
    await h.runtime.check("codex-main");
    expect(h.runtime.info("codex-main").limits.map((window) => window.limitId)).toEqual(["codex", "other"]);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    probeLimit.mockResolvedValueOnce([{ limitId: "other", windows: [{ windowKey: "secondary", durationMinutes: 10080, usedPercent: 10, resetsAt: null, state: "ok" }] }]);
    await h.runtime.check("codex-main"); expect(h.context().effectiveAccountId).toBe("codex-main");
    h.api("api"); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.check("api");
    expect(probeLimit).toHaveBeenCalledTimes(4);
  });

  it("orders quota events within one millisecond and discards a free read overtaken by a newer limit", async () => {
    let finish: ((value: Awaited<ReturnType<NonNullable<AccountRuntimeOptions["probeLimit"]>>>) => void) | undefined;
    const h = fixture({ probeLimit: async () => new Promise((resolve) => { finish = resolve; }) });
    await h.runtime.checkAll(); const context = h.context();
    const clock = vi.spyOn(Date, "now").mockReturnValue(Date.now());
    try {
      h.runtime.observeLimit(context, { type: "limit", state: "ok", usedPercent: 0, resetsAt: null });
      const first = h.runtime.info("codex-main").limits[0].observedAt;
      h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
      expect(h.runtime.info("codex-main").limits[0].observedAt > first).toBe(true);
      const checking = h.runtime.check("codex-main"); await waitFor(() => finish);
      h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null, limitId: "codex",
        windows: [{ windowKey: "primary", durationMinutes: 300, usedPercent: 100, resetsAt: null, state: "reached" }] });
      finish!([{ limitId: "codex", windows: [{ windowKey: "primary", durationMinutes: 300, usedPercent: 0, resetsAt: null, state: "ok" }] }]);
      await checking;
      expect(h.runtime.info("codex-main").limits).toEqual([expect.objectContaining({ limitId: "codex", state: "reached" })]);
    } finally { clock.mockRestore(); }
  });

  it("grants only one prepared turn across bots and leaves a failed retry blocked", async () => {
    const h = fixture(); await h.runtime.checkAll(); const original = h.context();
    h.runtime.observeLimit(original, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    const b = createBot(h.db, h.paths.bots, { name: "Other", color: "blue", shape: "circle", instructions: "Synthetic", provider: "codex", model: "gpt-5.3-codex" });
    h.runtime.retryLimit("codex-main");
    const one = h.context(), selected = h.runtime.inspect(b);
    expect(selected.ready).toBe(true); if (!selected.ready) throw new Error(selected.reason);
    await h.runtime.prepare(one);
    await expect(h.runtime.prepare(selected.context)).rejects.toThrow("limit");
    h.runtime.completeTurn(one, false); h.runtime.release(one);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    expect(h.runtime.info("codex-main").limits[0].state).toBe("reached");
  });

  it("does not let explicit retries bypass known resets, spending caps, pricing, or permit expiry", async () => {
    const h = fixture(); await h.runtime.checkAll(); const context = h.context();
    h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: new Date(Date.now() + 60_000).toISOString() });
    expect(() => h.runtime.retryLimit("codex-main")).toThrow("known resets");
    h.runtime.observeLimit(context, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null,
      windows: [{ windowKey: "primary", durationMinutes: 300, usedPercent: 100, resetsAt: null, state: "reached" }] });
    h.runtime.retryLimit("codex-main");
    const clock = vi.spyOn(Date, "now").mockReturnValue(Date.now() + 5 * 60_000);
    try { expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false); } finally { clock.mockRestore(); }
    h.api("api"); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.check("api");
    h.db.prepare("UPDATE bots SET account_id = 'api' WHERE id = ?").run(h.bot.id); const api = h.context();
    h.runtime.observeLimit(api, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    h.runtime.retryLimit("api"); h.db.prepare("UPDATE bots SET model = 'unpriced' WHERE id = ?").run(h.bot.id);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id))).toMatchObject({ ready: false, reason: expect.stringContaining("price") });
    recordUsage(h.db, { accountId: "api", botId: h.bot.id, provider: "codex", model: "gpt-5.3-codex", input: 1, cachedInput: 0, output: 0, reasoning: 0, costUsd: 1, costSource: "estimated" });
    expect(() => h.runtime.retryLimit("api")).toThrow("spending cap");
  });

  it.each([false, true])("clears only old unknown windows and preserves newer exhaustion (detailed %s)", async (detailed) => {
    const h = fixture(); await h.runtime.checkAll(); const initial = h.context();
    h.runtime.observeLimit(initial, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    h.runtime.retryLimit("codex-main"); const first = h.context(); await h.runtime.prepare(first);
    h.runtime.completeTurn(first, true); h.runtime.release(first); expect(h.context()).toBeDefined();
    h.runtime.observeLimit(initial, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    h.runtime.retryLimit("codex-main"); const second = h.context(); await h.runtime.prepare(second);
    h.runtime.observeLimit(second, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null,
      ...(detailed ? { windows: [{ windowKey: "primary", durationMinutes: 300, usedPercent: 100, resetsAt: null, state: "reached" as const }] } : {}) });
    h.runtime.completeTurn(second, true); h.runtime.release(second);
    expect(h.runtime.info("codex-main").limits[0].state).toBe("reached");
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
  });

  it("retries the selected pinned subscription or an already opted-in fallback without transferring permits", async () => {
    const h = fixture(); h.api("api", true); await h.runtime.setApiKey("api", "synthetic-key"); await h.runtime.checkAll();
    const pinned = h.context(); h.runtime.observeLimit(pinned, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    const fallback = h.context(); expect(fallback.effectiveAccountId).toBe("api");
    h.runtime.retryLimit("codex-main"); expect(h.context().effectiveAccountId).toBe("codex-main");
    const chosen = h.context(); await h.runtime.prepare(chosen); h.runtime.release(chosen);
    h.runtime.observeLimit(fallback, { type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    h.runtime.retryLimit("api"); const retry = h.context(); expect(retry.effectiveAccountId).toBe("api");
    updateAccount(h.db, "api", { isFallback: false }); expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
    updateAccount(h.db, "api", { isFallback: true }); await h.runtime.prepare(retry); h.runtime.completeTurn(retry, false); h.runtime.release(retry);
    expect(h.runtime.inspect(getStoredBot(h.db, h.bot.id)).ready).toBe(false);
  });
});
