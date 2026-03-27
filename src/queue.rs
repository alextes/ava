use std::sync::Arc;

use tokio::sync::mpsc;

use crate::message::{ChannelKind, OutboundMessage};
use crate::telegram::TelegramBot;
use crate::telegram_fmt::markdown_to_telegram_html;

/// routes agent responses back to the originating channel
pub enum ResponseSink {
    Telegram { chat_id: i64, bot: Arc<TelegramBot> },
}

/// a message queued for sequential agent processing
pub struct QueuedMessage {
    pub channel: ChannelKind,
    pub content: String,
    pub sink: ResponseSink,
}

pub type MessageSender = mpsc::Sender<QueuedMessage>;
pub type MessageReceiver = mpsc::Receiver<QueuedMessage>;

pub fn message_queue(buffer: usize) -> (MessageSender, MessageReceiver) {
    mpsc::channel(buffer)
}

/// send a response back through the appropriate channel
pub async fn send_response(sink: ResponseSink, outbound: OutboundMessage) {
    match sink {
        ResponseSink::Telegram { chat_id, bot } => {
            // send voice message if present
            if let Some(voice_bytes) = outbound.voice
                && let Err(e) = bot.send_voice(chat_id, voice_bytes).await
            {
                tracing::error!(%e, chat_id, "failed to send voice message");
            }

            // send text response (always — voice is supplementary)
            if !outbound.content.is_empty() {
                let html = markdown_to_telegram_html(&outbound.content);
                if let Err(e) = bot.send_message(chat_id, &html).await {
                    tracing::error!(%e, chat_id, "failed to send telegram response");
                }
            }
        }
    }
}

/// send an error message back through the appropriate channel
pub async fn send_error(sink: ResponseSink, msg: &str) {
    let outbound = OutboundMessage {
        content: msg.to_string(),
        voice: None,
    };
    send_response(sink, outbound).await;
}
