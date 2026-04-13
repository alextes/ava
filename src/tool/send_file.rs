//! send_file tool — sends a file as a telegram document attachment.

use std::fs;

use serde::Deserialize;

use super::ToolCall;
use super::filesystem::validate_existing_path;

pub const SEND_FILE_TOOL_NAME: &str = "send_file";

#[derive(Debug, Deserialize)]
struct SendFileInput {
    path: String,
    caption: Option<String>,
}

pub fn send_file_definition() -> super::ToolDefinition {
    super::ToolDefinition::Custom {
        name: SEND_FILE_TOOL_NAME,
        description: "send a file to the user as a telegram document attachment. \
            write content to a file first (e.g. /tmp/response.md), then use this tool \
            to deliver it. useful for long responses that exceed telegram's message limit.",
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "path to the file to send"
                },
                "caption": {
                    "type": "string",
                    "description": "optional short caption shown alongside the file"
                }
            },
            "additionalProperties": false
        }),
    }
}

pub(super) fn handle_send_file(call: &ToolCall) -> super::ToolCallResult {
    let input: SendFileInput = match serde_json::from_value(call.input.clone()) {
        Ok(i) => i,
        Err(err) => {
            return super::ToolCallResult {
                content: super::super::message::MessageContent::tool_result(
                    &call.id,
                    format!("invalid input: {err}"),
                ),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
                attachment: None,
            };
        }
    };

    let resolved = match validate_existing_path(&input.path) {
        Ok(p) => p,
        Err(e) => {
            return super::ToolCallResult {
                content: super::super::message::MessageContent::tool_result(&call.id, &e),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
                attachment: None,
            };
        }
    };

    let bytes = match fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            return super::ToolCallResult {
                content: super::super::message::MessageContent::tool_result(
                    &call.id,
                    format!("failed to read file: {e}"),
                ),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
                attachment: None,
            };
        }
    };

    let filename = resolved
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    super::ToolCallResult {
        content: super::super::message::MessageContent::tool_result(
            &call.id,
            format!("sent {filename} ({} bytes)", bytes.len()),
        ),
        switch_provider: None,
        complete: false,
        compact: false,
        voice: None,
        attachment: Some(super::FileAttachment {
            bytes,
            filename,
            caption: input.caption,
        }),
    }
}
