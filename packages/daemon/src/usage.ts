import { UsageQuery as QuerySchema, type AccountSpending, type Provider, type UsageAggregate, type UsageCost, type UsageQuery, type UsageReport } from "@tjommie/protocol";
import { newId, nowIso, type Db } from "./db.ts";
import { RpcError } from "./server.ts";
export type { UsageAggregate, UsageCost, UsageQuery, UsageReport } from "@tjommie/protocol";

export type UsageInput = {
  id?: string; botId: string; accountId: string; provider: Provider; model: string;
  input: number; cachedInput: number; output: number; reasoning: number;
  costUsd: number | null; costSource: "unknown" | "reported" | "estimated";
  pricingRevision?: string | null; createdAt?: string;
};
type UsageRow = {
  id: string; bot_id: string; account_id: string; provider: Provider; model: string;
  input: number; cached_input: number; output: number; reasoning: number;
  cost_usd: number | null; cost_source: string | null; created_at: string;
  account_kind: "subscription" | "api_key" | "unknown"; pricing_revision: string | null;
};
const bad = (message: string): never => { throw new RpcError("bad_params", message); };

// Caller owns the transaction, including any session cumulative-cost baseline update.
// A stable provider event id makes a repeated delivery harmless. A conflicting id is an error.
export function recordUsage(db: Db, input: UsageInput): boolean {
  for (const count of [input.input, input.cachedInput, input.output, input.reasoning]) {
    if (!Number.isSafeInteger(count) || count < 0) bad("Usage token counts must be nonnegative safe integers");
  }
  if (!Number.isSafeInteger(input.input + input.cachedInput + input.output)) bad("Usage token total is too large");
  if (input.costUsd !== null && (!Number.isFinite(input.costUsd) || input.costUsd < 0)) bad("Usage cost must be a nonnegative finite amount");
  if (!["unknown", "reported", "estimated"].includes(input.costSource) || (input.costUsd === null) !== (input.costSource === "unknown")) bad("Usage cost provenance must match its amount");
  if (!input.botId.trim() || !input.accountId.trim() || !input.model.trim() || input.id === "") bad("Usage identity is required");
  const account = db.prepare("SELECT provider, kind FROM accounts WHERE id = ?").get(input.accountId) as { provider: Provider; kind: string } | undefined;
  if (!account || account.provider !== input.provider || !["subscription", "api_key"].includes(account.kind)) bad("Usage must identify its actual provider account");
  const createdAt = input.createdAt ?? nowIso();
  if (!/^\d{4}-\d{2}-\d{2}T.+(?:Z|[+-]\d{2}:\d{2})$/.test(createdAt) || !Number.isFinite(Date.parse(createdAt))) bad("Usage timestamp must be an ISO instant");
  const id = input.id ?? newId();
  const row: UsageRow = { id, bot_id: input.botId, account_id: input.accountId, provider: input.provider, model: input.model,
    input: input.input, cached_input: input.cachedInput, output: input.output, reasoning: input.reasoning,
    cost_usd: input.costUsd, cost_source: input.costSource, created_at: new Date(createdAt).toISOString(),
    account_kind: account!.kind as UsageRow["account_kind"], pricing_revision: input.pricingRevision ?? null };
  const existing = db.prepare("SELECT * FROM usage WHERE id = ?").get(id) as UsageRow | undefined;
  if (existing) {
    for (const key of Object.keys(row) as (keyof UsageRow)[]) {
      if (key === "created_at" && input.createdAt === undefined) continue;
      if (existing[key] !== row[key]) throw new RpcError("bad_state", "Conflicting usage event identity");
    }
    return false;
  }
  db.prepare(`INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at, account_kind, pricing_revision)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(row.id, row.bot_id, row.account_id, row.provider, row.model, row.input, row.cached_input,
    row.output, row.reasoning, row.cost_usd, row.cost_source, row.created_at, row.account_kind, row.pricing_revision);
  return true;
}
const emptyCost = (): UsageCost => ({ usd: null, knownUsd: null, knownRecords: 0, unknownRecords: 0, reportedUsd: null, estimatedUsd: null });
const emptyAggregate = (): UsageAggregate => ({ records: 0, tokens: { input: 0, cachedInput: 0, output: 0, reasoning: 0, total: 0 },
  costs: { apiSpend: emptyCost(), apiValue: emptyCost(), unclassified: emptyCost() } });
function knownCost(row: UsageRow): boolean {
  return row.cost_usd !== null && Number.isFinite(row.cost_usd) && row.cost_usd >= 0 && (row.cost_source === "reported" || row.cost_source === "estimated");
}
function add(aggregate: UsageAggregate, row: UsageRow): void {
  aggregate.records++;
  aggregate.tokens.input += row.input;
  aggregate.tokens.cachedInput += row.cached_input;
  aggregate.tokens.output += row.output;
  aggregate.tokens.reasoning += row.reasoning;
  aggregate.tokens.total += row.input + row.cached_input + row.output;
  const cost = aggregate.costs[row.account_kind === "subscription" ? "apiValue" : row.account_kind === "api_key" ? "apiSpend" : "unclassified"];
  if (knownCost(row)) {
    cost.knownRecords++;
    cost.knownUsd = (cost.knownUsd ?? 0) + row.cost_usd!;
    if (row.cost_source === "reported") cost.reportedUsd = (cost.reportedUsd ?? 0) + row.cost_usd!;
    else cost.estimatedUsd = (cost.estimatedUsd ?? 0) + row.cost_usd!;
  } else cost.unknownRecords++;
  cost.usd = cost.unknownRecords > 0 ? null : cost.knownUsd;
}
export function queryUsage(db: Db, input: Omit<UsageQuery, "timeZone"> & { timeZone?: string }): UsageReport {
  const parsed = QuerySchema.safeParse(input);
  if (!parsed.success) bad("Usage query requires valid instants, time zone, and a positive range of at most 366 days");
  const query = parsed.data!;
  const from = new Date(query.from).toISOString();
  const to = new Date(query.to).toISOString();
  // julianday also handles old rows stored with different valid ISO offsets/precision.
  const filters = ["julianday(created_at) >= julianday(?)", "julianday(created_at) < julianday(?)"];
  const values: string[] = [from, to];
  for (const [column, value] of [["bot_id", query.botId], ["account_id", query.accountId], ["provider", query.provider], ["model", query.model]] as const) {
    if (value !== undefined) { filters.push(`${column} = ?`); values.push(value); }
  }
  const rows = db.prepare(`SELECT * FROM usage WHERE ${filters.join(" AND ")} ORDER BY created_at, id`).all(...values) as UsageRow[];
  const formatter = new Intl.DateTimeFormat("en-US", { timeZone: query.timeZone, year: "numeric", month: "2-digit", day: "2-digit" });
  const dayFor = (at: string): string => {
    const parts = formatter.formatToParts(new Date(at));
    return ["year", "month", "day"].map((key) => parts.find((part) => part.type === key)!.value).join("-");
  };
  const summary = emptyAggregate();
  const daily = new Map<string, UsageAggregate>();
  const bots = new Map<string, UsageAggregate>();
  const accounts = new Map<string, UsageAggregate>();
  const models = new Map<string, { provider: Provider; model: string; aggregate: UsageAggregate }>();
  const addGroup = (groups: Map<string, UsageAggregate>, key: string, row: UsageRow): void => {
    let value = groups.get(key);
    if (!value) { value = emptyAggregate(); groups.set(key, value); }
    add(value, row);
  };
  for (const row of rows) {
    add(summary, row);
    addGroup(daily, dayFor(row.created_at), row);
    addGroup(bots, row.bot_id, row);
    addGroup(accounts, row.account_id, row);
    const key = JSON.stringify([row.provider, row.model]);
    let model = models.get(key);
    if (!model) { model = { provider: row.provider, model: row.model, aggregate: emptyAggregate() }; models.set(key, model); }
    add(model.aggregate, row);
  }
  const entries = (groups: Map<string, UsageAggregate>) => [...groups.entries()].sort(([a], [b]) => a.localeCompare(b));
  return { from, to, timeZone: query.timeZone, summary,
    daily: entries(daily).map(([day, aggregate]) => ({ day, ...aggregate })),
    byBot: entries(bots).map(([botId, aggregate]) => ({ botId, ...aggregate })),
    byAccount: entries(accounts).map(([accountId, aggregate]) => ({ accountId, ...aggregate })),
    byModel: [...models.values()].sort((a, b) => a.provider.localeCompare(b.provider) || a.model.localeCompare(b.model)).map(({ provider, model, aggregate }) => ({ provider, model, ...aggregate })),
  };
}
export function accountBudget(db: Db, id: string): AccountSpending {
  const account = db.prepare("SELECT kind, spending_cap_usd FROM accounts WHERE id = ?").get(id) as { kind: string; spending_cap_usd: number | null } | undefined;
  if (!account) throw new RpcError("not_found", "Account not found");
  let knownUsd = 0;
  let unknownRows = 0;
  if (account.kind === "api_key") {
    const rows = db.prepare("SELECT * FROM usage WHERE account_id = ?").all(id) as UsageRow[];
    for (const row of rows) {
      if (row.account_kind === "api_key" && knownCost(row)) knownUsd += row.cost_usd!;
      else unknownRows++;
    }
  }
  const capUsd = account.spending_cap_usd;
  return { knownUsd, unknownRows, capUsd, blocked: account.kind === "api_key" && capUsd !== null && (unknownRows > 0 || knownUsd >= capUsd) };
}
