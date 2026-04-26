//! per-model pricing lookup for base input tokens, in USD per million tokens.
//!
//! used to estimate the cost of replaying a conversation uncached when the
//! prompt cache goes cold. values pulled from anthropic's pricing page
//! (<https://claude.com/pricing>) as of 2026-04. openai gpt-5 family uses
//! conservative estimates — if precise numbers land, update them here.
//!
//! the lookup returns `None` for unknown models so callers can decide whether
//! to fall back to a pessimistic default or skip the estimate entirely.

// `base_input_usd_per_mtok` and `estimate_replay_cost_usd` are kept public for
// future use; only `format_replay_cost` is consumed today.
#![allow(dead_code)]

/// base input price in USD per million tokens for a `"provider/model"` id.
///
/// match is exact on the id returned by `AnyProvider::model_id()`.
/// returns `None` for unrecognized ids.
pub fn base_input_usd_per_mtok(model_id: &str) -> Option<f64> {
    match model_id {
        // claude opus family
        "anthropic/claude-opus-4-7" => Some(5.0),
        "anthropic/claude-opus-4-6" => Some(5.0),
        "anthropic/claude-opus-4-5" => Some(5.0),
        "anthropic/claude-opus-4-1" => Some(15.0),
        "anthropic/claude-opus-4" => Some(15.0),

        // claude sonnet family
        "anthropic/claude-sonnet-4-6" => Some(3.0),
        "anthropic/claude-sonnet-4-5" => Some(3.0),
        "anthropic/claude-sonnet-4" => Some(3.0),

        // claude haiku family
        "anthropic/claude-haiku-4-5" => Some(1.0),
        "anthropic/claude-haiku-3-5" => Some(0.80),

        // openai — rough estimates pending a confirmed pricing source.
        // keep conservative so replay-cost estimates err on the high side.
        "openai/gpt-5.4" => Some(3.0),
        "openai/gpt-5-mini" => Some(0.50),

        // openrouter passes through to underlying providers — we don't
        // attempt to price every model, caller should treat as unknown.
        _ => None,
    }
}

/// estimate the USD cost of sending `input_tokens` as uncached base input
/// to the given model. returns `None` if the model is unknown.
pub fn estimate_replay_cost_usd(model_id: &str, input_tokens: u32) -> Option<f64> {
    let rate = base_input_usd_per_mtok(model_id)?;
    Some(input_tokens as f64 * rate / 1_000_000.0)
}

/// format a replay cost for display. returns `"~$X.YY"` or `"~$0.01"` for
/// small amounts. returns `"unknown"` if pricing isn't available.
pub fn format_replay_cost(model_id: &str, input_tokens: u32) -> String {
    match estimate_replay_cost_usd(model_id, input_tokens) {
        Some(usd) if usd < 0.01 => "~<$0.01".to_string(),
        Some(usd) => format!("~${usd:.2}"),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_4_6_lookup() {
        assert_eq!(
            base_input_usd_per_mtok("anthropic/claude-opus-4-6"),
            Some(5.0)
        );
    }

    #[test]
    fn sonnet_4_6_lookup() {
        assert_eq!(
            base_input_usd_per_mtok("anthropic/claude-sonnet-4-6"),
            Some(3.0)
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
    fn unknown_model_returns_none() {
        assert!(base_input_usd_per_mtok("anthropic/claude-future-99").is_none());
        assert!(base_input_usd_per_mtok("openrouter/google/gemini-3").is_none());
    }

    #[test]
    fn estimate_50k_on_opus() {
        // 50k tokens × $5/MTok = $0.25
        let cost = estimate_replay_cost_usd("anthropic/claude-opus-4-6", 50_000).unwrap();
        assert!((cost - 0.25).abs() < 0.0001);
    }

    #[test]
    fn estimate_unknown_returns_none() {
        assert!(estimate_replay_cost_usd("unknown/model", 10_000).is_none());
    }

    #[test]
    fn format_large_cost() {
        // 100k on opus 4.6 = $0.50
        let s = format_replay_cost("anthropic/claude-opus-4-6", 100_000);
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
