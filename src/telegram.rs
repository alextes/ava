use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::Error;

const API_BASE: &str = "https://api.telegram.org/bot";

pub struct TelegramBot {
    client: Client,
    token: String,
}

impl TelegramBot {
    pub fn new(token: String) -> Self {
        Self {
            client: Client::new(),
            token,
        }
    }

    pub fn from_env() -> Result<Self, Error> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| Error::MissingEnvVar("TELEGRAM_BOT_TOKEN"))?;
        Ok(Self::new(token))
    }

    /// fetch metadata about a chat the bot is a member of.
    #[tracing::instrument(skip(self))]
    pub async fn get_chat(&self, chat_id: i64) -> Result<Chat, Error> {
        let params = serde_json::json!({ "chat_id": chat_id });
        let response: ApiResponse<Chat> = self
            .client
            .post(self.api_url("getChat"))
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            response
                .result
                .ok_or_else(|| Error::Telegram("getChat returned no result".into()))
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }

    /// fetch the bot's own user profile via the getMe endpoint.
    #[tracing::instrument(skip(self))]
    pub async fn get_me(&self) -> Result<User, Error> {
        let response: ApiResponse<User> = self
            .client
            .post(self.api_url("getMe"))
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            response
                .result
                .ok_or_else(|| Error::Telegram("getMe returned no result".into()))
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}{}/{}", API_BASE, self.token, method)
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_updates(&self, offset: Option<i64>) -> Result<Vec<Update>, Error> {
        let params = GetUpdatesParams {
            timeout: 30,
            offset,
            allowed_updates: Some(vec!["message", "callback_query", "my_chat_member"]),
        };

        let response: ApiResponse<Vec<Update>> = self
            .client
            .post(self.api_url("getUpdates"))
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            Ok(response.result.unwrap_or_default())
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }

    #[tracing::instrument(skip(self, text), fields(chat_id))]
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), Error> {
        // try HTML parse mode first
        let params = SendMessageParams {
            chat_id,
            text,
            parse_mode: Some("HTML"),
            reply_markup: None,
        };

        let response: ApiResponse<serde_json::Value> = self
            .client
            .post(self.api_url("sendMessage"))
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            return Ok(());
        }

        // if HTML parsing failed, resend as plain text
        warn!(
            error = response.description.as_deref().unwrap_or("unknown error"),
            "telegram HTML parse failed, falling back to plain text"
        );

        let fallback = SendMessageParams {
            chat_id,
            text,
            parse_mode: None,
            reply_markup: None,
        };

        let response: ApiResponse<serde_json::Value> = self
            .client
            .post(self.api_url("sendMessage"))
            .json(&fallback)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            Ok(())
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }

    #[tracing::instrument(skip(self, text, reply_markup), fields(chat_id))]
    pub async fn send_message_with_keyboard(
        &self,
        chat_id: i64,
        text: &str,
        reply_markup: InlineKeyboardMarkup,
    ) -> Result<i64, Error> {
        let params = SendMessageParams {
            chat_id,
            text,
            parse_mode: None,
            reply_markup: Some(reply_markup),
        };

        let response: ApiResponse<SentMessage> = self
            .client
            .post(self.api_url("sendMessage"))
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            Ok(response.result.map(|m| m.message_id).unwrap_or_default())
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<(), Error> {
        let params = AnswerCallbackQueryParams {
            callback_query_id,
            text,
        };

        let response: ApiResponse<bool> = self
            .client
            .post(self.api_url("answerCallbackQuery"))
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            Ok(())
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }

    /// send an OGG Opus voice message. telegram displays these with an inline waveform player.
    #[tracing::instrument(skip(self, ogg_bytes), fields(chat_id, bytes_len = ogg_bytes.len()))]
    pub async fn send_voice(&self, chat_id: i64, ogg_bytes: Vec<u8>) -> Result<i64, Error> {
        let part = reqwest::multipart::Part::bytes(ogg_bytes)
            .file_name("voice.ogg")
            .mime_str("audio/ogg")
            .map_err(|e| Error::Telegram(format!("failed to build multipart: {e}")))?;

        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part("voice", part);

        let response: ApiResponse<SentMessage> = self
            .client
            .post(self.api_url("sendVoice"))
            .multipart(form)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            Ok(response.result.map(|m| m.message_id).unwrap_or_default())
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }

    #[tracing::instrument(skip(self, text), fields(chat_id, message_id))]
    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
    ) -> Result<(), Error> {
        let params = EditMessageTextParams {
            chat_id,
            message_id,
            text,
        };

        let response: ApiResponse<serde_json::Value> = self
            .client
            .post(self.api_url("editMessageText"))
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if response.ok {
            Ok(())
        } else {
            Err(Error::Telegram(
                response
                    .description
                    .unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }
}

// --- API request/response types ---

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct GetUpdatesParams<'a> {
    timeout: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_updates: Option<Vec<&'a str>>,
}

#[derive(Debug, Serialize)]
struct SendMessageParams<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<InlineKeyboardMarkup>,
}

#[derive(Debug, Serialize)]
struct AnswerCallbackQueryParams<'a> {
    callback_query_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct EditMessageTextParams<'a> {
    chat_id: i64,
    message_id: i64,
    text: &'a str,
}

// --- telegram types ---

#[derive(Debug, Clone, Serialize)]
pub struct InlineKeyboardMarkup {
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InlineKeyboardButton {
    pub text: String,
    pub callback_data: String,
}

#[derive(Debug, Deserialize)]
pub struct SentMessage {
    pub message_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
    pub my_chat_member: Option<ChatMemberUpdated>,
}

#[derive(Debug, Deserialize)]
pub struct ChatMemberUpdated {
    pub chat: Chat,
    pub new_chat_member: ChatMember,
}

#[derive(Debug, Deserialize)]
pub struct ChatMember {
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    pub text: Option<String>,
    pub reply_to_message: Option<Box<Message>>,
    pub entities: Option<Vec<MessageEntity>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub entity_type: String,
    #[allow(dead_code)]
    pub offset: i64,
    #[allow(dead_code)]
    pub length: i64,
    /// present for "text_mention" entities
    pub user: Option<User>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: Option<String>,
    #[allow(dead_code)]
    pub is_bot: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub chat_type: Option<String>,
    #[allow(dead_code)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    #[allow(dead_code)]
    pub from: User,
    pub message: Option<Message>,
    pub data: Option<String>,
}
