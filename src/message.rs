use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    /// internal system injections (context usage, budget warnings).
    /// stored as "system" in the DB, but sent to the API as "user".
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<MessageContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Image { source: ImageSource },
}

/// tool result content: either a plain string (backward compat) or an array of content blocks.
/// serde untagged handles old DB records automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl ToolResultContent {
    /// returns text content, replacing images with `[image]`
    pub fn as_display_str(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => {
                let mut parts = Vec::new();
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => parts.push(text.as_str()),
                        ContentBlock::Image { .. } => parts.push("[image]"),
                    }
                }
                parts.join("\n")
            }
        }
    }

    /// estimated length in chars for token counting heuristics
    pub fn estimated_len(&self) -> usize {
        match self {
            Self::Text(s) => s.len(),
            Self::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    // base64 images are large but we count a fixed estimate
                    // since the actual token cost depends on the provider
                    ContentBlock::Image { source } => source.data.len(),
                })
                .sum(),
        }
    }
}

impl fmt::Display for ToolResultContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_display_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
    },
}

impl MessageContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn tool_use(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(content.into()),
        }
    }

    #[allow(dead_code)]
    pub fn tool_result_with_blocks(
        tool_use_id: impl Into<String>,
        blocks: Vec<ContentBlock>,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Blocks(blocks),
        }
    }
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self::user_with_content(vec![MessageContent::text(content)])
    }

    #[allow(dead_code)]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::assistant_with_content(vec![MessageContent::text(content)])
    }

    pub fn user_with_content(content: Vec<MessageContent>) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }

    pub fn assistant_with_content(content: Vec<MessageContent>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }
}

/// where the message came from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Cli,
    Telegram,
}

impl ChannelKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChannelKind::Cli => "cli",
            ChannelKind::Telegram => "telegram",
        }
    }
}

/// a message coming into the agent
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub channel: ChannelKind,
    pub content: String,
    /// images attached to this message (e.g. telegram photos)
    pub images: Vec<ImageSource>,
}

/// a message going out from the agent
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub content: String,
    /// OGG Opus audio bytes for voice output (sent via sendVoice on telegram, played locally otherwise)
    pub voice: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_result_content_backward_compat_deserialization() {
        // old format: content is a plain string
        let json = r#"{"type":"tool_result","tool_use_id":"id1","content":"hello"}"#;
        let msg: MessageContent = serde_json::from_str(json).unwrap();
        match msg {
            MessageContent::ToolResult { content, .. } => {
                assert_eq!(content.as_display_str(), "hello");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_content_blocks_roundtrip() {
        let blocks = vec![
            ContentBlock::Text {
                text: "here is a screenshot".into(),
            },
            ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                },
            },
        ];
        let msg = MessageContent::tool_result_with_blocks("id1", blocks);
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: MessageContent = serde_json::from_str(&json).unwrap();
        match parsed {
            MessageContent::ToolResult { content, .. } => {
                assert_eq!(content.as_display_str(), "here is a screenshot\n[image]");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_content_display_str_text() {
        let content = ToolResultContent::Text("hello world".into());
        assert_eq!(content.as_display_str(), "hello world");
    }

    #[test]
    fn test_tool_result_content_display_str_blocks() {
        let content = ToolResultContent::Blocks(vec![
            ContentBlock::Text {
                text: "line 1".into(),
            },
            ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "abc".into(),
                },
            },
            ContentBlock::Text {
                text: "line 2".into(),
            },
        ]);
        assert_eq!(content.as_display_str(), "line 1\n[image]\nline 2");
    }

    #[test]
    fn test_tool_result_content_estimated_len_text() {
        let content = ToolResultContent::Text("hello".into());
        assert_eq!(content.estimated_len(), 5);
    }

    #[test]
    fn test_tool_result_content_estimated_len_blocks() {
        let content = ToolResultContent::Blocks(vec![
            ContentBlock::Text { text: "hi".into() },
            ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "abcdef".into(),
                },
            },
        ]);
        // "hi" = 2, "abcdef" = 6
        assert_eq!(content.estimated_len(), 8);
    }

    #[test]
    fn test_tool_result_text_constructor_wraps_in_text() {
        let msg = MessageContent::tool_result("id1", "ok");
        match msg {
            MessageContent::ToolResult { content, .. } => {
                assert!(matches!(content, ToolResultContent::Text(ref s) if s == "ok"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_real_jpeg_image_roundtrip() {
        use base64::Engine;

        // read the real jpeg file and base64-encode it
        let jpeg_bytes =
            std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/unknown-animal.jpeg")).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

        // construct a tool result with text + image blocks
        let blocks = vec![
            ContentBlock::Text {
                text: "here is the screenshot of the page".into(),
            },
            ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: "image/jpeg".into(),
                    data: b64.clone(),
                },
            },
        ];
        let msg = MessageContent::tool_result_with_blocks("call_42", blocks);

        // serialize to JSON
        let json_str = serde_json::to_string(&msg).unwrap();

        // deserialize back
        let parsed: MessageContent = serde_json::from_str(&json_str).unwrap();

        match &parsed {
            MessageContent::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "call_42");

                // display replaces image with [image]
                assert_eq!(
                    content.as_display_str(),
                    "here is the screenshot of the page\n[image]"
                );

                // estimated_len includes the full base64 data
                let text_len = "here is the screenshot of the page".len();
                assert_eq!(content.estimated_len(), text_len + b64.len());

                // verify the actual image data survived the round-trip
                if let ToolResultContent::Blocks(blocks) = content {
                    assert_eq!(blocks.len(), 2);
                    if let ContentBlock::Image { source } = &blocks[1] {
                        assert_eq!(source.source_type, "base64");
                        assert_eq!(source.media_type, "image/jpeg");
                        // decode and verify it matches the original bytes
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(&source.data)
                            .unwrap();
                        assert_eq!(decoded.len(), jpeg_bytes.len());
                        assert_eq!(decoded, jpeg_bytes);
                    } else {
                        panic!("expected Image block");
                    }
                } else {
                    panic!("expected Blocks variant");
                }
            }
            _ => panic!("expected ToolResult"),
        }

        // also test it works in a full Message (as it would appear in conversation history)
        let user_msg = Message::user_with_content(vec![parsed]);
        let msg_json = serde_json::to_string(&user_msg).unwrap();
        let msg_parsed: Message = serde_json::from_str(&msg_json).unwrap();
        assert_eq!(msg_parsed.role, Role::User);
        assert!(matches!(
            &msg_parsed.content[0],
            MessageContent::ToolResult { .. }
        ));
    }
}
