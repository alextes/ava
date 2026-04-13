use serde::Deserialize;
use serde_json::json;

use crate::chat_buffer::ChatBuffer;
use crate::db::Database;
use crate::message::MessageContent;

use super::{ToolCallResult, ToolDefinition};

pub const CHANNEL_HISTORY_TOOL_NAME: &str = "channel_short_history";

#[derive(Debug, Deserialize)]
struct ChannelHistoryInput {
    action: String,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
}

pub fn channel_history_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: CHANNEL_HISTORY_TOOL_NAME,
        description: "peek at recent messages in your channels. this is an in-memory buffer, not a \
            full history — it holds at most 50 messages or 30 minutes, whichever is less, and \
            messages are cleared once injected into your context on trigger. use action=list to \
            see channels with buffered messages, action=get to read a specific channel's buffer. \
            use thread_id for supergroup topics.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "get"],
                    "description": "list channels or get recent messages for a specific channel"
                },
                "chat_id": {
                    "type": "integer",
                    "description": "chat_id of the channel (required for action=get)"
                },
                "thread_id": {
                    "type": "integer",
                    "description": "thread_id (topic) within a supergroup. omit for non-topic messages or DMs."
                }
            },
            "required": ["action"]
        }),
    }
}

pub fn handle_channel_history(
    db: &Database,
    chat_buffer: Option<&ChatBuffer>,
    call_id: &str,
    input: &serde_json::Value,
) -> ToolCallResult {
    let parsed: ChannelHistoryInput = match serde_json::from_value(input.clone()) {
        Ok(v) => v,
        Err(err) => {
            return ToolCallResult {
                content: MessageContent::tool_result(call_id, format!("invalid input: {err}")),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
                attachment: None,
            };
        }
    };

    let result = match parsed.action.as_str() {
        "list" => list_channels(db, chat_buffer),
        "get" => match parsed.chat_id {
            Some(id) => get_history(chat_buffer, id, parsed.thread_id),
            None => "error: chat_id is required for action=get".into(),
        },
        other => format!("unknown action: {other}. use 'list' or 'get'."),
    };

    ToolCallResult {
        content: MessageContent::tool_result(call_id, result),
        switch_provider: None,
        complete: false,
        compact: false,
        voice: None,
        attachment: None,
    }
}

fn list_channels(db: &Database, chat_buffer: Option<&ChatBuffer>) -> String {
    let channels = db.list_channels().unwrap_or_default();
    if channels.is_empty() {
        return "no channels registered.".into();
    }

    // aggregate buffer counts by chat_id (sum across threads)
    let mut buffer_counts: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    if let Some(buf) = chat_buffer {
        for ((chat_id, _thread_id), count) in buf.active_chats() {
            *buffer_counts.entry(chat_id).or_default() += count;
        }
    }

    let mut output = String::new();
    for ch in &channels {
        let label = match ch.chat_type.as_str() {
            "private" => format!("DM with user {}", ch.chat_id),
            _ => ch
                .title
                .as_deref()
                .map(|t| format!("#{t}"))
                .unwrap_or_else(|| format!("chat {}", ch.chat_id)),
        };
        let count = buffer_counts.get(&ch.chat_id).copied().unwrap_or(0);
        output.push_str(&format!(
            "{label} ({}, chat_id: {}, buffered: {count} messages)\n",
            ch.chat_type, ch.chat_id
        ));
    }
    output.trim_end().to_string()
}

fn get_history(chat_buffer: Option<&ChatBuffer>, chat_id: i64, thread_id: Option<i64>) -> String {
    let Some(buf) = chat_buffer else {
        return "no message buffer available.".into();
    };

    match buf.format_context(chat_id, thread_id) {
        Some((ctx, _images)) => ctx,
        None => format!("no recent messages buffered for chat_id {chat_id}."),
    }
}
