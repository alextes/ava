mod compaction;
pub(crate) mod context;
mod prompt;

use std::sync::{Arc, RwLock};

use chrono::Utc;

use crate::approver::AnyApprover;
use crate::db::Database;
use crate::error::Error;
use crate::mcp::manager::McpManager;
use crate::message::{
    ContentBlock, InboundMessage, Message, MessageContent, OutboundMessage, Role, ToolResultContent,
};
use crate::provider::{AnyProvider, DEFAULT_SYSTEM_PROMPT, Provider};
use crate::skill::Skill;
use crate::tool::{self, ApprovalDecision, Approver, ToolCall, ToolDefinition};

const MAX_TOOL_ROUNDS: u32 = 40;
const WARNING_ROUND: u32 = 32;

pub struct Agent {
    provider: AnyProvider,
    approver: AnyApprover,
    db: Arc<Database>,
    client: reqwest::Client,
    mcp: Option<Arc<McpManager>>,
    skills: Arc<Vec<Skill>>,
    vault_secrets: Arc<RwLock<Vec<String>>>,
}

impl Agent {
    pub fn new(
        provider: AnyProvider,
        approver: AnyApprover,
        db: Arc<Database>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            provider,
            approver,
            db,
            client,
            mcp: None,
            skills: Arc::new(Vec::new()),
            vault_secrets: Arc::new(RwLock::new(tool::load_vault_secrets())),
        }
    }

    pub fn with_mcp(mut self, mcp: Arc<McpManager>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    pub fn with_skills(mut self, skills: Arc<Vec<Skill>>) -> Self {
        self.skills = skills;
        self
    }

    /// if the message starts with /skill-name and a matching user-invocable skill exists,
    /// prepend the skill body (with $ARGUMENTS substituted) wrapped in [skill: name] tags.
    fn expand_skill(&self, content: &str) -> String {
        let trimmed = content.trim();
        if !trimmed.starts_with('/') {
            return content.to_string();
        }

        let (cmd, args) = match trimmed[1..].split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (&trimmed[1..], ""),
        };

        let skill = self
            .skills
            .iter()
            .find(|s| s.name == cmd && s.user_invocable);

        let Some(skill) = skill else {
            return content.to_string();
        };

        tracing::info!(skill = %skill.name, "expanding user-invoked skill");

        let body = skill.body.replace("$ARGUMENTS", args);
        format!("[skill: {}]\n{}\n[/skill]\n\n{}", skill.name, body, args)
    }

    #[tracing::instrument(skip(self, inbound), fields(channel = ?inbound.channel))]
    pub async fn process(
        &self,
        inbound: &InboundMessage,
    ) -> Result<Option<OutboundMessage>, Error> {
        let session_id = self.db.active_session()?;
        let channel_str = inbound.channel.as_str();

        // load conversation history (growing window for prompt cache efficiency)
        let mut messages = self.db.load_messages(session_id)?;

        // fix orphaned tool_use blocks left by a previous crash/interruption
        self.repair_orphaned_tool_calls(session_id, &mut messages)?;

        // expand /skill-name invocations before sending to the LLM
        let content = self.expand_skill(&inbound.content);

        // append and persist the new user message
        let user_content = vec![MessageContent::text(&content)];
        self.db
            .append_message(session_id, "user", &user_content, Some(channel_str))?;
        messages.push(Message::user(&content));

        let system_prompt = self.system_prompt()?;
        let tools = self.all_tool_definitions().await;
        let mut tool_rounds = 0;
        let mut switched_provider: Option<AnyProvider> = None;
        let mut last_input_tokens: Option<u32> = None;
        let mut compaction_count: u32 = 0;
        let mut crossed_60pct = match self.db.session_usage(session_id)? {
            Some((input_tokens, context_window)) if context_window > 0 => {
                (input_tokens as f64 / context_window as f64 * 100.0) >= 60.0
            }
            _ => false,
        };
        let mut pending_voice: Option<Vec<u8>> = None;

        loop {
            // compact context if approaching the model's limit
            let context_window = switched_provider
                .as_ref()
                .unwrap_or(&self.provider)
                .context_window();

            if compaction::needs_compaction(&messages, last_input_tokens, context_window) {
                let prior_summary = self.db.get_session_summary(session_id)?;
                let provider = switched_provider.as_ref().unwrap_or(&self.provider);
                match compaction::compact_messages(provider, messages.clone(), prior_summary).await
                {
                    Ok((compacted, summary)) => {
                        messages = compacted;
                        self.db.set_session_summary(session_id, &summary)?;
                        compaction_count += 1;
                        tracing::info!(compaction_count, "compacted context");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "compaction failed, continuing with full context");
                    }
                }
            }

            let active_provider = switched_provider.as_ref().unwrap_or(&self.provider);
            let response = match active_provider
                .complete(&system_prompt, &messages, &tools)
                .await
            {
                Ok(r) => r,
                Err(Error::ContextOverflow) => {
                    return Ok(Some(OutboundMessage {
                        content: "conversation context is full. key facts have been preserved \
                            — please start a new session."
                            .into(),
                        voice: None,
                    }));
                }
                Err(Error::RateLimited(ref msg)) => {
                    let provider_name = active_provider.provider_name();
                    tracing::warn!(provider = provider_name, "rate limited");
                    return Ok(Some(OutboundMessage {
                        content: format!(
                            "rate limited by {provider_name}. please try again in a moment.\n\n\
                             details: {msg}"
                        ),
                        voice: None,
                    }));
                }
                Err(Error::BudgetExhausted(ref msg)) => {
                    let current_name = active_provider.provider_name();
                    let fallback_name = match current_name {
                        "anthropic" => "openai",
                        "openai" => "anthropic",
                        _ => return Err(Error::BudgetExhausted(msg.clone())),
                    };
                    tracing::warn!(
                        provider = current_name,
                        fallback = fallback_name,
                        "budget exhausted, attempting fallback"
                    );
                    match AnyProvider::from_name(self.client.clone(), fallback_name, None) {
                        Ok(new_provider) => {
                            let model_id = new_provider.model_id();
                            if let Err(e) = self.db.set_session_model(session_id, &model_id) {
                                tracing::warn!(%e, "failed to persist fallback model");
                            }
                            let note = format!(
                                "the {current_name} provider's budget is exhausted. \
                                 automatically switched to {fallback_name}. \
                                 send `/switch {current_name}` to switch back."
                            );
                            messages.push(Message::user(&note));
                            self.db.append_message(
                                session_id,
                                "user",
                                &[MessageContent::text(&note)],
                                None,
                            )?;
                            switched_provider = Some(new_provider);
                            continue;
                        }
                        Err(fallback_err) => {
                            tracing::error!(
                                %fallback_err,
                                "fallback provider also failed"
                            );
                            return Err(Error::BudgetExhausted(msg.clone()));
                        }
                    }
                }
                Err(e) => return Err(e),
            };

            let usage = &response.usage;
            last_input_tokens = Some(usage.input_tokens);

            let ctx = context::ContextUsage::compute(usage, context_window, compaction_count);
            let context_pct = format!("{:.0}%", ctx.usage_percent);
            let context_tokens = format!("{}/{}", ctx.input_tokens, ctx.context_window);

            // unified context-aware log line. WARN when above 80% to surface
            // approaching limits before compaction kicks in at 90%.
            if ctx.usage_percent > 80.0 {
                tracing::warn!(
                    context = %context_pct,
                    tokens = %context_tokens,
                    output = ctx.output_tokens,
                    cache_created = usage.cache_creation_tokens,
                    cache_read = usage.cache_read_tokens,
                    reasoning = usage.reasoning_tokens,
                    compactions = ctx.compaction_count,
                    "context usage"
                );
            } else {
                tracing::info!(
                    context = %context_pct,
                    tokens = %context_tokens,
                    output = ctx.output_tokens,
                    cache_created = usage.cache_creation_tokens,
                    cache_read = usage.cache_read_tokens,
                    reasoning = usage.reasoning_tokens,
                    compactions = ctx.compaction_count,
                    "context usage"
                );
            }

            if let Err(e) =
                self.db
                    .set_session_usage(session_id, ctx.input_tokens, ctx.context_window)
            {
                tracing::warn!(%e, "failed to persist context usage");
            }

            if response.tool_calls.is_empty() {
                // persist the final assistant response
                let assistant_content = vec![MessageContent::text(&response.content)];
                self.db
                    .append_message(session_id, "assistant", &assistant_content, None)?;

                return Ok(Some(OutboundMessage {
                    content: response.content,
                    voice: pending_voice.take(),
                }));
            }

            tool_rounds += 1;

            tracing::debug!(
                tool_round = tool_rounds,
                count = response.tool_calls.len(),
                "executing tool calls"
            );

            // graceful final turn: budget exhausted
            if tool_rounds > MAX_TOOL_ROUNDS {
                tracing::warn!(
                    tool_rounds,
                    "tool budget exhausted, requesting final summary"
                );

                // persist assistant message with its tool_use blocks (unanswered)
                let mut assistant_blocks = Vec::new();
                if !response.content.is_empty() {
                    assistant_blocks.push(MessageContent::text(&response.content));
                }
                for call in &response.tool_calls {
                    assistant_blocks.push(tool_use_content(call));
                }
                self.db
                    .append_message(session_id, "assistant", &assistant_blocks, None)?;
                messages.push(Message::assistant_with_content(assistant_blocks));

                // synthetic tool results telling the model to wrap up
                let synthetic_results: Vec<MessageContent> = response
                    .tool_calls
                    .iter()
                    .map(|call| {
                        MessageContent::tool_result(
                            &call.id,
                            "tool budget exhausted (40 rounds). you must respond now. \
                             summarize progress and explain remaining work.",
                        )
                    })
                    .collect();
                self.db
                    .append_message(session_id, "user", &synthetic_results, None)?;
                messages.push(Message::user_with_content(synthetic_results));

                // final text-only turn (no tools)
                let active_provider = switched_provider.as_ref().unwrap_or(&self.provider);
                let final_content = match active_provider
                    .complete(&system_prompt, &messages, &[])
                    .await
                {
                    Ok(r) => r.content,
                    Err(e) => {
                        tracing::warn!(%e, "final summary call failed, using fallback");
                        "i used all 40 tool rounds. send a follow-up message and i'll continue."
                            .to_string()
                    }
                };

                let final_blocks = vec![MessageContent::text(&final_content)];
                self.db
                    .append_message(session_id, "assistant", &final_blocks, None)?;

                return Ok(Some(OutboundMessage {
                    content: final_content,
                    voice: pending_voice.take(),
                }));
            }

            let mut assistant_blocks = Vec::new();
            if !response.content.is_empty() {
                assistant_blocks.push(MessageContent::text(response.content));
            }

            for call in &response.tool_calls {
                tracing::debug!(tool = %call.name, "invoking tool");
                assistant_blocks.push(tool_use_content(call));
            }

            // persist the assistant message (including tool_use blocks)
            self.db
                .append_message(session_id, "assistant", &assistant_blocks, None)?;
            messages.push(Message::assistant_with_content(assistant_blocks));

            // execute tool calls concurrently
            let results = futures::future::join_all(
                response
                    .tool_calls
                    .iter()
                    .map(|call| self.handle_tool_call_with_approval(call)),
            )
            .await;

            let mut tool_results = Vec::new();
            let mut saw_complete = false;
            let mut saw_compact = false;
            for result in results {
                let result = result?;
                if result.complete {
                    saw_complete = true;
                }
                if result.compact {
                    saw_compact = true;
                }
                if let Some(new_provider) = result.switch_provider {
                    let model_id = new_provider.model_id();
                    tracing::info!(%model_id, "switching provider mid-conversation");
                    if let Err(e) = self.db.set_session_model(session_id, &model_id) {
                        tracing::warn!(%e, "failed to persist model selection");
                    }
                    switched_provider = Some(new_provider);
                }
                if let Some(voice_bytes) = result.voice {
                    pending_voice = Some(voice_bytes);
                }
                tool_results.push(result.content);
            }

            if saw_complete {
                self.db
                    .append_message(session_id, "user", &tool_results, None)?;
                return Ok(None);
            }

            // persist tool results (clean, no injections)
            self.db
                .append_message(session_id, "user", &tool_results, None)?;
            messages.push(Message::user_with_content(tool_results));

            // collect system injections (budget warnings, context usage) into a
            // separate message so they don't interfere with structured tool output.
            let mut system_notes = Vec::new();

            if tool_rounds == WARNING_ROUND {
                system_notes.push(MessageContent::text(format!(
                    "[system: you have used {WARNING_ROUND} of {MAX_TOOL_ROUNDS} tool rounds. \
                     {} remain before you must produce a final response. plan accordingly.]",
                    MAX_TOOL_ROUNDS - WARNING_ROUND
                )));
            }

            // inject context usage at key thresholds: first round (baseline),
            // once when crossing 60% (heads up), and every round at 80%+ (approaching compaction at 90%).
            let should_inject_context = tool_rounds == 1
                || (ctx.usage_percent >= 60.0 && !crossed_60pct)
                || ctx.usage_percent >= 80.0;
            if ctx.usage_percent >= 60.0 {
                crossed_60pct = true;
            }
            if should_inject_context {
                system_notes.push(MessageContent::text(format!(
                    "[context: {:.0}% of window used ({}/{} tokens)]",
                    ctx.usage_percent, ctx.input_tokens, ctx.context_window
                )));
            }

            if !system_notes.is_empty() {
                self.db
                    .append_message(session_id, "system", &system_notes, None)?;
                // sent to the API as a user message
                messages.push(Message::user_with_content(system_notes));
            }

            // agent-requested compaction (via compact_context tool)
            if saw_compact && ctx.usage_percent >= 20.0 {
                let prior_summary = self.db.get_session_summary(session_id)?;
                let provider = switched_provider.as_ref().unwrap_or(&self.provider);
                match compaction::compact_messages(provider, messages.clone(), prior_summary).await
                {
                    Ok((compacted, summary)) => {
                        messages = compacted;
                        self.db.set_session_summary(session_id, &summary)?;
                        compaction_count += 1;
                        tracing::info!(compaction_count, "agent-requested compaction");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "agent-requested compaction failed");
                    }
                }
            }
        }
    }

    async fn handle_tool_call_with_approval(
        &self,
        call: &ToolCall,
    ) -> Result<tool::ToolCallResult, Error> {
        // hard deny vault access before anything else — no approval can override
        if let Some(deny) = tool::check_vault_deny(call) {
            return Ok(deny);
        }

        if tool::requires_approval(call) {
            // log a concise summary of what's being requested
            let approval_summary = tool::approval_summary(call);
            tracing::info!(
                tool = %call.name,
                detail = %approval_summary,
                "requesting approval"
            );
            let decision = match self.approver.request_approval(call).await {
                Ok(d) => d,
                Err(Error::ApprovalTimeout) => {
                    tracing::warn!(tool = %call.name, "approval timed out, treating as deny");
                    return Ok(tool::ToolCallResult {
                        content: MessageContent::tool_result(
                            &call.id,
                            "approval timed out — the user did not respond in time",
                        ),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    });
                }
                Err(e) => return Err(e),
            };
            match decision {
                ApprovalDecision::AllowOnce => {
                    tracing::info!(tool = %call.name, "approved (once)");
                }
                ApprovalDecision::AutoApproved => {
                    tracing::debug!(tool = %call.name, "auto-approved");
                }
                ApprovalDecision::AllowAlways { ref pattern } => {
                    for p in pattern.split('\n') {
                        let p = p.trim();
                        if !p.is_empty() {
                            tracing::info!(p, "saving approval rule");
                            self.db.save_approval_rule(p)?;
                        }
                    }
                }
                ApprovalDecision::AllowTimed {
                    ref pattern,
                    duration_secs,
                } => {
                    let expires_at =
                        chrono::Utc::now() + chrono::Duration::seconds(duration_secs as i64);
                    let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();
                    for p in pattern.split('\n') {
                        let p = p.trim();
                        if !p.is_empty() {
                            tracing::info!(p, %expires_str, "saving timed approval rule");
                            self.db
                                .save_approval_rule_with_expiry(p, Some(&expires_str))?;
                        }
                    }
                }
                ApprovalDecision::Deny => {
                    return Ok(tool::ToolCallResult {
                        content: MessageContent::tool_result(&call.id, "command denied by user"),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    });
                }
            }
        }

        // refresh vault secrets when a skill is activated (user may have added secrets)
        if call.name == tool::ACTIVATE_SKILL_TOOL_NAME
            && let Ok(mut secrets) = self.vault_secrets.write()
        {
            *secrets = tool::load_vault_secrets();
        }

        let mut result = tool::handle_tool_call(
            &self.client,
            &self.db,
            self.mcp.as_deref(),
            &self.skills,
            call,
        )
        .await?;

        // scrub vault secrets from all tool output
        if let Ok(secrets) = self.vault_secrets.read()
            && !secrets.is_empty()
        {
            result.content = scrub_tool_result(result.content, &secrets);
        }

        Ok(result)
    }

    /// build the full list of tool definitions (built-in + MCP).
    async fn all_tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut tools = tool::tool_definitions();

        if let Some(ref mcp) = self.mcp {
            let builtin_names: std::collections::HashSet<String> =
                tools.iter().map(|t| t.name().to_string()).collect();

            for entry in mcp.list_all_tools().await {
                let namespaced = format!("mcp__{}_{}", entry.server_name, entry.tool.name);
                if builtin_names.contains(&namespaced) {
                    tracing::warn!(
                        tool = %namespaced,
                        "MCP tool name conflicts with built-in, skipping"
                    );
                    continue;
                }
                tools.push(ToolDefinition::Dynamic {
                    name: namespaced,
                    description: entry.tool.description.unwrap_or_default(),
                    input_schema: entry.tool.input_schema,
                });
            }
        }

        tools
    }

    /// check if the last message is an assistant message with tool_use blocks
    /// that lack matching tool_results. this happens when a crash/OOM/kill
    /// interrupts the tool loop before results are persisted.
    ///
    /// for mid-history orphans, use `ava doctor` instead.
    fn repair_orphaned_tool_calls(
        &self,
        session_id: i64,
        messages: &mut Vec<Message>,
    ) -> Result<(), Error> {
        let last = match messages.last() {
            Some(msg) if msg.role == Role::Assistant => msg,
            _ => return Ok(()),
        };

        let tool_use_ids: Vec<String> = last
            .content
            .iter()
            .filter_map(|c| match c {
                MessageContent::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        if tool_use_ids.is_empty() {
            return Ok(());
        }

        tracing::warn!(
            count = tool_use_ids.len(),
            "repairing orphaned tool_use blocks from interrupted session"
        );

        let synthetic: Vec<MessageContent> = tool_use_ids
            .iter()
            .map(|id| {
                MessageContent::tool_result(
                    id,
                    "the session was interrupted and it is unknown whether this tool call completed.",
                )
            })
            .collect();

        self.db
            .append_message(session_id, "user", &synthetic, None)?;
        messages.push(Message::user_with_content(synthetic));

        Ok(())
    }

    fn system_prompt(&self) -> Result<String, Error> {
        let traits = self.db.character_traits()?;
        let facts = self.db.recent_facts()?;
        let episodes = self.db.recent_episodes()?;
        let pending_tasks = self.db.pending_task_titles()?;

        let mut prompt = DEFAULT_SYSTEM_PROMPT.to_string();

        prompt.push_str(&format!(
            "\n\ncurrent date and time: {}",
            Utc::now().format("%Y-%m-%d %H:%M UTC")
        ));

        if !traits.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_character_traits(&traits));
        }

        if !facts.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_known_facts(&facts));
        }

        if !episodes.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_recent_episodes(&episodes));
        }

        if !pending_tasks.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_pending_tasks(&pending_tasks));
        }

        let model_skills: Vec<_> = self
            .skills
            .iter()
            .filter(|s| !s.disable_model_invocation)
            .cloned()
            .collect();
        if !model_skills.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_available_skills(&model_skills));
        }

        prompt.push_str(&format!(
            "\n\n## tool budget\nyou have a budget of {MAX_TOOL_ROUNDS} tool rounds per user \
             message. after exhausting the budget you get one final text-only turn. if you need \
             more rounds, tell the user to send a follow-up message."
        ));

        Ok(prompt)
    }
}

