import type { Provider } from "@tjommie/protocol";

export const PRICING_REVISION = "openai-standard-text-2026-10-22";
export const PRICING_SOURCE = "https://developers.openai.com/api/docs/pricing";
type Rate = { input: number; cachedInput: number; output: number; longContext: boolean };
// USD per million text tokens, checked against the official model pages on 2026-10-22.
// Exact IDs only: a future alias or model must not silently inherit an older price.
const rates: Readonly<Record<string, Rate>> = {
  "gpt-6.1-sol": { input: 2, cachedInput: 0.1, output: 10, longContext: true },
  "gpt-6-luna": { input: 0.1, cachedInput: 0.01, output: 0.5, longContext: true },
  "gpt-6-astra": { input: 10, cachedInput: 1, output: 50, longContext: true },
  "gpt-5.4": { input: 2.5, cachedInput: 0.25, output: 15, longContext: true },
  "gpt-5.3-codex": { input: 1.75, cachedInput: 0.175, output: 14, longContext: false },
};

export function hasPricing(provider: Provider, model: string): boolean {
  return provider === "codex" && Object.hasOwn(rates, model);
}

/** A local text-token estimate, never an invoice or a guaranteed billing ceiling.
 * CLI turn totals lack request-level context, cache-write duration, service tier,
 * regional premiums and tool fees. The long-context tier is conservative when a
 * turn aggregates several short requests. Additional provider charges may remain.
 */
export function priceUsage(provider: Provider, model: string, usage: {
  input: number; cachedInput: number; output: number; reasoning: number;
}): { costUsd: number; pricingRevision: string } | null {
  if (!hasPricing(provider, model) || !Object.values(usage).every((value) => Number.isSafeInteger(value) && value >= 0)) return null;
  const rate = rates[model];
  const long = rate.longContext && usage.input + usage.cachedInput > 272_000;
  const inputFactor = long ? 2 : 1, outputFactor = long ? 1.5 : 1;
  // input is already uncached. Reasoning is a subset of output, not an extra charge.
  const costUsd = ((usage.input * rate.input + usage.cachedInput * rate.cachedInput) * inputFactor + usage.output * rate.output * outputFactor) / 1_000_000;
  if (!Number.isFinite(costUsd)) return null;
  return { costUsd, pricingRevision: PRICING_REVISION };
}
