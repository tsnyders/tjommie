import { expect, it } from "vitest";
import { hasPricing, priceUsage, PRICING_REVISION } from "./account-pricing.ts";

it("prices separate uncached and cached input without double-counting reasoning", () => {
  const result = priceUsage("codex", "gpt-5.3-codex", { input: 100_000, cachedInput: 200_000, output: 10_000, reasoning: 8000 });
  expect(result?.costUsd).toBeCloseTo(0.175 + 0.035 + 0.14, 10);
  expect(result?.pricingRevision).toBe(PRICING_REVISION);
  expect(priceUsage("codex", "gpt-5.3-codex", { input: 100_000, cachedInput: 200_000, output: 10_000, reasoning: 0 })).toEqual(result);
});

it("applies the published long-context tier after the threshold", () => {
  expect(priceUsage("codex", "gpt-6.1-sol", { input: 200_000, cachedInput: 100_000, output: 10_000, reasoning: 0 })?.costUsd).toBeCloseTo((0.4 + 0.01) * 2 + 0.1 * 1.5, 10);
  expect(priceUsage("codex", "gpt-6.1-sol", { input: 172_000, cachedInput: 100_000, output: 0, reasoning: 0 })?.costUsd).toBeCloseTo(0.344 + 0.01, 10);
});

it("does not guess prices for aliases, another provider or unknown models", () => {
  const usage = { input: 1, cachedInput: 0, output: 1, reasoning: 0 };
  for (const model of ["future-model", "gpt-6.1-sol-latest", "constructor", "__proto__"]) {
    expect(hasPricing("codex", model)).toBe(false);
    expect(priceUsage("codex", model, usage)).toBeNull();
  }
  expect(priceUsage("claude", "gpt-6.1-sol", usage)).toBeNull();
});

it("rejects invalid token counts while preserving genuinely zero cost", () => {
  const zero = { input: 0, cachedInput: 0, output: 0, reasoning: 0 };
  expect(priceUsage("codex", "gpt-6-luna", zero)?.costUsd).toBe(0);
  for (const input of [-1, 0.5, Number.NaN, Number.POSITIVE_INFINITY]) expect(priceUsage("codex", "gpt-6-luna", { ...zero, input })).toBeNull();
});