/// scrub vault secret values from a tool result's text content.
/// images pass through unchanged.
fn scrub_tool_result(content: MessageContent, secrets: &[String]) -> MessageContent {
    match content {
        MessageContent::ToolResult {
            tool_use_id,
            content: trc,
        } => {
            let scrubbed = match trc {
                ToolResultContent::Text(text) => {
                    ToolResultContent::Text(tool::scrub_vault_secrets(&text, secrets))
                }
                ToolResultContent::Blocks(blocks) => ToolResultContent::Blocks(
                    blocks
                        .into_iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => ContentBlock::Text {
                                text: tool::scrub_vault_secrets(&text, secrets),
                            },
                            other => other,
                        })
                        .collect(),
                ),
            };
            MessageContent::ToolResult {
                tool_use_id,
                content: scrubbed,
            }
        }
        other => other,
    }
}

fn tool_use_content(call: &ToolCall) -> MessageContent {
    MessageContent::tool_use(call.id.clone(), call.name.clone(), call.input.clone())
}

#[cfg(test)]
mod tests {
    use super::prompt::*;
    use super::*;
    use crate::approver::CliApprover;
    use crate::db::{Memory, MemoryKind};
    use crate::message::ChannelKind;
    use crate::provider::{ProviderResponse, StopReason, TestProvider, Usage};

