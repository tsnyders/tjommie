import type { AccountInfo, AccountLimitWindow, Bot, Provider } from "@tjommie/protocol";
import { SETUP_CODEX_SANDBOX_REASON, SETUP_SIGN_IN_PREFIX } from "@tjommie/protocol";
import { clock, dayName, timeAgo } from "./format";
import { PROVIDER_NAME, planLabel } from "./setup";

const HOUR_MS = 3_600_000;
const NEAR = 0.8; // a spending meter turns to the accent at this share of its cap

export const money = (usd: number): string => `$${usd.toFixed(2)}`;

// "9:14 AM" today, "Fri 9:14 AM" on another day.
export function when(value: string, now: Date): string {
  return dayName(value, now) === "Today" ? clock(value) : `${dayName(value, now, true)} ${clock(value)}`;
}

export const accountsOf = (accounts: AccountInfo[], provider: Provider): AccountInfo[] =>
  accounts.filter((account) => account.provider === provider).sort((a, b) => Number(b.isDefault) - Number(a.isDefault) || a.createdAt.localeCompare(b.createdAt));

export const defaultAccount = (accounts: AccountInfo[], provider: Provider): AccountInfo | undefined => accounts.find((account) => account.provider === provider && account.isDefault);

// The account a bot runs on: the one it is using now, else its pin, else its provider's default.
export function accountFor(bot: Pick<Bot, "provider" | "accountId" | "effectiveAccountId">, accounts: AccountInfo[]): AccountInfo | undefined {
  const id = bot.effectiveAccountId ?? bot.accountId;
  return (id ? accounts.find((account) => account.id === id) : undefined) ?? defaultAccount(accounts, bot.provider);
}

// The bots pinned to an account. While there are any, it can't be removed.
export const botsOn = <B extends Pick<Bot, "accountId">>(account: Pick<AccountInfo, "id">, bots: B[]): B[] => bots.filter((bot) => bot.accountId === account.id);
export const usedBy = (account: Pick<AccountInfo, "id">, bots: Pick<Bot, "accountId">[]): number => botsOn(account, bots).length;

// 300 minutes is "5-hour limit", 10080 is "Weekly limit"; other lengths in hours or days; no length, the window's own name.
export function limitLabel(window: Pick<AccountLimitWindow, "durationMinutes" | "windowKey">): string {
  const minutes = window.durationMinutes;
  if (minutes === null) return window.windowKey;
  if (minutes === 10_080) return "Weekly limit";
  if (minutes % 1440 === 0) return `${minutes / 1440}-day limit`;
  if (minutes % 60 === 0) return `${minutes / 60}-hour limit`;
  return `${Math.round(minutes / 6) / 10}-hour limit`;
}

// "42% · resets 3:10 PM", with how old the reading is once it is over an hour old.
export function limitValue(window: AccountLimitWindow, now: Date): string {
  const used = window.usedPercent === null ? "Usage unknown" : `${Math.round(window.usedPercent)}%`;
  const resets = window.resetsAt ? ` · resets ${when(window.resetsAt, now)}` : "";
  const stale = now.getTime() - Date.parse(window.observedAt) > HOUR_MS ? ` · updated ${timeAgo(window.observedAt, now)}` : "";
  return `${used}${resets}${stale}`;
}

export const limitNear = (window: Pick<AccountLimitWindow, "state">): boolean => window.state === "near" || window.state === "reached";

// When the account is limited until: the latest reset among its reached windows that is still ahead, or null.
export function limitedUntil(account: Pick<AccountInfo, "limits">, now: Date): string | null {
  const resets = account.limits.filter((window) => window.state === "reached" && window.resetsAt && Date.parse(window.resetsAt) > now.getTime()).map((window) => window.resetsAt as string);
  return resets.length === 0 ? null : resets.reduce((latest, next) => (Date.parse(next) > Date.parse(latest) ? next : latest));
}

// What a limit means for an account right now: blocked until a time, or blocked with no reset time known (a window that has reached
// its limit and says nothing of when it resets still blocks, as the runtime treats it), or not blocked.
export type LimitState = { kind: "until"; at: string } | { kind: "unknown" };

