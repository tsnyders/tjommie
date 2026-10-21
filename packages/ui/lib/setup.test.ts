import type { ProviderSetup } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { cardView, failedSetup, liveSignIns, planLabel, sameSignIn } from "./setup";

type Claude = Extract<ProviderSetup, { provider: "claude" }>;
type Codex = Extract<ProviderSetup, { provider: "codex" }>;
const claude = (extra: Partial<Claude> = {}): Claude => ({ provider: "claude", installed: true, version: "2.1.280", signedIn: false, account: null, plan: null, error: null, ...extra });
const codex = (extra: Partial<Codex> = {}): Codex => ({ provider: "codex", installed: true, version: "0.157.1", signedIn: false, account: null, plan: null, error: null, sandbox: "notConfigured", ...extra });
const quiet = { signingIn: false, preparing: false };

describe("cardView", () => {
  it("shows checking, not installed, a failed check, signed out and signing in", () => {
    expect(cardView(undefined, quiet)).toEqual({ kind: "checking" });
    expect(cardView(claude({ installed: false, version: null, error: "Claude Code is not installed" }), quiet)).toEqual({ kind: "not_installed" });
    expect(cardView(claude({ error: "Claude sign-in check failed" }), quiet)).toEqual({ kind: "problem", reason: "Claude sign-in check failed" });
    expect(cardView(claude(), quiet)).toEqual({ kind: "signed_out" });
    expect(cardView(claude(), { signingIn: true, preparing: false })).toEqual({ kind: "signing_in" });
    expect(cardView(claude({ signedIn: true }), { signingIn: true, preparing: false })).toEqual({ kind: "signing_in" });
  });

  it("shows the account, and whether Codex's sandbox still needs setting up", () => {
    expect(cardView(claude({ signedIn: true, account: "tyrel.jobs@gmail.com", plan: "max" }), quiet)).toEqual({ kind: "signed_in", account: "tyrel.jobs@gmail.com", plan: "max", sandbox: null });
    expect(cardView(codex({ signedIn: true }), quiet)).toMatchObject({ kind: "signed_in", sandbox: "needed" });
    expect(cardView(codex({ signedIn: true, sandbox: "updateRequired" }), quiet)).toMatchObject({ sandbox: "needed" });
    expect(cardView(codex({ signedIn: true }), { signingIn: false, preparing: true })).toMatchObject({ sandbox: "preparing" });
    expect(cardView(codex({ signedIn: true, sandbox: "unavailable" }), quiet)).toMatchObject({ sandbox: "unavailable" });
    expect(cardView(codex({ signedIn: true, sandbox: "ready" }), quiet)).toMatchObject({ sandbox: "ready" });
    expect(planLabel("max")).toBe("Max plan");
    expect(planLabel(null)).toBeNull();
  });

  it("keeps a sign-in going through checks that report the same sign-in", () => {
    const signedOut = claude();
    expect(sameSignIn({ ...signedOut, version: "2.1.281" }, signedOut)).toBe(true); // a new check, a new object
    expect(sameSignIn(failedSetup("The check failed")[0], signedOut)).toBe(true);
    expect(sameSignIn(claude({ signedIn: true, account: "tyrel.jobs@gmail.com" }), signedOut)).toBe(false);
    const signedIn = codex({ signedIn: true, account: "tyrel.jobs@gmail.com" });
    expect(sameSignIn({ ...signedIn, sandbox: "ready" }, signedIn)).toBe(true);
    expect(sameSignIn({ ...signedIn, account: "work@example.com" }, signedIn)).toBe(false);
  });

  it("shows a failed setup check on both cards, with Check again", () => {
    const entries = failedSetup("The check failed");
    expect(entries.map((entry) => entry.provider)).toEqual(["claude", "codex"]);
    for (const entry of entries) expect(cardView(entry, quiet)).toEqual({ kind: "problem", reason: "The check failed" });
  });
});

describe("liveSignIns", () => {
  const [claudeOut, codexOut] = failedSetup("x");
  const both = { claude: { since: claudeOut, attempt: "c1" }, codex: { since: codexOut, attempt: "x1" } };
  const setup = [claudeOut!, codexOut!];

  it("keeps both providers' sign-ins, and the same object, while neither ended", () => {
    expect(liveSignIns(both, [], setup)).toBe(both);
    expect(liveSignIns(both, ["other"], setup)).toBe(both);
  });

  it("ends only the provider whose attempt ended", () => {
    expect(liveSignIns(both, ["x1"], setup)).toEqual({ claude: both.claude });
    expect(liveSignIns(both, ["c1"], setup)).toEqual({ codex: both.codex });
  });

  it("ends only the provider whose report changed", () => {
    const signedIn = setup.map((entry) => (entry.provider === "claude" ? { ...entry, signedIn: true, account: "a@b.c" } : entry));
    expect(liveSignIns(both, [], signedIn)).toEqual({ codex: both.codex });
  });
});
