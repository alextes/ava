use crate::channel::Channel;
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

    pub async fn process(
        &self,
        inbound: InboundMessage,
        channel: &dyn Channel,
    ) -> Result<(), Error> {
        let messages = vec![Message::user(inbound.content)];

        let response = self.provider.complete(&messages).await?;

        channel.send(OutboundMessage {
            content: response.content,
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ChannelKind;
    use crate::provider::{ProviderResponse, StopReason};
    use std::cell::RefCell;

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

    struct MockChannel {
        sent: RefCell<Vec<OutboundMessage>>,
    }

    impl MockChannel {
        fn new() -> Self {
            Self {
                sent: RefCell::new(Vec::new()),
            }
        }
    }

    impl Channel for MockChannel {
        fn send(&self, message: OutboundMessage) -> Result<(), Error> {
            self.sent.borrow_mut().push(message);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_agent_processes_message() {
        let provider = MockProvider {
            response: "hi".into(),
        };
        let channel = MockChannel::new();
        let agent = Agent::new(provider);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        agent.process(inbound, &channel).await.unwrap();

        let sent = channel.sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].content, "hi");
    }

    struct FailingProvider;

    impl Provider for FailingProvider {
        async fn complete(&self, _messages: &[Message]) -> Result<ProviderResponse, Error> {
            Err(Error::Provider("provider failed".into()))
        }
    }

    struct FailingChannel;

    impl Channel for FailingChannel {
        fn send(&self, _message: OutboundMessage) -> Result<(), Error> {
            Err(Error::Provider("channel send failed".into()))
        }
    }

    #[tokio::test]
    async fn test_provider_error_propagates() {
        let provider = FailingProvider;
        let channel = MockChannel::new();
        let agent = Agent::new(provider);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let result = agent.process(inbound, &channel).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Provider(msg) if msg == "provider failed"));
    }

    #[tokio::test]
    async fn test_channel_error_propagates() {
        let provider = MockProvider {
            response: "hi".into(),
        };
        let channel = FailingChannel;
        let agent = Agent::new(provider);

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            content: "hello".into(),
        };

        let result = agent.process(inbound, &channel).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Provider(msg) if msg == "channel send failed"));
    }
}
