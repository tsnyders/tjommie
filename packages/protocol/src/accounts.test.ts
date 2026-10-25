import { describe, expect, it } from "vitest";
import { AccountInfo, AccountInput, AccountPatch, AccountProviderUsage, UsageQuery, accountMethods } from "./accounts.ts";

describe("account and usage contracts", () => {
  it("requires a cap and secret only when creating an API-key account", () => {
    expect(AccountInput.parse({ provider: "claude", label: " Work ", kind: "subscription" }).label).toBe("Work");
    expect(AccountInput.safeParse({ provider: "claude", label: "Work", kind: "api_key", apiKey: "synthetic" }).success).toBe(false);
    expect(AccountInput.safeParse({ provider: "claude", label: "Work", kind: "subscription", apiKey: "synthetic" }).success).toBe(false);
    for (const spendingCapUsd of [0, -1, Number.POSITIVE_INFINITY, Number.NaN]) {
      expect(AccountInput.safeParse({ provider: "codex", label: "API", kind: "api_key", apiKey: "synthetic", spendingCapUsd }).success).toBe(false);
    }
  });

  it("does not let account updates change the provider, kind or credential reference", () => {
    for (const patch of [{ provider: "claude" }, { kind: "api_key" }, { credentialRef: "private" }, { folder: "C:/private" }]) {
      expect(AccountPatch.safeParse(patch).success).toBe(false);
    }
  });

  it("bounds API keys by native UTF-8 bytes as well as rejecting whitespace", () => {
    for (const apiKey of ["a".repeat(2561), "é".repeat(1281), "synthetic\nkey", "synthetic key"]) {
      expect(AccountInput.safeParse({ provider: "codex", label: "API", kind: "api_key", apiKey, spendingCapUsd: 1 }).success).toBe(false);
      expect(AccountPatch.safeParse({ apiKey }).success).toBe(false);
    }
    expect(AccountPatch.safeParse({ apiKey: "é".repeat(1280) }).success).toBe(true);
  });

  it("returns safe metadata without a submitted secret or private folder", () => {
    const value = AccountInfo.parse({ id: "claude-main", provider: "claude", label: "Claude", kind: "subscription", isDefault: true,
      isFallback: false, spendingCapUsd: null, createdAt: "2026-10-22T00:00:00.000Z", hasCredential: false, setup: null,
      limits: [], signIn: null, spending: { knownUsd: 0, unknownRows: 0, capUsd: null, blocked: false }, apiKey: "synthetic", credentialRef: "private", folder: "private" });
    expect(value).not.toHaveProperty("apiKey");
    expect(value).not.toHaveProperty("credentialRef");
    expect(value).not.toHaveProperty("folder");
  });

  it("bounds queries by instants and validates the chart time zone", () => {
    const range = { from: "2026-10-01T00:00:00.000Z", to: "2026-11-01T00:00:00.000Z" };
    expect(UsageQuery.parse(range).timeZone).toBe("UTC");
    expect(UsageQuery.parse({ ...range, timeZone: "Africa/Johannesburg" }).timeZone).toBe("Africa/Johannesburg");
    for (const bad of [{ ...range, timeZone: "invented/zone" }, { ...range, to: range.from }, { ...range, to: "2028-01-01T00:00:00.000Z" }, { ...range, from: "yesterday" }]) {
      expect(UsageQuery.safeParse(bad).success).toBe(false);
    }
  });

  it("correlates cancellation to an account and a specific attempt", () => {
    expect(accountMethods["accounts.cancelSignIn"].params.safeParse({ id: "claude-main" }).success).toBe(false);
    expect(accountMethods["accounts.cancelSignIn"].params.parse({ id: "claude-main", attempt: "current" })).toEqual({ id: "claude-main", attempt: "current" });
  });

  it("requires an explicit account for a queued-work limit retry", () => {
    expect(accountMethods["accounts.retryLimit"].params.safeParse({}).success).toBe(false);
    expect(accountMethods["accounts.retryLimit"].params.parse({ id: "claude-main" })).toEqual({ id: "claude-main" });
  });

  it("keeps provider allowances separate from costs, preserving unknown readings and credit units", () => {
    const reading = {
      status: "available", checkedAt: "2026-10-22T12:00:00.000Z", lastSuccessfulAt: "2026-10-22T12:00:00.000Z", error: null,
      planLabel: "Pro", windows: [{ id: "weekly", label: "Weekly", scope: null, durationMinutes: 10080, usedPercent: null, resetsAt: null }],
      credits: { balance: "38917.83", hasCredits: true, unlimited: false }, extraUsageEnabled: null,
    };
    expect(AccountProviderUsage.parse(reading)).toMatchObject({ windows: [{ usedPercent: null, resetsAt: null }], credits: { balance: "38917.83" } });
    expect(AccountProviderUsage.safeParse({ ...reading, credits: { ...reading.credits, balance: "$38917.83" } }).success).toBe(false);
    expect(AccountProviderUsage.safeParse({ ...reading, windows: [{ ...reading.windows[0], usedPercent: 101 }] }).success).toBe(false);
    expect(AccountProviderUsage.safeParse({ ...reading, windows: Array.from({ length: 65 }, () => reading.windows[0]) }).success).toBe(false);
    const parsed = AccountProviderUsage.parse({ ...reading, accessToken: "synthetic-secret", credentialPath: "private" });
    expect(parsed).not.toHaveProperty("accessToken");
    expect(parsed).not.toHaveProperty("credentialPath");
  });
});
