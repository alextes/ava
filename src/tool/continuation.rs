use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::message::MessageContent;

use super::{ContinuationTarget, ToolCallResult, ToolDefinition};

pub const REQUEST_CONTINUATION_TOOL_NAME: &str = "request_continuation";

pub fn request_continuation_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: REQUEST_CONTINUATION_TOOL_NAME,
        description: "request one additional queued turn after this response completes. use when you need to end the current turn but want ava to continue without waiting for a user reply, such as after self_upgrade hot-swap. include a concise prompt for the next turn.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "what ava should do in the follow-up turn"
                },
                "reason": {
                    "type": "string",
                    "description": "optional short reason for requesting continuation"
                }
            },
            "required": ["prompt"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct RequestContinuationInput {
    prompt: String,
    reason: Option<String>,
}

pub fn handle_request_continuation(
    db: &Database,
    call_id: &str,
    input: &serde_json::Value,
    target: Option<ContinuationTarget>,
) -> ToolCallResult {
    let parsed: RequestContinuationInput = match serde_json::from_value(input.clone()) {
        Ok(input) => input,
        Err(err) => {
            return result(call_id, format!("invalid input: {err}"));
        }
    };

    let prompt = parsed.prompt.trim();
    if prompt.is_empty() {
        return result(call_id, "prompt must not be empty");
    }

    let Some(target) = target else {
        return result(call_id, "continuation is not available for this channel");
    };

    let reason = parsed
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let content = match reason {
        Some(reason) => format!(
            "[system: continuation requested by the assistant. reason: {reason}]\n\n{prompt}"
        ),
        None => format!("[system: continuation requested by the assistant.]\n\n{prompt}"),
    };

    match db.enqueue_message(
        target.channel,
        target.chat_id,
        target.thread_id,
        &content,
        &[],
    ) {
        Ok(id) => result(call_id, format!("continuation requested (queue_id={id})")),
        Err(err) => result(call_id, format!("failed to request continuation: {err}")),
    }
}

fn result(call_id: &str, msg: impl Into<String>) -> ToolCallResult {
    ToolCallResult {
        content: MessageContent::tool_result(call_id, msg.into()),
        switch_provider: None,
        complete: false,
        compact: false,
        voice: None,
        attachment: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ChannelKind;
    use serde_json::json;

    fn text(result: &ToolCallResult) -> String {
        match &result.content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected tool result"),
        }
    }

    #[test]
    fn test_request_continuation_enqueues_same_target() {
        let db = Database::open_in_memory().unwrap();
        let target = ContinuationTarget {
            channel: ChannelKind::Telegram,
            chat_id: 42,
            thread_id: Some(7),
        };

        let result = handle_request_continuation(
            &db,
            "call_1",
            &json!({
                "prompt": "verify the upgrade and report back",
                "reason": "self-upgrade restart",
            }),
            Some(target),
        );

        assert!(text(&result).contains("continuation requested"));
        let next = db.next_pending_message().unwrap().unwrap();
        assert_eq!(next.channel_kind(), Some(ChannelKind::Telegram));
        assert_eq!(next.chat_id, 42);
        assert_eq!(next.thread_id, Some(7));
        assert!(next.content.contains("self-upgrade restart"));
        assert!(next.content.contains("verify the upgrade"));
    }

    #[test]
    fn test_request_continuation_requires_target() {
        let db = Database::open_in_memory().unwrap();
        let result =
            handle_request_continuation(&db, "call_1", &json!({"prompt": "continue"}), None);

        assert_eq!(
            text(&result),
            "continuation is not available for this channel"
        );
        assert_eq!(db.next_pending_message().unwrap(), None);
    }

    #[test]
    fn test_request_continuation_rejects_empty_prompt() {
        let db = Database::open_in_memory().unwrap();
        let target = ContinuationTarget {
            channel: ChannelKind::Telegram,
            chat_id: 42,
            thread_id: None,
        };

        let result =
            handle_request_continuation(&db, "call_1", &json!({"prompt": "   "}), Some(target));

        assert_eq!(text(&result), "prompt must not be empty");
        assert_eq!(db.next_pending_message().unwrap(), None);
    }
}
