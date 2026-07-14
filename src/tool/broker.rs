use serde::{Deserialize, Serialize};

use super::{
    ACTIVATE_SKILL_TOOL_NAME, APPLY_PATCH_TOOL_NAME, BROWSER_TOOL_NAME, CHANNEL_HISTORY_TOOL_NAME,
    COMPACT_CONTEXT_TOOL_NAME, COMPLETE_SETUP_TOOL_NAME, COMPLETE_TOOL_NAME, CRON_TOOL_NAME,
    EXEC_TOOL_NAME, FORGET_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, MANAGE_ACCESS_TOOL_NAME,
    MANAGE_RULES_TOOL_NAME, RECALL_TOOL_NAME, REMEMBER_TOOL_NAME, REQUEST_CONTINUATION_TOOL_NAME,
    SEND_FILE_TOOL_NAME, SEND_PHOTO_TOOL_NAME, SPEAK_TOOL_NAME, SWITCH_MODEL_TOOL_NAME,
    TASKS_TOOL_NAME, TEXT_EDITOR_TOOL_NAME, ToolCall, ToolCallResult, UPGRADE_TOOL_NAME,
    WEB_FETCH_TOOL_NAME, WEB_SEARCH_TOOL_NAME,
};
use crate::chat_buffer::ChatBuffer;
use crate::db::Database;
use crate::error::Error;
use crate::mcp::manager::McpManager;
use crate::runtime::RuntimeState;
use crate::skill::Skill;
use std::sync::RwLock;

/// a security-relevant effect derived by the broker from a raw tool request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Effect {
    ArbitraryCodeExecution,
    ExternalCommunication,
    ExternalInteraction,
    FilesystemRead,
    FilesystemWrite,
    HarnessExtension,
    HarnessModification,
    LocalDataRead,
    NetworkAccess,
    PersistentState,
    PolicyModification,
    ProcessExecution,
    RuntimeControl,
    SecretUse,
    Unknown,
}

impl Effect {
    fn as_str(self) -> &'static str {
        match self {
            Self::ArbitraryCodeExecution => "arbitrary_code_execution",
            Self::ExternalCommunication => "external_communication",
            Self::ExternalInteraction => "external_interaction",
            Self::FilesystemRead => "filesystem_read",
            Self::FilesystemWrite => "filesystem_write",
            Self::HarnessExtension => "harness_extension",
            Self::HarnessModification => "harness_modification",
            Self::LocalDataRead => "local_data_read",
            Self::NetworkAccess => "network_access",
            Self::PersistentState => "persistent_state",
            Self::PolicyModification => "policy_modification",
            Self::ProcessExecution => "process_execution",
            Self::RuntimeControl => "runtime_control",
            Self::SecretUse => "secret_use",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BrokerDecision {
    Approve,
    #[allow(dead_code)] // reserved for the first restrictive broker policy
    Deny {
        reason: String,
    },
}

/// trusted local dependencies used to execute an approved request.
///
/// this exists only while the broker runs in-process. a future isolated broker
/// will receive the raw tool call over IPC and own these resources itself.
pub(crate) struct LocalExecutionContext<'a> {
    client: &'a reqwest::Client,
    db: &'a Database,
    mcp: Option<&'a McpManager>,
    skills: &'a [Skill],
    chat_buffer: Option<&'a ChatBuffer>,
    runtime: Option<&'a RuntimeState>,
    continuation_target: Option<super::ContinuationTarget>,
    vault_secrets: &'a RwLock<Vec<String>>,
}

impl<'a> LocalExecutionContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        client: &'a reqwest::Client,
        db: &'a Database,
        mcp: Option<&'a McpManager>,
        skills: &'a [Skill],
        chat_buffer: Option<&'a ChatBuffer>,
        runtime: Option<&'a RuntimeState>,
        continuation_target: Option<super::ContinuationTarget>,
        vault_secrets: &'a RwLock<Vec<String>>,
    ) -> Self {
        Self {
            client,
            db,
            mcp,
            skills,
            chat_buffer,
            runtime,
            continuation_target,
            vault_secrets,
        }
    }
}

