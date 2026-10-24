import { linkSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { AccountProviderUsage } from "@tjommie/protocol";
import { describe, expect, it, vi } from "vitest";
import { emptyProviderUsage, normalizeClaudeUsage, normalizeCodexUsage, readClaudeAccountUsage } from "./account-quota.ts";
import { parseCodexAccountLimits, type Account } from "./setup.ts";
import { tempRoot } from "./testing.ts";

const quota = { five_hour: { utilization: 8, resets_at: "2026-10-23T00:00:00Z" }, seven_day: { utilization: 5, resets_at: null }, extra_usage: { is_enabled: false } };
const primary = { usedPercent: 31, windowDurationMins: 10080, resetsAt: null };
const codex = (overrides: Record<string, unknown> = {}) => ({ ordinaryUsageAllowed: true, rateLimits: { primary, planType: "pro", spendControlReached: false,
  credits: { balance: "38917.83", hasCredits: true, unlimited: false } }, ...overrides });
const normalizeCodex = (value: unknown) => normalizeCodexUsage(value, parseCodexAccountLimits(value));

describe("provider quota normalization", () => {
  it("exposes Codex credits in provider units, plan, and reported-duration windows", () => {
    const result = normalizeCodex(codex()); expect(result.status).toBe("available"); if (result.status !== "available") throw new Error("Expected usage");
    expect(result).toMatchObject({ recoveryAllowed: true, usage: { planLabel: "pro", credits: { balance: "38917.83", hasCredits: true, unlimited: false },
      windows: [{ label: "Weekly", durationMinutes: 10080, usedPercent: 31, resetsAt: null }] } });
    expect(AccountProviderUsage.safeParse({ ...emptyProviderUsage(), ...result.usage }).success).toBe(true);
  });
  it.each([false, null, undefined])("does not infer permission recovery from healthy percentages when ordinaryUsageAllowed=%s", (permission) => {
    const result = normalizeCodex(codex({ ordinaryUsageAllowed: permission }));
    expect(result).toMatchObject({ status: "available", recoveryAllowed: false });
    if (permission === false) expect(result).toMatchObject({ limits: expect.arrayContaining([expect.objectContaining({ limitId: "provider", windows: [expect.objectContaining({ state: "reached" })] })]) });
  });
  it.each([true, null, undefined])("keeps spend-control denial or unavailable data separate from utilization (%s)", (spendControlReached) => {
    const result = normalizeCodex(codex({ rateLimits: { primary, spendControlReached } }));
    expect(result).toMatchObject({ status: "available", recoveryAllowed: false });
    if (spendControlReached === true) expect(result).toMatchObject({ limits: expect.arrayContaining([expect.objectContaining({ limitId: "provider" })]) });
  });
  it("requires every bucket's permission state and does not sum account credits", () => {
    const result = normalizeCodex(codex({ rateLimitsByLimitId: {
      codex: { primary, spendControlReached: false }, model: { primary: { ...primary, windowDurationMins: 300 }, spendControlReached: null, normalModelSlug: "model-x", limitName: "Model X" },
    } }));
    expect(result).toMatchObject({ status: "available", recoveryAllowed: false, usage: { credits: { balance: "38917.83" }, windows: [
      { id: "codex:primary", label: "Weekly" }, { id: "model:primary", label: "Session · Model X", scope: { kind: "model", id: "model-x", label: "Model X" } },
    ] } });
  });
  it("does not discard a legacy spend-control denial when the per-bucket map disagrees", () => {
    const value = codex({ rateLimits: { primary, spendControlReached: true }, rateLimitsByLimitId: { codex: { primary, spendControlReached: false } } });
    expect(normalizeCodex(value)).toMatchObject({ status: "available", recoveryAllowed: false, limits: expect.arrayContaining([expect.objectContaining({ limitId: "provider" })]) });
  });
  it("does not turn missing balances into zero or permit credits-only recovery", () => {
    expect(normalizeCodex(codex({ rateLimits: { credits: { balance: "not-a-number", unlimited: true } } }))).toMatchObject({ status: "available", recoveryAllowed: false,
      usage: { windows: [], credits: { balance: null, hasCredits: null, unlimited: true } } });
    expect(normalizeCodex(codex({ rateLimits: { primary: { ...primary, usedPercent: 101 } } }))).toMatchObject({ status: "error" });
  });
  it("reconciles legacy and modern Claude model windows while preserving surface identity and zero", () => {
    const result = normalizeClaudeUsage({ ...quota, seven_day_opus: { utilization: 4 }, limits: [
      { kind: "weekly_scoped", percent: 0, scope: { model: { id: "opus-1", display_name: "Opus" } } },
      { kind: "weekly_scoped", percent: 20, scope: { surface: { id: "opus-1", display_name: "Opus" } } },
    ] }, "Max 20x");
    expect(result).toMatchObject({ status: "available", recoveryAllowed: true, usage: { planLabel: "Max 20x", extraUsageEnabled: false, windows: [
      { label: "Session", usedPercent: 8 }, { label: "Weekly", usedPercent: 5 }, { scope: { kind: "model", id: "opus-1" }, usedPercent: 0 }, { scope: { kind: "surface", id: "opus-1" }, usedPercent: 20 },
    ] }, limits: [{ limitId: "claude", windows: [{ windowKey: "five_hour", state: "ok" }, { windowKey: "seven_day", state: "ok" }] }] });
  });
  it("keeps exhausted model/surface windows display-only but prevents clearing an existing generic block", () => {
    const result = normalizeClaudeUsage({ ...quota, limits: [{ kind: "weekly_scoped", percent: 100, scope: { surface: { display_name: "Code" } } }] }, null);
    expect(result).toMatchObject({ status: "available", recoveryAllowed: false, limits: [{ windows: [{ state: "ok" }, { state: "ok" }] }] });
    expect(normalizeClaudeUsage({ ...quota, seven_day_opus: { utilization: 100 }, limits: [{ kind: "weekly_scoped", percent: 0, scope: { model: { display_name: "Opus" } } }] }, null)).toMatchObject({ recoveryAllowed: false });
  });
  it.each([{ seven_day_opus: {} }, { limits: [{ kind: "future_shape" }] }, { seven_day: null }])("incomplete Claude responses cannot authorize recovery %#", (extra) => {
    expect(normalizeClaudeUsage({ ...quota, ...extra }, null)).toMatchObject({ status: "available", recoveryAllowed: false });
  });
  it("accepts numeric provider percentages without coercing missing values or booleans to zero", () => {
    expect(normalizeClaudeUsage({ ...quota, five_hour: { utilization: "8.5" } }, null)).toMatchObject({ status: "available", usage: { windows: [{ usedPercent: 8.5 }, { usedPercent: 5 }] } });
    for (const utilization of [null, false, "", " "]) expect(normalizeClaudeUsage({ ...quota, five_hour: { utilization } }, null)).toMatchObject({ recoveryAllowed: false, usage: { windows: [{ id: "seven_day" }] } });
  });
  it("bounds displayed windows and uses distinct IDs without truncation authorizing recovery", () => {
    const limits = Array.from({ length: 129 }, (_, i) => ({ kind: "weekly_scoped", percent: i === 128 ? 100 : 0, scope: { model: { id: String(i), display_name: "Model" } } }));
    const result = normalizeClaudeUsage({ ...quota, limits }, null);
    expect(result.status).toBe("available"); if (result.status !== "available") throw new Error("Expected usage");
    expect(result.usage.windows).toHaveLength(64); expect(result.recoveryAllowed).toBe(false);
    expect(new Set(result.usage.windows.map((window) => window.id)).size).toBe(64);
    expect(AccountProviderUsage.safeParse({ ...emptyProviderUsage(), ...result.usage }).success).toBe(true);
  });
});

function fixture(oauth: Record<string, unknown> = {}) {
  const root = tempRoot(), folder = join(root, "selected"); mkdirSync(folder);
  const account: Account = { id: "selected", provider: "claude", label: "Synthetic", folder };
  const credential = join(folder, ".credentials.json"), token = "synthetic-access-token-never-real";
  writeFileSync(credential, JSON.stringify({ claudeAiOauth: { accessToken: token, refreshToken: "synthetic-refresh-never-read", subscriptionType: "max", rateLimitTier: "default_claude_max_20x", ...oauth } }));
  const transport = vi.fn<typeof fetch>(async () => Response.json(quota));
  return { root, account, credential, token, transport, options: { accountsRoot: root, fetch: transport } };
}
describe("selected Claude account usage read", () => {
  it("uses only the fixed HTTPS GET with selected access token and never writes/returns credentials", async () => {
    const h = fixture(), before = readFileSync(h.credential, "utf8");
    const result = await readClaudeAccountUsage(h.account, h.options);
    expect(result).toMatchObject({ status: "available", usage: { planLabel: "Max 20x", extraUsageEnabled: false } });
    expect(h.transport).toHaveBeenCalledExactlyOnceWith("https://api.anthropic.com/api/oauth/usage", { method: "GET", redirect: "error", signal: expect.any(AbortSignal),
      headers: { Authorization: `Bearer ${h.token}`, Accept: "application/json", "anthropic-beta": "oauth-2025-04-20" } });
    expect(JSON.stringify(result)).not.toContain(h.token); expect(JSON.stringify(result)).not.toContain("refresh");
    expect(readFileSync(h.credential, "utf8")).toBe(before);
  });
  it.each([{ expiresAt: 1 }, { accessToken: "" }, { accessToken: "bad\r\nheader" }, { accessToken: undefined }])("does not fetch with an expired/missing/unsafe credential %#", async (oauth) => {
    const h = fixture(oauth); expect((await readClaudeAccountUsage(h.account, h.options)).status).toBe("unavailable"); expect(h.transport).not.toHaveBeenCalled();
  });
  it("rejects wrong identity, oversized credential files and credential-directory links before transport", async () => {
    const h = fixture();
    expect((await readClaudeAccountUsage({ ...h.account, id: "wrong" }, h.options)).status).toBe("unavailable");
    expect((await readClaudeAccountUsage({ ...h.account, id: "../selected" }, h.options)).status).toBe("unavailable");
    writeFileSync(h.credential, "x".repeat(64 * 1024 + 1));
    expect((await readClaudeAccountUsage(h.account, h.options)).status).toBe("unavailable");
    const other = fixture(), linkedRoot = join(tempRoot(), "linked"); symlinkSync(other.root, linkedRoot, "junction");
    expect((await readClaudeAccountUsage({ ...other.account, folder: join(linkedRoot, "selected") }, { accountsRoot: linkedRoot, fetch: h.transport })).status).toBe("unavailable");
    const root = tempRoot(), linkedFolder = join(root, "selected"); symlinkSync(other.account.folder, linkedFolder, "junction");
    expect((await readClaudeAccountUsage({ ...other.account, folder: linkedFolder }, { accountsRoot: root, fetch: h.transport })).status).toBe("unavailable");
    expect(h.transport).not.toHaveBeenCalled();
  });
  it("rejects a non-file credential target without reading its contents", async () => {
    const root = tempRoot(), folder = join(root, "selected"); mkdirSync(folder); mkdirSync(join(folder, ".credentials.json"));
    const transport = vi.fn<typeof fetch>();
    expect((await readClaudeAccountUsage({ id: "selected", folder, provider: "claude", label: "Synthetic" }, { accountsRoot: root, fetch: transport })).status).toBe("unavailable");
    expect(transport).not.toHaveBeenCalled();
  });
  it("rejects hard-linked credentials so the selected identity cannot alias another account file", async () => {
    const h = fixture(), linked = join(tempRoot(), "alias.json"); linkSync(h.credential, linked);
    expect((await readClaudeAccountUsage(h.account, h.options)).status).toBe("unavailable"); expect(h.transport).not.toHaveBeenCalled();
  });
  it("binds an abort deadline and redacts a transport's abort diagnostics", async () => {
    const h = fixture(), controller = new AbortController(), timeout = vi.spyOn(AbortSignal, "timeout").mockReturnValue(controller.signal);
    let started: () => void = () => {}; const ready = new Promise<void>((resolve) => { started = resolve; });
    h.transport.mockImplementationOnce((_url, options) => new Promise((_resolve, reject) => {
      options?.signal?.addEventListener("abort", () => reject(new Error(`private ${h.token}`)), { once: true }); started();
    }));
    try {
      const reading = readClaudeAccountUsage(h.account, h.options); await ready; expect(timeout).toHaveBeenCalledWith(15_000); controller.abort();
      expect(await reading).toEqual({ status: "error", error: "Provider usage could not be read. Try again later." });
    } finally { controller.abort(); timeout.mockRestore(); }
  });
  it.each([401, 403, 500, 302])("returns only generic errors and closes the body on HTTP %s", async (status) => {
    const h = fixture(), response = new Response(h.token, { status }); h.transport.mockResolvedValueOnce(response);
    const result = await readClaudeAccountUsage(h.account, h.options);
    expect(result.status).not.toBe("available"); expect(JSON.stringify(result)).not.toContain(h.token); expect(response.bodyUsed).toBe(true);
  });
  it("honors a 429 retry delay without another credential-bearing request", async () => {
    const h = fixture(); let now = Date.now();
    h.transport.mockResolvedValueOnce(new Response("private body", { status: 429, headers: { "retry-after": "30" } }));
    const options = { ...h.options, now: () => now };
    await readClaudeAccountUsage(h.account, options); await readClaudeAccountUsage(h.account, options); expect(h.transport).toHaveBeenCalledOnce();
    now += 30_001; expect((await readClaudeAccountUsage(h.account, options)).status).toBe("available"); expect(h.transport).toHaveBeenCalledTimes(2);
  });
  it("rejects redirects, oversized/invalid bodies and echoed credentials without exposing details", async () => {
    const h = fixture(); const redirected = Response.json(quota); Object.defineProperty(redirected, "url", { value: "https://example.invalid/stolen" });
    h.transport.mockResolvedValueOnce(redirected).mockResolvedValueOnce(new Response("x".repeat(128 * 1024 + 1)))
      .mockResolvedValueOnce(new Response("not JSON")).mockResolvedValueOnce(Response.json({ ...quota, limits: [{ kind: "weekly_scoped", percent: 0, scope: { model: { display_name: h.token } } }] }))
      .mockRejectedValueOnce(new Error(`private ${h.token}`));
    for (let i = 0; i < 5; i++) { const result = await readClaudeAccountUsage(h.account, h.options); expect(result.status).toBe("error"); expect(JSON.stringify(result)).not.toContain(h.token); }
  });
});
