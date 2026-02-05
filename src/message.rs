use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<MessageContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
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
            content: content.into(),
        }
    }
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self::user_with_content(vec![MessageContent::text(content)])
    }

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

/// a message coming into the agent
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub channel: ChannelKind,
    pub content: String,
}

/// a message going out from the agent
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub content: String,
}
