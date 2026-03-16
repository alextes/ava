mod complete;
mod cron;
mod exec;
mod filesystem;
mod memory;
mod search;
mod tasks;
mod upgrade;
mod web;

use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Database;
use crate::error::Error;
use crate::mcp::manager::McpManager;
use crate::message::MessageContent;
use crate::provider::AnyProvider;

pub use complete::COMPLETE_TOOL_NAME;
pub use cron::CRON_TOOL_NAME;
pub use exec::{EXEC_TOOL_NAME, references_sensitive_env};
pub use filesystem::TEXT_EDITOR_TOOL_NAME;
pub use search::{GLOB_TOOL_NAME, GREP_TOOL_NAME};
pub use tasks::TASKS_TOOL_NAME;
pub use upgrade::UPGRADE_TOOL_NAME;
pub use web::{WEB_FETCH_TOOL_NAME, WEB_SEARCH_TOOL_NAME};

pub(crate) use memory::{FORGET_TOOL_NAME, RECALL_TOOL_NAME, REMEMBER_TOOL_NAME};

pub const SWITCH_MODEL_TOOL_NAME: &str = "switch_model";
pub const MANAGE_RULES_TOOL_NAME: &str = "manage_rules";

// --- tool call types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum ToolDefinition {
    Custom {
        name: &'static str,
        description: &'static str,
        input_schema: serde_json::Value,
    },
    BuiltIn {
        tool_type: &'static str,
        name: &'static str,
    },
    /// dynamically defined tool (e.g. from an MCP server).
    Dynamic {
        name: String,
        description: String,
        input_schema: serde_json::Value,
    },
}

impl ToolDefinition {
    pub fn name(&self) -> &str {
        match self {
            Self::Custom { name, .. } => name,
            Self::BuiltIn { name, .. } => name,
            Self::Dynamic { name, .. } => name,
        }
    }
}

pub struct ToolCallResult {
    pub content: MessageContent,
    pub switch_provider: Option<AnyProvider>,
    pub complete: bool,
}

// --- approver trait ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowAlways { pattern: String },
    Deny,
    AutoApproved,
}

pub trait Approver: Send + Sync {
    fn request_approval(
        &self,
        tool_call: &ToolCall,
    ) -> impl Future<Output = Result<ApprovalDecision, Error>> + Send;
}

/// returns true if this tool call requires approval
pub fn requires_approval(tool_call: &ToolCall) -> bool {
    match tool_call.name.as_str() {
        EXEC_TOOL_NAME => true,
        MANAGE_RULES_TOOL_NAME => tool_call
            .input
            .get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|a| a == "add"),
        TEXT_EDITOR_TOOL_NAME => {
            let cmd = tool_call
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            matches!(cmd, "str_replace" | "create" | "insert")
        }
        _ => false,
    }
}

// --- tool definitions ---

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        memory::remember_definition(),
        memory::forget_definition(),
        memory::recall_definition(),
        exec::exec_definition(),
        web::web_search_definition(),
        web::web_fetch_definition(),
        switch_model_definition(),
        manage_rules_definition(),
        cron::cron_definition(),
        tasks::tasks_definition(),
        complete::complete_definition(),
        upgrade::upgrade_definition(),
        search::grep_definition(),
        search::glob_definition(),
        ToolDefinition::BuiltIn {
            tool_type: "text_editor_20250728",
            name: TEXT_EDITOR_TOOL_NAME,
        },
    ]
}

// --- tool dispatch ---