export function limitState(account: Pick<AccountInfo, "limits">, now: Date): LimitState | null {
  if (account.limits.some((window) => window.state === "reached" && window.resetsAt === null)) return { kind: "unknown" };
  const at = limitedUntil(account, now);
  return at ? { kind: "until", at } : null;
}

export const limitedText = (state: LimitState, now: Date): string => (state.kind === "until" ? `Limited until ${when(state.at, now)}` : "Limited. Reset time unknown.");

// The window with the highest use, if any use is known (the bot panel's Gauge row).
export function topLimit(accounts: Pick<AccountInfo, "limits">[]): AccountLimitWindow | null {
  const known = accounts.flatMap((account) => account.limits).filter((window) => window.usedPercent !== null);
  return known.length === 0 ? null : known.reduce((top, next) => ((next.usedPercent ?? 0) > (top.usedPercent ?? 0) ? next : top));
}

// Spent against the cap: a share from 0 to 1, whether it is near the cap, and the line saying so.
export function spendingMeter(spending: AccountInfo["spending"]): { share: number; near: boolean; text: string } {
  const cap = spending.capUsd;
  const unknown = spending.unknownRows > 0 ? ` + ${spending.unknownRows} ${spending.unknownRows === 1 ? "turn" : "turns"} with no price` : "";
  const share = cap && cap > 0 ? Math.min(1, spending.knownUsd / cap) : 0;
  const text = cap === null ? `Spent ${money(spending.knownUsd)} in total${unknown}` : `Spent ${money(spending.knownUsd)} of ${money(cap)} cap in total${unknown}`;
  return { share, near: share >= NEAR || spending.blocked, text };
}

export type StatusAction = "openAgain" | "cancel" | "tryAgain" | "check" | "signIn" | "addKey" | "raiseCap" | "retryLimit";
export type StatusLine = { tone: "busy" | "ok" | "limited" | "muted" | "problem"; text: string; actions: StatusAction[] };

// A limit with no known reset can be tried once, by hand, to see whether it has lifted: only while nothing known and in the future also
// blocks it, and never past a cap (the daemon still gates on the cap, the price and the sign-in).
export const canRetryLimit = (account: AccountInfo, now: Date): boolean => limitState(account, now)?.kind === "unknown" && limitedUntil(account, now) === null && !account.spending.blocked;

// What a limited account offers: Retry queued work when that is allowed, and, for a Codex subscription, Check again, which asks Codex's own
// (free) quota check. The two are different: one lets a queued turn run, the other only looks.
function limitActions(account: AccountInfo, now: Date): StatusAction[] {
  const actions: StatusAction[] = [];
  if (canRetryLimit(account, now)) actions.push("retryLimit");
  if (account.provider === "codex" && account.kind === "subscription" && limitState(account, now)?.kind === "unknown") actions.push("check");
  return actions;
}

// Whether the control a link pointed at is on the card right now. A card that is still checking has no Sign in or Retry yet; the focus waits
// for it, and happens once when it appears.
export function focusReady(focus: "signin" | "cap" | "retry", account: Pick<AccountInfo, "kind">, actions: StatusAction[]): boolean {
  if (focus === "signin") return actions.includes("signIn") || actions.includes("tryAgain");
  if (focus === "retry") return actions.includes("retryLimit");
  return account.kind === "api_key"; // Edit cap is always on an API key's card
}

export const RETRY_CAPTION = "Retries one queued turn; it may use subscription allowance or API credit.";

