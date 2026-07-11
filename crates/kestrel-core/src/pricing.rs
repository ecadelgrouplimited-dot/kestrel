//! Rough per-model pricing, for the UI cost meter.
//!
//! Token economy is a core focus, so Kestrel shows the estimated size and cost
//! of what it's about to send. Prices are USD per million tokens. Only models
//! with published, stable pricing are listed; for others the meter still shows
//! the token estimate (cost is simply unknown, not guessed).

/// Input/output price for a model, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

/// The published price for a model id, matched by family prefix, or `None` when
/// pricing isn't known (the meter then shows tokens only).
pub fn model_price(model: &str) -> Option<ModelPrice> {
    let m = model.to_lowercase();
    let price = |input, output| {
        Some(ModelPrice {
            input_per_million: input,
            output_per_million: output,
        })
    };
    if m.starts_with("claude-fable") || m.starts_with("claude-mythos") {
        price(10.0, 50.0)
    } else if m.starts_with("claude-opus") {
        price(5.0, 25.0)
    } else if m.starts_with("claude-sonnet") {
        price(3.0, 15.0)
    } else if m.starts_with("claude-haiku") {
        price(1.0, 5.0)
    } else {
        None
    }
}

/// Estimated USD cost of a request given input and (expected) output tokens.
pub fn estimate_cost(price: ModelPrice, input_tokens: usize, output_tokens: usize) -> f64 {
    input_tokens as f64 / 1_000_000.0 * price.input_per_million
        + output_tokens as f64 / 1_000_000.0 * price.output_per_million
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_known_families_and_skips_unknown() {
        assert_eq!(
            model_price("claude-opus-4-8").unwrap().input_per_million,
            5.0
        );
        assert_eq!(
            model_price("claude-haiku-4-5").unwrap().output_per_million,
            5.0
        );
        assert!(model_price("deepseek-v4-pro").is_none());
        assert!(model_price("glm-5.2").is_none());
    }

    #[test]
    fn cost_adds_input_and_output() {
        let p = model_price("claude-opus-4-8").unwrap();
        // 1M input @ $5 + 1M output @ $25 = $30.
        assert!((estimate_cost(p, 1_000_000, 1_000_000) - 30.0).abs() < 1e-9);
    }
}
