use serde_json::json;

use super::{ToolCallResult, ToolDefinition};
use crate::db::{Database, MemoryKind};
use crate::error::Error;
use crate::message::MessageContent;
use crate::runtime::RuntimeState;

pub const COMPLETE_SETUP_TOOL_NAME: &str = "complete_setup";

pub(super) fn complete_setup_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: COMPLETE_SETUP_TOOL_NAME,
        description: "mark initial setup as complete. call this once the user has chosen a name \
                      and optionally specified identity traits. this finalizes the setup process.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "the name the user chose for this agent"
                }
            },
            "required": ["name"]
        }),
    }
}

#[derive(Debug, serde::Deserialize)]
struct CompleteSetupInput {
    name: String,
}

pub(super) fn handle_complete_setup(
    db: &Database,
    runtime: Option<&RuntimeState>,
    call_id: &str,
    input: &serde_json::Value,
) -> Result<ToolCallResult, Error> {
    let parsed: CompleteSetupInput = match serde_json::from_value(input.clone()) {
        Ok(i) => i,
        Err(err) => {
            return Ok(ToolCallResult {
                content: MessageContent::tool_result(call_id, format!("invalid input: {err}")),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            });
        }
    };

    let name = parsed.name.trim();
    if name.is_empty() {
        return Ok(ToolCallResult {
            content: MessageContent::tool_result(
                call_id,
                "name cannot be empty. ask the user what they'd like to call you.",
            ),
            switch_provider: None,
            complete: false,
            compact: false,
            voice: None,
        });
    }

    // store the name as an identity trait
    db.remember(MemoryKind::Identity, name, None, Some("name"))?;
    if let Some(runtime) = runtime {
        runtime.set_telegram_display_name(name);
    }

    // mark setup as complete
    db.mark_setup_complete()?;

    tracing::info!(name, "initial setup completed");

    Ok(ToolCallResult {
        content: MessageContent::tool_result(
            call_id,
            format!(
                "setup complete! your name is now \"{name}\". identity traits can be updated \
                 at any time using the remember tool with kind=identity."
            ),
        ),
        switch_provider: None,
        complete: false,
        compact: false,
        voice: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complete_setup_definition() {
        let def = complete_setup_definition();
        assert_eq!(def.name(), COMPLETE_SETUP_TOOL_NAME);
    }

    #[test]
    fn test_handle_complete_setup() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({"name": "ren"});
        let result = handle_complete_setup(&db, None, "test_id", &input).unwrap();

        let text = match &result.content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected ToolResult"),
        };
        assert!(text.contains("ren"), "result: {text}");

        // verify name was stored
        let traits = db.identity_traits().unwrap();
        let name_trait = traits.iter().find(|t| t.key.as_deref() == Some("name"));
        assert_eq!(name_trait.unwrap().content, "ren");

        // verify setup_completed flag was set
        assert!(db.is_setup_complete().unwrap());
    }

    #[test]
    fn test_handle_complete_setup_empty_name() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({"name": "  "});
        let result = handle_complete_setup(&db, None, "test_id", &input).unwrap();

        let text = match &result.content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected ToolResult"),
        };
        assert!(text.contains("cannot be empty"), "result: {text}");
        assert!(!db.is_setup_complete().unwrap());
    }
}
