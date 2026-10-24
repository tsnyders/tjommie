import type { AccountInfo } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ProviderUsage } from "./provider-usage";

const NOW = new Date("2026-10-22T10:00:00.000Z");
const later = (hours: number): string => new Date(NOW.getTime() + hours * 3_600_000).toISOString();
const setup = (patch: Record<string, unknown> = {}) => ({ provider: "claude" as const, installed: true, version: "2.0.0", signedIn: true, account: "you@example.com", plan: "Max 20x", error: null, ...patch });
const account = (patch: Partial<AccountInfo> = {}): AccountInfo =>
  ({
    id: "a1",
    provider: "claude",
    label: "Claude (Personal)",
    kind: "subscription",
    isDefault: true,
    isFallback: false,
    spendingCapUsd: null,
    createdAt: "2026-10-01T00:00:00.000Z",
    hasCredential: false,
    setup: setup(),
    limits: [
      { accountId: "a1", limitId: "s", windowKey: "primary", durationMinutes: 300, usedPercent: 8, resetsAt: later(2), state: "ok", observedAt: NOW.toISOString() },
      { accountId: "a1", limitId: "w", windowKey: "secondary", durationMinutes: 10_080, usedPercent: 5, resetsAt: later(140), state: "ok", observedAt: NOW.toISOString() },
    ],
    signIn: null,
    spending: { knownUsd: 0, unknownRows: 0, capUsd: null, blocked: false },
    ...patch,
  }) as AccountInfo;
const key = (): AccountInfo => account({ id: "k1", label: "Key", kind: "api_key", hasCredential: true, setup: null, limits: [] });
const html = (accounts: AccountInfo[], extra: { busy?: string | null; problem?: string | null } = {}) => renderToStaticMarkup(<ProviderUsage accounts={accounts} now={NOW} busy={extra.busy ?? null} problem={extra.problem ?? null} onRefresh={() => {}} />);

describe("ProviderUsage", () => {
  it("shows a subscription's provider, plan, each limit window with its use, and who is signed in", () => {
    const out = html([account()]);
    expect(out).toContain("Claude Code");
    expect(out).toContain("Max 20x");
    expect(out).toContain("5-hour limit");
    expect(out).toContain("Weekly limit");
    expect(out).toContain("8%");
    expect(out).toContain("Claude (Personal) · you@example.com");
  });

  it("has a refresh for each account and for all of them, which only ask for a check", () => {
    const out = html([account(), account({ id: "a2", label: "Work", provider: "codex", setup: setup({ provider: "codex", sandbox: "ready", plan: "pro" }) })]);
    expect(out).toContain('aria-label="Refresh all limits"');
    expect(out).toContain('aria-label="Refresh Claude (Personal)"');
    expect(out).toContain('aria-label="Refresh Work"');
    expect(out).toContain(">pro<");
  });

  it("says honestly when there are no limits, and never invents Extra usage or Credits", () => {
    const out = html([account({ limits: [] })]);
    expect(out).toContain("Limits aren&#x27;t available for this account yet.");
    expect(out).not.toContain("Extra usage");
    expect(out).not.toContain("Credits");
    expect(out).not.toContain("meter");
  });

  it("shows checking, signed out and a problem as themselves, and a refresh problem above the cards", () => {
    expect(html([account({ setup: null, limits: [] })])).toContain("Checking");
    expect(html([account({ setup: setup({ signedIn: false }), limits: [] })])).toContain("Not signed in, so there are no limits to show.");
    expect(html([account({ setup: setup({ error: "CLI failed" }), limits: [] })])).toContain("CLI failed");
    expect(html([account()], { problem: "Offline" })).toContain("Offline");
  });

  it("disables the refreshes while one is running, and ignores API keys, which have no subscription quota", () => {
    const busy = html([account()], { busy: "all" });
    expect(busy.match(/disabled=""/g)?.length).toBeGreaterThanOrEqual(2);
    expect(html([key()])).toBe("");
  });
});

describe("ProviderUsage's clock", () => {
  it("shows resets against the time it is given, so a later time reads differently", () => {
    const soon = renderToStaticMarkup(<ProviderUsage accounts={[account()]} now={NOW} busy={null} problem={null} onRefresh={() => {}} />);
    const overnight = renderToStaticMarkup(<ProviderUsage accounts={[account()]} now={new Date(NOW.getTime() + 3 * 86_400_000)} busy={null} problem={null} onRefresh={() => {}} />);
    expect(soon).not.toBe(overnight); // the same windows, three days on: the 5-hour reset is long past, and its note and age change
  });
});

