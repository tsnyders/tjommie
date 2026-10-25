import type { AccountInfo, AccountLimitWindow, Bot } from "@tjommie/protocol";
import { SETUP_SIGN_IN_PREFIX } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { clock } from "./format";
import { creditsText, providerView, windowName, quotaCard, focusReady, RETRY_CAPTION, canRetryLimit, closeEpoch, limitState, takeKey, rememberUrl, botsOn, accountFor, accountHelp, accountsOf, bannerFor, chosenAccount, keyReady, limitLabel, limitValue, limitedUntil, parseCap, pickerState, spendingMeter, statusLine, topLimit } from "./accounts";

const NOW = new Date("2026-10-22T10:00:00.000Z");
const later = (hours: number): string => new Date(NOW.getTime() + hours * 3_600_000).toISOString();

const window = (patch: Partial<AccountLimitWindow> = {}): AccountLimitWindow => ({ accountId: "a1", limitId: "l", windowKey: "primary", durationMinutes: 300, usedPercent: 42, resetsAt: later(2), state: "ok", observedAt: NOW.toISOString(), ...patch });
const account = (patch: Partial<AccountInfo> = {}): AccountInfo => ({
  id: "a1",
  provider: "claude",
  label: "Claude (Personal)",
  kind: "subscription",
  isDefault: true,
  isFallback: false,
  spendingCapUsd: null,
  createdAt: "2026-10-01T00:00:00.000Z",
  hasCredential: false,
  setup: { provider: "claude", installed: true, version: "2.0.0", signedIn: true, account: "me@example.com", plan: "max", error: null },
  limits: [],
  signIn: null,
  spending: { knownUsd: 0, unknownRows: 0, capUsd: null, blocked: false },
  ...patch,
});
const key = (patch: Partial<AccountInfo> = {}): AccountInfo =>
  account({ id: "k1", label: "Claude key", kind: "api_key", isDefault: false, isFallback: true, spendingCapUsd: 20, hasCredential: true, setup: null, spending: { knownUsd: 5, unknownRows: 0, capUsd: 20, blocked: false }, ...patch });
const bot = (patch: Partial<Bot> = {}): Bot =>
  ({ id: "b1", name: "Scout", provider: "claude", model: "sonnet", accountId: null, effectiveAccountId: null, usingFallback: false, state: "idle", setupReason: null, limitedUntil: null, ...patch }) as Bot;

describe("limit labels and values", () => {
  it("names the 5-hour and weekly windows, other lengths, and a window with no length", () => {
    expect([300, 10_080, 1440, 2880, 120, 90].map((durationMinutes) => limitLabel({ durationMinutes, windowKey: "w" }))).toEqual(["5-hour limit", "Weekly limit", "1-day limit", "2-day limit", "2-hour limit", "1.5-hour limit"]);
    expect(limitLabel({ durationMinutes: null, windowKey: "monthly" })).toBe("monthly");
  });

  it("says the use and the reset, 'Usage unknown' for no reading, and how old an old reading is", () => {
    expect(limitValue(window(), NOW)).toBe(`42% · resets ${clock(later(2))}`);
    expect(limitValue(window({ usedPercent: null, resetsAt: null }), NOW)).toBe("Usage unknown");
    expect(limitValue(window({ observedAt: later(-3) }), NOW)).toContain("updated 3 hours ago");
  });

  it("is limited until the latest reset among reached windows still ahead", () => {
    const limited = account({ limits: [window({ state: "reached", resetsAt: later(1) }), window({ limitId: "w", state: "reached", resetsAt: later(30) }), window({ limitId: "x", state: "ok", resetsAt: later(99) })] });
    expect(limitedUntil(limited, NOW)).toBe(later(30));
    expect(limitedUntil(account({ limits: [window({ state: "reached", resetsAt: later(-1) })] }), NOW)).toBeNull();
    expect(limitedUntil(account({ limits: [window({ state: "near" })] }), NOW)).toBeNull();
  });

  it("finds the window with the highest known use", () => {
    expect(topLimit([account({ limits: [window({ usedPercent: 10 }), window({ limitId: "w", usedPercent: 90 }), window({ limitId: "u", usedPercent: null })] })])?.usedPercent).toBe(90);
    expect(topLimit([account({ limits: [window({ usedPercent: null })] })])).toBeNull();
  });
});