// The one line an account card shows, by the first rule that fits. `stuck` is a subscription whose check has gone on too long;
// `canReopen` is whether the sign-in page's address was kept here, so it can be opened again.
export function statusLine(account: AccountInfo, now: Date, flags: { stuck?: boolean; canReopen?: boolean } = {}): StatusLine {
  const signIn = account.signIn;
  if (signIn?.state === "waiting") return { tone: "busy", text: "Finish signing in in your browser", actions: flags.canReopen ? ["openAgain", "cancel"] : ["cancel"] };
  if (signIn?.state === "failed") return { tone: "problem", text: signIn.message ?? "Sign-in didn't finish.", actions: ["tryAgain"] };
  if (account.kind === "subscription") {
    if (account.setup === null) return flags.stuck ? { tone: "muted", text: "Couldn't check yet", actions: ["check"] } : { tone: "busy", text: "Checking", actions: [] };
    if (account.setup.error) return { tone: "problem", text: account.setup.error, actions: ["check"] };
    if (!account.setup.signedIn) return { tone: "muted", text: "Not signed in. Bots on it wait.", actions: ["signIn"] };
    const limit = limitState(account, now);
    if (limit) return { tone: "limited", text: limitedText(limit, now), actions: limitActions(account, now) };
    return { tone: "ok", text: "Signed in", actions: [] };
  }
  if (!account.hasCredential) return { tone: "muted", text: "No key saved", actions: ["addKey"] };
  if (account.spending.blocked) {
    if (account.spending.unknownRows > 0) return { tone: "problem", text: `Stopped: ${account.spending.unknownRows} ${account.spending.unknownRows === 1 ? "turn has" : "turns have"} no price, so the cap can't be checked.`, actions: [] };
    return { tone: "problem", text: `Reached its ${money(account.spending.capUsd ?? 0)} cap. Bots stopped using it.`, actions: ["raiseCap"] };
  }
  const limit = limitState(account, now);
  if (limit) return { tone: "limited", text: limitedText(limit, now), actions: limitActions(account, now) };
  return { tone: "ok", text: "Key saved", actions: [] };
}

// What the picker shows on an account's row: limited, not signed in, or what kind it is.
export function pickerState(account: AccountInfo, now: Date): string {
  const limit = limitState(account, now);
  if (limit) return limit.kind === "until" ? `Limited until ${when(limit.at, now)}` : "Limited";
  if (account.kind === "subscription" && account.setup && !account.setup.signedIn) return "Not signed in";
  if (account.isDefault) return "Default";
  return account.kind === "api_key" ? "API key" : "";
}

// The account a provider's tab stands for: the one the bot has if it is on this provider, else the provider's default.
export function chosenAccount(accounts: AccountInfo[], provider: Provider, current: string | null): AccountInfo | undefined {
  return (current ? accounts.find((account) => account.id === current && account.provider === provider) : undefined) ?? defaultAccount(accounts, provider);
}

// A sentence under the Account and model row when the account in use can't take work, or when changing it has a cost.
export function accountHelp(account: AccountInfo | undefined, now: Date, changed: boolean): string | null {
  if (account) {
    if (account.kind === "subscription" && account.setup && !account.setup.signedIn) return `${account.label} isn't signed in.`;
    const limit = limitState(account, now);
    if (limit) return limit.kind === "until" ? `${account.label} is limited until ${when(limit.at, now)}.` : `${account.label} is limited, and its reset time isn't known.`;
    if (account.spending.blocked) return `${account.label} reached its cap.`;
  }
  return changed ? "Switching accounts starts a fresh session. The chat stays." : null;
}

export const subscriptionLabel = (account: AccountInfo): string => {
  const parts = [planLabel(account.setup?.plan ?? null), account.setup?.account ?? null].filter((part): part is string => part !== null);
  return ["Subscription", ...parts].join(" · ");
};

export type UsageRow = { key: string; label: string; share: number | null; value: string; near: boolean };
export type ProviderView = {
  plan: string | null;
  rows: UsageRow[];
  stale: string | null; // set when the windows shown are from an earlier successful check: when
  problem: string | null; // why the latest check failed, if it did
  extras: { label: string; value: string }[]; // Extra usage and Credits, only what the provider said
};

// "Weekly · Fable": a window's name, with the model or surface it is for.
export const windowName = (window: { label: string; scope: { label: string } | null }): string => (window.scope ? `${window.label} · ${window.scope.label}` : window.label);

// "42% · resets 3:10 PM". An unknown use is "Usage unknown", never 0%. A reset already past (an old reading) is left out rather than shown as ahead.
function windowValue(window: { usedPercent: number | null; resetsAt: string | null }, now: Date): string {
  const used = window.usedPercent === null ? "Usage unknown" : `${Math.round(window.usedPercent)}%`;
  const resets = window.resetsAt && Date.parse(window.resetsAt) > now.getTime() ? ` · resets ${when(window.resetsAt, now)}` : "";
  return `${used}${resets}`;
}

// Credits are the provider's own units, not dollars: a number, "Unlimited" only when the provider said so, and unknown otherwise.
export function creditsText(credits: { balance: string | null; unlimited: boolean | null }): string {
  if (credits.unlimited === true) return "Unlimited";
  if (credits.balance === null) return "Unknown";
  return `${Number(credits.balance).toLocaleString("en-US", { maximumFractionDigits: 2 })} left`;
}

