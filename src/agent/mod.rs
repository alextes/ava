use crate::db::Database;
use crate::error::Error;
use crate::message::{InboundMessage, Message, MessageContent, OutboundMessage};
use crate::provider::Provider;
use crate::tool;

pub struct Agent<P> {
    provider: P,
    db: Database,
}

impl<P: Provider> Agent<P> {
    pub fn new(provider: P, db: Database) -> Self {
        Self { provider, db }
    }

    pub async fn process(&self, inbound: InboundMessage) -> Result<OutboundMessage, Error> {
        let mut messages = vec![Message::user(inbound.content)];
        let mut tool_rounds = 0;

        loop {
            let response = self.provider.complete(&messages).await?;

            if response.tool_calls.is_empty() {
                return Ok(OutboundMessage {
                    content: response.content,
                });
            }

            tool_rounds += 1;
            if tool_rounds > 5 {
                return Err(Error::Provider("tool loop exceeded".into()));
            }

            let mut assistant_blocks = Vec::new();
            if !response.content.is_empty() {
                assistant_blocks.push(MessageContent::text(response.content));
            }

            for call in &response.tool_calls {
                assistant_blocks.push(MessageContent::tool_use(
                    call.id.clone(),
                    call.name.clone(),
                    call.input.clone(),
                ));
            }

            messages.push(Message::assistant_with_content(assistant_blocks));

            let tool_results = tool::handle_tool_calls(&self.db, &response.tool_calls)?;
            messages.push(Message::user_with_content(tool_results));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::message::ChannelKind;
    use crate::provider::{ProviderResponse, StopReason};

    struct MockProvider {
        response: String,
    }

    impl Provider for MockProvider {
        async fn complete(&self, _messages: &[Message]) -> Result<ProviderResponse, Error> {
            Ok(ProviderResponse {
                content: self.response.clone(),
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
            })
        }
    }

    #[tokio::test]
    async fn test_agent_processes_message() {
        let provider = MockProvider {
            response: "hi".into(),
        };
        let db = Database::open_in_memory().unwrap();
        let agent = Agent::new(provider, db);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let outbound = agent.process(inbound).await.unwrap();
        assert_eq!(outbound.content, "hi");
    }

    struct FailingProvider;

    impl Provider for FailingProvider {
        async fn complete(&self, _messages: &[Message]) -> Result<ProviderResponse, Error> {
            Err(Error::Provider("provider failed".into()))
        }
    }

    #[tokio::test]
    async fn test_provider_error_propagates() {
        let provider = FailingProvider;
        let db = Database::open_in_memory().unwrap();
        let agent = Agent::new(provider, db);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let result = agent.process(inbound).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Provider(msg) if msg == "provider failed"));
    }
}