describe("ProviderUsage with the provider's own reading", () => {
  const usage = (patch: Record<string, unknown> = {}) => ({
    status: "available",
    checkedAt: NOW.toISOString(),
    lastSuccessfulAt: NOW.toISOString(),
    error: null,
    planLabel: "Max 20x",
    windows: [
      { id: "session", label: "Session", scope: null, durationMinutes: 300, usedPercent: 8, resetsAt: later(2) },
      { id: "weekly", label: "Weekly", scope: null, durationMinutes: 10_080, usedPercent: 5, resetsAt: later(140) },
      { id: "weekly-fable", label: "Weekly", scope: { kind: "model", id: "fable", label: "Fable" }, durationMinutes: 10_080, usedPercent: 0, resetsAt: later(140) },
    ],
    credits: null,
    extraUsageEnabled: false,
    ...patch,
  });
  const live = (patch: Record<string, unknown> = {}) => account({ providerUsage: usage(patch) } as Partial<AccountInfo>);

  it("shows the provider's windows and plan, a scoped window with its model, and Extra usage when it is known", () => {
    const out = html([live()]);
    expect(out).toContain("Session");
    expect(out).toContain("Weekly · Fable");
    expect(out).toContain("Max 20x");
    expect(out).toContain("Extra usage");
    expect(out).toContain("Disabled");
    expect(out).not.toContain("5-hour limit"); // the provider's names win over the daemon's observed windows
  });

  it("shows Credits as the provider's own number, never as dollars, and Unlimited only when it said so", () => {
    const out = html([live({ credits: { balance: "38917.83", hasCredits: true, unlimited: false } })]);
    expect(out).toContain("Credits");
    expect(out).toContain("38,917.83 left");
    expect(out).not.toContain("$38");
    expect(html([live({ credits: { balance: null, hasCredits: null, unlimited: true } })])).toContain("Unlimited");
    expect(html([live({ credits: { balance: null, hasCredits: null, unlimited: null } })])).toContain("Unknown");
  });

  it("leaves Extra usage and Credits out unless the provider said", () => {
    const out = html([live({ extraUsageEnabled: null, credits: null })]);
    expect(out).not.toContain("Extra usage");
    expect(out).not.toContain("Credits");
    expect(html([live({ extraUsageEnabled: true })])).toContain("Enabled");
  });

  it("keeps the old windows after a failed or unavailable check, marked with when they were last updated, never as fresh or zero", () => {
    const stale = html([live({ status: "error", error: "Provider didn't answer", lastSuccessfulAt: later(-3), checkedAt: NOW.toISOString() })]);
    expect(stale).toContain("Weekly · Fable");
    expect(stale).toContain("8%"); // the last known reading, not cleared
    expect(stale).toContain("Last updated 3 hours ago");
    expect(stale).toContain("Provider didn&#x27;t answer");
    expect(html([live({ status: "unavailable", lastSuccessfulAt: null })])).toContain("Last update time unknown");
    expect(html([live()])).not.toContain("Last updated");
  });

  it("doesn't turn an unknown use into zero: the meter has no value", () => {
    const unknown = html([live({ windows: [{ id: "s", label: "Session", scope: null, durationMinutes: 300, usedPercent: null, resetsAt: null }] })]);
    expect(unknown).toContain("Usage unknown");
    expect(unknown).not.toMatch(/aria-valuenow/);
    expect(html([live()])).toMatch(/aria-valuenow="8"/);
  });

  it("falls back to the windows the daemon observed when there is no live reading, and says honestly when there is neither", () => {
    expect(html([account()])).toContain("5-hour limit"); // an older daemon, no providerUsage
    const none = html([account({ limits: [], providerUsage: usage({ windows: [], status: "unavailable", extraUsageEnabled: null }) } as Partial<AccountInfo>)]);
    expect(none).toContain("Limits aren&#x27;t available for this account yet.");
    expect(html([account({ limits: [], providerUsage: usage({ windows: [], status: "error", error: "No answer" }) } as Partial<AccountInfo>)])).toContain("No answer");
  });

  it("names the signed-in account from setup", () => {
    expect(html([live()])).toContain("you@example.com");
  });
});

describe("ProviderUsage extras without windows, the plan badge and the meter color", () => {
  const reading = (patch: Record<string, unknown>) => ({ status: "available", checkedAt: NOW.toISOString(), lastSuccessfulAt: NOW.toISOString(), error: null, planLabel: null, windows: [], credits: null, extraUsageEnabled: null, ...patch });
  const bare = (patch: Record<string, unknown>) => account({ limits: [], providerUsage: reading(patch) } as Partial<AccountInfo>);

  it("shows Credits for a credits-only reading, with no meter made up", () => {
    const out = html([bare({ credits: { balance: "38917.83", hasCredits: true, unlimited: false } })]);
    expect(out).toContain("Credits");
    expect(out).toContain("38,917.83 left");
    expect(out).toContain("Limits aren&#x27;t available for this account yet.");
    expect(out).not.toContain('role="meter"');
  });

  it("shows Extra usage for an extra-only reading", () => {
    const out = html([bare({ extraUsageEnabled: true })]);
    expect(out).toContain("Extra usage");
    expect(out).toContain("Enabled");
    expect(out).not.toContain('role="meter"');
  });

  it("marks a failed extras-only reading stale, with when it was last updated", () => {
    const out = html([bare({ status: "error", error: "Offline", lastSuccessfulAt: later(-2), credits: { balance: "10", hasCredits: true, unlimited: false } })]);
    expect(out).toContain("10 left");
    expect(out).toContain("Last updated 2 hours ago");
    expect(out).toContain("Offline");
    expect(html([bare({ status: "unavailable", lastSuccessfulAt: null, extraUsageEnabled: false })])).toContain("Last update time unknown");
  });

  it("shows nothing for an empty reading", () => {
    const out = html([bare({})]);
    expect(out).toContain("Limits aren&#x27;t available for this account yet. They show after its first message, or when you refresh.");
    expect(out).not.toContain("Extra usage");
    expect(out).not.toContain("Credits");
  });

  it("cuts a long plan short inside the card, so Refresh stays in view", () => {
    const plan = "A very long plan label ".repeat(5).trim();
    const out = html([account({ providerUsage: reading({ planLabel: plan }) } as Partial<AccountInfo>)]);
    expect(out).toContain(`title="${plan}"`);
    expect(out).toContain("max-w-[45%]");
    expect(out).toContain("truncate");
    expect(out).toContain('aria-label="Refresh Claude (Personal)"');
  });

  it("fills a healthy usage meter in the working color, and the account cards' meters keep the body color", async () => {
    expect(html([account()])).toContain("bg-working");
    const { Meter } = await import("../accounts/limit-meter");
    const plain = renderToStaticMarkup(<Meter label="x" value="y" share={0.3} near={false} />);
    expect(plain).toContain("bg-body");
    expect(plain).not.toContain("bg-working");
  });
});