describe("a limit with no reset time", () => {
  const unknown = account({ limits: [window({ state: "reached", resetsAt: null })] });

  it("still blocks, as its own state, and a reset in the past does not", () => {
    expect(limitState(unknown, NOW)).toEqual({ kind: "unknown" });
    expect(limitState(account({ limits: [window({ state: "reached", resetsAt: later(2) })] }), NOW)).toEqual({ kind: "until", at: later(2) });
    expect(limitState(account({ limits: [window({ state: "reached", resetsAt: later(-1) })] }), NOW)).toBeNull();
    expect(limitState(account({ limits: [window({ state: "reached", resetsAt: null }), window({ limitId: "w", state: "reached", resetsAt: later(2) })] }), NOW)).toEqual({ kind: "unknown" });
  });

  it("says the reset is unknown in the card, the picker and the help line", () => {
    expect(statusLine(unknown, NOW)).toMatchObject({ tone: "limited", text: "Limited. Reset time unknown." });
    expect(pickerState(unknown, NOW)).toBe("Limited");
    expect(accountHelp(unknown, NOW, false)).toBe("Claude (Personal) is limited, and its reset time isn't known.");
  });

  it("makes a banner for a limited bot, with no time, and doesn't offer the account as a way out", () => {
    const other = account({ id: "a2", label: "Work", isDefault: false });
    const banner = bannerFor(bot({ state: "limited", limitedUntil: null }), [unknown, other, account({ id: "a3", label: "Same", isDefault: false, limits: [window({ state: "reached", resetsAt: null })] })], NOW);
    expect(banner).toMatchObject({ kind: "limited", until: null, window: "5-hour limit" });
    expect(banner?.kind === "limited" ? banner.alternatives.map((each) => each.id) : []).toEqual(["a2"]);
  });

  it("explains a fallback whose pinned account has no reset time", () => {
    const banner = bannerFor(bot({ state: "working", usingFallback: true, accountId: "a1", effectiveAccountId: "k1" }), [unknown, key()], NOW);
    expect(banner).toMatchObject({ kind: "fallback", until: null });
  });
});

describe("Retry queued work", () => {
  const unknown = (patch: Partial<AccountInfo> = {}) => account({ limits: [window({ state: "reached", resetsAt: null })], ...patch });

  it("is offered for an unknown reset with nothing known and in the future, and not otherwise", () => {
    expect(canRetryLimit(unknown(), NOW)).toBe(true);
    expect(statusLine(unknown(), NOW).actions).toEqual(["retryLimit"]);
    // a known future window also blocks it: wait for that
    const both = account({ limits: [window({ state: "reached", resetsAt: null }), window({ limitId: "w", state: "reached", resetsAt: later(3) })] });
    expect(canRetryLimit(both, NOW)).toBe(false);
    expect(statusLine(both, NOW).actions).toEqual([]);
    // a known reset alone, or no limit at all: nothing to retry
    expect(canRetryLimit(account({ limits: [window({ state: "reached", resetsAt: later(2) })] }), NOW)).toBe(false);
    expect(canRetryLimit(account(), NOW)).toBe(false);
  });

  it("is not offered past a cap", () => {
    expect(canRetryLimit(key({ limits: [window({ state: "reached", resetsAt: null })], spending: { knownUsd: 20, unknownRows: 0, capUsd: 20, blocked: true } }), NOW)).toBe(false);
    expect(statusLine(key({ limits: [window({ state: "reached", resetsAt: null })] }), NOW).actions).toEqual(["retryLimit"]);
  });

  it("keeps Check again apart from it, and only for a Codex subscription", () => {
    const codex = unknown({ provider: "codex", setup: { provider: "codex", installed: true, version: null, signedIn: true, account: null, plan: null, error: null, sandbox: "ready" } });
    expect(statusLine(codex, NOW).actions).toEqual(["retryLimit", "check"]);
    expect(statusLine(unknown(), NOW).actions).not.toContain("check"); // Claude
    expect(statusLine(key({ provider: "codex", limits: [window({ state: "reached", resetsAt: null })] }), NOW).actions).toEqual(["retryLimit"]);
  });

  it("says what a retry may cost", () => {
    expect(RETRY_CAPTION).toBe("Retries one queued turn; it may use subscription allowance or API credit.");
  });
});

