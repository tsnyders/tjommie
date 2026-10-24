import type { AccountInfo, Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { AccountCard } from "./account-card";
import { CapBody, KeyBody, RenameBody, ReplaceBody, SubscriptionBody } from "./account-dialogs";
import { LimitMeter, Meter } from "./limit-meter";

const requested = vi.hoisted(() => vi.fn());
vi.mock("@/lib/connection", () => ({ request: requested, daemon: () => ({ subscribe: () => () => {} }) }));

const NOW = new Date("2026-10-22T10:00:00.000Z");
const later = (hours: number): string => new Date(NOW.getTime() + hours * 3_600_000).toISOString();
const sub = (patch: Partial<AccountInfo> = {}): AccountInfo => ({
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
  limits: [{ accountId: "a1", limitId: "l", windowKey: "primary", durationMinutes: 300, usedPercent: 42, resetsAt: later(2), state: "ok", observedAt: NOW.toISOString() }],
  signIn: null,
  spending: { knownUsd: 0, unknownRows: 0, capUsd: null, blocked: false },
  ...patch,
});
const key = (patch: Partial<AccountInfo> = {}): AccountInfo =>
  sub({ id: "k1", label: "Claude key", kind: "api_key", isDefault: false, isFallback: true, spendingCapUsd: 20, hasCredential: true, setup: null, limits: [], spending: { knownUsd: 5, unknownRows: 0, capUsd: 20, blocked: false }, ...patch });
const bot = { id: "b1", name: "Scout", color: "blue", shape: "circle", accountId: "a1" } as Bot;

const card = (account: AccountInfo, extra: { reopenUrl?: string | null; focus?: "signin" | "cap" | "retry" | null; siblings?: AccountInfo[] } = {}) =>
  renderToStaticMarkup(<AccountCard account={account} siblings={extra.siblings ?? [account]} bots={[bot]} now={NOW} reopenUrl={extra.reopenUrl ?? null} focus={extra.focus ?? null} onDialog={() => {}} onSignedIn={() => {}} />);

describe("AccountCard", () => {
  it("shows a signed-in subscription: its plan, one meter per limit, and who uses it", () => {
    const out = card(sub());
    expect(out).toContain("Claude (Personal)");
    expect(out).toContain("Default");
    expect(out).toContain("Subscription · Max plan · me@example.com");
    expect(out).toContain("Signed in");
    expect(out).toContain("5-hour limit");
    expect(out).toContain("42%");
    expect(out).toContain("1 bot");
    expect(out).toContain('aria-label="More for Claude (Personal)"');
  });

  it("offers Sign in on a signed-out subscription, and Cancel and the page again while one is waiting", () => {
    const out = card(sub({ setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } }));
    expect(out).toContain("Not signed in. Bots on it wait.");
    expect(out).toContain('data-account-action="signin"');
    const waiting = card(sub({ signIn: { attempt: "t1", state: "waiting" } }), { reopenUrl: "https://example.test/signin" });
    expect(waiting).toContain("Finish signing in in your browser");
    expect(waiting).toContain("Cancel");
    expect(waiting).toContain('href="https://example.test/signin"');
    expect(card(sub({ signIn: { attempt: "t1", state: "waiting" } }))).not.toContain("Open the page again");
  });

  it("says how an API key is doing: spending against its cap, the fallback switch, and what replaces", () => {
    const out = card(key(), { siblings: [key(), key({ id: "k2", label: "Other key", isFallback: true })] });
    expect(out).toContain("API key");
    expect(out).toContain("Spent $5.00 of $20.00 cap in total");
    expect(out).toContain("Fallback for Claude");
    expect(out).toContain("Edit cap");
    const replacing = card(key({ isFallback: false }), { siblings: [key({ isFallback: false }), key({ id: "k2", label: "Other key", isFallback: true })] });
    expect(replacing).toContain("Replaces Other key.");
  });

  it("stops an API key at its cap, and at unpriced turns, with the right way forward", () => {
    const capped = card(key({ spending: { knownUsd: 20, unknownRows: 0, capUsd: 20, blocked: true } }));
    expect(capped).toContain("Reached its $20.00 cap. Bots stopped using it.");
    expect(capped).toContain("Raise cap");
    const unpriced = card(key({ spending: { knownUsd: 3, unknownRows: 2, capUsd: 20, blocked: true } }));
    expect(unpriced).toContain("Stopped: 2 turns have no price, so the cap can&#x27;t be checked.");
    expect(unpriced).not.toContain("Raise cap");
  });

  it("offers Retry queued work, with its caption, for an unknown reset, and never starts one because a link pointed at it", () => {
    requested.mockClear();
    const out = card(sub({ limits: [{ ...sub().limits[0], state: "reached", resetsAt: null }] }), { focus: "retry" });
    expect(out).toContain("Limited. Reset time unknown.");
    expect(out).toContain("Retry queued work");
    expect(out).toContain('data-account-action="retry"');
    expect(out).toContain("Retries one queued turn; it may use subscription allowance or API credit.");
    expect(requested).not.toHaveBeenCalled();
    expect(card(sub())).not.toContain("Retry queued work");
  });

  it("never starts a sign-in because a link pointed at the card", () => {
    requested.mockClear();
    const out = card(sub({ setup: { provider: "claude", installed: true, version: null, signedIn: false, account: null, plan: null, error: null } }), { focus: "signin" });
    expect(out).toContain('data-account-action="signin"');
    expect(requested).not.toHaveBeenCalledWith("accounts.signIn", expect.anything());
    expect(requested).not.toHaveBeenCalledWith("accounts.create", expect.anything());
  });
});

