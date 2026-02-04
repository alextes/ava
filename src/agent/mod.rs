use crate::error::Error;
use crate::message::{InboundMessage, Message, OutboundMessage};
use crate::provider::Provider;

pub struct Agent<P> {
    provider: P,
}

impl<P: Provider> Agent<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    pub async fn process(&self, inbound: InboundMessage) -> Result<OutboundMessage, Error> {
        let messages = vec![Message::user(inbound.content)];

        let response = self.provider.complete(&messages).await?;

        Ok(OutboundMessage {
            content: response.content,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let agent = Agent::new(provider);

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
        let agent = Agent::new(provider);

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