describe("focusReady", () => {
  it("waits for the control a link names: not while the card is still checking, and once it appears", () => {
    const checking = statusLine(account({ setup: null }), NOW);
    expect(focusReady("retry", account(), checking.actions)).toBe(false);
    expect(focusReady("signin", account(), checking.actions)).toBe(false);
    // the check came back: the account is limited with no reset known, or signed out
    const limited = statusLine(account({ limits: [window({ state: "reached", resetsAt: null })] }), NOW);
    expect(focusReady("retry", account(), limited.actions)).toBe(true);
    const signedOut = statusLine(account({ setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } }), NOW);
    expect(focusReady("signin", account(), signedOut.actions)).toBe(true);
    expect(focusReady("signin", account(), statusLine(account({ signIn: { attempt: "t", state: "failed" } }), NOW).actions)).toBe(true); // Try again
  });

  it("is ready for a cap on an API key at once, and never for the wrong control", () => {
    expect(focusReady("cap", key(), [])).toBe(true);
    expect(focusReady("cap", account(), [])).toBe(false);
    expect(focusReady("retry", account(), ["signIn"])).toBe(false);
    expect(focusReady("signin", account(), ["retryLimit"])).toBe(false);
  });
});

describe("closeEpoch", () => {
  it("closes only the dialog that asked, so a late save can't close the next one", () => {
    const first = { epoch: 1, kind: "cap" };
    const second = { epoch: 2, kind: "rename" };
    expect(closeEpoch(first, 1)).toBeNull();
    expect(closeEpoch(second, 1)).toBe(second); // the first finished late, after the second opened
    expect(closeEpoch(null, 1)).toBeNull();
  });
});

describe("statusLine", () => {
  it("takes the first rule that fits, in order", () => {
    const waiting = account({ signIn: { attempt: "t1", state: "waiting" }, setup: null });
    expect(statusLine(waiting, NOW, { canReopen: true })).toMatchObject({ tone: "busy", actions: ["openAgain", "cancel"] });
    expect(statusLine(waiting, NOW).actions).toEqual(["cancel"]);
    expect(statusLine(account({ signIn: { attempt: "t1", state: "failed", message: "Denied." } }), NOW)).toMatchObject({ text: "Denied.", actions: ["tryAgain"] });
    expect(statusLine(account({ signIn: { attempt: "t1", state: "failed" } }), NOW).text).toBe("Sign-in didn't finish.");
    expect(statusLine(account({ setup: null }), NOW)).toMatchObject({ text: "Checking", actions: [] });
    expect(statusLine(account({ setup: null }), NOW, { stuck: true })).toMatchObject({ actions: ["check"] });
    expect(statusLine(account({ setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: "CLI failed" } }), NOW)).toMatchObject({ text: "CLI failed", actions: ["check"] });
    expect(statusLine(account({ setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } }), NOW)).toMatchObject({ text: "Not signed in. Bots on it wait.", actions: ["signIn"] });
    expect(statusLine(account(), NOW)).toMatchObject({ text: "Signed in", tone: "ok" });
  });

  it("says limited until a time, ahead of plain signed in", () => {
    const limited = account({ limits: [window({ state: "reached", resetsAt: later(2) })] });
    expect(statusLine(limited, NOW)).toMatchObject({ tone: "limited", text: `Limited until ${clock(later(2))}` });
  });

  it("reads an API key's state: no key, over its cap, stopped for unpriced turns, or fine", () => {
    expect(statusLine(key({ hasCredential: false }), NOW)).toMatchObject({ text: "No key saved", actions: ["addKey"] });
    expect(statusLine(key({ spending: { knownUsd: 20, unknownRows: 0, capUsd: 20, blocked: true } }), NOW)).toMatchObject({ text: "Reached its $20.00 cap. Bots stopped using it.", actions: ["raiseCap"] });
    const unpriced = statusLine(key({ spending: { knownUsd: 3, unknownRows: 4, capUsd: 20, blocked: true } }), NOW);
    expect(unpriced.text).toBe("Stopped: 4 turns have no price, so the cap can't be checked.");
    expect(unpriced.actions).toEqual([]);
    expect(statusLine(key(), NOW).tone).toBe("ok");
  });
});

