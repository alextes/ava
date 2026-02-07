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
        let token =
            std::env::var("TELOXIDE_TOKEN").map_err(|_| Error::MissingEnvVar("TELOXIDE_TOKEN"))?;
        Ok(Self::new(token))
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}{}/{}", API_BASE, self.token, method)
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_updates(&self, offset: Option<i64>) -> Result<Vec<Update>, Error> {
        let params = GetUpdatesParams {
            timeout: 30,
            offset,
            allowed_updates: Some(vec!["message", "callback_query"]),
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
}

#[derive(Debug, Deserialize)]
pub struct Message {
    #[allow(dead_code)]
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    #[allow(dead_code)]
    pub from: User,
    pub message: Option<Message>,
    pub data: Option<String>,
}
