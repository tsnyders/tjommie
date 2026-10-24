import { z } from "zod";
import { Provider, ProviderSetup } from "./model.ts";

export const AccountKind = z.enum(["subscription", "api_key"]);
export type AccountKind = z.infer<typeof AccountKind>;

const Id = z.string().min(1).max(256);
const Money = z.number().nonnegative();
const Cap = z.number().positive().max(1_000_000);
const Count = z.number().int().nonnegative();
const ApiKey = z.string().trim().min(1).max(2560).refine((value) => new TextEncoder().encode(value).length <= 2560 && !/[\x00-\x20\x7f]/.test(value), "Expected one API key of at most 2560 UTF-8 bytes");
const Instant = z.string().datetime({ offset: true });
const TimeZone = z.string().min(1).max(100).refine((value) => {
  try { new Intl.DateTimeFormat("en", { timeZone: value }); return true; }
  catch { return false; }
}, "Expected an IANA time zone");

export const AccountLimitWindow = z.object({
  accountId: Id,
  limitId: z.string().min(1).max(256),
  windowKey: z.string().min(1).max(100),
  durationMinutes: z.number().positive().nullable(),
  usedPercent: z.number().min(0).max(100).nullable(),
  resetsAt: Instant.nullable(),
  state: z.enum(["ok", "near", "reached"]),
  observedAt: Instant,
});
export type AccountLimitWindow = z.infer<typeof AccountLimitWindow>;

export const AccountSpending = z.object({
  knownUsd: Money,
  unknownRows: Count,
  capUsd: Cap.nullable(),
  blocked: z.boolean(),
});
export type AccountSpending = z.infer<typeof AccountSpending>;

export const AccountSignIn = z.object({
  attempt: Id,
  state: z.enum(["waiting", "failed"]),
  message: z.string().max(1000).optional(),
});
export type AccountSignIn = z.infer<typeof AccountSignIn>;

// Provider-reported allowance metadata. This is separate from recorded token costs
// and local API spending caps; in particular, provider credits are not dollars.
export const AccountUsageWindow = z.object({
  id: z.string().min(1).max(256),
  label: z.string().min(1).max(120),
  scope: z.object({ kind: z.enum(["model", "surface"]), id: z.string().max(256).nullable(), label: z.string().min(1).max(120) }).nullable(),
  durationMinutes: z.number().positive().nullable(),
  usedPercent: z.number().min(0).max(100).nullable(),
  resetsAt: Instant.nullable(),
});
export type AccountUsageWindow = z.infer<typeof AccountUsageWindow>;

export const AccountProviderUsage = z.object({
  status: z.enum(["available", "unavailable", "error"]),
  checkedAt: Instant.nullable(),
  lastSuccessfulAt: Instant.nullable(),
  error: z.string().max(1000).nullable(),
  planLabel: z.string().max(120).nullable(),
  windows: z.array(AccountUsageWindow).max(64),
  credits: z.object({
    balance: z.string().max(100).regex(/^-?\d+(?:\.\d+)?$/).nullable(),
    hasCredits: z.boolean().nullable(),
    unlimited: z.boolean().nullable(),
  }).nullable(),
  extraUsageEnabled: z.boolean().nullable(),
});
export type AccountProviderUsage = z.infer<typeof AccountProviderUsage>;

// Public metadata only. Credential references, folders and secrets stay in the daemon.
export const AccountInfo = z.object({
  id: Id,
  provider: Provider,
  label: z.string(),
  kind: AccountKind,
  isDefault: z.boolean(),
  isFallback: z.boolean(),
  spendingCapUsd: Cap.nullable(),
  createdAt: Instant,
  hasCredential: z.boolean(),
  setup: ProviderSetup.nullable(),
  limits: z.array(AccountLimitWindow),
  signIn: AccountSignIn.nullable(),
  spending: AccountSpending,
  providerUsage: AccountProviderUsage.optional(), // Older snapshots have no live provider reading.
});
export type AccountInfo = z.infer<typeof AccountInfo>;

