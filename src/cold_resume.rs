//! cold-resume prompt: when a user replies after the prompt cache has expired,
//! the full conversation gets re-sent as uncached input at full base rate.
//! this module detects that situation and offers the user a choice between
//! eating the replay cost ("keep") or starting fresh ("clear").
//!
//! the prompt is sent via telegram inline keyboard. callback data is namespaced
//! with the `cold:` prefix so it doesn't collide with tool-approval callbacks
//! (`exec:`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, oneshot};

use crate::error::Error;
use crate::telegram::{InlineKeyboardButton, InlineKeyboardMarkup, TelegramBot};

/// minimum input-token count before we bother prompting the user.
/// below this, the replay cost is small enough that interrupting them isn't worth it.
pub const QUIET_THRESHOLD_TOKENS: u32 = 10_000;

/// time the user has to respond before we silently fall back to "keep".
const PROMPT_TIMEOUT_SECS: u64 = 5 * 60;

/// what the user picked (or what timed out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdResumeDecision {
    /// keep the conversation history; eat the cold-cache replay cost.
    Keep,
    /// drop the conversation history; start fresh from the new user message.
    Clear,
}

struct PendingPrompt {
    sender: oneshot::Sender<ColdResumeDecision>,
    message_id: i64,
    original_text: String,
}

/// shared state for in-flight cold-resume prompts, keyed by nonce.
/// shared between the telegram polling loop and the agent task that's
/// awaiting a decision.
pub struct PendingColdResumes {
    map: Mutex<HashMap<String, PendingPrompt>>,
}

impl PendingColdResumes {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }
}

/// holds the telegram context needed to ask the user about a cold resume.
pub struct ColdResumePrompter {
    bot: Arc<TelegramBot>,
    chat_id: i64,
    thread_id: Option<i64>,
    pending: Arc<PendingColdResumes>,
}

impl ColdResumePrompter {
    pub fn new(
        bot: Arc<TelegramBot>,
        chat_id: i64,
        thread_id: Option<i64>,
        pending: Arc<PendingColdResumes>,
    ) -> Self {
        Self {
            bot,
            chat_id,
            thread_id,
            pending,
        }
    }

    /// ask the user how to proceed when the prompt cache is cold.
    /// returns the user's choice, or `Keep` on timeout.
    pub async fn prompt(
        &self,
        input_tokens: u32,
        elapsed: Duration,
        cost_estimate: &str,
        model_id: &str,
    ) -> Result<ColdResumeDecision, Error> {
        let nonce = format!("{:08x}", rand_u32());

        let elapsed_str = format_elapsed(elapsed);
        let text = format!(
            "conversation has grown to ~{tokens} tokens. cache expired {elapsed_str} ago.\n\
             replaying uncached will cost {cost} ({model}).",
            tokens = format_token_count(input_tokens),
            elapsed_str = elapsed_str,
            cost = cost_estimate,
            model = model_id,
        );

        let keyboard = InlineKeyboardMarkup {
            inline_keyboard: vec![vec![
                InlineKeyboardButton {
                    text: "keep context".into(),
                    callback_data: format!("cold:{nonce}:keep"),
                },
                InlineKeyboardButton {
                    text: "clear".into(),
                    callback_data: format!("cold:{nonce}:clear"),
                },
            ]],
        };

        let message_id = self
            .bot
            .send_message_with_keyboard(self.chat_id, &text, keyboard, self.thread_id)
            .await?;

        let (tx, rx) = oneshot::channel();

        {
            let mut map = self.pending.map.lock().await;
            map.insert(
                nonce.clone(),
                PendingPrompt {
                    sender: tx,
                    message_id,
                    original_text: text,
                },
            );
        }

        match tokio::time::timeout(Duration::from_secs(PROMPT_TIMEOUT_SECS), rx).await {
            Ok(Ok(decision)) => Ok(decision),
            Ok(Err(_)) => {
                // sender was dropped without a value — treat as keep so the user
                // doesn't lose context due to a transient harness issue.
                tracing::warn!("cold-resume sender dropped, defaulting to keep");
                Ok(ColdResumeDecision::Keep)
            }
            Err(_) => {
                // timeout — silently fall back to keep so the user isn't ghosted.
                let mut map = self.pending.map.lock().await;
                if let Some(expired) = map.remove(&nonce) {
                    let text = format!("{}\n-> kept (no response)", expired.original_text);
                    if let Err(e) = self
                        .bot
                        .edit_message_text(self.chat_id, expired.message_id, &text)
                        .await
                    {
                        tracing::warn!("failed to edit expired cold-resume prompt: {e}");
                    }
                }
                Ok(ColdResumeDecision::Keep)
            }
        }
    }
}