/// the single entry point from the agent into effectful tool execution.
///
/// the initial policy approves every request. keeping review and execution in
/// this component gives us one boundary to move behind IPC and OS isolation.
#[derive(Debug, Default)]
pub(crate) struct LocalToolBroker;

impl LocalToolBroker {
    pub(crate) async fn execute(
        &self,
        context: LocalExecutionContext<'_>,
        call: &ToolCall,
    ) -> Result<ToolCallResult, Error> {
        let effects = classify_effects(call);
        let decision = self.review(call, &effects);
        let effect_names = effects
            .iter()
            .map(|effect| effect.as_str())
            .collect::<Vec<_>>()
            .join(",");

        tracing::info!(
            tool = %call.name,
            effects = %effect_names,
            policy = "approve_all",
            decision = decision_name(&decision),
            "broker reviewed tool execution request"
        );

        match decision {
            BrokerDecision::Approve => {
                let result = super::handle_tool_call(
                    context.client,
                    context.db,
                    context.mcp,
                    context.skills,
                    call,
                    context.chat_buffer,
                    context.runtime,
                    context.continuation_target,
                )
                .await?;

                if call.name == ACTIVATE_SKILL_TOOL_NAME
                    && let Ok(mut secrets) = context.vault_secrets.write()
                {
                    *secrets = super::load_vault_secrets();
                }

                Ok(result)
            }
            BrokerDecision::Deny { reason } => Ok(super::ToolCallResult {
                content: crate::message::MessageContent::tool_result(
                    &call.id,
                    format!("tool execution denied by broker: {reason}"),
                ),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
                attachment: None,
            }),
        }
    }

    fn review(&self, _call: &ToolCall, _effects: &[Effect]) -> BrokerDecision {
        BrokerDecision::Approve
    }
}

fn decision_name(decision: &BrokerDecision) -> &'static str {
    match decision {
        BrokerDecision::Approve => "approve",
        BrokerDecision::Deny { .. } => "deny",
    }
}

fn classify_effects(call: &ToolCall) -> Vec<Effect> {
    use Effect::*;

    let effects = if call.name.starts_with("mcp__") {
        vec![
            ArbitraryCodeExecution,
            ExternalInteraction,
            FilesystemRead,
            FilesystemWrite,
            NetworkAccess,
        ]
    } else {
        match call.name.as_str() {
            EXEC_TOOL_NAME => vec![ArbitraryCodeExecution],
            UPGRADE_TOOL_NAME => vec![
                ArbitraryCodeExecution,
                FilesystemWrite,
                HarnessModification,
                RuntimeControl,
            ],
            ACTIVATE_SKILL_TOOL_NAME => vec![FilesystemRead, HarnessExtension, SecretUse],
            MANAGE_RULES_TOOL_NAME | MANAGE_ACCESS_TOOL_NAME => {
                vec![PersistentState, PolicyModification]
            }
            WEB_SEARCH_TOOL_NAME | WEB_FETCH_TOOL_NAME => vec![NetworkAccess, SecretUse],
            BROWSER_TOOL_NAME => {
                vec![ExternalInteraction, NetworkAccess, ProcessExecution]
            }
            SEND_FILE_TOOL_NAME | SEND_PHOTO_TOOL_NAME => {
                vec![ExternalCommunication, FilesystemRead]
            }
            SPEAK_TOOL_NAME => vec![ExternalInteraction, ProcessExecution],
            TEXT_EDITOR_TOOL_NAME => classify_text_editor(call),
            APPLY_PATCH_TOOL_NAME => classify_file_write(call),
            GREP_TOOL_NAME => vec![FilesystemRead, ProcessExecution],
            GLOB_TOOL_NAME => vec![FilesystemRead],
            REMEMBER_TOOL_NAME | FORGET_TOOL_NAME | CRON_TOOL_NAME | TASKS_TOOL_NAME => {
                vec![PersistentState]
            }
            RECALL_TOOL_NAME | CHANNEL_HISTORY_TOOL_NAME => vec![LocalDataRead],
            COMPLETE_SETUP_TOOL_NAME => vec![PersistentState, RuntimeControl],
            SWITCH_MODEL_TOOL_NAME
            | COMPLETE_TOOL_NAME
            | REQUEST_CONTINUATION_TOOL_NAME
            | COMPACT_CONTEXT_TOOL_NAME => vec![RuntimeControl],
            _ => vec![Unknown],
        }
    };

    deduplicate(effects)
}