const createFields = { provider: Provider, label: z.string().trim().min(1).max(60) };
export const AccountInput = z.discriminatedUnion("kind", [
  z.strictObject({ ...createFields, kind: z.literal("subscription") }),
  z.strictObject({ ...createFields, kind: z.literal("api_key"), apiKey: ApiKey, spendingCapUsd: Cap, isFallback: z.boolean().optional() }),
]);
export type AccountInput = z.infer<typeof AccountInput>;
export const AccountPatch = z.strictObject({
  label: createFields.label.optional(),
  isDefault: z.boolean().optional(),
  isFallback: z.boolean().optional(),
  spendingCapUsd: Cap.optional(),
  apiKey: ApiKey.optional(),
});
export type AccountPatch = z.infer<typeof AccountPatch>;

export const UsageTokens = z.object({ input: Count, cachedInput: Count, output: Count, reasoning: Count, total: Count });
export type UsageTokens = z.infer<typeof UsageTokens>;
export const UsageCost = z.object({
  usd: Money.nullable(),
  knownUsd: Money.nullable(),
  knownRecords: Count,
  unknownRecords: Count,
  reportedUsd: Money.nullable(),
  estimatedUsd: Money.nullable(),
});
export type UsageCost = z.infer<typeof UsageCost>;
export const UsageAggregate = z.object({
  records: Count,
  tokens: UsageTokens,
  costs: z.object({ apiSpend: UsageCost, apiValue: UsageCost, unclassified: UsageCost }),
});
export type UsageAggregate = z.infer<typeof UsageAggregate>;
export const UsageQuery = z.strictObject({
  from: Instant,
  to: Instant,
  timeZone: TimeZone.default("UTC"),
  botId: Id.optional(),
  accountId: Id.optional(),
  provider: Provider.optional(),
  model: z.string().min(1).max(256).optional(),
}).refine((value) => {
  const duration = Date.parse(value.to) - Date.parse(value.from);
  return duration > 0 && duration <= 366 * 86_400_000;
}, "Expected a positive range of at most 366 days");
export type UsageQuery = z.infer<typeof UsageQuery>;
export const UsageReport = z.object({
  from: Instant,
  to: Instant,
  timeZone: TimeZone,
  summary: UsageAggregate,
  daily: z.array(UsageAggregate.extend({ day: z.string().regex(/^\d{4}-\d{2}-\d{2}$/) })),
  byBot: z.array(UsageAggregate.extend({ botId: Id })),
  byAccount: z.array(UsageAggregate.extend({ accountId: Id })),
  byModel: z.array(UsageAggregate.extend({ provider: Provider, model: z.string() })),
});
export type UsageReport = z.infer<typeof UsageReport>;

export const accountMethods = {
  "accounts.list": { params: z.strictObject({}), result: z.array(AccountInfo) },
  "accounts.create": { params: AccountInput, result: AccountInfo },
  "accounts.update": { params: z.strictObject({ id: Id, patch: AccountPatch }), result: AccountInfo },
  "accounts.remove": { params: z.strictObject({ id: Id }), result: z.object({}) },
  "accounts.check": { params: z.strictObject({ id: Id.optional() }), result: z.array(AccountInfo) },
  "accounts.retryLimit": { params: z.strictObject({ id: Id }), result: AccountInfo },
  "accounts.signIn": { params: z.strictObject({ id: Id }), result: z.object({ url: z.string().nullable(), attempt: Id }) },
  "accounts.cancelSignIn": { params: z.strictObject({ id: Id, attempt: Id }), result: z.object({}) },
  "accounts.prepareCodexSandbox": { params: z.strictObject({ id: Id }), result: AccountInfo },
  "usage.query": { params: UsageQuery, result: UsageReport },
} as const;
export const accountEvents = {
  "accounts.changed": z.array(AccountInfo),
  "accounts.signInEnded": z.object({ accountId: Id, attempt: Id }),
  "usage.changed": z.object({ accountId: Id, botId: Id }),
} as const;
