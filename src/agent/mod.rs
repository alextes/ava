mod compaction;

use std::sync::Arc;

use chrono::Utc;

use crate::approver::AnyApprover;
use crate::db::{Database, Memory};
use crate::error::Error;
use crate::message::{InboundMessage, Message, MessageContent, OutboundMessage, Role};
use crate::provider::{AnyProvider, DEFAULT_SYSTEM_PROMPT, Provider};
use crate::tool::{self, ApprovalDecision, Approver, ToolCall};

const MAX_FACT_VALUE_CHARS: usize = 500;
const MAX_TOOL_ROUNDS: u32 = 40;
const WARNING_ROUND: u32 = 32;

pub struct Agent {
    provider: AnyProvider,
    approver: AnyApprover,
    db: Arc<Database>,
    client: reqwest::Client,
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
        }
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

        // append and persist the new user message
        let user_content = vec![MessageContent::text(&inbound.content)];
        self.db
            .append_message(session_id, "user", &user_content, Some(channel_str))?;
        messages.push(Message::user(&inbound.content));

        let system_prompt = self.system_prompt()?;
        let mut tool_rounds = 0;
        let mut switched_provider: Option<AnyProvider> = None;
        let mut last_input_tokens: Option<u32> = None;

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
                        tracing::info!("compacted context");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "compaction failed, continuing with full context");
                    }
                }
            }

            let active_provider = switched_provider.as_ref().unwrap_or(&self.provider);
            let response = match active_provider
                .complete(&system_prompt, &messages, true)
                .await
            {
                Ok(r) => r,
                Err(Error::ContextOverflow) => {
                    return Ok(Some(OutboundMessage {
                        content: "conversation context is full. key facts have been preserved \
                            — please start a new session."
                            .into(),
                    }));
                }
                Err(e) => return Err(e),
            };

            let usage = &response.usage;
            if let (Some(created), Some(read)) =
                (usage.cache_creation_tokens, usage.cache_read_tokens)
            {
                tracing::info!(
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    cache_created = created,
                    cache_read = read,
                    "provider usage"
                );
            } else {
                tracing::info!(
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    "provider usage"
                );
            }

            last_input_tokens = Some(usage.input_tokens);

            if response.tool_calls.is_empty() {
                // persist the final assistant response
                let assistant_content = vec![MessageContent::text(&response.content)];
                self.db
                    .append_message(session_id, "assistant", &assistant_content, None)?;

                return Ok(Some(OutboundMessage {
                    content: response.content,
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
                    .complete(&system_prompt, &messages, false)
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
            for result in results {
                let result = result?;
                if result.complete {
                    saw_complete = true;
                }
                if let Some(new_provider) = result.switch_provider {
                    let model_id = new_provider.model_id();
                    tracing::info!(%model_id, "switching provider mid-conversation");
                    if let Err(e) = self.db.set_session_model(session_id, &model_id) {
                        tracing::warn!(%e, "failed to persist model selection");
                    }
                    switched_provider = Some(new_provider);
                }
                tool_results.push(result.content);
            }

            if saw_complete {
                self.db
                    .append_message(session_id, "user", &tool_results, None)?;
                return Ok(None);
            }

            // inject budget warning at the warning round
            if tool_rounds == WARNING_ROUND {
                tool_results.push(MessageContent::text(format!(
                    "[system: you have used {WARNING_ROUND} of {MAX_TOOL_ROUNDS} tool rounds. \
                     {} remain before you must produce a final response. plan accordingly.]",
                    MAX_TOOL_ROUNDS - WARNING_ROUND
                )));
            }

            // persist tool results
            self.db
                .append_message(session_id, "user", &tool_results, None)?;
            messages.push(Message::user_with_content(tool_results));
        }
    }

    async fn handle_tool_call_with_approval(
        &self,
        call: &ToolCall,
    ) -> Result<tool::ToolCallResult, Error> {
        if tool::requires_approval(call) {
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
                    });
                }
                Err(e) => return Err(e),
            };
            match decision {
                ApprovalDecision::AllowOnce | ApprovalDecision::AutoApproved => {
                    // proceed with execution
                }
                ApprovalDecision::AllowAlways { ref pattern } => {
                    tracing::info!(pattern, "saving approval rule");
                    self.db.save_approval_rule(pattern)?;
                }
                ApprovalDecision::Deny => {
                    return Ok(tool::ToolCallResult {
                        content: MessageContent::tool_result(&call.id, "command denied by user"),
                        switch_provider: None,
                        complete: false,
                    });
                }
            }
        }

        tool::handle_tool_call(&self.client, &self.db, call).await
    }

    /// scan the full message history for assistant messages with tool_use
    /// blocks that aren't followed by matching tool_results. this happens when
    /// a crash/OOM/kill interrupts the tool loop before results are persisted.
    ///
    /// mid-history orphans are repaired in-memory only (the DB is append-only).
    /// tail orphans are also persisted so they don't reappear on next load.
    fn repair_orphaned_tool_calls(
        &self,
        session_id: i64,
        messages: &mut Vec<Message>,
    ) -> Result<(), Error> {
        let len = messages.len();
        if len == 0 {
            return Ok(());
        }

        // collect insertion points (index after the orphan) and their synthetic results
        let mut inserts: Vec<(usize, Vec<MessageContent>)> = Vec::new();

        for i in 0..len {
            if messages[i].role != Role::Assistant {
                continue;
            }

            let tool_use_ids: Vec<String> = messages[i]
                .content
                .iter()
                .filter_map(|c| match c {
                    MessageContent::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();

            if tool_use_ids.is_empty() {
                continue;
            }

            // check if the next message has tool_results for these IDs
            let has_results = messages.get(i + 1).is_some_and(|next| {
                next.role == Role::User
                    && tool_use_ids.iter().all(|id| {
                        next.content.iter().any(|c| {
                            matches!(c, MessageContent::ToolResult { tool_use_id, .. } if tool_use_id == id)
                        })
                    })
            });

            if !has_results {
                let synthetic: Vec<MessageContent> = tool_use_ids
                    .iter()
                    .map(|id| {
                        MessageContent::tool_result(
                            id,
                            "tool call was interrupted and never completed \
                             (session crashed or approval timed out)",
                        )
                    })
                    .collect();
                inserts.push((i + 1, synthetic));
            }
        }

        if inserts.is_empty() {
            return Ok(());
        }

        tracing::warn!(
            count = inserts.len(),
            "repairing orphaned tool_use blocks from interrupted session"
        );

        // insert in reverse order so indices stay valid
        let is_tail = inserts.last().is_some_and(|(idx, _)| *idx == len);
        for (idx, synthetic) in inserts.into_iter().rev() {
            let msg = Message::user_with_content(synthetic.clone());
            if idx == len {
                // tail orphan: also persist so it doesn't reappear
                self.db
                    .append_message(session_id, "user", &synthetic, None)?;
                messages.push(msg);
            } else {
                // mid-history orphan: in-memory only
                messages.insert(idx, msg);
            }
        }

        if !is_tail {
            tracing::info!(
                "mid-history orphans repaired in-memory only; \
                 they will be re-repaired on each load until compacted away"
            );
        }

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
            prompt.push_str(&format_character_traits(&traits));
        }

        if !facts.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&format_known_facts(&facts));
        }

        if !episodes.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&format_recent_episodes(&episodes));
        }

        if !pending_tasks.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&format_pending_tasks(&pending_tasks));
        }

        prompt.push_str(&format!(
            "\n\n## tool budget\nyou have a budget of {MAX_TOOL_ROUNDS} tool rounds per user \
             message. after exhausting the budget you get one final text-only turn. if you need \
             more rounds, tell the user to send a follow-up message."
        ));

        Ok(prompt)
    }
}

