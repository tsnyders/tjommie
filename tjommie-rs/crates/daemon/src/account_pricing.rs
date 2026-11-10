//! Local token prices for API-key accounts. T3.4 ports the rest of `account-pricing.ts` (the rates and `priceUsage`)
//! with its tests.

use protocol::model::Provider;

/// Exact IDs only: a future alias or model must not silently inherit an older price.
const PRICED_CODEX_MODELS: [&str; 5] = [
    "gpt-6.1-sol",
    "gpt-6-luna",
    "gpt-6-astra",
    "gpt-5.4",
    "gpt-5.3-codex",
];

pub fn has_pricing(provider: Provider, model: &str) -> bool {
    provider == Provider::Codex && PRICED_CODEX_MODELS.contains(&model)
}
