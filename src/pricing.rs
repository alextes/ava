//! per-model pricing lookup, in USD per million tokens.
//!
//! used to estimate the cost of replaying a conversation uncached when the
//! prompt cache goes cold, and to estimate per-response spend from provider
//! usage metadata. values verified against the providers' pricing pages as of
//! 2026-07:
//! - anthropic: <https://platform.claude.com/docs/en/about-claude/pricing>
//! - openai:   <https://openai.com/api/pricing/>
//! - gemini:   <https://ai.google.dev/gemini-api/docs/pricing>
//! - deepseek: <https://api-docs.deepseek.com/>
//! - openrouter: per-model pages on openrouter.ai
//!
//! note: opus 4.7 ships with a tokenizer that emits ~35% more tokens for the
//! same source text vs 4.6, so effective cost is higher despite the unchanged
//! rate. context-overflow / cold-resume estimates use this rate as-is and
//! will under-estimate 4.7 spend by that margin.
//!
//! the lookup returns `None` for unknown models so callers can decide whether
//! to fall back to a pessimistic default or skip the estimate entirely.

use crate::provider::Usage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub base_input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    pub cache_write_usd_per_mtok: Option<f64>,
    pub cache_read_usd_per_mtok: Option<f64>,
}

impl ModelPricing {
    const fn new(base_input: f64, output: f64) -> Self {
        Self {
            base_input_usd_per_mtok: base_input,
            output_usd_per_mtok: output,
            cache_write_usd_per_mtok: None,
            cache_read_usd_per_mtok: None,
        }
    }

    const fn with_cache(mut self, cache_write: Option<f64>, cache_read: f64) -> Self {
        self.cache_write_usd_per_mtok = cache_write;
        self.cache_read_usd_per_mtok = Some(cache_read);
        self
    }
}

/// token prices in USD per million tokens for a `"provider/model"` id.
///
/// match is exact on the id returned by `AnyProvider::model_id()`.
/// returns `None` for unrecognized ids.
pub fn model_pricing(model_id: &str) -> Option<ModelPricing> {
    match model_id {
        // claude opus family
        "anthropic/claude-opus-4-7" => {
            Some(ModelPricing::new(5.0, 25.0).with_cache(Some(6.25), 0.50))
        }
        "anthropic/claude-opus-4-6" => {
            Some(ModelPricing::new(5.0, 25.0).with_cache(Some(6.25), 0.50))
        }
        "anthropic/claude-opus-4-5" => {
            Some(ModelPricing::new(5.0, 25.0).with_cache(Some(6.25), 0.50))
        }
        "anthropic/claude-opus-4-1" | "anthropic/claude-opus-4" => {
            Some(ModelPricing::new(15.0, 75.0).with_cache(Some(18.75), 1.50))
        }

        // claude sonnet family
        "anthropic/claude-sonnet-4-6" => {
            Some(ModelPricing::new(3.0, 15.0).with_cache(Some(3.75), 0.30))
        }
        "anthropic/claude-sonnet-4-5" | "anthropic/claude-sonnet-4" => {
            Some(ModelPricing::new(3.0, 15.0).with_cache(Some(3.75), 0.30))
        }

        // claude haiku family
        "anthropic/claude-haiku-4-5" => {
            Some(ModelPricing::new(1.0, 5.0).with_cache(Some(1.25), 0.10))
        }
        "anthropic/claude-haiku-3-5" => {
            Some(ModelPricing::new(0.80, 4.0).with_cache(Some(1.0), 0.08))
        }

        // gemini — google ai studio prices as of may 2026.
        "gemini/gemini-3.5-flash" => Some(ModelPricing::new(1.50, 9.0).with_cache(None, 0.15)),
        "gemini/gemini-3.1-pro-preview" => {
            Some(ModelPricing::new(2.00, 12.0).with_cache(None, 0.20))
        }

        // openai — current selectable models.
        "openai/gpt-5.6-luna" => Some(ModelPricing::new(1.0, 6.0).with_cache(Some(1.25), 0.10)),
        "openai/gpt-5.6-sol" => Some(ModelPricing::new(5.0, 30.0).with_cache(Some(6.25), 0.50)),

        // historical openai models retained for usage records.
        "openai/gpt-5.5" => Some(ModelPricing::new(5.0, 30.0).with_cache(None, 0.50)),
        "openai/gpt-5.4" => Some(ModelPricing::new(2.5, 15.0).with_cache(None, 0.25)),
        "openai/gpt-5-mini" => Some(ModelPricing::new(0.25, 2.0).with_cache(None, 0.025)),

        // deepseek — first-party prices for the V4 models.
        "deepseek/deepseek-v4-pro" => {
            Some(ModelPricing::new(0.435, 0.87).with_cache(None, 0.003625))
        }
        "deepseek/deepseek-v4-flash" => {
            Some(ModelPricing::new(0.14, 0.28).with_cache(None, 0.0028))
        }

        // openrouter — pricing for the few models we know; falls through
        // to None for the long tail.
        "openrouter/deepseek/deepseek-v4-pro" => {
            Some(ModelPricing::new(0.435, 0.87).with_cache(None, 0.003625))
        }
        "openrouter/deepseek/deepseek-v4-flash" => {
            Some(ModelPricing::new(0.14, 0.28).with_cache(None, 0.0028))
        }

        _ => None,
    }
}

