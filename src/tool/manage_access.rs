use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::message::MessageContent;

use super::{ToolCallResult, ToolDefinition};

pub const MANAGE_ACCESS_TOOL_NAME: &str = "manage_access";

#[derive(Debug, Deserialize)]
struct ManageAccessInput {
    action: String,
    id: Option<i64>,
}

pub fn manage_access_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: MANAGE_ACCESS_TOOL_NAME,
        description: "manage the user and chat whitelists. action=add_user: whitelist a telegram user by id. action=remove_user: remove a user from the whitelist. action=add_chat: whitelist a telegram chat/group by id. action=remove_chat: remove a chat from the whitelist. action=list: show all whitelisted users and chats.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add_user", "remove_user", "add_chat", "remove_chat", "list"],
                    "description": "action to perform"
                },
                "id": {
                    "type": "integer",
                    "description": "user_id or chat_id (required for add/remove actions)"
                }
            },
            "required": ["action"]
        }),
    }
}

pub fn handle_manage_access(
    db: &Database,
    call_id: &str,
    input: &serde_json::Value,
) -> ToolCallResult {
    let parsed: ManageAccessInput = match serde_json::from_value(input.clone()) {
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
        "add_user" => match parsed.id {
            Some(id) => match db.add_allowed_user(id, "agent") {
                Ok(()) => format!("added user {id} to the whitelist."),
                Err(e) => format!("error adding user: {e}"),
            },
            None => "error: id is required for add_user.".into(),
        },
        "remove_user" => match parsed.id {
            Some(id) => match db.remove_allowed_user(id) {
                Ok(()) => format!("removed user {id} from the whitelist."),
                Err(e) => format!("error removing user: {e}"),
            },
            None => "error: id is required for remove_user.".into(),
        },
        "add_chat" => match parsed.id {
            Some(id) => match db.add_allowed_chat(id, "agent") {
                Ok(()) => format!("added chat {id} to the whitelist."),
                Err(e) => format!("error adding chat: {e}"),
            },
            None => "error: id is required for add_chat.".into(),
        },
        "remove_chat" => match parsed.id {
            Some(id) => match db.remove_allowed_chat(id) {
                Ok(()) => format!("removed chat {id} from the whitelist."),
                Err(e) => format!("error removing chat: {e}"),
            },
            None => "error: id is required for remove_chat.".into(),
        },
        "list" => {
            let users = db.list_allowed_users().unwrap_or_default();
            let chats = db.list_allowed_chats().unwrap_or_default();
            let mut output = String::from("allowed users:");
            if users.is_empty() {
                output.push_str(" (none)");
            } else {
                for id in &users {
                    output.push_str(&format!("\n  - {id}"));
                }
            }
            output.push_str("\n\nallowed chats:");
            if chats.is_empty() {
                output.push_str(" (none)");
            } else {
                for id in &chats {
                    output.push_str(&format!("\n  - {id}"));
                }
            }
            output
        }
        other => format!("unknown action: {other}."),
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
