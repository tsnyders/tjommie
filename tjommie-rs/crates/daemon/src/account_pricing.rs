//! `account-pricing.ts`: local token prices for API-key accounts.

use protocol::model::Provider;

use crate::providers::TokenUsage;

pub const PRICING_REVISION: &str = "openai-standard-text-2026-10-22";
pub const PRICING_SOURCE: &str = "https://developers.openai.com/api/docs/pricing";

/// USD per million text tokens.
struct Rate {
    input: f64,
    cached_input: f64,
    output: f64,
    long_context: bool,
}

/// Checked against the official model pages on 2026-10-22. Exact IDs only: a future alias or model must not silently
/// inherit an older price.
fn rate(model: &str) -> Option<Rate> {
    let (input, cached_input, output, long_context) = match model {
        "gpt-6.1-sol" => (2.0, 0.1, 10.0, true),
        "gpt-6-luna" => (0.1, 0.01, 0.5, true),
        "gpt-6-astra" => (10.0, 1.0, 50.0, true),
        "gpt-5.4" => (2.5, 0.25, 15.0, true),
        "gpt-5.3-codex" => (1.75, 0.175, 14.0, false),
        _ => return None,
    };
    Some(Rate {
        input,
        cached_input,
        output,
        long_context,
    })
}

pub fn has_pricing(provider: Provider, model: &str) -> bool {
    provider == Provider::Codex && rate(model).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricedUsage {
    pub cost_usd: f64,
    pub pricing_revision: &'static str,
}

fn safe_count(value: f64) -> bool {
    value.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&value)
}

/// A local text-token estimate, never an invoice or a guaranteed billing ceiling. CLI turn totals lack request-level
/// context, cache-write duration, service tier, regional premiums and tool fees. The long-context tier is conservative
/// when a turn aggregates several short requests. Additional provider charges may remain.
pub fn price_usage(provider: Provider, model: &str, usage: TokenUsage) -> Option<PricedUsage> {
    if !has_pricing(provider, model)
        || ![
            usage.input,
            usage.cached_input,
            usage.output,
            usage.reasoning,
        ]
        .into_iter()
        .all(safe_count)
    {
        return None;
    }
    let rate = rate(model)?;
    let long = rate.long_context && usage.input + usage.cached_input > 272_000.0;
    let (input_factor, output_factor) = if long { (2.0, 1.5) } else { (1.0, 1.0) };
    // input is already uncached. Reasoning is a subset of output, not an extra charge.
    let cost_usd = ((usage.input * rate.input + usage.cached_input * rate.cached_input)
        * input_factor
        + usage.output * rate.output * output_factor)
        / 1_000_000.0;
    cost_usd.is_finite().then_some(PricedUsage {
        cost_usd,
        pricing_revision: PRICING_REVISION,
    })
}
