use serde::Deserialize;
use serde_json::json;

use crate::message::MessageContent;

use super::{ToolCallResult, ToolDefinition};

pub const COMPLETE_TOOL_NAME: &str = "complete";

pub fn complete_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: COMPLETE_TOOL_NAME,
        description: "finish processing without sending a reply. use this when: (1) a background/scheduled task completes silently, (2) you were mentioned in a group chat but have nothing useful to add, or (3) the message doesn't need a response. include a reason for logging.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "optional reason for completing silently (for logging)"
                }
            },
            "required": []
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CompleteInput {
    #[allow(dead_code)]
    reason: Option<String>,
}

pub fn handle_complete(call_id: &str, input: &serde_json::Value) -> ToolCallResult {
    let parsed: CompleteInput =
        serde_json::from_value(input.clone()).unwrap_or(CompleteInput { reason: None });

    if let Some(ref reason) = parsed.reason {
        tracing::info!(%reason, "completing silently");
    } else {
        tracing::info!("completing silently");
    }

    ToolCallResult {
        content: MessageContent::tool_result(call_id, "ok"),
        switch_provider: None,
        complete: true,
        compact: false,
        voice: None,
        attachment: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageContent;
    use serde_json::json;

    fn extract_text(content: &MessageContent) -> String {
        match content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_handle_complete() {
        let result = handle_complete("t1", &json!({}));
        assert_eq!(extract_text(&result.content), "ok");
        assert!(result.complete);
        assert!(result.switch_provider.is_none());
    }

    #[test]
    fn test_handle_complete_with_reason() {
        let result = handle_complete("t1", &json!({"reason": "memory distillation done"}));
        assert_eq!(extract_text(&result.content), "ok");
        assert!(result.complete);
        assert!(result.switch_provider.is_none());
    }
}