#[derive(Debug, Deserialize)]
struct SwitchModelInput {
    provider: String,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManageRulesInput {
    action: String,
    id: Option<i64>,
    pattern: Option<String>,
}

pub async fn handle_tool_call(
    client: &reqwest::Client,
    db: &Database,
    mcp: Option<&McpManager>,
    call: &ToolCall,
) -> Result<ToolCallResult, Error> {
    tracing::info!(tool = %call.name, input = %call.input, "handling tool call");

    // route MCP tool calls (prefixed with "mcp__")
    if call.name.starts_with("mcp__") {
        return handle_mcp_tool_call(mcp, call).await;
    }

    match call.name.as_str() {
        REMEMBER_TOOL_NAME => memory::handle_remember(db, call).await,
        FORGET_TOOL_NAME => memory::handle_forget(db, call).await,
        RECALL_TOOL_NAME => memory::handle_recall(db, call).await,
        EXEC_TOOL_NAME => {
            let result = exec::handle_exec(call).await;
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, result),
                switch_provider: None,
                complete: false,
            })
        }
        WEB_SEARCH_TOOL_NAME => {
            let result = web::handle_web_search(client, call).await;
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, result),
                switch_provider: None,
                complete: false,
            })
        }
        WEB_FETCH_TOOL_NAME => {
            let result = web::handle_web_fetch(client, call).await;
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, result),
                switch_provider: None,
                complete: false,
            })
        }
        GREP_TOOL_NAME => {
            let result = search::handle_grep(call).await;
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, result),
                switch_provider: None,
                complete: false,
            })
        }
        GLOB_TOOL_NAME => {
            let result = search::handle_glob(call);
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, result),
                switch_provider: None,
                complete: false,
            })
        }
        TEXT_EDITOR_TOOL_NAME => {
            let result = filesystem::handle_text_editor(call).await;
            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, result),
                switch_provider: None,
                complete: false,
            })
        }
        CRON_TOOL_NAME => Ok(cron::handle_cron(db, &call.id, &call.input)),
        TASKS_TOOL_NAME => Ok(tasks::handle_tasks(db, &call.id, &call.input)),
        COMPLETE_TOOL_NAME => Ok(complete::handle_complete(&call.id, &call.input)),
        UPGRADE_TOOL_NAME => Ok(upgrade::handle_upgrade(&call.id)),
        SWITCH_MODEL_TOOL_NAME => {
            match serde_json::from_value::<SwitchModelInput>(call.input.clone()) {
                Ok(input) => {
                    match AnyProvider::from_name(
                        client.clone(),
                        &input.provider,
                        input.model.as_deref(),
                    ) {
                        Ok(provider) => {
                            let model_info = input.model.as_deref().unwrap_or("default");
                            Ok(ToolCallResult {
                                content: MessageContent::tool_result(
                                    &call.id,
                                    format!(
                                        "switched to provider: {}, model: {model_info}",
                                        input.provider
                                    ),
                                ),
                                switch_provider: Some(provider),
                                complete: false,
                            })
                        }
                        Err(err) => Ok(ToolCallResult {
                            content: MessageContent::tool_result(
                                &call.id,
                                format!("failed to switch: {err}"),
                            ),
                            switch_provider: None,
                            complete: false,
                        }),
                    }
                }
                Err(err) => Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                    switch_provider: None,
                    complete: false,
                }),
            }
        }
        MANAGE_RULES_TOOL_NAME => {
            match serde_json::from_value::<ManageRulesInput>(call.input.clone()) {
                Ok(input) => match input.action.as_str() {
                    "list" => {
                        let rules = db.list_approval_rules()?;
                        if rules.is_empty() {
                            return Ok(ToolCallResult {
                                content: MessageContent::tool_result(
                                    &call.id,
                                    "no approval rules saved",
                                ),
                                switch_provider: None,
                                complete: false,
                            });
                        }
                        let mut output = String::new();
                        for (i, rule) in rules.iter().enumerate() {
                            if i > 0 {
                                output.push('\n');
                            }
                            output.push_str(&format!("id={}: {}", rule.id, rule.pattern));
                        }
                        Ok(ToolCallResult {
                            content: MessageContent::tool_result(&call.id, output),
                            switch_provider: None,
                            complete: false,
                        })
                    }
                    "add" => match input.pattern {
                        Some(ref pattern) if !pattern.trim().is_empty() => {
                            db.save_approval_rule(pattern.trim())?;
                            Ok(ToolCallResult {
                                content: MessageContent::tool_result(
                                    &call.id,
                                    format!("saved rule: {}", pattern.trim()),
                                ),
                                switch_provider: None,
                                complete: false,
                            })
                        }
                        _ => Ok(ToolCallResult {
                            content: MessageContent::tool_result(
                                &call.id,
                                "add requires a non-empty pattern",
                            ),
                            switch_provider: None,
                            complete: false,
                        }),
                    },
                    "delete" => match input.id {
                        Some(id) => {
                            let deleted = db.delete_approval_rule(id)?;
                            let msg = if deleted { "deleted" } else { "not found" };
                            Ok(ToolCallResult {
                                content: MessageContent::tool_result(&call.id, msg),
                                switch_provider: None,
                                complete: false,
                            })
                        }
                        None => Ok(ToolCallResult {
                            content: MessageContent::tool_result(&call.id, "delete requires id"),
                            switch_provider: None,
                            complete: false,
                        }),
                    },
                    other => Ok(ToolCallResult {
                        content: MessageContent::tool_result(
                            &call.id,
                            format!("invalid action: {other}"),
                        ),
                        switch_provider: None,
                        complete: false,
                    }),
                },
                Err(err) => Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                    switch_provider: None,
                    complete: false,
                }),
            }
        }
        _ => {
            tracing::warn!(tool = %call.name, "unknown tool");
            Ok(ToolCallResult {
                content: MessageContent::tool_result(
                    &call.id,
                    format!("unknown tool: {}", call.name),
                ),
                switch_provider: None,
                complete: false,
            })
        }
    }
}