describe("spending and the picker", () => {
  it("shows spend against the cap, near it from 80%, and what has no price", () => {
    expect(spendingMeter({ knownUsd: 5, unknownRows: 0, capUsd: 20, blocked: false })).toEqual({ share: 0.25, near: false, text: "Spent $5.00 of $20.00 cap in total" });
    expect(spendingMeter({ knownUsd: 16, unknownRows: 2, capUsd: 20, blocked: false })).toMatchObject({ near: true, text: "Spent $16.00 of $20.00 cap in total + 2 turns with no price" });
    expect(spendingMeter({ knownUsd: 99, unknownRows: 0, capUsd: 20, blocked: true }).share).toBe(1);
  });

  it("orders a provider's accounts with the default first, and finds the default", () => {
    const list = [key(), account({ id: "a2", label: "Second", isDefault: false, createdAt: "2026-10-02T00:00:00.000Z" }), account(), account({ id: "c1", provider: "codex", label: "Codex", isDefault: true })];
    expect(accountsOf(list, "claude").map((each) => each.id)).toEqual(["a1", "k1", "a2"]);
    expect(chosenAccount(list, "codex", null)?.id).toBe("c1");
    expect(chosenAccount(list, "claude", "a2")?.id).toBe("a2");
    expect(chosenAccount(list, "claude", "c1")?.id).toBe("a1"); // another provider's account isn't this tab's
  });

  it("names an account's state on its picker row", () => {
    expect(pickerState(account(), NOW)).toBe("Default");
    expect(pickerState(account({ isDefault: false }), NOW)).toBe("");
    expect(pickerState(key(), NOW)).toBe("API key");
    expect(pickerState(account({ setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } }), NOW)).toBe("Not signed in");
    expect(pickerState(account({ limits: [window({ state: "reached", resetsAt: later(1) })] }), NOW)).toBe(`Limited until ${clock(later(1))}`);
  });

  it("warns about the account in use, and about a switch", () => {
    expect(accountHelp(account(), NOW, false)).toBeNull();
    expect(accountHelp(account(), NOW, true)).toBe("Switching accounts starts a fresh session. The chat stays.");
    expect(accountHelp(account({ label: "Work", setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } }), NOW, true)).toBe("Work isn't signed in.");
    expect(accountHelp(key({ label: "Key", spending: { knownUsd: 20, unknownRows: 0, capUsd: 20, blocked: true } }), NOW, false)).toBe("Key reached its cap.");
  });

  it("finds the account a bot runs on: in use, pinned, or the default", () => {
    const list = [account(), key()];
    expect(accountFor(bot({ effectiveAccountId: "k1", accountId: "a1" }), list)?.id).toBe("k1");
    expect(accountFor(bot({ accountId: "k1" }), list)?.id).toBe("k1");
    expect(accountFor(bot(), list)?.id).toBe("a1");
  });
});

describe("bannerFor", () => {
  const signedOut = account({ setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } });

  it("asks to sign in the account a setup bot is on, and keeps the sandbox banner", () => {
    expect(bannerFor(bot({ state: "needs_setup", setupReason: `${SETUP_SIGN_IN_PREFIX} Claude Code` }), [signedOut], NOW)).toMatchObject({ kind: "signIn", account: { id: "a1" } });
    expect(bannerFor(bot({ state: "needs_setup", setupReason: "Set up Codex's sandbox", provider: "codex" }), [], NOW)).toMatchObject({ kind: "setup", sandbox: true });
  });

  it("says a capped account's cap first, with no switch offered", () => {
    const capped = key({ spending: { knownUsd: 20, unknownRows: 0, capUsd: 20, blocked: true } });
    expect(bannerFor(bot({ accountId: "k1", state: "limited" }), [account(), capped], NOW)).toMatchObject({ kind: "cap", unknownPrices: 0 });
    const unpriced = key({ spending: { knownUsd: 3, unknownRows: 2, capUsd: 20, blocked: true } });
    expect(bannerFor(bot({ accountId: "k1" }), [unpriced], NOW)).toMatchObject({ kind: "cap", unknownPrices: 2 });
  });

  it("offers only the accounts that could take over for a limited bot, and never switches by itself", () => {
    const limited = account({ limits: [window({ state: "reached", resetsAt: later(2) })] });
    const ok = account({ id: "a2", label: "Work", isDefault: false });
    const out = account({ id: "a3", label: "Out", isDefault: false, setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } });
    const alsoLimited = account({ id: "a4", isDefault: false, limits: [window({ state: "reached", resetsAt: later(3) })] });
    const other = account({ id: "c1", provider: "codex", isDefault: true });
    const banner = bannerFor(bot({ state: "limited", limitedUntil: later(2) }), [limited, ok, out, alsoLimited, other, key({ hasCredential: false, id: "k2" })], NOW);
    expect(banner).toMatchObject({ kind: "limited", window: "5-hour limit", until: later(2) });
    expect(banner?.kind === "limited" ? banner.alternatives.map((each) => each.id) : []).toEqual(["a2"]);
  });

  it("explains a bot that carries on with its fallback key, with nothing to press", () => {
    const limited = account({ limits: [window({ state: "reached", resetsAt: later(2) })] });
    const banner = bannerFor(bot({ state: "working", usingFallback: true, accountId: "a1", effectiveAccountId: "k1" }), [limited, key()], NOW);
    expect(banner).toMatchObject({ kind: "fallback", pinned: { id: "a1" }, using: { id: "k1" }, until: later(2) });
  });

  it("has nothing to say for a healthy bot, or before the accounts are known", () => {
    expect(bannerFor(bot(), [account()], NOW)).toBeNull();
    expect(bannerFor(bot({ state: "limited" }), null, NOW)).toBeNull();
  });
});