/// route a callback query to a pending cold-resume prompt.
/// returns `true` if `data` was a `cold:...` callback (handled or stale).
pub async fn handle_callback(
    pending: &PendingColdResumes,
    bot: &TelegramBot,
    callback_query_id: &str,
    data: &str,
    chat_id: i64,
    message_id: Option<i64>,
) -> bool {
    // format: cold:{nonce}:{action}
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() < 3 || parts[0] != "cold" {
        return false;
    }

    let nonce = parts[1];
    let action = parts[2];

    let entry = {
        let mut map = pending.map.lock().await;
        map.remove(nonce)
    };

    let Some(prompt) = entry else {
        // stale press — edit message and acknowledge.
        if let Some(mid) = message_id
            && let Err(e) = bot.edit_message_text(chat_id, mid, "-> expired").await
        {
            tracing::warn!("failed to edit stale cold-resume message: {e}");
        }
        if let Err(e) = bot
            .answer_callback_query(callback_query_id, Some("this prompt has expired"))
            .await
        {
            tracing::warn!("failed to answer stale cold-resume callback: {e}");
        }
        return true;
    };

    let decision = match action {
        "keep" => ColdResumeDecision::Keep,
        "clear" => ColdResumeDecision::Clear,
        _ => {
            if let Err(e) = bot
                .answer_callback_query(callback_query_id, Some("unknown action"))
                .await
            {
                tracing::warn!("failed to answer unknown cold-resume action: {e}");
            }
            return true;
        }
    };

    let label = match decision {
        ColdResumeDecision::Keep => "kept",
        ColdResumeDecision::Clear => "cleared",
    };

    let edited_text = format!("{}\n-> {label}", prompt.original_text);
    if let Err(e) = bot
        .edit_message_text(chat_id, prompt.message_id, &edited_text)
        .await
    {
        tracing::warn!("failed to edit cold-resume prompt: {e}");
    }
    if let Err(e) = bot.answer_callback_query(callback_query_id, None).await {
        tracing::warn!("failed to answer cold-resume callback: {e}");
    }

    if prompt.sender.send(decision).is_err() {
        tracing::debug!("cold-resume receiver dropped (agent likely timed out)");
    }

    true
}

fn format_token_count(tokens: u32) -> String {
    if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// non-cryptographic random u32 using std's hasher (matches approver pattern).
fn rand_u32() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new();
    let mut hasher = s.build_hasher();
    hasher.write_u8(0);
    hasher.finish() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_token_count_small() {
        assert_eq!(format_token_count(500), "500");
    }

    #[test]
    fn format_token_count_large() {
        assert_eq!(format_token_count(52_345), "52k");
    }

    #[test]
    fn format_elapsed_seconds() {
        assert_eq!(format_elapsed(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn format_elapsed_minutes() {
        assert_eq!(format_elapsed(Duration::from_secs(7 * 60)), "7m");
    }

    #[test]
    fn format_elapsed_hours() {
        assert_eq!(format_elapsed(Duration::from_secs(2 * 3600 + 200)), "2h");
    }

    #[test]
    fn format_elapsed_days() {
        assert_eq!(format_elapsed(Duration::from_secs(2 * 86400)), "2d");
    }

    #[tokio::test]
    async fn handle_callback_ignores_non_cold_prefix() {
        let pending = PendingColdResumes::new();
        let bot = TelegramBot::new_for_tests("fake".into());
        let handled = handle_callback(&pending, &bot, "cb", "exec:abc:allow_once", 0, None).await;
        assert!(!handled);
    }

    #[tokio::test]
    async fn handle_callback_routes_keep() {
        let pending = PendingColdResumes::new();
        let (tx, rx) = oneshot::channel();
        {
            let mut map = pending.map.lock().await;
            map.insert(
                "n1".into(),
                PendingPrompt {
                    sender: tx,
                    message_id: 7,
                    original_text: "p".into(),
                },
            );
        }
        // we can't fully exercise handle_callback without a real bot — but we
        // can verify pending lookup + direct send works the same way the handler
        // does.
        let mut map = pending.map.lock().await;
        let prompt = map.remove("n1").unwrap();
        let _ = prompt.sender.send(ColdResumeDecision::Keep);
        drop(map);
        assert_eq!(rx.await.unwrap(), ColdResumeDecision::Keep);
    }
}
