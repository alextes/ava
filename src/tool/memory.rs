use serde::Deserialize;
use serde_json::json;

use crate::db::{Database, MemoryKind};
use crate::error::Error;
use crate::message::MessageContent;
use crate::runtime::RuntimeState;

use super::{ToolCall, ToolCallResult, ToolDefinition};

pub(crate) const REMEMBER_TOOL_NAME: &str = "remember";
pub(crate) const FORGET_TOOL_NAME: &str = "forget";
pub(crate) const RECALL_TOOL_NAME: &str = "recall";

// --- input structs ---

#[derive(Debug, Deserialize)]
struct RememberInput {
    content: String,
    kind: String,
    category: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ForgetInput {
    kind: String,
    category: Option<String>,
    key: Option<String>,
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RecallInput {
    query: String,
    limit: Option<u32>,
}

// --- tool definitions ---

pub(crate) fn remember_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: REMEMBER_TOOL_NAME,
        description: "store something in long-term memory. kind=fact: structured knowledge (requires category + key, e.g. user/name: alex). kind=episode: events, decisions, context worth preserving. kind=identity: identity traits — name, personality, behavioral preferences (requires key).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "the value or text to remember"
                },
                "kind": {
                    "type": "string",
                    "enum": ["fact", "episode", "identity"],
                    "description": "memory type"
                },
                "category": {
                    "type": "string",
                    "description": "fact namespace (required for kind=fact)"
                },
                "key": {
                    "type": "string",
                    "description": "key within category (required for kind=fact and kind=identity)"
                }
            },
            "required": ["content", "kind"]
        }),
    }
}

pub(crate) fn forget_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: FORGET_TOOL_NAME,
        description: "delete a memory. for facts: provide kind+category+key. for identity traits: provide kind+key. for episodes: provide kind+id.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["fact", "episode", "identity"],
                    "description": "memory type to delete"
                },
                "category": {
                    "type": "string",
                    "description": "fact category (for kind=fact)"
                },
                "key": {
                    "type": "string",
                    "description": "key (for kind=fact or kind=identity)"
                },
                "id": {
                    "type": "integer",
                    "description": "memory id (for kind=episode)"
                }
            },
            "required": ["kind"]
        }),
    }
}

pub(crate) fn recall_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: RECALL_TOOL_NAME,
        description: "search stored memories by keyword or phrase. use this proactively to look up past context when relevant.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "max results to return (default 10, max 50)"
                }
            },
            "required": ["query"]
        }),
    }
}

// --- handlers ---

pub(crate) async fn handle_remember(
    db: &Database,
    call: &ToolCall,
    runtime: Option<&RuntimeState>,
) -> Result<ToolCallResult, Error> {
    match serde_json::from_value::<RememberInput>(call.input.clone()) {
        Ok(input) => {
            let kind = match MemoryKind::from_str(&input.kind) {
                Some(k) => k,
                None => {
                    return Ok(ToolCallResult {
                        content: MessageContent::tool_result(
                            &call.id,
                            format!("invalid kind: {}", input.kind),
                        ),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    });
                }
            };
            let id = db.remember(
                kind,
                &input.content,
                input.category.as_deref(),
                input.key.as_deref(),
            )?;
            if kind == MemoryKind::Identity
                && input.key.as_deref() == Some("name")
                && let Some(runtime) = runtime
            {
                runtime.set_telegram_display_name(input.content.clone());
            }
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, format!("ok (id={id})")),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            })
        }
        Err(err) => Ok(ToolCallResult {
            content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
            switch_provider: None,
            complete: false,
            compact: false,
            voice: None,
        }),
    }
}

pub(crate) async fn handle_forget(db: &Database, call: &ToolCall) -> Result<ToolCallResult, Error> {
    match serde_json::from_value::<ForgetInput>(call.input.clone()) {
        Ok(input) => {
            let deleted = match input.kind.as_str() {
                "fact" => {
                    let cat = input.category.as_deref().unwrap_or("");
                    let key = input.key.as_deref().unwrap_or("");
                    db.forget_fact(cat, key)?
                }
                "identity" => {
                    let key = input.key.as_deref().unwrap_or("");
                    db.forget_identity(key)?
                }
                "episode" => match input.id {
                    Some(id) => db.forget_memory(id)?,
                    None => {
                        return Ok(ToolCallResult {
                            content: MessageContent::tool_result(
                                &call.id,
                                "episode forget requires id",
                            ),
                            switch_provider: None,
                            complete: false,
                            compact: false,
                            voice: None,
                        });
                    }
                },
                other => {
                    return Ok(ToolCallResult {
                        content: MessageContent::tool_result(
                            &call.id,
                            format!("invalid kind: {other}"),
                        ),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    });
                }
            };
            let msg = if deleted { "deleted" } else { "not found" };
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, msg),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            })
        }
        Err(err) => Ok(ToolCallResult {
            content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
            switch_provider: None,
            complete: false,
            compact: false,
            voice: None,
        }),
    }
}

pub(crate) async fn handle_recall(db: &Database, call: &ToolCall) -> Result<ToolCallResult, Error> {
    match serde_json::from_value::<RecallInput>(call.input.clone()) {
        Ok(input) => {
            let limit = input.limit.unwrap_or(10).min(50);
            let memories = db.search_memories(&input.query, limit)?;
            if memories.is_empty() {
                return Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, "no memories found"),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                    voice: None,
                });
            }
            let mut output = String::new();
            for (i, m) in memories.iter().enumerate() {
                if i > 0 {
                    output.push('\n');
                }
                match m.kind {
                    MemoryKind::Fact => {
                        let cat = m.category.as_deref().unwrap_or("?");
                        let key = m.key.as_deref().unwrap_or("?");
                        output.push_str(&format!("[fact] {cat}/{key}: {}", m.content));
                    }
                    MemoryKind::Episode => {
                        let date = m.created_at.split(' ').next().unwrap_or(&m.created_at);
                        output.push_str(&format!("[episode] {date}: {}", m.content));
                    }
                    MemoryKind::Identity => {
                        let key = m.key.as_deref().unwrap_or("?");
                        output.push_str(&format!("[identity] {key}: {}", m.content));
                    }
                }
            }
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, output),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            })
        }
        Err(err) => Ok(ToolCallResult {
            content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
            switch_provider: None,
            complete: false,
            compact: false,
            voice: None,
        }),
    }
}