describe("botsOn", () => {
  it("lists the bots pinned to an account, which keep it from being removed", () => {
    const bots = [bot({ id: "b1", accountId: "a1" }), bot({ id: "b2", accountId: null }), bot({ id: "b3", accountId: "k1" })];
    expect(botsOn({ id: "a1" }, bots).map((each) => each.id)).toEqual(["b1"]);
    expect(botsOn({ id: "zz" }, bots)).toEqual([]);
  });
});

describe("rememberUrl", () => {
  it("only keeps the address, by account, and drops it when a new sign-in gave none", () => {
    expect(rememberUrl({}, "a1", "https://x.test/")).toEqual({ a1: "https://x.test/" });
    expect(rememberUrl({ a1: "https://old.test/", a2: "https://y.test/" }, "a1", null)).toEqual({ a2: "https://y.test/" });
    expect(rememberUrl({ a1: "https://old.test/" }, "a1", "https://new.test/")).toEqual({ a1: "https://new.test/" });
  });
});

describe("takeKey", () => {
  it("clears the field before it hands the key over, so a failing or pending request never leaves it", () => {
    const steps: string[] = [];
    const key = takeKey("  sk-example  ", () => steps.push("cleared"));
    steps.push(`sent ${key}`);
    expect(steps).toEqual(["cleared", "sent sk-example"]);
  });
});

describe("what a person types", () => {
  it("takes a cap in dollars, positive, with at most two decimals", () => {
    expect([parseCap("20"), parseCap(" 12.5 "), parseCap("0.01")]).toEqual([20, 12.5, 0.01]);
    expect([parseCap(""), parseCap("0"), parseCap("-3"), parseCap("1.234"), parseCap("abc"), parseCap("1000000.01")]).toEqual([null, null, null, null, null, null]);
  });

  it("takes a key that is something typed and fits the credential store", () => {
    expect(keyReady("")).toBe(false);
    expect(keyReady("   ")).toBe(false);
    expect(keyReady(" sk-example ")).toBe(true);
    expect(keyReady("k".repeat(2560))).toBe(true);
    expect(keyReady("k".repeat(2561))).toBe(false);
    expect(keyReady("é".repeat(1281))).toBe(false); // counted in bytes, not characters
    expect(keyReady("sk-one two")).toBe(false); // no whitespace inside
    expect(keyReady("sk-one\ntwo")).toBe(false);
  });
});

describe("quotaCard", () => {
  const signedOutSetup = { provider: "claude" as const, installed: true, version: null, signedIn: false, account: null, plan: null, error: null };

  it("shows limits when there are windows, and says so honestly when there are none", () => {
    expect(quotaCard(account({ limits: [window()] }))).toEqual({ kind: "limits" });
    expect(quotaCard(account({ limits: [] }))).toEqual({ kind: "unavailable" });
  });

  it("is checking before the first answer, or a sign-in is waiting, and shows a problem or a signed-out account as itself", () => {
    expect(quotaCard(account({ setup: null }))).toEqual({ kind: "checking" });
    expect(quotaCard(account({ signIn: { attempt: "t", state: "waiting" } }))).toEqual({ kind: "checking" });
    expect(quotaCard(account({ setup: { ...signedOutSetup, error: "CLI failed" } }))).toEqual({ kind: "problem", text: "CLI failed" });
    expect(quotaCard(account({ setup: signedOutSetup }))).toEqual({ kind: "signedOut" });
  });
});