/// base input price in USD per million tokens for a `"provider/model"` id.
///
/// match is exact on the id returned by `AnyProvider::model_id()`.
/// returns `None` for unrecognized ids.
pub fn base_input_usd_per_mtok(model_id: &str) -> Option<f64> {
    Some(model_pricing(model_id)?.base_input_usd_per_mtok)
}

/// estimate the USD cost of sending `input_tokens` as uncached base input
/// to the given model. returns `None` if the model is unknown.
pub fn estimate_replay_cost_usd(model_id: &str, input_tokens: u32) -> Option<f64> {
    let rate = base_input_usd_per_mtok(model_id)?;
    Some(input_tokens as f64 * rate / 1_000_000.0)
}

/// estimate the USD cost of a provider call from reported usage.
///
/// anthropic reports cache write/read tokens separately from base input.
/// openai/openrouter-style cached tokens are subsets of input tokens, so those
/// are subtracted from base input before applying the cached-input rate.
pub fn estimate_usage_cost_usd(model_id: &str, usage: &Usage) -> Option<f64> {
    let pricing = model_pricing(model_id)?;
    let cache_read_tokens = usage.cache_read_tokens.unwrap_or(0);
    let cache_creation_tokens = usage.cache_creation_tokens.unwrap_or(0);

    let base_input_tokens = if has_separate_cache_accounting(model_id) {
        usage.input_tokens
    } else {
        usage.input_tokens.saturating_sub(cache_read_tokens)
    };

    let mut usd = base_input_tokens as f64 * pricing.base_input_usd_per_mtok / 1_000_000.0;
    usd += usage.output_tokens as f64 * pricing.output_usd_per_mtok / 1_000_000.0;

    if cache_creation_tokens > 0 {
        let rate = pricing.cache_write_usd_per_mtok?;
        usd += cache_creation_tokens as f64 * rate / 1_000_000.0;
    }

    if cache_read_tokens > 0 {
        let rate = pricing.cache_read_usd_per_mtok?;
        usd += cache_read_tokens as f64 * rate / 1_000_000.0;
    }

    Some(usd)
}

fn has_separate_cache_accounting(model_id: &str) -> bool {
    model_id.starts_with("anthropic/")
}

/// format a replay cost for display. returns `"~$X.YY"` or `"~$0.01"` for
/// small amounts. returns `"unknown"` if pricing isn't available.
pub fn format_replay_cost(model_id: &str, input_tokens: u32) -> String {
    format_cost(estimate_replay_cost_usd(model_id, input_tokens))
}

/// format a provider-call usage cost for display.
pub fn format_usage_cost(model_id: &str, usage: &Usage) -> String {
    format_cost(estimate_usage_cost_usd(model_id, usage))
}

