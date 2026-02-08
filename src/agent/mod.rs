mod compaction;

use crate::db::Database;
use crate::db::Fact;
use crate::error::Error;
use crate::message::{InboundMessage, Message, MessageContent, OutboundMessage};
use crate::provider::{AnyProvider, DEFAULT_SYSTEM_PROMPT, Provider};
use crate::tool::{self, ApprovalDecision, Approver, ToolCall};

const MAX_FACT_VALUE_CHARS: usize = 500;

pub struct Agent<A> {
    provider: AnyProvider,
    approver: A,
    db: Database,
}

impl<A: Approver> Agent<A> {
    pub fn new(provider: AnyProvider, approver: A, db: Database) -> Self {
        Self {
            provider,
            approver,
            db,
        }
    }

    #[tracing::instrument(skip(self, inbound), fields(channel = ?inbound.channel))]
    pub async fn process(self, inbound: InboundMessage) -> Result<OutboundMessage, Error> {
        let session_id = self.db.active_session()?;
        let channel_str = inbound.channel.as_str();

        // load conversation history (growing window for prompt cache efficiency)
        let mut messages = self.db.load_messages(session_id)?;

        // append and persist the new user message
        let user_content = vec![MessageContent::text(&inbound.content)];
        self.db
            .append_message(session_id, "user", &user_content, Some(channel_str))?;
        messages.push(Message::user(inbound.content));

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
            let response = match active_provider.complete(&system_prompt, &messages).await {
                Ok(r) => r,
                Err(Error::ContextOverflow) => {
                    return Ok(OutboundMessage {
                        content: "conversation context is full. key facts have been preserved \
                            — please start a new session."
                            .into(),
                    });
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

                return Ok(OutboundMessage {
                    content: response.content,
                });
            }

            tracing::debug!(
                tool_round = tool_rounds,
                count = response.tool_calls.len(),
                "executing tool calls"
            );

            tool_rounds += 1;
            if tool_rounds > 5 {
                return Err(Error::Provider("tool loop exceeded".into()));
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

            let mut tool_results = Vec::new();
            for call in &response.tool_calls {
                let result = self.handle_tool_call_with_approval(call).await?;
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
            let decision = self.approver.request_approval(call).await?;
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
                    });
                }
            }
        }

        tool::handle_tool_call(&self.db, call).await
    }

    fn system_prompt(&self) -> Result<String, Error> {
        let facts = self.db.recent_facts()?;
        if facts.is_empty() {
            return Ok(DEFAULT_SYSTEM_PROMPT.to_string());
        }

        Ok(format!(
            "{DEFAULT_SYSTEM_PROMPT}\n\n{}",
            format_known_facts(&facts)
        ))
    }
}

fn tool_use_content(call: &ToolCall) -> MessageContent {
    MessageContent::tool_use(call.id.clone(), call.name.clone(), call.input.clone())
}

fn format_known_facts(facts: &[Fact]) -> String {
    let mut grouped: Vec<(String, Vec<(String, String)>)> = Vec::new();

    for fact in facts {
        let value = truncate_chars(&fact.value, MAX_FACT_VALUE_CHARS);

        if let Some((_, entries)) = grouped
            .iter_mut()
            .find(|(category, _)| category == &fact.category)
        {
            entries.push((fact.key.clone(), value));
        } else {
            grouped.push((fact.category.clone(), vec![(fact.key.clone(), value)]));
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

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ChannelKind;
    use crate::provider::{ProviderResponse, StopReason, TestProvider, Usage};
    use crate::tool::CliApprover;

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
        let db = Database::open_in_memory().unwrap();
        let agent = Agent::new(provider, CliApprover, db);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let outbound = agent.process(inbound).await.unwrap();
        assert_eq!(outbound.content, "hi");
    }

    #[tokio::test]
    async fn test_provider_error_propagates() {
        let provider = make_failing_provider();
        let db = Database::open_in_memory().unwrap();
        let agent = Agent::new(provider, CliApprover, db);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let result = agent.process(inbound).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Provider(msg) if msg == "provider failed"));
    }

    #[tokio::test]
    async fn test_agent_injects_facts_into_system_prompt() {
        use std::sync::{Arc, Mutex};

        let seen_prompt = Arc::new(Mutex::new(None));
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

        let db = Database::open_in_memory().unwrap();
        db.remember_fact("user", "name", "alex").unwrap();
        let agent = Agent::new(provider, CliApprover, db);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        agent.process(inbound).await.unwrap();

        let prompt = seen_prompt.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("## known facts"));
        assert!(prompt.contains("### user"));
        assert!(prompt.contains("- name: alex"));
    }

    #[tokio::test]
    async fn test_agent_loads_history_from_session() {
        use std::sync::{Arc, Mutex};

        let db = Database::open_in_memory().unwrap();
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
        let seen_msgs = Arc::new(Mutex::new(None));
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

        let agent = Agent::new(provider, CliApprover, db);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "what is my name?".into(),
        };
        agent.process(inbound).await.unwrap();

        // provider should have seen 3 messages: 2 history + 1 new
        let msg_count = seen_msgs.lock().unwrap().unwrap();
        assert_eq!(msg_count, 3);
    }

    #[test]
    fn test_format_known_facts_groups_by_category() {
        let facts = vec![
            Fact {
                category: "user".into(),
                key: "name".into(),
                value: "alex".into(),
            },
            Fact {
                category: "preferences".into(),
                key: "response_style".into(),
                value: "concise".into(),
            },
            Fact {
                category: "user".into(),
                key: "timezone".into(),
                value: "Europe/Amsterdam".into(),
            },
        ];

        let formatted = format_known_facts(&facts);

        assert_eq!(
            formatted,
            "## known facts\n\n### user\n- name: alex\n- timezone: Europe/Amsterdam\n\n### preferences\n- response_style: concise"
        );
    }

    #[test]
    fn test_format_known_facts_truncates_values() {
        let facts = vec![Fact {
            category: "user".into(),
            key: "bio".into(),
            value: "x".repeat(MAX_FACT_VALUE_CHARS + 10),
        }];

        let formatted = format_known_facts(&facts);
        let expected = format!("- bio: {}", "x".repeat(MAX_FACT_VALUE_CHARS));

        assert!(formatted.contains(&expected));
        assert!(!formatted.contains(&"x".repeat(MAX_FACT_VALUE_CHARS + 1)));
    }
}
