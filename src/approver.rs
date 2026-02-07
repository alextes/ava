use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};

use crate::db::generate_pattern;
use crate::error::Error;
use crate::telegram::{InlineKeyboardButton, InlineKeyboardMarkup, TelegramBot};
use crate::tool::{ApprovalDecision, Approver, ToolCall, references_sensitive_env};

const APPROVAL_TIMEOUT_SECS: u64 = 300; // 5 minutes

struct PendingApproval {
    sender: oneshot::Sender<ApprovalDecision>,
    message_id: i64,
}

/// shared state for pending approval requests.
/// keyed by nonce — shared between the polling loop and spawned agent tasks.
pub struct PendingApprovals {
    map: Mutex<HashMap<String, PendingApproval>>,
}

impl PendingApprovals {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }
}

pub struct TelegramApprover {
    bot: Arc<TelegramBot>,
    chat_id: i64,
    pending: Arc<PendingApprovals>,
}

impl TelegramApprover {
    pub fn new(bot: Arc<TelegramBot>, chat_id: i64, pending: Arc<PendingApprovals>) -> Self {
        Self {
            bot,
            chat_id,
            pending,
        }
    }

    /// route a callback query to a pending approval request.
    /// returns true if the callback was handled.
    pub async fn handle_callback(
        pending: &PendingApprovals,
        bot: &TelegramBot,
        callback_query_id: &str,
        data: &str,
        chat_id: i64,
    ) -> bool {
        // format: exec:{nonce}:{action}
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        if parts.len() != 3 || parts[0] != "exec" {
            return false;
        }

        let nonce = parts[1];
        let action = parts[2];

        let entry = {
            let mut map = pending.map.lock().await;
            map.remove(nonce)
        };

        let Some(approval) = entry else {
            // stale button press
            let _ = bot
                .answer_callback_query(callback_query_id, Some("this approval request has expired"))
                .await;
            return true;
        };

        let decision = match action {
            "allow_once" => ApprovalDecision::AllowOnce,
            "allow_always" => {
                // the actual pattern will be generated from the tool call input
                // on the approver side when the decision is received
                ApprovalDecision::AllowAlways {
                    pattern: String::new(),
                }
            }
            "deny" => ApprovalDecision::Deny,
            _ => {
                let _ = bot
                    .answer_callback_query(callback_query_id, Some("unknown action"))
                    .await;
                return true;
            }
        };

        let decision_text = match &decision {
            ApprovalDecision::AllowOnce => "approved (once)",
            ApprovalDecision::AllowAlways { .. } => "approved (always)",
            ApprovalDecision::Deny => "denied",
            ApprovalDecision::AutoApproved => "auto-approved",
        };

        // edit the message to show the decision
        let _ = bot
            .edit_message_text(chat_id, approval.message_id, &format!("-> {decision_text}"))
            .await;

        let _ = bot.answer_callback_query(callback_query_id, None).await;
        let _ = approval.sender.send(decision);

        true
    }
}

impl Approver for TelegramApprover {
    async fn request_approval(&self, tool_call: &ToolCall) -> Result<ApprovalDecision, Error> {
        let command = tool_call
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown command>");

        // generate nonce
        let nonce = format!("{:08x}", rand_u32());

        // build keyboard
        let has_sensitive = references_sensitive_env(command);
        let mut buttons = vec![InlineKeyboardButton {
            text: "allow once".into(),
            callback_data: format!("exec:{nonce}:allow_once"),
        }];

        if !has_sensitive {
            buttons.push(InlineKeyboardButton {
                text: "allow always".into(),
                callback_data: format!("exec:{nonce}:allow_always"),
            });
        }

        buttons.push(InlineKeyboardButton {
            text: "deny".into(),
            callback_data: format!("exec:{nonce}:deny"),
        });

        let keyboard = InlineKeyboardMarkup {
            inline_keyboard: vec![buttons],
        };

        let mut text = format!("command: {command}");
        if has_sensitive {
            text.push_str("\n⚠ references sensitive environment variables");
        }

        let message_id = self
            .bot
            .send_message_with_keyboard(self.chat_id, &text, keyboard)
            .await?;

        // create oneshot channel
        let (tx, rx) = oneshot::channel();

        {
            let mut map = self.pending.map.lock().await;
            map.insert(
                nonce.clone(),
                PendingApproval {
                    sender: tx,
                    message_id,
                },
            );
        }

        // await response with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECS), rx).await
        {
            Ok(Ok(mut decision)) => {
                // if allow_always, generate the actual pattern from the command
                if matches!(decision, ApprovalDecision::AllowAlways { .. }) {
                    let pattern = generate_pattern(command);
                    decision = ApprovalDecision::AllowAlways { pattern };
                }
                Ok(decision)
            }
            Ok(Err(_)) => {
                // sender dropped (e.g. bot restart)
                Err(Error::ApprovalTimeout)
            }
            Err(_) => {
                // timeout
                let mut map = self.pending.map.lock().await;
                map.remove(&nonce);
                Err(Error::ApprovalTimeout)
            }
        }
    }
}

/// simple non-cryptographic random u32 using thread_rng-like approach
fn rand_u32() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new();
    let mut hasher = s.build_hasher();
    hasher.write_u8(0);
    hasher.finish() as u32
}