// --- tool definition builders ---

fn switch_model_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: SWITCH_MODEL_TOOL_NAME,
        description: "switch the ai provider and model for the remainder of this conversation. use this to delegate to a different model (e.g. a cheaper one for simple tasks, or a more capable one for hard tasks).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "provider": {
                    "type": "string",
                    "enum": ["anthropic", "openai"],
                    "description": "the provider to switch to"
                },
                "model": {
                    "type": "string",
                    "enum": ["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5", "gpt-5.4", "gpt-5-mini"],
                    "description": "model name. must match the chosen provider. anthropic: claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5. openai: gpt-5.4, gpt-5-mini. if omitted, uses the provider's default."
                }
            },
            "required": ["provider"]
        }),
    }
}

fn manage_rules_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: MANAGE_RULES_TOOL_NAME,
        description: "manage approval rules for command execution. action=list: show all saved rules. action=add: propose a new rule (requires human approval). action=delete: remove a rule by id. patterns use wildcard matching (e.g. 'cargo *' matches any cargo subcommand).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "add", "delete"],
                    "description": "action to perform"
                },
                "pattern": {
                    "type": "string",
                    "description": "approval pattern to add (required for action=add). supports wildcards, e.g. 'cargo *', 'git push *'"
                },
                "id": {
                    "type": "integer",
                    "description": "rule id to delete (required for action=delete)"
                }
            },
            "required": ["action"]
        }),
    }
}