// What a subscription's usage card shows, from the provider's own live reading when the daemon has one, else from the windows it last observed. A
// failed or unavailable check after a good one keeps the old windows, marked with when they were last updated: never shown as zero or as fresh.
export function providerView(account: AccountInfo, now: Date): ProviderView {
  const live = account.providerUsage;
  const plan = live?.planLabel ?? account.setup?.plan ?? null;
  if (live && live.windows.length > 0) {
    const rows = live.windows.map((window) => ({
      key: window.id,
      label: windowName(window),
      share: window.usedPercent === null ? null : window.usedPercent / 100,
      value: windowValue(window, now),
      near: window.usedPercent !== null && window.usedPercent >= 80,
    }));
    const when_ = live.lastSuccessfulAt;
    const stale = live.status === "available" ? null : when_ ? `Last updated ${timeAgo(when_, now)}` : "Last update time unknown";
    const extras: { label: string; value: string }[] = [];
    if (live.extraUsageEnabled !== null) extras.push({ label: "Extra usage", value: live.extraUsageEnabled ? "Enabled" : "Disabled" });
    if (live.credits !== null) extras.push({ label: "Credits", value: creditsText(live.credits) });
    return { plan, rows, stale, problem: live.status === "error" ? (live.error ?? "Couldn't check the limits.") : null, extras };
  }
  const observed = account.limits.map((window) => ({
    key: `${window.limitId}:${window.windowKey}`,
    label: limitLabel(window),
    share: window.usedPercent === null ? null : window.usedPercent / 100,
    value: limitValue(window, now),
    near: limitNear(window),
  }));
  const extras: { label: string; value: string }[] = [];
  if (live?.extraUsageEnabled != null) extras.push({ label: "Extra usage", value: live.extraUsageEnabled ? "Enabled" : "Disabled" });
  if (live?.credits) extras.push({ label: "Credits", value: creditsText(live.credits) });
  // Extras kept from an earlier success after a failed or unavailable check are stale too, and say when they were last updated.
  const staleExtras = live && live.status !== "available" && extras.length > 0 ? (live.lastSuccessfulAt ? `Last updated ${timeAgo(live.lastSuccessfulAt, now)}` : "Last update time unknown") : null;
  return { plan, rows: observed, stale: staleExtras, problem: live?.status === "error" ? (live.error ?? "Couldn't check the limits.") : null, extras };
}

export type QuotaCard =
  | { kind: "limits" } // the account has limit windows, or Extra usage or Credits, to show
  | { kind: "checking" }
  | { kind: "problem"; text: string }
  | { kind: "signedOut" }
  | { kind: "unavailable" }; // signed in, but no limit windows have been reported (yet)

// What a subscription's usage card says besides its title. Nothing is made up: with no limit windows reported it says so, and Extra usage and
// Credits are shown only when the provider reported them. A card with only those (no windows) is still a card.
export function quotaCard(account: AccountInfo): QuotaCard {
  if (account.signIn?.state === "waiting") return { kind: "checking" };
  if (account.setup === null) return { kind: "checking" };
  if (account.setup.error) return { kind: "problem", text: account.setup.error };
  if (!account.setup.signedIn) return { kind: "signedOut" };
  const live = account.providerUsage;
  const hasExtras = live !== undefined && (live.extraUsageEnabled !== null || live.credits !== null);
  if ((live && live.windows.length > 0) || account.limits.length > 0 || hasExtras) return { kind: "limits" };
  if (live?.status === "error") return { kind: "problem", text: live.error ?? "Couldn't check the limits." };
  return { kind: "unavailable" };
}

export const providerName = (provider: Provider): string => PROVIDER_NAME[provider];

const canTakeWork = (account: AccountInfo, now: Date): boolean =>
  !account.spending.blocked && limitState(account, now) === null && (account.kind === "api_key" ? account.hasCredential : account.setup !== null && account.setup.signedIn);

