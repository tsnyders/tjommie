import type { AccountRuntime } from "./account-runtime.ts";
import { priceUsage } from "./account-pricing.ts";
import { getStoredBot } from "./bots.ts";
import { transaction, type Db } from "./db.ts";
import type { ReviewContext } from "./project-review.ts";
import type { ReviewAccountSelection } from "./project-review-runtime.ts";
import type { Broadcast } from "./server.ts";
import { recordUsage } from "./usage.ts";

export function reviewAccountResolver(db: Db, accounts: AccountRuntime, broadcast: Broadcast) {
  return async (context: ReviewContext, signal: AbortSignal): Promise<ReviewAccountSelection> => {
    signal.throwIfAborted();
    const selection = accounts.inspect(getStoredBot(db, context.botId));
    if (!selection.ready) return { unavailable: selection.reason };
    const identity = selection.context;
    if (identity.provider !== "claude" || identity.kind !== "api_key" || identity.usingFallback) {
      return { unavailable: "Enforced review currently requires an explicitly assigned Claude API account. Subscription and Codex review isolation is unavailable." };
    }
    const prepared = await accounts.prepare(identity);
    try {
      signal.throwIfAborted();
      accounts.assertCurrent(identity);
      let released = false;
      return {
        provider: identity.provider, model: identity.model, kind: identity.kind,
        authentication: prepared.authentication,
        assertCurrent: () => { signal.throwIfAborted(); accounts.assertCurrent(identity); },
        getMaxBudgetUsd: () => {
          accounts.assertCurrent(identity);
          const budget = accounts.info(identity.effectiveAccountId).spending;
          return budget.capUsd === null ? undefined : Math.max(0, budget.capUsd - budget.knownUsd);
        },
        onEvent: (event) => {
          if (event.type === "limit") { accounts.observeLimit(identity, event); return; }
          if (event.type !== "usage") return;
          const estimated = event.costUsd === null ? priceUsage(identity.provider, event.model, event) : null;
          let inserted = false;
          transaction(db, () => {
            inserted = recordUsage(db, { ...event, botId: identity.botId, accountId: identity.effectiveAccountId, provider: identity.provider,
              ...(event.eventId ? { id: `review:${identity.runId}:${event.eventId}` } : {}),
              costUsd: event.costUsd ?? estimated?.costUsd ?? null,
              costSource: event.costUsd !== null ? "reported" : estimated ? "estimated" : "unknown",
              pricingRevision: estimated?.pricingRevision ?? null });
          });
          if (inserted) {
            accounts.usageChanged(identity.effectiveAccountId);
            broadcast("usage.changed", { accountId: identity.effectiveAccountId, botId: identity.botId });
          }
        },
        release: () => { if (!released) { released = true; accounts.release(identity); } },
      };
    } catch (error) { accounts.release(identity); throw error; }
  };
}