/// handle a tool call for an MCP-namespaced tool (mcp__<server>_<tool>).
async fn handle_mcp_tool_call(
    mcp: Option<&McpManager>,
    call: &ToolCall,
) -> Result<ToolCallResult, Error> {
    let Some(mcp) = mcp else {
        return Ok(ToolCallResult {
            content: MessageContent::tool_result(&call.id, "MCP not available"),
            switch_provider: None,
            complete: false,
        });
    };

    // parse "mcp__<server>_<tool>" — first underscore after "mcp__" separates server from tool
    let rest = &call.name["mcp__".len()..];
    let Some((server_name, tool_name)) = rest.split_once('_') else {
        return Ok(ToolCallResult {
            content: MessageContent::tool_result(
                &call.id,
                format!("invalid MCP tool name: {}", call.name),
            ),
            switch_provider: None,
            complete: false,
        });
    };

    match mcp
        .call_tool(server_name, tool_name, call.input.clone())
        .await
    {
        Ok(result) => {
            let text = result
                .content
                .iter()
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n");

            let text = if result.is_error == Some(true) {
                format!("[MCP error] {text}")
            } else {
                text
            };

            Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, text),
                switch_provider: None,
                complete: false,
            })
        }
        Err(e) => Ok(ToolCallResult {
            content: MessageContent::tool_result(&call.id, format!("MCP tool call failed: {e}")),
            switch_provider: None,
            complete: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approver::CliApprover;
    use crate::db::{Database, MemoryKind};

    #[test]
    fn test_requires_approval_exec() {
        let call = ToolCall {
            id: "test".into(),
            name: EXEC_TOOL_NAME.into(),
            input: json!({"command": "ls"}),
        };
        assert!(requires_approval(&call));
    }

    #[test]
    fn test_requires_approval_remember() {
        let call = ToolCall {
            id: "test".into(),
            name: REMEMBER_TOOL_NAME.into(),
            input: json!({"content": "alex", "kind": "fact", "category": "user", "key": "name"}),
        };
        assert!(!requires_approval(&call));
    }

    #[test]
    fn test_requires_approval_forget() {
        let call = ToolCall {
            id: "test".into(),
            name: FORGET_TOOL_NAME.into(),
            input: json!({"kind": "fact", "category": "user", "key": "name"}),
        };
        assert!(!requires_approval(&call));
    }

    #[test]
    fn test_requires_approval_recall() {
        let call = ToolCall {
            id: "test".into(),
            name: RECALL_TOOL_NAME.into(),
            input: json!({"query": "rust"}),
        };
        assert!(!requires_approval(&call));
    }

    #[tokio::test]
    async fn test_cli_approver_auto_approves() {
        let approver = CliApprover;
        let call = ToolCall {
            id: "test".into(),
            name: EXEC_TOOL_NAME.into(),
            input: json!({"command": "ls"}),
        };
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }

    // --- handle_tool_call integration tests ---

    fn make_call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test_id".into(),
            name: name.into(),
            input,
        }
    }

    fn extract_tool_result_text(content: &MessageContent) -> String {
        match content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected ToolResult"),
        }
    }

    // remember tool

    #[tokio::test]
    async fn test_handle_remember_fact() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "remember",
            json!({"content": "alex", "kind": "fact", "category": "user", "key": "name"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.starts_with("ok (id="));
        assert!(result.switch_provider.is_none());

        // verify persisted
        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "alex");
    }

    #[tokio::test]
    async fn test_handle_remember_episode() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "remember",
            json!({"content": "discussed migration plan", "kind": "episode"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert!(extract_tool_result_text(&result.content).starts_with("ok (id="));

        let episodes = db.recent_episodes().unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].content, "discussed migration plan");
    }

    #[tokio::test]
    async fn test_handle_remember_character() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "remember",
            json!({"content": "formal and precise", "kind": "character", "key": "tone"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert!(extract_tool_result_text(&result.content).starts_with("ok (id="));

        let traits = db.character_traits().unwrap();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].content, "formal and precise");
    }

    #[tokio::test]
    async fn test_handle_remember_invalid_kind() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("remember", json!({"content": "test", "kind": "bogus"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "invalid kind: bogus"
        );
    }

    #[tokio::test]
    async fn test_handle_remember_missing_fields() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("remember", json!({"kind": "fact"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.starts_with("invalid input:"));
    }

    #[tokio::test]
    async fn test_handle_remember_fact_upserts() {
        let db = Database::open_in_memory().unwrap();
        let call1 = make_call(
            "remember",
            json!({"content": "v1", "kind": "fact", "category": "user", "key": "name"}),
        );
        let call2 = make_call(
            "remember",
            json!({"content": "v2", "kind": "fact", "category": "user", "key": "name"}),
        );
        handle_tool_call(&reqwest::Client::new(), &db, None, &call1)
            .await
            .unwrap();
        handle_tool_call(&reqwest::Client::new(), &db, None, &call2)
            .await
            .unwrap();

        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "v2");
    }

    // forget tool

    #[tokio::test]
    async fn test_handle_forget_fact() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();

        let call = make_call(
            "forget",
            json!({"kind": "fact", "category": "user", "key": "name"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
        assert!(db.recent_facts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_forget_character() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Character, "formal", None, Some("tone"))
            .unwrap();

        let call = make_call("forget", json!({"kind": "character", "key": "tone"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
        assert!(db.character_traits().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_forget_episode_by_id() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .remember(MemoryKind::Episode, "some event", None, None)
            .unwrap();

        let call = make_call("forget", json!({"kind": "episode", "id": id}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
    }

    #[tokio::test]
    async fn test_handle_forget_episode_missing_id() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("forget", json!({"kind": "episode"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "episode forget requires id"
        );
    }

    #[tokio::test]
    async fn test_handle_forget_not_found() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "forget",
            json!({"kind": "fact", "category": "user", "key": "nonexistent"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "not found");
    }

    #[tokio::test]
    async fn test_handle_forget_invalid_kind() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("forget", json!({"kind": "bogus"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "invalid kind: bogus"
        );
    }

    // recall tool

    #[tokio::test]
    async fn test_handle_recall_finds_results() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "loves rust programming",
            Some("user"),
            Some("hobby"),
        )
        .unwrap();
        db.remember(
            MemoryKind::Episode,
            "discussed python migration",
            None,
            None,
        )
        .unwrap();
        db.remember(MemoryKind::Character, "formal", None, Some("tone"))
            .unwrap();

        let call = make_call("recall", json!({"query": "rust"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("[fact] user/hobby: loves rust programming"));
    }

    #[tokio::test]
    async fn test_handle_recall_no_results() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("recall", json!({"query": "nonexistent"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "no memories found"
        );
    }

    #[tokio::test]
    async fn test_handle_recall_formats_all_kinds() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "rust developer",
            Some("user"),
            Some("role"),
        )
        .unwrap();
        db.remember(MemoryKind::Episode, "discussed rust project", None, None)
            .unwrap();
        db.remember(
            MemoryKind::Character,
            "rust enthusiast",
            None,
            Some("personality"),
        )
        .unwrap();

        let call = make_call("recall", json!({"query": "rust"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("[fact]"));
        assert!(text.contains("[episode]"));
        assert!(text.contains("[character]"));
    }

    // manage_rules tool

    #[tokio::test]
    async fn test_handle_manage_rules_list_empty() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "list"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "no approval rules saved"
        );
    }

    #[tokio::test]
    async fn test_handle_manage_rules_list() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        db.save_approval_rule("cargo *").unwrap();

        let call = make_call("manage_rules", json!({"action": "list"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("ls *"));
        assert!(text.contains("cargo *"));
    }

    #[tokio::test]
    async fn test_handle_manage_rules_delete() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        let rules = db.list_approval_rules().unwrap();

        let call = make_call(
            "manage_rules",
            json!({"action": "delete", "id": rules[0].id}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
        assert!(db.list_approval_rules().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_manage_rules_delete_not_found() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "delete", "id": 999}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "not found");
    }

    #[tokio::test]
    async fn test_handle_manage_rules_delete_missing_id() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "delete"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "delete requires id"
        );
    }

    #[tokio::test]
    async fn test_handle_manage_rules_invalid_action() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "update"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "invalid action: update"
        );
    }

    #[tokio::test]
    async fn test_handle_manage_rules_add() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "manage_rules",
            json!({"action": "add", "pattern": "cargo *"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "saved rule: cargo *"
        );

        let rules = db.list_approval_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "cargo *");
    }

    #[tokio::test]
    async fn test_handle_manage_rules_add_missing_pattern() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "add"}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "add requires a non-empty pattern"
        );
    }

    #[tokio::test]
    async fn test_handle_manage_rules_add_empty_pattern() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "add", "pattern": "  "}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "add requires a non-empty pattern"
        );
    }

    #[tokio::test]
    async fn test_requires_approval_manage_rules_add() {
        let call = make_call(
            "manage_rules",
            json!({"action": "add", "pattern": "cargo *"}),
        );
        assert!(requires_approval(&call));
    }

    #[tokio::test]
    async fn test_requires_approval_manage_rules_list() {
        let call = make_call("manage_rules", json!({"action": "list"}));
        assert!(!requires_approval(&call));
    }

    #[tokio::test]
    async fn test_requires_approval_manage_rules_delete() {
        let call = make_call("manage_rules", json!({"action": "delete", "id": 1}));
        assert!(!requires_approval(&call));
    }

    // text_editor tool

    #[tokio::test]
    async fn test_handle_text_editor_view_not_found() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "str_replace_based_edit_tool",
            json!({"command": "view", "path": "/tmp/nonexistent_ava_test_12345.txt"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(
            text.contains("does not exist"),
            "expected 'does not exist' but got: {text}"
        );
    }

    #[tokio::test]
    async fn test_handle_text_editor_unknown_command() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "str_replace_based_edit_tool",
            json!({"command": "bogus", "path": "/tmp/whatever"}),
        );
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(
            text.contains("unknown command"),
            "expected 'unknown command' but got: {text}"
        );
    }

    // unknown tool

    #[tokio::test]
    async fn test_handle_unknown_tool() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("nonexistent_tool", json!({}));
        let result = handle_tool_call(&reqwest::Client::new(), &db, None, &call)
            .await
            .unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("unknown tool: nonexistent_tool"));
    }

    #[test]
    fn test_requires_approval_complete() {
        let call = make_call("complete", json!({}));
        assert!(!requires_approval(&call));
    }
}