pub fn format_cost(cost: Option<f64>) -> String {
    match cost {
        Some(usd) if usd < 0.01 => "~<$0.01".to_string(),
        Some(usd) => format!("~${usd:.2}"),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_4_7_lookup() {
        assert_eq!(
            base_input_usd_per_mtok("anthropic/claude-opus-4-7"),
            Some(5.0)
        );
    }

    #[test]
    fn sonnet_4_6_lookup() {
        assert_eq!(
            base_input_usd_per_mtok("anthropic/claude-sonnet-4-6"),
            Some(3.0)
        );
        assert_eq!(
            model_pricing("anthropic/claude-sonnet-4-6").map(|p| p.output_usd_per_mtok),
            Some(15.0)
        );
    }

    #[test]
    fn haiku_4_5_lookup() {
        assert_eq!(
            base_input_usd_per_mtok("anthropic/claude-haiku-4-5"),
            Some(1.0)
        );
    }

    #[test]
    fn deepseek_v4_pro_lookup() {
        assert_eq!(
            base_input_usd_per_mtok("deepseek/deepseek-v4-pro"),
            Some(0.435)
        );
        assert_eq!(
            model_pricing("deepseek/deepseek-v4-pro").map(|p| p.output_usd_per_mtok),
            Some(0.87)
        );
    }

    #[test]
    fn gpt_5_6_pricing_includes_cache_rates() {
        assert_eq!(
            model_pricing("openai/gpt-5.6-luna"),
            Some(ModelPricing::new(1.0, 6.0).with_cache(Some(1.25), 0.10))
        );
        assert_eq!(
            model_pricing("openai/gpt-5.6-sol"),
            Some(ModelPricing::new(5.0, 30.0).with_cache(Some(6.25), 0.50))
        );
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(base_input_usd_per_mtok("anthropic/claude-future-99").is_none());
        assert!(base_input_usd_per_mtok("openrouter/google/gemini-3").is_none());
        assert!(base_input_usd_per_mtok("openrouter/anthropic/claude-sonnet-4-6").is_none());
    }

    #[test]
    fn estimate_50k_on_opus() {
        // 50k tokens × $5/MTok = $0.25
        let cost = estimate_replay_cost_usd("anthropic/claude-opus-4-7", 50_000).unwrap();
        assert!((cost - 0.25).abs() < 0.0001);
    }

    #[test]
    fn estimate_unknown_returns_none() {
        assert!(estimate_replay_cost_usd("unknown/model", 10_000).is_none());
    }

    #[test]
    fn estimate_usage_cost_counts_output() {
        let usage = Usage {
            input_tokens: 100_000,
            output_tokens: 10_000,
            ..Default::default()
        };
        let cost = estimate_usage_cost_usd("anthropic/claude-sonnet-4-6", &usage).unwrap();
        // 100k × $3/MTok + 10k × $15/MTok = $0.45
        assert!((cost - 0.45).abs() < 0.0001);
    }

    #[test]
    fn estimate_usage_cost_counts_anthropic_cache_separately() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 2_000,
            cache_creation_tokens: Some(10_000),
            cache_read_tokens: Some(100_000),
            ..Default::default()
        };
        let cost = estimate_usage_cost_usd("anthropic/claude-sonnet-4-6", &usage).unwrap();
        // base input 0.003 + output 0.030 + write 0.0375 + read 0.030
        assert!((cost - 0.1005).abs() < 0.0001);
    }

    #[test]
    fn estimate_usage_cost_subtracts_cached_input_for_openai() {
        let usage = Usage {
            input_tokens: 100_000,
            output_tokens: 10_000,
            cache_read_tokens: Some(80_000),
            ..Default::default()
        };
        let cost = estimate_usage_cost_usd("openai/gpt-5.6-luna", &usage).unwrap();
        // uncached input 20k × $1 + cached input 80k × $0.10 + output 10k × $6
        assert!((cost - 0.088).abs() < 0.0001);
    }

    #[test]
    fn format_large_cost() {
        // 100k on opus 4.7 = $0.50
        let s = format_replay_cost("anthropic/claude-opus-4-7", 100_000);
        assert_eq!(s, "~$0.50");
    }

    #[test]
    fn format_tiny_cost() {
        // 100 tokens on haiku = $0.0001
        let s = format_replay_cost("anthropic/claude-haiku-4-5", 100);
        assert_eq!(s, "~<$0.01");
    }

    #[test]
    fn format_unknown() {
        assert_eq!(format_replay_cost("unknown/model", 10_000), "unknown");
    }
}
