use crate::provider::Usage;

/// snapshot of context window usage after a provider call.
#[allow(dead_code)]
#[derive(Clone)]
pub struct ContextUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub context_window: u32,
    /// input_tokens / context_window * 100
    pub usage_percent: f64,
    /// whether compaction has happened this session
    pub compacted: bool,
    /// how many times compaction has run this session
    pub compaction_count: u32,
}

impl ContextUsage {
    pub fn compute(usage: &Usage, context_window: u32, compaction_count: u32) -> Self {
        // total context = uncached input + cache creation + cache read.
        // all three contribute to the context window size for the request.
        let total_tokens = usage.input_tokens
            + usage.cache_creation_tokens.unwrap_or(0)
            + usage.cache_read_tokens.unwrap_or(0);
        let usage_percent = if context_window > 0 {
            total_tokens as f64 / context_window as f64 * 100.0
        } else {
            0.0
        };
        Self {
            input_tokens: total_tokens,
            output_tokens: usage.output_tokens,
            context_window,
            usage_percent,
            compacted: compaction_count > 0,
            compaction_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_usage_compute() {
        let usage = Usage {
            input_tokens: 84_000,
            output_tokens: 1_200,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            reasoning_tokens: None,
        };
        let cu = ContextUsage::compute(&usage, 200_000, 0);
        assert_eq!(cu.input_tokens, 84_000);
        assert_eq!(cu.output_tokens, 1_200);
        assert_eq!(cu.context_window, 200_000);
        assert!((cu.usage_percent - 42.0).abs() < 0.01);
        assert!(!cu.compacted);
        assert_eq!(cu.compaction_count, 0);
    }

    #[test]
    fn test_context_usage_includes_cache_read() {
        // input_tokens=1000 + cache_read=135000 = 136000 / 1_000_000 = 13.6%
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 50,
            cache_creation_tokens: None,
            cache_read_tokens: Some(135_000),
            reasoning_tokens: None,
        };
        let cu = ContextUsage::compute(&usage, 1_000_000, 0);
        assert_eq!(cu.input_tokens, 136_000);
        assert!((cu.usage_percent - 13.6).abs() < 0.01);
    }

    #[test]
    fn test_context_usage_after_compaction() {
        let usage = Usage {
            input_tokens: 50_000,
            output_tokens: 500,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            reasoning_tokens: None,
        };
        let cu = ContextUsage::compute(&usage, 200_000, 2);
        assert!(cu.compacted);
        assert_eq!(cu.compaction_count, 2);
        assert!((cu.usage_percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_context_usage_includes_cache_creation() {
        // first request: most tokens go to cache_creation, only a few uncached
        // input=3 + cache_creation=9997 + cache_read=0 = 10000 / 1_000_000 = 1%
        let usage = Usage {
            input_tokens: 3,
            output_tokens: 100,
            cache_creation_tokens: Some(9_997),
            cache_read_tokens: Some(0),
            reasoning_tokens: None,
        };
        let cu = ContextUsage::compute(&usage, 1_000_000, 0);
        assert_eq!(cu.input_tokens, 10_000);
        assert!((cu.usage_percent - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_context_usage_zero_window() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            reasoning_tokens: None,
        };
        let cu = ContextUsage::compute(&usage, 0, 0);
        assert_eq!(cu.usage_percent, 0.0);
    }
}