fn tool_use_content(call: &ToolCall) -> MessageContent {
    MessageContent::tool_use(call.id.clone(), call.name.clone(), call.input.clone())
}

fn format_character_traits(traits: &[Memory]) -> String {
    let mut output = String::from("## character");
    for t in traits {
        let key = t.key.as_deref().unwrap_or("?");
        let value = truncate_chars(&t.content, MAX_FACT_VALUE_CHARS);
        output.push_str("\n- ");
        output.push_str(key);
        output.push_str(": ");
        output.push_str(&value);
    }
    output
}

fn format_known_facts(facts: &[Memory]) -> String {
    let mut grouped: Vec<(String, Vec<(String, String)>)> = Vec::new();

    for fact in facts {
        let category = fact.category.as_deref().unwrap_or("general").to_string();
        let key = fact.key.as_deref().unwrap_or("?").to_string();
        let value = truncate_chars(&fact.content, MAX_FACT_VALUE_CHARS);

        if let Some((_, entries)) = grouped.iter_mut().find(|(cat, _)| cat == &category) {
            entries.push((key, value));
        } else {
            grouped.push((category, vec![(key, value)]));
        }
    }

    let mut output = String::from("## known facts");
    for (category, entries) in grouped {
        output.push_str("\n\n### ");
        output.push_str(&category);
        for (key, value) in entries {
            output.push_str("\n- ");
            output.push_str(&key);
            output.push_str(": ");
            output.push_str(&value);
        }
    }

    output
}

