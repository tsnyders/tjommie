import { afterEach, beforeEach, expect, it } from "vitest";
import { createAccount, removeAccount } from "./accounts-store.ts";
import { UsageReport } from "@tjommie/protocol";
import { openDb, type Db } from "./db.ts";
import { queryUsage, recordUsage, type UsageInput } from "./usage.ts";
let db: Db;
beforeEach(() => {
  db = openDb(":memory:");
  createAccount(db, { id: "sub", provider: "codex", label: "Subscription", kind: "subscription", folder: "test/sub" });
  createAccount(db, { id: "key", provider: "codex", label: "API", kind: "api_key", folder: "test/key", spendingCapUsd: 10 });
});
afterEach(() => { db.close(); });
const input = (extra: Partial<UsageInput> = {}): UsageInput => ({ botId: "bot", accountId: "sub", provider: "codex", model: "model", input: 10, cachedInput: 5, output: 3, reasoning: 2, costUsd: null, costSource: "unknown", createdAt: "2026-10-22T23:30:00.000Z", ...extra });
const range = { from: "2026-10-22T00:00:00Z", to: "2026-10-24T00:00:00Z" };

it("separates subscription value from API spend and never converts unknown into zero", () => {
  recordUsage(db, input({ costUsd: 2, costSource: "estimated" }));
  recordUsage(db, input({ accountId: "key", costUsd: 0, costSource: "reported" }));
  recordUsage(db, input({ accountId: "key" }));
  const result = queryUsage(db, range);
  expect(result.summary.tokens).toEqual({ input: 30, cachedInput: 15, output: 9, reasoning: 6, total: 54 });
  expect(result.summary.costs.apiValue).toMatchObject({ usd: 2, estimatedUsd: 2 });
  expect(result.summary.costs.apiSpend).toMatchObject({ usd: null, knownUsd: 0, unknownRecords: 1 });
  expect(result.byAccount).toHaveLength(2);
  expect(queryUsage(db, { ...range, accountId: "sub" }).summary.records).toBe(1);
});

it("uses local date buckets through DST and inclusive/exclusive UTC boundaries", () => {
  for (const createdAt of ["2026-11-01T05:30:00Z", "2026-11-01T06:30:00Z", "2026-11-02T05:00:00Z"]) recordUsage(db, input({ createdAt }));
  const result = queryUsage(db, { from: "2026-11-01T05:30:00Z", to: "2026-11-02T05:00:00Z", timeZone: "America/New_York" });
  expect(result.daily).toMatchObject([{ day: "2026-11-01", records: 2 }]);
  expect(queryUsage(db, { ...range, timeZone: "Africa/Johannesburg" }).daily).toEqual([]);
  recordUsage(db, input());
  expect(queryUsage(db, { ...range, timeZone: "Africa/Johannesburg" }).daily[0].day).toBe("2026-10-23");
});

it("validates timezones, ranges, counts and cost provenance", () => {
  expect(() => queryUsage(db, { ...range, timeZone: "Mars/Olympus" })).toThrow();
  expect(() => queryUsage(db, { from: range.to, to: range.from })).toThrow();
  expect(() => queryUsage(db, { from: "2020-01-01T00:00:00Z", to: range.to })).toThrow();
  expect(() => queryUsage(db, { ...range, from: "2026-10-22" })).toThrow();
  expect(() => recordUsage(db, input({ input: -1 }))).toThrow();
  expect(() => recordUsage(db, input({ costUsd: NaN, costSource: "reported" }))).toThrow();
  expect(() => recordUsage(db, input({ costUsd: 2, costSource: "unknown" }))).toThrow();
});

it("leaves transaction ownership with its caller", () => {
  db.exec("BEGIN");
  recordUsage(db, input());
  db.exec("ROLLBACK");
  expect(queryUsage(db, range).summary.records).toBe(0);
});

it("deduplicates stable event identities but rejects conflicting deliveries", () => {
  expect(recordUsage(db, input({ id: "event" }))).toBe(true);
  expect(recordUsage(db, input({ id: "event" }))).toBe(false);
  expect(() => recordUsage(db, input({ id: "event", input: 20 }))).toThrow(/Conflicting/);
  expect(queryUsage(db, range).summary.records).toBe(1);
});

it("preserves unclassified legacy history and absent pricing without making up costs", () => {
  db.prepare(`INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at)
    VALUES ('legacy', 'deleted', 'missing', 'claude', 'unknown-model', 1, 0, 2, 1, NULL, 'unknown', '2026-10-22T12:00:00Z')`).run();
  recordUsage(db, input());
  expect(db.prepare("SELECT pricing_revision FROM usage WHERE account_id = 'sub'").get()).toEqual({ pricing_revision: null });
  removeAccount(db, "sub");
  recordUsage(db, input({ id: "late-event" })); // account identity survives retirement
  const result = queryUsage(db, range);
  expect(UsageReport.safeParse(result).success).toBe(true);
  expect(result.summary.costs.unclassified).toEqual({ usd: null, knownUsd: null, knownRecords: 0, unknownRecords: 1, reportedUsd: null, estimatedUsd: null });
  expect(result.summary.costs.apiValue.unknownRecords).toBe(2);
  expect(result.byBot.map((group) => group.botId)).toContain("deleted");
});

it("does not round tiny measured costs to zero and applies every grouping filter", () => {
  recordUsage(db, input({ costUsd: 0.000000123, costSource: "reported", pricingRevision: null }));
  recordUsage(db, input({ botId: "other", accountId: "key", model: "other-model", costUsd: 1, costSource: "estimated", pricingRevision: "test-prices" }));
  const result = queryUsage(db, { ...range, botId: "bot", provider: "codex", model: "model" });
  expect(result.summary.records).toBe(1);
  expect(result.summary.costs.apiValue.reportedUsd).toBe(0.000000123);
  expect(result.byModel).toMatchObject([{ provider: "codex", model: "model", records: 1 }]);
  expect(result.summary.costs.apiSpend.knownUsd).toBeNull();
  expect(queryUsage(db, { ...range, provider: "claude" }).summary.records).toBe(0);
});