fn classify_text_editor(call: &ToolCall) -> Vec<Effect> {
    let command = call
        .input
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if command == "view" {
        vec![Effect::FilesystemRead]
    } else {
        classify_file_write(call)
    }
}

fn classify_file_write(call: &ToolCall) -> Vec<Effect> {
    let mut effects = vec![Effect::FilesystemWrite];
    if targets_harness_source(call) {
        effects.push(Effect::HarnessModification);
    }
    deduplicate(effects)
}

fn targets_harness_source(call: &ToolCall) -> bool {
    let Some(path) = call.input.get("path").and_then(|value| value.as_str()) else {
        return false;
    };
    let path = std::path::PathBuf::from(path);
    let resolved = if path.is_absolute() {
        path
    } else {
        let Ok(cwd) = std::env::current_dir() else {
            return false;
        };
        cwd.join(path)
    };

    let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_root = source_root
        .canonicalize()
        .unwrap_or_else(|_| source_root.to_path_buf());
    let resolved = resolved.canonicalize().unwrap_or(resolved);
    resolved.starts_with(source_root)
}

fn deduplicate(mut effects: Vec<Effect>) -> Vec<Effect> {
    effects.sort_unstable();
    effects.dedup();
    effects
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tool::{BuiltInKind, ToolDefinition, tool_definitions};

    fn call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test".into(),
            name: name.into(),
            input,
        }
    }

    #[test]
    fn approve_all_policy_approves_harness_modification() {
        let broker = LocalToolBroker;
        let call = call(UPGRADE_TOOL_NAME, json!({}));
        let effects = classify_effects(&call);

        assert!(effects.contains(&Effect::HarnessModification));
        assert_eq!(broker.review(&call, &effects), BrokerDecision::Approve);
    }

    #[test]
    fn arbitrary_exec_is_explicitly_high_risk() {
        let effects = classify_effects(&call(EXEC_TOOL_NAME, json!({"command": "true"})));

        assert_eq!(effects, vec![Effect::ArbitraryCodeExecution]);
    }

    #[test]
    fn unknown_tools_are_explicitly_classified() {
        let effects = classify_effects(&call("future_tool", json!({})));

        assert_eq!(effects, vec![Effect::Unknown]);
    }

    #[test]
    fn editing_ava_source_is_a_harness_modification() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/tool/broker.rs")
            .to_string_lossy()
            .to_string();
        let effects = classify_effects(&call(
            APPLY_PATCH_TOOL_NAME,
            json!({"operation": "update_file", "path": path}),
        ));

        assert!(effects.contains(&Effect::FilesystemWrite));
        assert!(effects.contains(&Effect::HarnessModification));
    }

    #[test]
    fn every_builtin_tool_has_a_known_effect_classification() {
        for definition in tool_definitions(false) {
            let name = match definition {
                ToolDefinition::Custom { name, .. } => name,
                ToolDefinition::BuiltIn { kind } => match kind {
                    BuiltInKind::AnthropicTextEditor | BuiltInKind::OpenAiApplyPatch => {
                        kind.tool_name()
                    }
                },
                ToolDefinition::Dynamic { .. } => continue,
            };
            let effects = classify_effects(&call(name, json!({})));
            assert_ne!(effects, vec![Effect::Unknown], "unclassified tool: {name}");
        }
    }

    #[test]
    fn deny_decision_is_ready_for_a_restrictive_policy() {
        let decision = BrokerDecision::Deny {
            reason: "not allowed".into(),
        };

        assert_eq!(decision_name(&decision), "deny");
    }
}
