import type { UsageAggregate, UsageCost, UsageReport } from "@tjommie/protocol";
import { describe, expect, it, vi } from "vitest";
import { untilUsageDay, usageDayKey } from "./usage";
import { chartData, monthLabel, monthlyBars, combinedCost, dayLabel, dayTicks, daysBetween, matching, paddedDaily, throttled, usageHref, accountUsageText, botDayText, breakdownRows, costLines, costText, dailyBars, emptyUsage, filtersFrom, rangeFor, summaryLine, tokensText, usageQuery } from "./usage";

const cost = (patch: Partial<UsageCost> = {}): UsageCost => ({ usd: 0, knownUsd: 0, knownRecords: 0, unknownRecords: 0, reportedUsd: null, estimatedUsd: null, ...patch });

describe("usage calendar refresh", () => {
  it("changes identity at local midnight and on timezone changes", () => {
    const before = new Date("2026-10-22T21:59:59.000Z");
    expect(untilUsageDay(before, "Africa/Johannesburg")).toBe(1001);
    expect(usageDayKey(before, "Africa/Johannesburg")).not.toBe(usageDayKey(new Date(before.getTime() + 1000), "Africa/Johannesburg"));
    expect(usageDayKey(before, "UTC")).not.toBe(usageDayKey(before, "Africa/Johannesburg"));
  });
  it("schedules midnight across 23-hour and 25-hour daylight saving days", () => {
    expect(untilUsageDay(new Date("2026-03-08T05:00:00Z"), "America/New_York")).toBe(23 * 3600000 + 1);
    expect(untilUsageDay(new Date("2026-11-01T04:00:00Z"), "America/New_York")).toBe(25 * 3600000 + 1);
  });
});
const aggregate = (total: number, costs: Partial<UsageAggregate["costs"]> = {}): UsageAggregate => ({
  records: total > 0 ? 1 : 0,
  tokens: { input: total, cachedInput: 0, output: 0, reasoning: 0, total },
  costs: { apiSpend: cost(), apiValue: cost(), unclassified: cost(), ...costs },
});

describe("rangeFor", () => {
  it("starts today at the zone's midnight, not UTC's", () => {
    // 2026-10-22 01:30 in Johannesburg (UTC+2) is still the 21st in UTC.
    const now = new Date("2026-10-21T23:30:00.000Z");
    expect(rangeFor("today", now, "Africa/Johannesburg")).toEqual({ from: "2026-10-21T22:00:00.000Z", to: now.toISOString() });
    expect(rangeFor("today", now, "UTC").from).toBe("2026-10-21T00:00:00.000Z");
  });

  it("starts the year on January 1st at the zone's midnight, to now", () => {
    const now = new Date("2026-10-22T10:00:00.000Z");
    expect(rangeFor("year", now, "UTC")).toEqual({ from: "2026-01-01T00:00:00.000Z", to: now.toISOString() });
    expect(rangeFor("year", now, "Africa/Johannesburg").from).toBe("2025-12-31T22:00:00.000Z");
    expect(rangeFor("year", now, "America/New_York").from).toBe("2026-01-01T05:00:00.000Z"); // EST, not the October offset
    expect(rangeFor("year", new Date("2026-01-01T00:30:00.000Z"), "America/New_York").from).toBe("2025-01-01T05:00:00.000Z"); // still the 31st there
    const span = Date.parse(rangeFor("year", new Date("2026-12-31T23:00:00.000Z"), "UTC").to) - Date.parse(rangeFor("year", new Date("2026-12-31T23:00:00.000Z"), "UTC").from);
    expect(span).toBeLessThanOrEqual(366 * 86_400_000); // inside what the daemon takes
  });

  it("starts the week on Monday and the month on the 1st", () => {
    const now = new Date("2026-10-22T10:00:00.000Z"); // a Thursday
    expect(rangeFor("week", now, "UTC").from).toBe("2026-10-19T00:00:00.000Z");
    expect(rangeFor("month", now, "UTC").from).toBe("2026-10-01T00:00:00.000Z");
    expect(rangeFor("week", new Date("2026-10-18T10:00:00.000Z"), "UTC").from).toBe("2026-10-12T00:00:00.000Z"); // a Sunday belongs to the week before
  });

  it("lands on local midnight across a daylight saving change", () => {
    // New York: clocks went back on 2026-11-01, so Nov 1 is a 25-hour day and the month starts at EDT midnight.
    const now = new Date("2026-11-03T15:00:00.000Z");
    expect(rangeFor("month", now, "America/New_York").from).toBe("2026-11-01T04:00:00.000Z");
    expect(rangeFor("today", now, "America/New_York").from).toBe("2026-11-03T05:00:00.000Z"); // EST by then
    // Spring forward on 2026-03-08: midnight of that day is still EST, the next day's is EDT.
    expect(rangeFor("today", new Date("2026-03-08T20:00:00.000Z"), "America/New_York").from).toBe("2026-03-08T05:00:00.000Z");
    expect(rangeFor("today", new Date("2026-03-09T20:00:00.000Z"), "America/New_York").from).toBe("2026-03-09T04:00:00.000Z");
  });

  it("builds a query with the zone and only the filters set", () => {
    const now = new Date("2026-10-22T10:00:00.000Z");
    expect(usageQuery("today", {}, now, "UTC")).toEqual({ from: "2026-10-22T00:00:00.000Z", to: now.toISOString(), timeZone: "UTC" });
    expect(usageQuery("today", { bot: "b1", account: "a1", model: "m" }, now, "UTC")).toMatchObject({ botId: "b1", accountId: "a1", model: "m" });
  });

  it("writes the address for some filters, and reads it back", () => {
    expect(usageHref({})).toBe("/settings/?section=usage");
    const href = usageHref({ bot: "b 1", model: "gpt" });
    expect(href).toBe("/settings/?section=usage&bot=b+1&model=gpt");
    expect(filtersFrom(new URL(href, "http://x").searchParams)).toEqual({ bot: "b 1", model: "gpt" });
  });

  it("reads the filters from the address, ignoring empty ones", () => {
    const params = new URLSearchParams("section=usage&bot=b1&account=&model=gpt");
    expect(filtersFrom(params)).toEqual({ bot: "b1", model: "gpt" });
  });
});

