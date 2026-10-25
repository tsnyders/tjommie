import type { AccountInfo, Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Banner } from "@/lib/accounts";
import { BannerView } from "./setup-banner";

vi.mock("@/lib/connection", () => ({ request: vi.fn(), daemon: vi.fn() }));

const bot = { id: "b1", name: "Scout", provider: "claude" } as Bot;
const account = (patch: Partial<AccountInfo> = {}): AccountInfo =>
  ({ id: "a1", provider: "claude", label: "Claude (Personal)", kind: "subscription", isDefault: true, isFallback: false, spendingCapUsd: null, hasCredential: false, setup: null, limits: [], signIn: null, spending: { knownUsd: 0, unknownRows: 0, capUsd: null, blocked: false }, ...patch }) as AccountInfo;
const until = "2026-10-22T14:00:00.000Z";
const html = (banner: Banner) => renderToStaticMarkup(<BannerView bot={bot} banner={banner} />);

describe("BannerView", () => {
  it("sends a sign-in banner to the account's card, which only points at the button", () => {
    const out = html({ kind: "signIn", account: account() });
    expect(out).toContain("Claude (Personal) isn&#x27;t signed in.");
    expect(out).toContain("/settings?section=accounts&amp;account=a1&amp;action=signin");
    expect(out).toContain("Sign in");
  });

  it("raises a cap through the account's card, and has no button when the cap can't be checked", () => {
    const capped = html({ kind: "cap", account: account({ kind: "api_key", spending: { knownUsd: 20, unknownRows: 0, capUsd: 20, blocked: true } }), unknownPrices: 0 });
    expect(capped).toContain("reached its $20.00 cap.");
    expect(capped).toContain("action=cap");
    const unpriced = html({ kind: "cap", account: account({ kind: "api_key" }), unknownPrices: 2 });
    expect(unpriced).toContain("2 turns have no price");
    expect(unpriced).not.toContain("Raise cap");
  });

  it("offers one account to switch to, a menu for several, and a way to add one for none, with no automatic or 'always' choice", () => {
    const other = (id: string, label: string) => account({ id, label, isDefault: false });
    const one = html({ kind: "limited", account: account(), window: "5-hour limit", until, alternatives: [other("a2", "Work")] });
    expect(one).toContain("hit its 5-hour limit");
    expect(one).toContain("Scout resumes by itself");
    expect(one).toContain("Switch to Work");
    expect(one).toContain("Switching starts a fresh session. The chat stays.");
    const several = html({ kind: "limited", account: account(), window: "Weekly limit", until, alternatives: [other("a2", "Work"), other("a3", "Spare")] });
    expect(several).toContain("Switch account");
    expect(several).not.toContain("Switch to ");
    const none = html({ kind: "limited", account: account(), window: "5-hour limit", until, alternatives: [] });
    expect(none).toContain("Add an account");
    expect(none).toContain("/settings?section=accounts");
    expect(none).not.toContain("fresh session");
    for (const out of [one, several, none]) expect(out.toLowerCase()).not.toContain("always");
  });

  it("says a limited bot resumes when it resets, when no time is known", () => {
    const out = html({ kind: "limited", account: account(), window: "5-hour limit", until: null, alternatives: [] });
    expect(out).toContain("Scout resumes by itself when it resets.");
    expect(out).toContain("Manage account");
    expect(out).toContain("action=retry");
    expect(html({ kind: "limited", account: account(), window: "5-hour limit", until: "2026-10-22T14:00:00.000Z", alternatives: [] })).not.toContain("Manage account");
    expect(html({ kind: "fallback", pinned: account(), using: account({ id: "k1", label: "Claude key" }), until: null })).toContain("is limited, and its reset time isn&#x27;t known.");
  });

  it("wraps its buttons under the sentence on a phone, and cuts a long account name short in its button, keeping it whole in the tooltip", () => {
    const long = "A".repeat(60);
    const out = html({ kind: "limited", account: account(), window: "5-hour limit", until: null, alternatives: [account({ id: "a2", label: long, isDefault: false })] });
    expect(out).toContain("max-sm:flex-wrap");
    expect(out).toContain(`title="Switch to ${long}"`);
    expect(out).toContain("truncate");
    expect(out).toContain("max-w-full");
    expect(out).toContain("Manage account");
    expect(out).toContain("[overflow-wrap:anywhere]"); // an unbroken long name in the sentence breaks instead of overflowing
  });

  it("explains a fallback with nothing to press", () => {
    const out = html({ kind: "fallback", pinned: account(), using: account({ id: "k1", label: "Claude key" }), until });
    expect(out).toContain("Claude (Personal) is limited until");
    expect(out).toContain("Scout is using Claude key until then.");
    expect(out).not.toContain("<button");
    expect(out).not.toContain("<a ");
  });

  it("keeps the sandbox banner as it was", () => {
    const out = html({ kind: "setup", text: "Codex's sandbox isn't set up yet. Windows asks for admin permission once.", sandbox: true, signIn: false });
    expect(out).toContain("sandbox isn&#x27;t set up yet");
    expect(out).toContain("Set up");
  });
});
