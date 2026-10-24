import type { AccountInfo, Bot, UsageAggregate, UsageCost, UsageQuery, UsageReport } from "@tjommie/protocol";

export type RangeKind = "today" | "week" | "month" | "year";
export type UsageFilters = { bot?: string; account?: string; model?: string };
export type By = "bot" | "account" | "model";

export const localZone = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone;

const PARTS = { year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric", hourCycle: "h23" } as const;

// The zone's offset from UTC at an instant, in milliseconds.
function offsetAt(ms: number, timeZone: string): number {
  const read = new Intl.DateTimeFormat("en-US", { timeZone, ...PARTS }).formatToParts(new Date(ms));
  const at = (type: string): number => Number(read.find((part) => part.type === type)?.value);
  return Date.UTC(at("year"), at("month") - 1, at("day"), at("hour"), at("minute"), at("second")) - Math.floor(ms / 1000) * 1000;
}

// Midnight of a calendar day in the zone, as an instant: the guess is corrected by the zone's offset at it, twice, which
// lands right across a daylight saving change.
function midnight(year: number, month: number, day: number, timeZone: string): number {
  const base = Date.UTC(year, month - 1, day);
  const first = base - offsetAt(base, timeZone);
  return base - offsetAt(first, timeZone);
}

function dayIn(ms: number, timeZone: string): { year: number; month: number; day: number } {
  const read = new Intl.DateTimeFormat("en-US", { timeZone, year: "numeric", month: "numeric", day: "numeric" }).formatToParts(new Date(ms));
  const at = (type: string): number => Number(read.find((part) => part.type === type)?.value);
  return { year: at("year"), month: at("month"), day: at("day") };
}

// Calendar identity, independent of the report's end instant. A new local day or zone
// must not display yesterday's report while its replacement is loading.
export function usageDayKey(now: Date, timeZone: string): string {
  const { year, month, day } = dayIn(now.getTime(), timeZone);
  return `${timeZone}:${year}-${month}-${day}`;
}
export function untilUsageDay(now: Date, timeZone: string): number {
  const { year, month, day } = dayIn(now.getTime(), timeZone);
  return Math.max(1, midnight(year, month, day + 1, timeZone) - now.getTime() + 1);
}

// From local midnight (today, Monday, the 1st, or January 1st) in the zone, to now.
export function rangeFor(kind: RangeKind, now: Date, timeZone: string): { from: string; to: string } {
  const today = dayIn(now.getTime(), timeZone);
  let { year, month, day } = today;
  if (kind === "week") {
    const sinceMonday = (new Date(Date.UTC(year, month - 1, day)).getUTCDay() + 6) % 7;
    const monday = new Date(Date.UTC(year, month - 1, day - sinceMonday));
    year = monday.getUTCFullYear();
    month = monday.getUTCMonth() + 1;
    day = monday.getUTCDate();
  } else if (kind === "month") {
    day = 1;
  } else if (kind === "year") {
    month = 1;
    day = 1;
  }
  return { from: new Date(midnight(year, month, day, timeZone)).toISOString(), to: now.toISOString() };
}

export function usageQuery(kind: RangeKind, filters: UsageFilters, now: Date, timeZone: string): UsageQuery {
  return { ...rangeFor(kind, now, timeZone), timeZone, ...(filters.bot ? { botId: filters.bot } : {}), ...(filters.account ? { accountId: filters.account } : {}), ...(filters.model ? { model: filters.model } : {}) };
}

// The Usage screen's address for some filters (a bot's panel links to it with its bot; a chip's x links to it without that filter).
export function usageHref(filters: UsageFilters): string {
  const query = new URLSearchParams({ section: "usage" });
  if (filters.bot) query.set("bot", filters.bot);
  if (filters.account) query.set("account", filters.account);
  if (filters.model) query.set("model", filters.model);
  return `/settings/?${query.toString()}`;
}

export function filtersFrom(params: { get(name: string): string | null }): UsageFilters {
  const filters: UsageFilters = {};
  const bot = params.get("bot");
  const account = params.get("account");
  const model = params.get("model");
  if (bot) filters.bot = bot;
  if (account) filters.account = account;
  if (model) filters.model = model;
  return filters;
}

// 950, 12.4K, 1.2M, 3.4B.
export function tokensText(count: number): string {
  const scaled = (divisor: number, suffix: string): string => `${(Math.round((count / divisor) * 10) / 10).toString()}${suffix}`;
  if (count >= 999_950_000) return scaled(1e9, "B");
  if (count >= 999_950) return scaled(1e6, "M");
  if (count >= 1000) return scaled(1e3, "K");
  return String(Math.round(count));
}

export const usd = (amount: number): string => `$${amount.toFixed(2)}`;

// The three kinds of cost added into one figure, for the combined estimate: what the API keys spent, what the subscriptions' tokens would have cost on the
// API, and what is unclassified. It is a total of known prices only, so it is exact when every turn was priced, "At least" when some were not, and
// unknown when none were. Tokens are never summed here (they are not costs), and nothing is counted twice: each turn is in exactly one kind.
export function combinedCost(costs: UsageAggregate["costs"]): UsageCost {
  const all = [costs.apiSpend, costs.apiValue, costs.unclassified];
  const knownRecords = all.reduce((sum, each) => sum + each.knownRecords, 0);
  const unknownRecords = all.reduce((sum, each) => sum + each.unknownRecords, 0);
  const knownUsd = all.reduce((sum, each) => sum + (each.knownUsd ?? 0), 0);
  const estimated = all.reduce((sum, each) => sum + (each.estimatedUsd ?? 0), 0);
  const priced = knownRecords > 0;
  return {
    usd: unknownRecords === 0 ? knownUsd : null,
    knownUsd: priced ? knownUsd : null,
    knownRecords,
    unknownRecords,
    reportedUsd: null,
    estimatedUsd: estimated > 0 ? estimated : null,
  };
}

// A cost as words. A total that isn't known is never shown as $0: with some prices missing it is "At least", with none, "Unknown".
export function costText(cost: UsageCost): { text: string; note: string | null } {
  if (cost.knownRecords + cost.unknownRecords === 0) return { text: usd(0), note: null };
  if (cost.usd !== null) return { text: usd(cost.usd), note: cost.estimatedUsd !== null && cost.estimatedUsd > 0 ? "Includes estimates" : null };
  if (cost.knownUsd !== null) return { text: `At least ${usd(cost.knownUsd)}`, note: `${cost.unknownRecords} ${cost.unknownRecords === 1 ? "turn has" : "turns have"} no price` };
  return { text: "Unknown", note: null };
}

const hasRecords = (cost: UsageCost): boolean => cost.knownRecords + cost.unknownRecords > 0;

// The cost lines of one aggregate: API spend and API value stay apart, and are never added together.
export function costLines(aggregate: UsageAggregate): { label: string; text: string; note: string | null }[] {
  const { apiSpend, apiValue, unclassified } = aggregate.costs;
  const lines = [
    { label: "Spent", cost: apiSpend },
    { label: "API value", cost: apiValue },
    { label: "Other", cost: unclassified },
  ].filter((line) => hasRecords(line.cost));
  return lines.map((line) => ({ label: line.label, ...costText(line.cost) }));
}

// A cost after its label: "Spent at least $2.10", not "Spent At least $2.10".
export const labelled = (label: string, text: string): string => `${label} ${text.startsWith("At least") ? `at least${text.slice("At least".length)}` : text}`;

// One short line for a card: "12.4K tokens · $1.20" (the cost of whichever kind it has).
export function summaryLine(aggregate: UsageAggregate): string {
  const lines = costLines(aggregate);
  const cost = lines.length === 0 ? usd(0) : lines.map((line) => `${line.text} ${line.label.toLowerCase()}`).join(" · ");
  return `${tokensText(aggregate.tokens.total)} tokens · ${cost}`;
}

const MAX_DAYS = 400; // a report covers at most 366 days; this only keeps a bad range from looping long

// The calendar days a report covers, in the report's own zone, first to last, as "2026-10-19": a day with no usage is still a day.
export function daysBetween(from: string, to: string, timeZone: string): string[] {
  const first = dayIn(Date.parse(from), timeZone);
  const last = dayIn(Date.parse(to), timeZone);
  const end = Date.UTC(last.year, last.month - 1, last.day);
  const days: string[] = [];
  // Stepping whole calendar days in UTC, so a 23- or 25-hour day in the zone is still one day.
  for (let at = Date.UTC(first.year, first.month - 1, first.day); at <= end && days.length < MAX_DAYS; at += 86_400_000) days.push(new Date(at).toISOString().slice(0, 10));
  return days;
}

const NONE: UsageAggregate = { records: 0, tokens: { input: 0, cachedInput: 0, output: 0, reasoning: 0, total: 0 }, costs: { apiSpend: noCost(), apiValue: noCost(), unclassified: noCost() } };
function noCost(): UsageCost {
  return { usd: 0, knownUsd: 0, knownRecords: 0, unknownRecords: 0, reportedUsd: null, estimatedUsd: null };
}

// The report's days with a slot for every calendar day in its range, the quiet ones empty.
export function paddedDaily(report: Pick<UsageReport, "from" | "to" | "timeZone" | "daily">): UsageReport["daily"] {
  const known = new Map(report.daily.map((day) => [day.day, day]));
  return daysBetween(report.from, report.to, report.timeZone).map((day) => known.get(day) ?? { day, ...NONE });
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// "Oct 19" for "2026-10-19".
export const dayLabel = (day: string): string => `${MONTHS[Number(day.slice(5, 7)) - 1]} ${Number(day.slice(8, 10))}`;

// A few dates to put under the bars: the first, the last, and some between, never closer than a bar-label's width allows.
export function dayTicks(days: string[], most = 5): { index: number; label: string }[] {
  if (days.length === 0) return [];
  const step = Math.max(1, Math.ceil((days.length - 1) / (most - 1)));
  const indexes: number[] = [];
  for (let index = 0; index < days.length - 1; index += step) indexes.push(index);
  if (indexes.length > 0 && days.length - 1 - indexes[indexes.length - 1] < step / 2) indexes.pop(); // too near the last to read
  indexes.push(days.length - 1);
  return indexes.map((index) => ({ index, label: dayLabel(days[index]) }));
}

export type Bar = { day: string; share: number };

// Bars for the days: each day's tokens against the busiest day's.
export function dailyBars(daily: UsageReport["daily"]): { bars: Bar[]; label: string } {
  const top = daily.reduce((best, day) => (day.tokens.total > best.tokens.total ? day : best), daily[0]);
  if (!top || top.tokens.total === 0) return { bars: daily.map((day) => ({ day: day.day, share: 0 })), label: "No tokens in this range" };
  return { bars: daily.map((day) => ({ day: day.day, share: day.tokens.total / top.tokens.total })), label: `Tokens per day, highest ${tokensText(top.tokens.total)} on ${top.day}` };
}

// "Oct" for "2026-10".
export const monthLabel = (month: string): string => MONTHS[Number(month.slice(5, 7)) - 1];

// A year is too many days for bars, so its days are added up by month ("2026-10") in the report's own zone: a slot for every month from January
// to the last one in the range, the empty ones too. Leap days and the 31st are in their month because the days come already padded and zoned.
export function monthlyBars(days: UsageReport["daily"]): { bars: Bar[]; label: string; ticks: { index: number; label: string }[] } {
  const totals: { month: string; total: number }[] = [];
  for (const day of days) {
    const month = day.day.slice(0, 7);
    const last = totals[totals.length - 1];
    if (last && last.month === month) last.total += day.tokens.total;
    else totals.push({ month, total: day.tokens.total });
  }
  const top = totals.reduce((best, each) => (each.total > best.total ? each : best), totals[0]);
  const bars = totals.map((each) => ({ day: each.month, share: top && top.total > 0 ? each.total / top.total : 0 }));
  const ticks = totals.map((each, index) => ({ index, label: monthLabel(each.month) }));
  const label = top && top.total > 0 ? `Tokens per month, highest ${tokensText(top.total)} in ${monthLabel(top.month)}` : "No tokens in this range";
  return { bars, label, ticks };
}

// What the chart draws for a report: days for a week or a month (a slot for every calendar day, with a few dates under them), months for a year.
export function chartData(report: Pick<UsageReport, "from" | "to" | "timeZone" | "daily">, range: RangeKind): { bars: Bar[]; label: string; ticks: { index: number; label: string }[]; unit: "day" | "month" } {
  const days = paddedDaily(report);
  if (range === "year") return { ...monthlyBars(days), unit: "month" };
  return { ...dailyBars(days), ticks: dayTicks(days.map((day) => day.day)), unit: "day" };
}

export type Row = { key: string; name: string; aggregate: UsageAggregate };

// The breakdown rows, busiest first, with a name for something that has been deleted.
export function breakdownRows(report: UsageReport, by: By, bots: Pick<Bot, "id" | "name">[], accounts: Pick<AccountInfo, "id" | "label">[]): Row[] {
  const rows: Row[] =
    by === "bot"
      ? report.byBot.map((row) => ({ key: row.botId, name: bots.find((bot) => bot.id === row.botId)?.name ?? "Deleted bot", aggregate: row }))
      : by === "account"
        ? report.byAccount.map((row) => ({ key: row.accountId, name: accounts.find((account) => account.id === row.accountId)?.label ?? "Removed account", aggregate: row }))
        : report.byModel.map((row) => ({ key: `${row.provider}:${row.model}`, name: row.model, aggregate: row }));
  return rows.sort((a, b) => b.aggregate.tokens.total - a.aggregate.tokens.total);
}

// An account card's usage, "12.4K tokens · $3.10 API value": a subscription shows what its tokens would have cost on the API, an API
// key what it spent. Never one added to the other.
export function accountUsageText(aggregate: UsageAggregate, kind: AccountInfo["kind"]): string {
  const subscription = kind === "subscription";
  const cost = costText(subscription ? aggregate.costs.apiValue : aggregate.costs.apiSpend);
  return `${tokensText(aggregate.tokens.total)} tokens · ${cost.text} ${subscription ? "API value" : "spent"}`;
}

// A bot's day for its panel: "Today: 12.4K tokens · $1.20" (the cost of whichever kind it has, or $0.00).
export function botDayText(aggregate: UsageAggregate): string {
  const lines = costLines(aggregate);
  return `Today: ${tokensText(aggregate.tokens.total)} tokens · ${lines.length === 0 ? usd(0) : lines.map((line) => labelled(line.label, line.text)).join(" · ")}`;
}

// What was read, shown only for the spec it answered: another range or filter has no report (not the last one's) until its own arrives.
export function matching<T>(held: { key: string; value: T } | null, key: string | null): T | null {
  return held && held.key === key ? held.value : null;
}

// Runs `run` at most once per `ms` while poked, never later than `ms` after the first poke: more pokes while one is waiting change nothing,
// so a steady stream of events can't push the run off for ever (a debounce would).
export function throttled(run: () => void, ms: number): { poke: () => void; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return {
    poke: () => {
      timer ??= setTimeout(() => {
        timer = undefined;
        run();
      }, ms);
    },
    cancel: () => {
      clearTimeout(timer);
      timer = undefined;
    },
  };
}

export const emptyUsage = (report: UsageReport): boolean => report.summary.records === 0;