describe("provider reading", () => {
  const live = (patch: Record<string, unknown> = {}) =>
    account({ providerUsage: { status: "available", checkedAt: NOW.toISOString(), lastSuccessfulAt: NOW.toISOString(), error: null, planLabel: null, windows: [], credits: null, extraUsageEnabled: null, ...patch } } as unknown as Partial<AccountInfo>);

  it("names a window with its model or surface", () => {
    expect(windowName({ label: "Weekly", scope: { label: "Fable" } })).toBe("Weekly · Fable");
    expect(windowName({ label: "Session", scope: null })).toBe("Session");
  });

  it("writes credits as the provider's units", () => {
    expect(creditsText({ balance: "38917.83", unlimited: null })).toBe("38,917.83 left");
    expect(creditsText({ balance: "0", unlimited: false })).toBe("0 left");
    expect(creditsText({ balance: "12", unlimited: true })).toBe("Unlimited");
    expect(creditsText({ balance: null, unlimited: null })).toBe("Unknown");
  });

  it("takes the plan from the reading before setup, and the rows from the reading before the observed windows", () => {
    const view = providerView(live({ planLabel: "Max 20x", windows: [{ id: "w", label: "Weekly", scope: null, durationMinutes: 10_080, usedPercent: 85, resetsAt: later(30) }] }), NOW);
    expect(view.plan).toBe("Max 20x");
    expect(view.rows).toEqual([{ key: "w", label: "Weekly", share: 0.85, value: expect.stringContaining("85%"), near: true }]);
    expect(providerView(account(), NOW).plan).toBe("max"); // no reading: setup's plan
  });

  it("marks windows kept from an earlier success as stale, and leaves out a reset that has passed", () => {
    const view = providerView(live({ status: "error", error: "Offline", lastSuccessfulAt: later(-5), windows: [{ id: "w", label: "Weekly", scope: null, durationMinutes: null, usedPercent: 40, resetsAt: later(-1) }] }), NOW);
    expect(view.stale).toBe("Last updated 5 hours ago");
    expect(view.problem).toBe("Offline");
    expect(view.rows[0].value).toBe("40%"); // not "resets" a time that is already behind
  });

  it("never shows an unknown use as 0%", () => {
    const view = providerView(live({ windows: [{ id: "w", label: "Weekly", scope: null, durationMinutes: null, usedPercent: null, resetsAt: null }] }), NOW);
    expect(view.rows[0]).toMatchObject({ share: null, value: "Usage unknown", near: false });
  });

  it("is a card, not 'unavailable', when only Extra usage or Credits is known", () => {
    expect(quotaCard(live({ credits: { balance: "5", hasCredits: true, unlimited: false } }))).toEqual({ kind: "limits" });
    expect(quotaCard(live({ extraUsageEnabled: false }))).toEqual({ kind: "limits" });
    expect(quotaCard(account({ limits: [], providerUsage: { status: "available", checkedAt: null, lastSuccessfulAt: null, error: null, planLabel: null, windows: [], credits: null, extraUsageEnabled: null } } as Partial<AccountInfo>))).toEqual({ kind: "unavailable" });
  });

  it("says an extras-only reading is stale after a failed check", () => {
    const view = providerView(live({ status: "error", error: "Offline", lastSuccessfulAt: later(-4), credits: { balance: "10", hasCredits: true, unlimited: false } }), NOW);
    expect(view.rows).toEqual([]);
    expect(view.extras).toEqual([{ label: "Credits", value: "10 left" }]);
    expect(view.stale).toBe("Last updated 4 hours ago");
    expect(providerView(live({ credits: { balance: "10", hasCredits: true, unlimited: false } }), NOW).stale).toBeNull();
  });

  it("is a problem card when the check failed and there is nothing earlier to show", () => {
    expect(quotaCard(account({ limits: [], providerUsage: { status: "error", checkedAt: null, lastSuccessfulAt: null, error: "No answer", planLabel: null, windows: [], credits: null, extraUsageEnabled: null } } as Partial<AccountInfo>))).toEqual({ kind: "problem", text: "No answer" });
  });
});
