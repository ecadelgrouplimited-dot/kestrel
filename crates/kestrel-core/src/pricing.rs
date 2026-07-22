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

/// USD cost of actual [`Usage`], honouring cache economics: cache reads bill at
/// ~10% of input, cache writes at ~25% over input.
pub fn cost_of_usage(price: ModelPrice, usage: &crate::providers::Usage) -> f64 {
    (usage.input_tokens as f64 * price.input_per_million
        + usage.cache_read as f64 * price.input_per_million * 0.1
        + usage.cache_write as f64 * price.input_per_million * 1.25
        + usage.output_tokens as f64 * price.output_per_million)
        / 1_000_000.0
}

/// The context-window size (in tokens) for a model, matched by family, with a
/// conservative default when unknown.
pub fn model_context_window(model: &str) -> u64 {
    let m = model.to_lowercase();
    if m.starts_with("claude-haiku") {
        200_000
    } else if m.starts_with("claude-") {
        1_000_000
    } else if m.starts_with("gpt-5") || m.starts_with("gpt-4.1") {
        400_000
    } else if m.starts_with("kimi-k") {
        // Moonshot's K2/K3 generation is 256k; only the legacy moonshot-v1 line
        // is smaller, and it carries its size in the name (handled below).
        256_000
    } else if let Some(rest) = m.strip_prefix("moonshot-v1-") {
        match rest.split('-').next().unwrap_or("") {
            "8k" => 8_000,
            "32k" => 32_000,
            _ => 128_000,
        }
    } else {
        // DeepSeek / GLM / Kimi and unknowns: a safe common floor.
        128_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_context_windows_match_moonshots_published_sizes() {
        // K2/K3 are 256k — defaulting them to 128k halved the context gauge.
        assert_eq!(model_context_window("kimi-k3"), 256_000);
        assert_eq!(model_context_window("kimi-k2.7-code"), 256_000);
        assert_eq!(model_context_window("kimi-k2-thinking"), 256_000);
        // The legacy line carries its size in the name.
        assert_eq!(model_context_window("moonshot-v1-8k"), 8_000);
        assert_eq!(model_context_window("moonshot-v1-32k"), 32_000);
        assert_eq!(model_context_window("moonshot-v1-128k"), 128_000);
        assert_eq!(
            model_context_window("moonshot-v1-32k-vision-preview"),
            32_000
        );
    }

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

    #[test]
    fn cache_reads_cost_a_tenth_of_input() {
        let p = model_price("claude-opus-4-8").unwrap();
        let usage = crate::providers::Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 1_000_000,
            cache_write: 0,
        };
        // 1M cache-read @ 10% of $5 = $0.50.
        assert!((cost_of_usage(p, &usage) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn context_windows_match_families() {
        assert_eq!(model_context_window("claude-opus-4-8"), 1_000_000);
        assert_eq!(model_context_window("claude-haiku-4-5"), 200_000);
        assert_eq!(model_context_window("glm-5.2"), 128_000);
    }
}