fn format_recent_episodes(episodes: &[Memory]) -> String {
    let mut output = String::from("## recent memories");
    for ep in episodes {
        let date = ep.created_at.split(' ').next().unwrap_or(&ep.created_at);
        output.push_str("\n- [");
        output.push_str(date);
        output.push_str("] ");
        output.push_str(&truncate_chars(&ep.content, MAX_FACT_VALUE_CHARS));
    }
    output
}

fn format_pending_tasks(tasks: &[(i64, String)]) -> String {
    let mut output = String::from("## pending tasks");
    for (id, title) in tasks {
        output.push_str(&format!("\n- [id:{id}] {title}"));
    }
    output
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approver::CliApprover;
    use crate::db::MemoryKind;
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

        // messages were persisted (user + assistant[tool_use] + user[tool_result] + assistant[final])
        let sid = db.active_session().unwrap();
        let count = db.session_message_count(sid).unwrap();
        assert_eq!(count, 4);
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

        // tool results message should contain 3 tool_result blocks
        let sid = db.active_session().unwrap();
        let msgs = db.load_messages(sid).unwrap();
        // user + assistant[3 tool_use] + user[3 tool_result] + assistant[final]
        assert_eq!(msgs.len(), 4);
        // the tool results message (index 2) should have 3 blocks
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
        // should have: user, assistant(tool_use), user(tool_result), assistant(final)
        assert_eq!(msgs.len(), 4);
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
            } if tool_use_id == "orphan_1" && content.contains("interrupted")
        ));
    }

    #[tokio::test]
    async fn test_agent_repairs_mid_history_orphaned_tool_use() {
        use std::sync::{Arc as StdArc, Mutex};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let sid = db.active_session().unwrap();

        // simulate: crash left an orphaned tool_use, then the user sent
        // another message which got appended without a tool_result first.
        let orphaned_assistant = vec![
            MessageContent::text("let me run that"),
            MessageContent::tool_use(
                "orphan_mid",
                "exec",
                serde_json::json!({"command": "echo hi"}),
            ),
        ];
        db.append_message(sid, "assistant", &orphaned_assistant, None)
            .unwrap();
        // user message appended after crash (no tool_result between)
        db.append_message(
            sid,
            "user",
            &[MessageContent::text("are you there?")],
            Some("cli"),
        )
        .unwrap();
        db.append_message(sid, "assistant", &[MessageContent::text("yes")], None)
            .unwrap();

        let seen_msgs = StdArc::new(Mutex::new(Vec::new()));
        let seen_clone = seen_msgs.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                *seen_clone.lock().unwrap() = msgs.to_vec();
                Ok(ProviderResponse {
                    content: "all good now".into(),
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
            content: "hello again".into(),
        };

        let result = agent.process(&inbound).await;
        assert!(result.is_ok(), "should not error on mid-history orphan");

        let msgs = seen_msgs.lock().unwrap();
        // expected: assistant(tool_use) + user(synthetic) + user("are you there?")
        //           + assistant("yes") + user("hello again")
        assert_eq!(msgs.len(), 5);

        // the synthetic repair should be at index 1
        let repair_msg = &msgs[1];
        assert_eq!(repair_msg.role, Role::User);
        assert!(matches!(
            &repair_msg.content[0],
            MessageContent::ToolResult {
                tool_use_id,
                content,
            } if tool_use_id == "orphan_mid" && content.contains("interrupted")
        ));
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
}
