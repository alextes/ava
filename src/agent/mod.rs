use crate::db::Database;
use crate::db::Fact;
use crate::error::Error;
use crate::message::{InboundMessage, Message, MessageContent, OutboundMessage};
use crate::provider::{DEFAULT_SYSTEM_PROMPT, Provider};
use crate::tool::{self, ApprovalDecision, Approver, ToolCall};

const MAX_FACT_VALUE_CHARS: usize = 500;

pub struct Agent<P, A> {
    provider: P,
    approver: A,
    db: Database,
}

impl<P: Provider, A: Approver> Agent<P, A> {
    pub fn new(provider: P, approver: A, db: Database) -> Self {
        Self {
            provider,
            approver,
            db,
        }
    }

    #[tracing::instrument(skip(self, inbound), fields(channel = ?inbound.channel))]
    pub async fn process(self, inbound: InboundMessage) -> Result<OutboundMessage, Error> {
        let mut messages = vec![Message::user(inbound.content)];
        let system_prompt = self.system_prompt()?;
        let mut tool_rounds = 0;

        loop {
            let response = self.provider.complete(&system_prompt, &messages).await?;

            if response.tool_calls.is_empty() {
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

            messages.push(Message::assistant_with_content(assistant_blocks));

            let mut tool_results = Vec::new();
            for call in &response.tool_calls {
                let result = self.handle_tool_call_with_approval(call).await?;
                tool_results.push(result);
            }
            messages.push(Message::user_with_content(tool_results));
        }
    }

    async fn handle_tool_call_with_approval(
        &self,
        call: &ToolCall,
    ) -> Result<MessageContent, Error> {
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
                    return Ok(MessageContent::tool_result(
                        &call.id,
                        "command denied by user",
                    ));
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
    use crate::provider::{ProviderResponse, StopReason};
    use crate::tool::CliApprover;
    use std::sync::{Arc, Mutex};

    struct MockProvider {
        response: String,
        system_prompt: Arc<Mutex<Option<String>>>,
    }

    impl Provider for MockProvider {
        async fn complete(
            &self,
            system_prompt: &str,
            _messages: &[Message],
        ) -> Result<crate::provider::ProviderResponse, Error> {
            *self.system_prompt.lock().unwrap() = Some(system_prompt.to_string());
            Ok(ProviderResponse {
                content: self.response.clone(),
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
            })
        }
    }

    #[tokio::test]
    async fn test_agent_processes_message() {
        let seen_prompt = Arc::new(Mutex::new(None));
        let provider = MockProvider {
            response: "hi".into(),
            system_prompt: seen_prompt.clone(),
        };
        let db = Database::open_in_memory().unwrap();
        let agent = Agent::new(provider, CliApprover, db);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let outbound = agent.process(inbound).await.unwrap();
        assert_eq!(outbound.content, "hi");
        assert_eq!(
            seen_prompt.lock().unwrap().as_deref(),
            Some(DEFAULT_SYSTEM_PROMPT)
        );
    }

    struct FailingProvider;

    impl Provider for FailingProvider {
        async fn complete(
            &self,
            _system_prompt: &str,
            _messages: &[Message],
        ) -> Result<ProviderResponse, Error> {
            Err(Error::Provider("provider failed".into()))
        }
    }

    #[tokio::test]
    async fn test_provider_error_propagates() {
        let provider = FailingProvider;
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
        let seen_prompt = Arc::new(Mutex::new(None));
        let provider = MockProvider {
            response: "hi".into(),
            system_prompt: seen_prompt.clone(),
        };
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
