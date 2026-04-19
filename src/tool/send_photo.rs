//! send_photo tool — sends an image as an inline telegram photo (with preview).

use std::fs;

use serde::Deserialize;

use super::ToolCall;
use super::filesystem::validate_existing_path;

pub const SEND_PHOTO_TOOL_NAME: &str = "send_photo";

const ALLOWED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

#[derive(Debug, Deserialize)]
struct SendPhotoInput {
    path: String,
    caption: Option<String>,
}

pub fn send_photo_definition() -> super::ToolDefinition {
    super::ToolDefinition::Custom {
        name: SEND_PHOTO_TOOL_NAME,
        description: "send an image to the user as an inline telegram photo with preview. \
            use for screenshots, charts, or any image you want displayed inline. \
            supported formats: png, jpg, jpeg, gif, webp. for non-image files, use send_file.",
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "path to the image file to send"
                },
                "caption": {
                    "type": "string",
                    "description": "optional short caption shown under the photo"
                }
            },
            "additionalProperties": false
        }),
    }
}

pub(super) fn handle_send_photo(call: &ToolCall) -> super::ToolCallResult {
    let input: SendPhotoInput = match serde_json::from_value(call.input.clone()) {
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

    let ext = resolved
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some(e) if ALLOWED_EXTENSIONS.contains(&e) => {}
        _ => {
            return super::ToolCallResult {
                content: super::super::message::MessageContent::tool_result(
                    &call.id,
                    format!(
                        "unsupported image format (expected one of {}); \
                         use send_file for non-image attachments",
                        ALLOWED_EXTENSIONS.join(", ")
                    ),
                ),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
                attachment: None,
            };
        }
    }

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
        .unwrap_or_else(|| "photo".to_string());

    super::ToolCallResult {
        content: super::super::message::MessageContent::tool_result(
            &call.id,
            format!("sent {filename} ({} bytes) as inline photo", bytes.len()),
        ),
        switch_provider: None,
        complete: false,
        compact: false,
        voice: None,
        attachment: Some(super::FileAttachment {
            bytes,
            filename,
            caption: input.caption,
            kind: super::AttachmentKind::Photo,
        }),
    }
}
