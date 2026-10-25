import type { UsageAggregate, UsageCost, UsageReport } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { RangeChips, UsageReportView } from "./usage-section";

vi.mock("@/lib/connection", () => ({ request: vi.fn(), daemon: () => ({ subscribe: () => () => {} }) }));

const cost = (patch: Partial<UsageCost> = {}): UsageCost => ({ usd: 0, knownUsd: 0, knownRecords: 0, unknownRecords: 0, reportedUsd: null, estimatedUsd: null, ...patch });
const aggregate = (total: number, costs: Partial<UsageAggregate["costs"]> = {}): UsageAggregate => ({
  records: total > 0 ? 1 : 0,
  tokens: { input: total, cachedInput: 0, output: 0, reasoning: 0, total },
  costs: { apiSpend: cost(), apiValue: cost(), unclassified: cost(), ...costs },
});
const report = (summary: UsageAggregate, extra: Partial<UsageReport> = {}): UsageReport => ({
  from: "2026-10-19T00:00:00.000Z",
  to: "2026-10-22T10:00:00.000Z",
  timeZone: "UTC",
  summary,
  daily: [{ day: "2026-10-21", ...aggregate(2000) }, { day: "2026-10-22", ...aggregate(500) }],
  byBot: [{ botId: "b1", ...summary }],
  byAccount: [],
  byModel: [],
  ...extra,
});
const view = (usage: UsageReport, range: "today" | "week" | "month" | "year" = "week", by: "bot" | "account" | "model" = "bot") =>
  renderToStaticMarkup(<UsageReportView report={usage} range={range} by={by} onBy={() => {}} bots={[{ id: "b1", name: "Scout" }]} accounts={[]} />);

describe("RangeChips", () => {
  it("is the phone's labelled row of 44 px toggles, with only the current range pressed", () => {
    const out = renderToStaticMarkup(<RangeChips value="month" onChange={() => {}} />);
    expect(out).toContain('role="group"');
    expect(out).toContain('aria-label="Range"');
    expect(out.match(/aria-pressed="true"/g)).toHaveLength(1);
    expect(out).toMatch(/aria-pressed="true"[^>]*>This month</);
    expect(out.match(/h-11/g)).toHaveLength(4);
  });
});

describe("UsageReportView", () => {
  it("keeps tokens, API spend and API value in tiles of their own", () => {
    const out = view(report(aggregate(2500, { apiSpend: cost({ usd: 3.1, knownRecords: 1 }), apiValue: cost({ usd: 8, knownRecords: 2 }) })));
    expect(out).toContain("Tokens");
    expect(out).toContain("2.5K");
    expect(out).toContain("Spent (API keys)");
    expect(out).toContain("$3.10");
    expect(out).toContain("API value (subscriptions)");
    expect(out).toContain("$8.00");
    expect(out).not.toContain("$11.10");
    expect(out).not.toContain(">Other<");
  });

  it("adds a combined cost estimate beside the separate tiles, labelled as API-equivalent value and not a bill", () => {
    const out = view(report(aggregate(2500, { apiSpend: cost({ usd: 3, knownUsd: 3, knownRecords: 1 }), apiValue: cost({ usd: 8, knownUsd: 8, knownRecords: 2 }), unclassified: cost({ usd: 1, knownUsd: 1, knownRecords: 1 }) })));
    expect(out).toContain("Combined cost estimate");
    expect(out).toContain("$12.00");
    expect(out).toContain("API-equivalent value, not your subscription bill");
    expect(out).toContain("Spent (API keys)"); // the separate tiles stay
    expect(out).toContain("API value (subscriptions)");
    expect(out).toContain("$3.00");
    expect(out).toContain("$8.00");
  });

  it("says at least or unknown for the combined figure when prices are missing, and never $0", () => {
    const partial = view(report(aggregate(900, { apiSpend: cost({ usd: null, knownUsd: 0.5, knownRecords: 1, unknownRecords: 3 }), apiValue: cost({ usd: 2, knownUsd: 2, knownRecords: 1 }) })));
    expect(partial).toContain("At least $2.50");
    const unknown = view(report(aggregate(900, { apiSpend: cost({ usd: null, knownUsd: null, unknownRecords: 2 }) })));
    expect(unknown).toMatch(/Combined cost estimate<\/p><p[^>]*>Unknown</);
  });

  it("shows a cost that isn't fully known as such, never as $0", () => {
    const out = view(report(aggregate(900, { apiSpend: cost({ usd: null, knownUsd: 0.5, knownRecords: 1, unknownRecords: 3 }), apiValue: cost({ usd: null, knownUsd: null, unknownRecords: 2 }) })));
    expect(out).toContain("At least $0.50");
    expect(out).toContain("3 turns have no price");
    expect(out).toContain("Unknown");
    expect(out).not.toContain("$0.00");
  });

  it("says estimates are included, and shows Other only when there is something in it", () => {
    expect(view(report(aggregate(900, { apiSpend: cost({ usd: 1, knownRecords: 1, estimatedUsd: 0.4 }) })))).toContain("Includes estimates");
    expect(view(report(aggregate(900, { unclassified: cost({ usd: 2, knownRecords: 1 }) })))).toContain("Other");
  });

  it("draws the days as bars with a sentence for them, but not for a single day", () => {
    const week = view(report(aggregate(2500)));
    expect(week).toContain('role="img"');
    expect(week).toContain("Tokens per day, highest 2K on 2026-10-21");
    expect(week).toContain("Days in UTC");
    expect(week).toContain("Oct 19");
    expect(view(report(aggregate(2500)), "today")).not.toContain('role="img"');
  });

  it("draws a year as twelve month bars, not hundreds of day bars that would overflow a phone", () => {
    const yearReport = report(aggregate(2500), { from: "2026-01-01T00:00:00.000Z", to: "2026-12-31T23:00:00.000Z", daily: [{ day: "2026-10-21", ...aggregate(2000) }] });
    const out = view(yearReport, "year");
    expect(out).toContain("Tokens per month, highest 2K in Oct");
    expect(out).toContain("Months in UTC");
    expect(out.match(/px-0\.5/g)).toHaveLength(12);
    expect(out).not.toContain("px-px");
    for (const month of ["Jan", "Jun", "Dec"]) expect(out).toContain(`>${month}<`);
    expect(view(report(aggregate(2500)), "week")).toContain("Days in UTC");
  });

  it("lists the breakdown by bot with its name and cost lines, and names a deleted bot", () => {
    const usage = report(aggregate(2500, { apiSpend: cost({ usd: 3, knownRecords: 1 }) }), { byBot: [{ botId: "b1", ...aggregate(2000) }, { botId: "gone", ...aggregate(500, { apiSpend: cost({ usd: 3, knownRecords: 1 }) }) }] });
    const out = view(usage);
    expect(out).toContain("Scout");
    expect(out).toContain("Deleted bot");
    expect(out).toContain("Spent $3.00");
    expect(out).toContain("By account");
  });

  it("says there is no usage yet when there are no records", () => {
    expect(view(report(aggregate(0)))).toContain("No usage yet. It shows here after your bots&#x27; first messages.");
  });
});