describe("meters", () => {
  it("is never red: the fill is the body color, or the accent when near full", () => {
    expect(renderToStaticMarkup(<Meter label="x" value="y" share={0.3} near={false} />)).toContain("bg-body");
    const near = renderToStaticMarkup(<Meter label="x" value="y" share={0.9} near />);
    expect(near).toContain("bg-accent");
    expect(near).not.toContain("danger");
    expect(near).toContain('aria-valuenow="90"');
  });

  it("labels a limit window and says its reading", () => {
    const out = renderToStaticMarkup(<LimitMeter window={{ ...sub().limits[0], usedPercent: null }} now={NOW} />);
    expect(out).toContain("5-hour limit");
    expect(out).toContain("Usage unknown");
  });
});

describe("dialog forms", () => {
  const noop = async (): Promise<void> => {};

  it("holds the API key in a masked field the browser is told to leave alone, with a Show button", () => {
    const out = renderToStaticMarkup(<KeyBody provider="claude" onClose={() => {}} onSubmit={noop} />);
    expect(out).toMatch(/<input[^>]*id="account-key"[^>]*type="password"|<input[^>]*type="password"[^>]*id="account-key"/);
    for (const attribute of ['autoComplete="off"', 'spellCheck="false"', 'autoCapitalize="off"', 'autoCorrect="off"']) expect(out).toContain(attribute);
    expect(out).toContain('aria-label="Show key"');
    expect(out).not.toContain('aria-pressed="true"');
    expect(out).toContain("Stored in Windows Credential Manager and given only to the Claude Code process.");
  });

  it("keeps Add key disabled until there is a name, a key and a cap", () => {
    const out = renderToStaticMarkup(<KeyBody provider="codex" onClose={() => {}} onSubmit={noop} />);
    expect(out).toMatch(/<button[^>]*type="submit"[^>]*disabled=""[^>]*>Add key|<button[^>]*disabled=""[^>]*type="submit"[^>]*>Add key/);
    expect(out).toContain("Fallback for Codex");
    expect(out).toContain('inputMode="decimal"');
  });

  it("replaces a key with the key alone: no name, cap or fallback to save with it", () => {
    const out = renderToStaticMarkup(<ReplaceBody account={key()} onClose={() => {}} onSubmit={noop} />);
    expect(out).toContain('id="account-key"');
    expect(out).not.toContain('id="account-name"');
    expect(out).not.toContain('id="account-cap"');
    expect(out).toContain("Save key");
  });

  it("warns when a cap is below what is already spent", () => {
    const spent = key({ spendingCapUsd: 10, spending: { knownUsd: 25, unknownRows: 0, capUsd: 10, blocked: true } });
    expect(renderToStaticMarkup(<CapBody account={spent} onClose={() => {}} onSubmit={noop} />)).toContain("This is below the $25.00 already spent, so bots stop using the key right away.");
    expect(renderToStaticMarkup(<CapBody account={key()} onClose={() => {}} onSubmit={noop} />)).not.toContain("already spent");
  });

  it("names the account being added, and the account being renamed", () => {
    expect(renderToStaticMarkup(<SubscriptionBody provider="claude" onClose={() => {}} onSubmit={noop} />)).toContain('placeholder="Claude (Personal)"');
    expect(renderToStaticMarkup(<SubscriptionBody provider="codex" onClose={() => {}} onSubmit={noop} />)).toContain("Add and sign in");
    expect(renderToStaticMarkup(<RenameBody account={sub()} onClose={() => {}} onSubmit={noop} />)).toContain('value="Claude (Personal)"');
  });
});