describe("tokensText and costText", () => {
  it("shortens counts: 950, 12.4K, 1.2M, 3.4B", () => {
    expect([950, 12_400, 1_200_000, 3_400_000_000, 999_999].map(tokensText)).toEqual(["950", "12.4K", "1.2M", "3.4B", "1M"]);
  });

  it("is $0.00 only when there is nothing, and never shows an unknown cost as $0", () => {
    expect(costText(cost())).toEqual({ text: "$0.00", note: null });
    expect(costText(cost({ usd: 1.2, knownRecords: 3 }))).toEqual({ text: "$1.20", note: null });
    expect(costText(cost({ usd: 1.2, knownRecords: 3, estimatedUsd: 0.4 }))).toEqual({ text: "$1.20", note: "Includes estimates" });
    expect(costText(cost({ usd: null, knownUsd: 0.5, knownRecords: 1, unknownRecords: 2 }))).toEqual({ text: "At least $0.50", note: "2 turns have no price" });
    expect(costText(cost({ usd: null, knownUsd: null, unknownRecords: 4 }))).toEqual({ text: "Unknown", note: null });
  });

  it("keeps spend and API value on lines of their own and never adds them", () => {
    const both = aggregate(1000, { apiSpend: cost({ usd: 2, knownRecords: 1 }), apiValue: cost({ usd: 7, knownRecords: 2 }) });
    expect(costLines(both).map((line) => `${line.label} ${line.text}`)).toEqual(["Spent $2.00", "API value $7.00"]);
    expect(costLines(aggregate(0))).toEqual([]);
    expect(summaryLine(both)).toBe("1K tokens · $2.00 spent · $7.00 api value");
    expect(summaryLine(aggregate(0))).toBe("0 tokens · $0.00");
  });
});

