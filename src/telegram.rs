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
        let mut params = vec![("timeout", "30".to_string())];
        if let Some(off) = offset {
            params.push(("offset", off.to_string()));
        }

        let response: ApiResponse<Vec<Update>> = self
            .client
            .get(self.api_url("getUpdates"))
            .query(&params)
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
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageParams<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
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
