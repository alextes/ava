use serde::Deserialize;
use serde_json::json;

use crate::chat_buffer::ChatBuffer;
use crate::db::Database;
use crate::message::MessageContent;

use super::{ToolCallResult, ToolDefinition};

pub const CHANNEL_HISTORY_TOOL_NAME: &str = "channel_history";

#[derive(Debug, Deserialize)]
struct ChannelHistoryInput {
    action: String,
    chat_id: Option<i64>,
}

pub fn channel_history_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: CHANNEL_HISTORY_TOOL_NAME,
        description: "view recent message history from your channels. action=list: show all channels with active buffers (chat_id, title, type, message count). action=get: retrieve recent messages for a specific channel by chat_id.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "get"],
                    "description": "list channels or get history for a specific channel"
                },
                "chat_id": {
                    "type": "integer",
                    "description": "chat_id of the channel to retrieve history for (required for action=get)"
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
            };
        }
    };

    let result = match parsed.action.as_str() {
        "list" => list_channels(db, chat_buffer),
        "get" => match parsed.chat_id {
            Some(id) => get_history(chat_buffer, id),
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
    }
}

fn list_channels(db: &Database, chat_buffer: Option<&ChatBuffer>) -> String {
    let channels = db.list_channels().unwrap_or_default();
    if channels.is_empty() {
        return "no channels registered.".into();
    }

    let buffer_counts: std::collections::HashMap<i64, usize> = chat_buffer
        .map(|buf| buf.active_chats().into_iter().collect())
        .unwrap_or_default();

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

fn get_history(chat_buffer: Option<&ChatBuffer>, chat_id: i64) -> String {
    let Some(buf) = chat_buffer else {
        return "no message buffer available.".into();
    };

    match buf.format_context(chat_id) {
        Some(ctx) => ctx,
        None => format!("no recent messages buffered for chat_id {chat_id}."),
    }
}