describe("bars and breakdown", () => {
  it("sizes each day against the busiest, and says the highest in words", () => {
    const days = [{ day: "2026-10-20", ...aggregate(500) }, { day: "2026-10-21", ...aggregate(2000) }, { day: "2026-10-22", ...aggregate(0) }];
    const { bars, label } = dailyBars(days);
    expect(bars.map((bar) => bar.share)).toEqual([0.25, 1, 0]);
    expect(label).toBe("Tokens per day, highest 2K on 2026-10-21");
    expect(dailyBars([]).bars).toEqual([]);
    expect(dailyBars([{ day: "2026-10-22", ...aggregate(0) }]).label).toBe("No tokens in this range");
  });

  it("lists rows busiest first, naming what was deleted", () => {
    const report = {
      byBot: [{ botId: "gone", ...aggregate(10) }, { botId: "b1", ...aggregate(900) }],
      byAccount: [{ accountId: "a9", ...aggregate(5) }, { accountId: "a1", ...aggregate(50) }],
      byModel: [{ provider: "claude", model: "sonnet", ...aggregate(7) }],
    } as unknown as UsageReport;
    expect(breakdownRows(report, "bot", [{ id: "b1", name: "Scout" }], []).map((row) => row.name)).toEqual(["Scout", "Deleted bot"]);
    expect(breakdownRows(report, "account", [], [{ id: "a1", label: "Personal" }]).map((row) => row.name)).toEqual(["Personal", "Removed account"]);
    expect(breakdownRows(report, "model", [], []).map((row) => row.name)).toEqual(["sonnet"]);
  });

  it("is empty when there are no records", () => {
    expect(emptyUsage({ summary: aggregate(0) } as unknown as UsageReport)).toBe(true);
    expect(emptyUsage({ summary: aggregate(10) } as unknown as UsageReport)).toBe(false);
  });
});

describe("card lines", () => {
  const both = aggregate(12_400, { apiSpend: cost({ usd: 3.1, knownRecords: 1 }), apiValue: cost({ usd: 8, knownRecords: 2 }) });

  it("shows a subscription's API value and an API key's spend, never both", () => {
    expect(accountUsageText(both, "subscription")).toBe("12.4K tokens · $8.00 API value");
    expect(accountUsageText(both, "api_key")).toBe("12.4K tokens · $3.10 spent");
    expect(accountUsageText(aggregate(0), "api_key")).toBe("0 tokens · $0.00 spent");
  });

  it("shows a bot's day with the cost it has, or $0.00 for nothing", () => {
    expect(botDayText(both)).toBe("Today: 12.4K tokens · Spent $3.10 · API value $8.00");
    expect(botDayText(aggregate(900, { apiValue: cost({ usd: 4, knownRecords: 1 }) }))).toBe("Today: 900 tokens · API value $4.00"); // a lone subscription value is still labelled
    expect(botDayText(aggregate(0))).toBe("Today: 0 tokens · $0.00");
  });
});

describe("matching", () => {
  it("shows a held report only for the spec it answered", () => {
    const held = { key: "week", value: "report" };
    expect(matching(held, "week")).toBe("report");
    expect(matching(held, "month")).toBeNull(); // another range doesn't show the last one's totals
    expect(matching(null, "week")).toBeNull();
    expect(matching(held, null)).toBeNull();
  });
});

