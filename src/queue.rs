use std::sync::Arc;

use tokio::sync::mpsc;

use crate::message::{ChannelKind, ImageSource, OutboundMessage};
use crate::telegram::TelegramBot;
use crate::telegram_fmt::markdown_to_telegram_html;

/// routes agent responses back to the originating channel
pub enum ResponseSink {
    Telegram {
        chat_id: i64,
        thread_id: Option<i64>,
        bot: Arc<TelegramBot>,
    },
}

impl ResponseSink {
    /// the chat id this response will be sent to.
    pub fn chat_id(&self) -> i64 {
        match self {
            ResponseSink::Telegram { chat_id, .. } => *chat_id,
        }
    }

    /// the thread id (topic) this response will be sent to.
    pub fn thread_id(&self) -> Option<i64> {
        match self {
            ResponseSink::Telegram { thread_id, .. } => *thread_id,
        }
    }
}

/// a message queued for sequential agent processing
pub struct QueuedMessage {
    pub channel: ChannelKind,
    pub content: String,
    pub images: Vec<ImageSource>,
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
        ResponseSink::Telegram {
            chat_id,
            thread_id,
            bot,
        } => {
            // send voice message if present
            if let Some(voice_bytes) = outbound.voice
                && let Err(e) = bot.send_voice(chat_id, voice_bytes).await
            {
                tracing::error!(%e, chat_id, "failed to send voice message");
            }

            // send file attachments
            for attachment in outbound.attachments {
                let filename = attachment.filename.clone();
                let result = match attachment.kind {
                    crate::tool::AttachmentKind::Photo => bot
                        .send_photo(chat_id, attachment.bytes, attachment.caption.as_deref())
                        .await
                        .map(|_| ()),
                    crate::tool::AttachmentKind::Document => bot
                        .send_document(
                            chat_id,
                            attachment.bytes,
                            &filename,
                            attachment.caption.as_deref(),
                        )
                        .await
                        .map(|_| ()),
                };
                if let Err(e) = result {
                    tracing::error!(%e, chat_id, %filename, "failed to send attachment");
                }
            }

            // send text response (always — voice is supplementary)
            if !outbound.content.is_empty() {
                let html = markdown_to_telegram_html(&outbound.content);
                if let Err(e) = bot.send_message(chat_id, &html, thread_id).await {
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
        attachments: vec![],
    };
    send_response(sink, outbound).await;
}
