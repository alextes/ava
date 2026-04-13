use serde_json::json;

use crate::message::MessageContent;

use super::{ToolCallResult, ToolDefinition};

pub const COMPACT_CONTEXT_TOOL_NAME: &str = "compact_context";

pub fn compact_context_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: COMPACT_CONTEXT_TOOL_NAME,
        description: "trigger context compaction to free up space in the conversation window. \
            use this after completing a large task when you want to reclaim context for the next one. \
            the conversation history will be summarized and compressed. \
            no-op if context usage is below 20%.",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    }
}

pub fn handle_compact_context(call_id: &str) -> ToolCallResult {
    tracing::info!("agent requested context compaction");
    ToolCallResult {
        content: MessageContent::tool_result(call_id, "compaction requested"),
        switch_provider: None,
        complete: false,
        compact: true,
        voice: None,
        attachment: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_compact_context_signals_compact() {
        let result = handle_compact_context("t1");
        assert!(result.compact);
        assert!(!result.complete);
        assert!(result.switch_provider.is_none());
    }
}
