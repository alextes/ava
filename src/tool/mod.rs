use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Database;
use crate::error::Error;
use crate::message::MessageContent;

pub const REMEMBER_FACT_TOOL_NAME: &str = "remember_fact";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![remember_fact_definition()]
}

#[derive(Debug, Deserialize)]
struct RememberFactInput {
    category: String,
    key: String,
    value: String,
}

pub fn handle_tool_calls(
    db: &Database,
    tool_calls: &[ToolCall],
) -> Result<Vec<MessageContent>, Error> {
    let mut results = Vec::new();

    for call in tool_calls {
        match call.name.as_str() {
            REMEMBER_FACT_TOOL_NAME => {
                match serde_json::from_value::<RememberFactInput>(call.input.clone()) {
                    Ok(input) => {
                        db.remember_fact(&input.category, &input.key, &input.value)?;
                        results.push(MessageContent::tool_result(&call.id, "ok"));
                    }
                    Err(err) => {
                        results.push(MessageContent::tool_result(
                            &call.id,
                            format!("invalid input: {err}"),
                        ));
                    }
                }
            }
            _ => {
                results.push(MessageContent::tool_result(
                    &call.id,
                    format!("unknown tool: {}", call.name),
                ));
            }
        }
    }

    Ok(results)
}

fn remember_fact_definition() -> ToolDefinition {
    ToolDefinition {
        name: REMEMBER_FACT_TOOL_NAME,
        description: "store a user fact for future conversations",
        input_schema: json!({
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "description": "fact namespace, such as user or preferences"
                },
                "key": {
                    "type": "string",
                    "description": "fact key within the category"
                },
                "value": {
                    "type": "string",
                    "description": "fact value to store"
                }
            },
            "required": ["category", "key", "value"]
        }),
    }
}