describe("throttled", () => {
  it("runs once per interval under a steady stream of pokes, never pushed off", () => {
    vi.useFakeTimers();
    try {
      const run = vi.fn();
      const refresh = throttled(run, 1000);
      for (let ms = 0; ms < 3500; ms += 100) {
        refresh.poke();
        vi.advanceTimersByTime(100);
      }
      expect(run).toHaveBeenCalledTimes(3); // a debounce would not have run at all
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not run without a poke, and cancel drops a waiting run", () => {
    vi.useFakeTimers();
    try {
      const run = vi.fn();
      const refresh = throttled(run, 1000);
      vi.advanceTimersByTime(5000);
      expect(run).not.toHaveBeenCalled();
      refresh.poke();
      refresh.cancel();
      vi.advanceTimersByTime(5000);
      expect(run).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("days of a range", () => {
  it("lists every calendar day from the first to the last in the zone, empty ones too", () => {
    expect(daysBetween("2026-10-19T00:00:00.000Z", "2026-10-22T10:00:00.000Z", "UTC")).toEqual(["2026-10-19", "2026-10-20", "2026-10-21", "2026-10-22"]);
    expect(daysBetween("2026-10-22T10:00:00.000Z", "2026-10-22T11:00:00.000Z", "UTC")).toEqual(["2026-10-22"]);
  });

  it("reads the days in the report's zone, not UTC's", () => {
    // 22:00Z on the 21st is already the 22nd in Johannesburg.
    expect(daysBetween("2026-10-21T22:00:00.000Z", "2026-10-22T10:00:00.000Z", "Africa/Johannesburg")).toEqual(["2026-10-22"]);
  });

  it("counts a 25-hour and a 23-hour day as one day each across daylight saving", () => {
    expect(daysBetween("2026-11-01T04:00:00.000Z", "2026-11-03T15:00:00.000Z", "America/New_York")).toEqual(["2026-11-01", "2026-11-02", "2026-11-03"]);
    expect(daysBetween("2026-03-07T05:00:00.000Z", "2026-03-10T20:00:00.000Z", "America/New_York")).toEqual(["2026-03-07", "2026-03-08", "2026-03-09", "2026-03-10"]);
  });

  it("stays bounded for a bad range", () => {
    expect(daysBetween("2020-01-01T00:00:00.000Z", "2030-01-01T00:00:00.000Z", "UTC").length).toBeLessThanOrEqual(400);
    expect(daysBetween("2026-10-22T00:00:00.000Z", "2026-10-01T00:00:00.000Z", "UTC")).toEqual([]);
  });

  it("pads a report's days so one busy day takes its own slot, not the whole range", () => {
    const report = { from: "2026-10-19T00:00:00.000Z", to: "2026-10-25T10:00:00.000Z", timeZone: "UTC", daily: [{ day: "2026-10-21", ...aggregate(900) }] };
    const padded = paddedDaily(report);
    expect(padded.map((day) => day.day)).toEqual(["2026-10-19", "2026-10-20", "2026-10-21", "2026-10-22", "2026-10-23", "2026-10-24", "2026-10-25"]);
    expect(padded.map((day) => day.tokens.total)).toEqual([0, 0, 900, 0, 0, 0, 0]);
    const { bars } = dailyBars(padded);
    expect(bars).toHaveLength(7);
    expect(bars.filter((bar) => bar.share > 0)).toHaveLength(1);
  });

  it("puts modest date ticks under the bars: the first, the last, and a few between", () => {
    const days = daysBetween("2026-10-01T00:00:00.000Z", "2026-10-31T10:00:00.000Z", "UTC");
    const ticks = dayTicks(days);
    expect(ticks[0]).toEqual({ index: 0, label: "Oct 1" });
    expect(ticks[ticks.length - 1]).toEqual({ index: 30, label: "Oct 31" });
    expect(ticks.length).toBeLessThanOrEqual(6);
    expect(dayTicks(["2026-10-22"])).toEqual([{ index: 0, label: "Oct 22" }]);
    expect(dayTicks([])).toEqual([]);
    expect(dayLabel("2026-03-09")).toBe("Mar 9");
  });
});

describe("combinedCost", () => {
  const costs = (spend: Partial<UsageCost>, value: Partial<UsageCost>, other: Partial<UsageCost> = {}) => ({ apiSpend: cost(spend), apiValue: cost(value), unclassified: cost(other) });

  it("adds the known costs of spend, value and unclassified, once each", () => {
    const sum = combinedCost(costs({ usd: 3, knownUsd: 3, knownRecords: 2 }, { usd: 8, knownUsd: 8, knownRecords: 4 }, { usd: 1, knownUsd: 1, knownRecords: 1 }));
    expect(sum).toMatchObject({ usd: 12, knownUsd: 12, knownRecords: 7, unknownRecords: 0 });
    expect(costText(sum).text).toBe("$12.00");
  });

  it("says at least when some turns have no price, and unknown when none do", () => {
    const partial = combinedCost(costs({ usd: null, knownUsd: 3, knownRecords: 2, unknownRecords: 1 }, { usd: 8, knownUsd: 8, knownRecords: 4 }));
    expect(partial).toMatchObject({ usd: null, knownUsd: 11, unknownRecords: 1 });
    expect(costText(partial)).toEqual({ text: "At least $11.00", note: "1 turn has no price" });
    const none = combinedCost(costs({ usd: null, knownUsd: null, unknownRecords: 2 }, { usd: null, knownUsd: null, unknownRecords: 3 }));
    expect(none).toMatchObject({ usd: null, knownUsd: null, unknownRecords: 5 });
    expect(costText(none).text).toBe("Unknown");
  });

  it("is $0.00 for nothing at all, and carries estimates through", () => {
    expect(costText(combinedCost(costs({}, {}))).text).toBe("$0.00");
    const estimated = combinedCost(costs({ usd: 1, knownUsd: 1, knownRecords: 1, estimatedUsd: 0.4 }, { usd: 2, knownUsd: 2, knownRecords: 1, estimatedUsd: 0.1 }));
    expect(estimated.estimatedUsd).toBeCloseTo(0.5);
    expect(costText(estimated)).toEqual({ text: "$3.00", note: "Includes estimates" });
  });
});

describe("a year as months", () => {
  const year = (from: string, to: string, timeZone: string, daily: { day: string; total: number }[]) => ({ from, to, timeZone, daily: daily.map((each) => ({ day: each.day, ...aggregate(each.total) })) });

  it("draws twelve bars for a full year, however many days it has, with every month named", () => {
    const report = year("2026-01-01T00:00:00.000Z", "2026-12-31T23:00:00.000Z", "UTC", [{ day: "2026-03-05", total: 400 }, { day: "2026-10-22", total: 900 }]);
    const { bars, ticks, label, unit } = chartData(report, "year");
    expect(unit).toBe("month");
    expect(bars).toHaveLength(12);
    expect(ticks.map((tick) => tick.label)).toEqual(["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]);
    expect(bars[9].share).toBe(1);
    expect(bars[2].share).toBeCloseTo(400 / 900);
    expect(bars.filter((bar) => bar.share === 0)).toHaveLength(10); // quiet months are still slots
    expect(label).toBe("Tokens per month, highest 900 in Oct");
  });

  it("keeps a leap year's extra day in February and the 31st in December", () => {
    const report = year("2028-01-01T00:00:00.000Z", "2028-12-31T23:30:00.000Z", "UTC", [{ day: "2028-02-29", total: 300 }, { day: "2028-02-28", total: 100 }, { day: "2028-12-31", total: 800 }]);
    const { bars } = chartData(report, "year");
    expect(bars).toHaveLength(12); // 366 days, still twelve slots
    expect(bars[1].share).toBeCloseTo(400 / 800); // Feb 28 + Feb 29
    expect(bars[11].share).toBe(1); // Dec 31 is December's
  });

  it("stops at the month of the range's last day, and reads months in the report's zone", () => {
    // Near the end of the year in Johannesburg the 31st has already come when UTC is still on the 30th, so December is the last slot either way.
    const dec = chartData(year("2026-12-30T22:00:00.000Z", "2026-12-31T21:30:00.000Z", "Africa/Johannesburg", []), "year");
    expect(dec.ticks.map((tick) => tick.label)).toEqual(["Dec"]);
    // New York's first moments of January are still December 31st there, so a range that ends then has no January slot.
    const edge = chartData(year("2026-01-01T05:00:00.000Z", "2027-01-01T04:30:00.000Z", "America/New_York", []), "year");
    expect(edge.bars).toHaveLength(12);
    expect(edge.ticks[edge.ticks.length - 1].label).toBe("Dec");
    // The year range to date: only the months so far.
    const now = new Date("2026-10-22T10:00:00.000Z");
    const range = rangeFor("year", now, "UTC");
    expect(chartData({ ...range, timeZone: "UTC", daily: [] }, "year").bars).toHaveLength(10);
  });

  it("keeps weeks and months as days, with a slot for every day", () => {
    const week = chartData(year("2026-10-19T00:00:00.000Z", "2026-10-25T10:00:00.000Z", "UTC", [{ day: "2026-10-21", total: 900 }]), "week");
    expect(week.unit).toBe("day");
    expect(week.bars).toHaveLength(7);
  });

  it("sums by month without losing or doubling tokens", () => {
    const days = [{ day: "2026-01-30", ...aggregate(10) }, { day: "2026-01-31", ...aggregate(20) }, { day: "2026-02-01", ...aggregate(5) }];
    const { bars } = monthlyBars(days);
    expect(bars.map((bar) => bar.day)).toEqual(["2026-01", "2026-02"]);
    expect(bars.map((bar) => bar.share)).toEqual([1, 5 / 30]);
    expect(monthLabel("2026-11")).toBe("Nov");
    expect(monthlyBars([]).bars).toEqual([]);
  });
});