export type Banner =
  | { kind: "setup"; text: string; sandbox: boolean; signIn: boolean }
  | { kind: "signIn"; account: AccountInfo }
  | { kind: "cap"; account: AccountInfo; unknownPrices: number }
  | { kind: "limited"; account: AccountInfo; window: string; until: string | null; alternatives: AccountInfo[] } // until: null when the reset is unknown
  | { kind: "fallback"; pinned: AccountInfo; using: AccountInfo; until: string | null };

// The banner for a bot, by the first rule that fits; null for none. It only ever offers to switch accounts: no switch happens by
// itself, and none is remembered.
export function bannerFor(bot: Bot, accounts: AccountInfo[] | null, now: Date): Banner | null {
  const account = accounts ? accountFor(bot, accounts) : undefined;
  if (bot.state === "needs_setup") {
    const reason = bot.setupReason ?? "";
    if (reason === SETUP_CODEX_SANDBOX_REASON) return { kind: "setup", text: "Codex's sandbox isn't set up yet. Windows asks for admin permission once.", sandbox: true, signIn: false };
    if (reason.startsWith(SETUP_SIGN_IN_PREFIX) && account) return { kind: "signIn", account };
    return { kind: "setup", text: reason.startsWith(SETUP_SIGN_IN_PREFIX) ? `${PROVIDER_NAME[bot.provider]} isn't signed in for Tjommie yet.` : reason, sandbox: false, signIn: reason.startsWith(SETUP_SIGN_IN_PREFIX) };
  }
  if (!accounts || !account) return null;
  if (account.spending.blocked) return { kind: "cap", account, unknownPrices: account.spending.unknownRows };
  if (bot.state === "limited" && !bot.usingFallback) {
    const reached = account.limits.filter((window) => window.state === "reached");
    const hit = reached.find((window) => window.resetsAt === null) ?? reached.reduce<AccountLimitWindow | null>((latest, next) => (!latest || Date.parse(next.resetsAt ?? "") > Date.parse(latest.resetsAt ?? "") ? next : latest), null);
    const alternatives = accountsOf(accounts, bot.provider).filter((other) => other.id !== account.id && canTakeWork(other, now));
    return { kind: "limited", account, window: hit ? limitLabel(hit) : "limit", until: bot.limitedUntil ?? hit?.resetsAt ?? null, alternatives };
  }
  if (bot.usingFallback) {
    const pinned = bot.accountId ? accounts.find((other) => other.id === bot.accountId) : defaultAccount(accounts, bot.provider);
    const limit = pinned ? limitState(pinned, now) : null;
    if (pinned && pinned.id !== account.id && limit) return { kind: "fallback", pinned, using: account, until: limit.kind === "until" ? limit.at : null };
  }
  return null;
}

// A dialog is closed only by its own completion: one that finishes after another dialog opened must not close that one. Each open dialog
// has an epoch; a close for any other epoch changes nothing.
export function closeEpoch<T extends { epoch: number }>(current: T | null, epoch: number): T | null {
  return current && current.epoch === epoch ? null : current;
}

// The sign-in pages kept by account. A sign-in that gave no address keeps nothing, and an older address is dropped with it.
export function rememberUrl(urls: Record<string, string>, id: string, url: string | null): Record<string, string> {
  const rest = Object.fromEntries(Object.entries(urls).filter(([key]) => key !== id));
  return url ? { ...rest, [id]: url } : rest;
}

// Takes the key out of the field to send it: the field is cleared at once, before anything is awaited, so a request that is slow or
// fails never leaves the key on screen or in state. Sending again means typing it again.
export function takeKey(typed: string, clear: () => void): string {
  const key = typed.trim();
  clear();
  return key;
}

const DOLLARS = /^\d+(\.\d{1,2})?$/;

// The cap a person typed, in dollars, or null if it isn't a positive amount the daemon takes (at most a million).
export function parseCap(text: string): number | null {
  const value = text.trim();
  if (!DOLLARS.test(value)) return null;
  const amount = Number(value);
  return amount > 0 && amount <= 1_000_000 ? amount : null;
}

// Whether an API key can be sent: something typed, no whitespace inside (a pasted line break would make it a different key), within
// the credential store's 2560 bytes. Never inspected beyond that.
export const keyReady = (key: string): boolean => {
  const typed = key.trim();
  return typed.length > 0 && !/\s/.test(typed) && new TextEncoder().encode(typed).length <= 2560;
};