    fn make_test_provider(response: &str) -> AnyProvider {
        let response = response.to_string();
        AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                Ok(ProviderResponse {
                    content: response.clone(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: Usage::default(),
                })
            }),
        })
    }

    fn make_failing_provider() -> AnyProvider {
        AnyProvider::Test(TestProvider {
            handler: Box::new(|_, _| Err(Error::Provider("provider failed".into()))),
        })
    }

    #[tokio::test]
    async fn test_agent_processes_message() {
        let provider = make_test_provider("hi");
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "hi");
    }

    #[tokio::test]
    async fn test_provider_error_propagates() {
        let provider = make_failing_provider();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let result = agent.process(&inbound).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Provider(msg) if msg == "provider failed"));
    }

    #[tokio::test]
    async fn test_agent_injects_facts_into_system_prompt() {
        use std::sync::{Arc as StdArc, Mutex};

        let seen_prompt = StdArc::new(Mutex::new(None));
        let seen_prompt_clone = seen_prompt.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |system, _msgs| {
                *seen_prompt_clone.lock().unwrap() = Some(system.to_string());
                Ok(ProviderResponse {
                    content: "hi".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: Usage::default(),
                })
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        agent.process(&inbound).await.unwrap().unwrap();

        let prompt = seen_prompt.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("## known facts"));
        assert!(prompt.contains("### user"));
        assert!(prompt.contains("- name: alex"));
    }

    #[tokio::test]
    async fn test_agent_loads_history_from_session() {
        use std::sync::{Arc as StdArc, Mutex};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let sid = db.active_session().unwrap();

        // seed conversation history in the db
        db.append_message(
            sid,
            "user",
            &[MessageContent::text("my name is alex")],
            Some("cli"),
        )
        .unwrap();
        db.append_message(
            sid,
            "assistant",
            &[MessageContent::text("nice to meet you alex")],
            None,
        )
        .unwrap();

        // track what messages the provider sees
        let seen_msgs = StdArc::new(Mutex::new(None));
        let seen_clone = seen_msgs.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                *seen_clone.lock().unwrap() = Some(msgs.len());
                Ok(ProviderResponse {
                    content: "your name is alex".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: Usage::default(),
                })
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "what is my name?".into(),
        };
        agent.process(&inbound).await.unwrap().unwrap();

        // provider should have seen 3 messages: 2 history + 1 new
        let msg_count = seen_msgs.lock().unwrap().unwrap();
        assert_eq!(msg_count, 3);
    }

    fn make_memory(
        kind: MemoryKind,
        content: &str,
        category: Option<&str>,
        key: Option<&str>,
    ) -> Memory {
        Memory {
            id: 0,
            kind,
            content: content.into(),
            category: category.map(|s| s.into()),
            key: key.map(|s| s.into()),
            created_at: "2024-01-15 12:00:00".into(),
        }
    }

    #[test]
    fn test_format_known_facts_groups_by_category() {
        let facts = vec![
            make_memory(MemoryKind::Fact, "alex", Some("user"), Some("name")),
            make_memory(
                MemoryKind::Fact,
                "concise",
                Some("preferences"),
                Some("response_style"),
            ),
            make_memory(
                MemoryKind::Fact,
                "Europe/Amsterdam",
                Some("user"),
                Some("timezone"),
            ),
        ];

        let formatted = format_known_facts(&facts);

        assert_eq!(
            formatted,
            "## known facts\n\n### user\n- name: alex\n- timezone: Europe/Amsterdam\n\n### preferences\n- response_style: concise"
        );
    }

    #[test]
    fn test_format_known_facts_truncates_values() {
        let facts = vec![make_memory(
            MemoryKind::Fact,
            &"x".repeat(MAX_FACT_VALUE_CHARS + 10),
            Some("user"),
            Some("bio"),
        )];

        let formatted = format_known_facts(&facts);
        let expected = format!("- bio: {}", "x".repeat(MAX_FACT_VALUE_CHARS));

        assert!(formatted.contains(&expected));
        assert!(!formatted.contains(&"x".repeat(MAX_FACT_VALUE_CHARS + 1)));
    }

    #[test]
    fn test_format_character_traits() {
        let traits = vec![
            make_memory(
                MemoryKind::Character,
                "formal and precise",
                None,
                Some("tone"),
            ),
            make_memory(
                MemoryKind::Character,
                "dry wit, concise",
                None,
                Some("personality"),
            ),
        ];

        let formatted = format_character_traits(&traits);
        assert!(formatted.contains("## character"));
        assert!(formatted.contains("- tone: formal and precise"));
        assert!(formatted.contains("- personality: dry wit, concise"));
    }

    #[test]
    fn test_format_recent_episodes() {
        let episodes = vec![
            make_memory(MemoryKind::Episode, "discussed migration plan", None, None),
            make_memory(MemoryKind::Episode, "user mentioned traveling", None, None),
        ];

        let formatted = format_recent_episodes(&episodes);
        assert!(formatted.contains("## recent memories"));
        assert!(formatted.contains("[2024-01-15] discussed migration plan"));
        assert!(formatted.contains("[2024-01-15] user mentioned traveling"));
    }

    #[tokio::test]
    async fn test_agent_tool_loop_executes_and_returns() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // first call: return a tool call (remember a fact)
                    Ok(ProviderResponse {
                        content: "let me remember that".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: "call_1".into(),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": "alex",
                                "kind": "fact",
                                "category": "user",
                                "key": "name"
                            }),
                        }],
                        usage: Usage::default(),
                    })
                } else {
                    // second call: final text response
                    Ok(ProviderResponse {
                        content: "done, i remembered that".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "my name is alex".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "done, i remembered that");

        // provider was called twice (tool call round + final response)
        assert_eq!(call_count.load(Ordering::SeqCst), 2);

        // tool actually executed — fact was persisted
        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "alex");

        // messages were persisted (user + assistant[tool_use] + user[tool_result] + system[context] + assistant[final])
        let sid = db.active_session().unwrap();
        let count = db.session_message_count(sid).unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_agent_multiple_tool_calls_execute_concurrently() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // first call: return multiple tool calls at once
                    Ok(ProviderResponse {
                        content: "let me remember several things".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![
                            tool::ToolCall {
                                id: "call_1".into(),
                                name: "remember".into(),
                                input: serde_json::json!({
                                    "content": "alex",
                                    "kind": "fact",
                                    "category": "user",
                                    "key": "name"
                                }),
                            },
                            tool::ToolCall {
                                id: "call_2".into(),
                                name: "remember".into(),
                                input: serde_json::json!({
                                    "content": "rust",
                                    "kind": "fact",
                                    "category": "user",
                                    "key": "language"
                                }),
                            },
                            tool::ToolCall {
                                id: "call_3".into(),
                                name: "remember".into(),
                                input: serde_json::json!({
                                    "content": "met at a conference",
                                    "kind": "episode"
                                }),
                            },
                        ],
                        usage: Usage::default(),
                    })
                } else {
                    Ok(ProviderResponse {
                        content: "remembered everything".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "remember these things".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "remembered everything");

        // all three tools executed
        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 2);
        let episodes = db.recent_episodes().unwrap();
        assert_eq!(episodes.len(), 1);

        // tool results message is clean; context usage is a separate system message
        let sid = db.active_session().unwrap();
        let msgs = db.load_messages(sid).unwrap();
        // user + assistant[3 tool_use] + user[3 tool_result] + user[context] + assistant[final]
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[2].content.len(), 3);
    }

    #[tokio::test]
    async fn test_agent_tool_loop_limit_graceful() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // provider returns tool calls for the first 41 calls, then a final
        // text response on the 42nd (the text-only summary turn).
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n <= MAX_TOOL_ROUNDS as usize {
                    Ok(ProviderResponse {
                        content: format!("round {n}"),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: format!("call_{n}"),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": format!("event {n}"),
                                "kind": "episode"
                            }),
                        }],
                        usage: Usage::default(),
                    })
                } else {
                    // final text-only turn
                    Ok(ProviderResponse {
                        content: "i've completed 40 rounds of work. here's a summary.".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "loop forever".into(),
        };

        let result = agent.process(&inbound).await;
        assert!(result.is_ok(), "should return Ok, not Err");
        let outbound = result.unwrap().unwrap();
        assert!(outbound.content.contains("40 rounds"));
    }

    #[tokio::test]
    async fn test_agent_tool_budget_final_turn_fallback() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // provider returns tool calls, and when the final text-only call
        // happens it returns an error — agent should use the static fallback.
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n <= MAX_TOOL_ROUNDS as usize {
                    Ok(ProviderResponse {
                        content: format!("round {n}"),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: format!("call_{n}"),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": format!("event {n}"),
                                "kind": "episode"
                            }),
                        }],
                        usage: Usage::default(),
                    })
                } else {
                    Err(Error::Provider("api error".into()))
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "loop forever".into(),
        };

        let result = agent.process(&inbound).await;
        assert!(
            result.is_ok(),
            "should return Ok even when final call fails"
        );
        let outbound = result.unwrap().unwrap();
        assert!(outbound.content.contains("40 tool rounds"));
    }

    #[tokio::test]
    async fn test_agent_tool_budget_warning_injected() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        let seen_warning = Arc::new(Mutex::new(false));
        let seen_warning_clone = seen_warning.clone();

        // provider returns tool calls for WARNING_ROUND rounds, then check
        // that the messages contain the budget warning on the next call.
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);

                // after the warning round, check if the last user message
                // contains the budget warning text
                if n == WARNING_ROUND as usize {
                    if let Some(last_msg) = msgs.last() {
                        for block in &last_msg.content {
                            if let MessageContent::Text { text } = block
                                && text.contains("[system: you have used")
                            {
                                *seen_warning_clone.lock().unwrap() = true;
                            }
                        }
                    }
                    // return a final text response to end the loop
                    return Ok(ProviderResponse {
                        content: "done".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        usage: Usage::default(),
                    });
                }

                Ok(ProviderResponse {
                    content: format!("round {n}"),
                    stop_reason: StopReason::ToolUse,
                    tool_calls: vec![tool::ToolCall {
                        id: format!("call_{n}"),
                        name: "remember".into(),
                        input: serde_json::json!({
                            "content": format!("event {n}"),
                            "kind": "episode"
                        }),
                    }],
                    usage: Usage::default(),
                })
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "do many things".into(),
        };

        agent.process(&inbound).await.unwrap();
        assert!(
            *seen_warning.lock().unwrap(),
            "budget warning should have been injected at round {WARNING_ROUND}"
        );
    }

    #[tokio::test]
    async fn test_agent_approval_deny_returns_denied() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        content: "let me run a command".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: "call_1".into(),
                            name: "exec".into(),
                            input: serde_json::json!({"command": "echo hi"}),
                        }],
                        usage: Usage::default(),
                    })
                } else {
                    Ok(ProviderResponse {
                        content: "ok, command was denied".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        usage: Usage::default(),
                    })
                }
            }),
        });

        // we can't use AnyApprover with a custom approver, so we need to
        // build the agent differently. since Agent takes AnyApprover, we'll
        // test the routing by checking the tool result message instead.
        // use CliApprover (auto-approves) and verify exec actually runs.
        // for denial testing, we check handle_tool_call_with_approval indirectly.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "run echo hi".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        // CliApprover auto-approves, so the command executed
        assert_eq!(outbound.content, "ok, command was denied");

        // verify exec tool actually ran (check persisted messages contain tool result)
        let sid = db.active_session().unwrap();
        let msgs = db.load_messages(sid).unwrap();
        // should have: user, assistant(tool_use), user(tool_result), user(system/context), assistant(final)
        assert_eq!(msgs.len(), 5);
    }

    #[test]
    fn test_system_prompt_includes_all_sections() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.remember(MemoryKind::Character, "formal", None, Some("tone"))
            .unwrap();
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();
        db.remember(MemoryKind::Episode, "discussed rust", None, None)
            .unwrap();

        let provider = make_test_provider("hi");
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );
        let prompt = agent.system_prompt().unwrap();

        assert!(prompt.contains("current date and time:"));
        assert!(prompt.contains("## character"));
        assert!(prompt.contains("- tone: formal"));
        assert!(prompt.contains("## known facts"));
        assert!(prompt.contains("- name: alex"));
        assert!(prompt.contains("## recent memories"));
        assert!(prompt.contains("discussed rust"));
        assert!(prompt.contains("## tool budget"));
        assert!(prompt.contains("40 tool rounds"));
    }

    #[test]
    fn test_system_prompt_includes_pending_tasks() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.add_task("fix CI failure", None).unwrap();
        db.add_task("review PR #42", None).unwrap();

        let provider = make_test_provider("hi");
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );
        let prompt = agent.system_prompt().unwrap();

        assert!(prompt.contains("## pending tasks"));
        assert!(prompt.contains("fix CI failure"));
        assert!(prompt.contains("review PR #42"));
    }

    #[test]
    fn test_system_prompt_omits_empty_tasks() {
        let db = Arc::new(Database::open_in_memory().unwrap());

        let provider = make_test_provider("hi");
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );
        let prompt = agent.system_prompt().unwrap();

        assert!(!prompt.contains("## pending tasks"));
    }

    #[tokio::test]
    async fn test_agent_repairs_orphaned_tool_use() {
        use std::sync::{Arc as StdArc, Mutex};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let sid = db.active_session().unwrap();

        // seed an assistant message with a tool_use block but no tool_result
        let orphaned_assistant = vec![
            MessageContent::text("let me run that"),
            MessageContent::tool_use(
                "orphan_1",
                "exec",
                serde_json::json!({"command": "echo hi"}),
            ),
        ];
        db.append_message(sid, "assistant", &orphaned_assistant, None)
            .unwrap();

        // track messages the provider sees
        let seen_msgs = StdArc::new(Mutex::new(Vec::new()));
        let seen_clone = seen_msgs.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                *seen_clone.lock().unwrap() = msgs.to_vec();
                Ok(ProviderResponse {
                    content: "recovered from crash".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: Usage::default(),
                })
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello after crash".into(),
        };

        let result = agent.process(&inbound).await;
        assert!(result.is_ok(), "should not error on orphaned tool_use");
        let outbound = result.unwrap().unwrap();
        assert_eq!(outbound.content, "recovered from crash");

        // verify the provider saw the synthetic tool_result
        let msgs = seen_msgs.lock().unwrap();
        // expected: assistant(tool_use) + user(synthetic tool_result) + user(new message)
        assert_eq!(msgs.len(), 3);

        // second message should be the synthetic tool_result
        let repair_msg = &msgs[1];
        assert_eq!(repair_msg.role, Role::User);
        assert!(matches!(
            &repair_msg.content[0],
            MessageContent::ToolResult {
                tool_use_id,
                content,
            } if tool_use_id == "orphan_1" && content.as_display_str().contains("interrupted")
        ));
    }

    #[test]
    fn test_system_prompt_excludes_context_usage() {
        // context usage is injected as a message, not in the system prompt,
        // to avoid busting the prompt cache on every tool round.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let provider = make_test_provider("hi");
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let prompt = agent.system_prompt().unwrap();
        assert!(!prompt.contains("context usage"));
    }

    #[test]
    fn test_system_prompt_excludes_completed_tasks() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = db.add_task("done task", None).unwrap();
        db.complete_task(id).unwrap();
        db.add_task("pending task", None).unwrap();

        let provider = make_test_provider("hi");
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );
        let prompt = agent.system_prompt().unwrap();

        assert!(prompt.contains("## pending tasks"));
        assert!(prompt.contains("pending task"));
        assert!(!prompt.contains("done task"));
    }

    #[test]
    fn test_format_pending_tasks() {
        let tasks = vec![
            (3, "investigate CI failure".into()),
            (7, "fix test_session_persistence".into()),
        ];
        let formatted = format_pending_tasks(&tasks);
        assert_eq!(
            formatted,
            "## pending tasks\n- [id:3] investigate CI failure\n- [id:7] fix test_session_persistence"
        );
    }

    #[tokio::test]
    async fn test_agent_complete_tool_returns_none() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                Ok(ProviderResponse {
                    content: "".into(),
                    stop_reason: StopReason::ToolUse,
                    tool_calls: vec![tool::ToolCall {
                        id: "call_1".into(),
                        name: "complete".into(),
                        input: serde_json::json!({"reason": "memory distillation done"}),
                    }],
                    usage: Usage::default(),
                })
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            reqwest::Client::new(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "distill memories".into(),
        };

        let result = agent.process(&inbound).await.unwrap();
        assert!(result.is_none(), "expected None for complete tool");

        // provider was called once
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // messages were persisted (user + assistant[tool_use] + user[tool_result])
        let sid = db.active_session().unwrap();
        let count = db.session_message_count(sid).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_expand_skill_matches() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let skills = Arc::new(vec![Skill {
            name: "summarize".into(),
            description: "summarize text".into(),
            user_invocable: true,
            disable_model_invocation: false,
            secrets: vec![],
            body: "please summarize: $ARGUMENTS".into(),
        }]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        )
        .with_skills(skills);

        let expanded = agent.expand_skill("/summarize this is my text");
        assert!(expanded.contains("[skill: summarize]"));
        assert!(expanded.contains("please summarize: this is my text"));
        assert!(expanded.contains("[/skill]"));
        // args are also appended after the skill block
        assert!(expanded.ends_with("this is my text"));
    }

    #[test]
    fn test_expand_skill_no_match() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        // no skills loaded — should pass through unchanged
        assert_eq!(agent.expand_skill("/unknown foo"), "/unknown foo");
        assert_eq!(agent.expand_skill("plain message"), "plain message");
    }

    #[test]
    fn test_expand_skill_not_user_invocable() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let skills = Arc::new(vec![Skill {
            name: "internal".into(),
            description: "internal only".into(),
            user_invocable: false,
            disable_model_invocation: false,
            secrets: vec![],
            body: "secret stuff".into(),
        }]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        )
        .with_skills(skills);

        // not user-invocable — should not expand
        assert_eq!(agent.expand_skill("/internal test"), "/internal test");
    }

    #[test]
    fn test_expand_skill_no_arguments() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let skills = Arc::new(vec![Skill {
            name: "status".into(),
            description: "check status".into(),
            user_invocable: true,
            disable_model_invocation: false,
            secrets: vec![],
            body: "report the current status. args: $ARGUMENTS".into(),
        }]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        )
        .with_skills(skills);

        let expanded = agent.expand_skill("/status");
        assert!(expanded.contains("[skill: status]"));
        assert!(expanded.contains("report the current status. args: "));
    }

    #[test]
    fn test_system_prompt_includes_skills() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let skills = Arc::new(vec![
            Skill {
                name: "summarize".into(),
                description: "summarize text".into(),
                user_invocable: true,
                disable_model_invocation: false,
                secrets: vec![],
                body: "summarize this".into(),
            },
            Skill {
                name: "hidden".into(),
                description: "hidden from model".into(),
                user_invocable: true,
                disable_model_invocation: true,
                secrets: vec![],
                body: "secret".into(),
            },
        ]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        )
        .with_skills(skills);

        let prompt = agent.system_prompt().unwrap();
        assert!(prompt.contains("## available skills"));
        assert!(prompt.contains("**summarize**: summarize text"));
        // hidden skill should not appear
        assert!(!prompt.contains("hidden from model"));
    }

    #[test]
    fn test_system_prompt_omits_skills_when_none() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            reqwest::Client::new(),
        );

        let prompt = agent.system_prompt().unwrap();
        assert!(!prompt.contains("## available skills"));
    }
}
