//! Port of `packages/daemon/src/account-pricing.test.ts`.

use daemon::account_pricing::{PRICING_REVISION, has_pricing, price_usage};
use daemon::providers::TokenUsage;
use protocol::model::Provider;

fn usage(input: f64, cached_input: f64, output: f64, reasoning: f64) -> TokenUsage {
    TokenUsage {
        input,
        cached_input,
        output,
        reasoning,
    }
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-10
}

#[test]
fn prices_separate_uncached_and_cached_input_without_double_counting_reasoning() {
    let result = price_usage(
        Provider::Codex,
        "gpt-5.3-codex",
        usage(100_000.0, 200_000.0, 10_000.0, 8000.0),
    )
    .unwrap();
    assert!(
        close(result.cost_usd, 0.175 + 0.035 + 0.14),
        "{}",
        result.cost_usd
    );
    assert_eq!(result.pricing_revision, PRICING_REVISION);
    assert_eq!(
        price_usage(
            Provider::Codex,
            "gpt-5.3-codex",
            usage(100_000.0, 200_000.0, 10_000.0, 0.0)
        ),
        Some(result)
    );
}

#[test]
fn applies_the_published_long_context_tier_after_the_threshold() {
    let long = price_usage(
        Provider::Codex,
        "gpt-6.1-sol",
        usage(200_000.0, 100_000.0, 10_000.0, 0.0),
    )
    .unwrap();
    assert!(
        close(long.cost_usd, (0.4 + 0.01) * 2.0 + 0.1 * 1.5),
        "{}",
        long.cost_usd
    );
    let short = price_usage(
        Provider::Codex,
        "gpt-6.1-sol",
        usage(172_000.0, 100_000.0, 0.0, 0.0),
    )
    .unwrap();
    assert!(close(short.cost_usd, 0.344 + 0.01), "{}", short.cost_usd);
}

#[test]
fn does_not_guess_prices_for_aliases_another_provider_or_unknown_models() {
    let one = usage(1.0, 0.0, 1.0, 0.0);
    for model in [
        "future-model",
        "gpt-6.1-sol-latest",
        "constructor",
        "__proto__",
    ] {
        assert!(!has_pricing(Provider::Codex, model));
        assert_eq!(price_usage(Provider::Codex, model, one), None);
    }
    assert_eq!(price_usage(Provider::Claude, "gpt-6.1-sol", one), None);
}

#[test]
fn rejects_invalid_token_counts_while_preserving_genuinely_zero_cost() {
    assert_eq!(
        price_usage(Provider::Codex, "gpt-6-luna", usage(0.0, 0.0, 0.0, 0.0))
            .map(|priced| priced.cost_usd),
        Some(0.0)
    );
    for input in [-1.0, 0.5, f64::NAN, f64::INFINITY] {
        assert_eq!(
            price_usage(Provider::Codex, "gpt-6-luna", usage(input, 0.0, 0.0, 0.0)),
            None,
            "{input}"
        );
    }
}
