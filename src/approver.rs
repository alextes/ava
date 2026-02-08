use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};

use crate::db::{Database, generate_pattern};
use crate::error::Error;
use crate::telegram::{InlineKeyboardButton, InlineKeyboardMarkup, TelegramBot};
use crate::tool::{
    ApprovalDecision, Approver, MANAGE_RULES_TOOL_NAME, ToolCall, references_sensitive_env,
};

/// auto-approves all tool calls (used for CLI)
pub struct CliApprover;

impl Approver for CliApprover {
    async fn request_approval(&self, _tool_call: &ToolCall) -> Result<ApprovalDecision, Error> {
        Ok(ApprovalDecision::AutoApproved)
    }
}

/// enum wrapper for all approver variants (non-object-safe trait, same pattern as AnyProvider)
pub enum AnyApprover {
    Cli(CliApprover),
    Telegram(TelegramApprover),
}

impl Approver for AnyApprover {
    async fn request_approval(&self, tool_call: &ToolCall) -> Result<ApprovalDecision, Error> {
        match self {
            Self::Cli(a) => a.request_approval(tool_call).await,
            Self::Telegram(a) => a.request_approval(tool_call).await,
        }
    }
}

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
    db: Arc<Database>,
}

impl TelegramApprover {
    pub fn new(
        bot: Arc<TelegramBot>,
        chat_id: i64,
        pending: Arc<PendingApprovals>,
        db: Arc<Database>,
    ) -> Self {
        Self {
            bot,
            chat_id,
            pending,
            db,
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
        let is_rule_add = tool_call.name == MANAGE_RULES_TOOL_NAME;

        let command = tool_call
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // for exec commands, check stored approval rules before prompting
        if !is_rule_add && let Ok(Some(_rule_id)) = self.db.find_matching_rule(command) {
            tracing::debug!(command, "auto-approved by stored rule");
            return Ok(ApprovalDecision::AutoApproved);
        }

        // generate nonce
        let nonce = format!("{:08x}", rand_u32());

        // build prompt text and keyboard based on tool type
        let (text, show_allow_always) = if is_rule_add {
            let pattern = tool_call
                .input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown pattern>");
            (format!("proposed rule: {pattern}"), false)
        } else {
            let has_sensitive = references_sensitive_env(command);
            let mut text = format!("command: {command}");
            if has_sensitive {
                text.push_str("\n⚠ references sensitive environment variables");
            }
            (text, !has_sensitive)
        };

        let mut buttons = vec![InlineKeyboardButton {
            text: "approve".into(),
            callback_data: format!("exec:{nonce}:allow_once"),
        }];

        if show_allow_always {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::EXEC_TOOL_NAME;
    use serde_json::json;

    fn make_call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test".into(),
            name: name.into(),
            input,
        }
    }

    #[tokio::test]
    async fn test_cli_approver_auto_approves() {
        let approver = CliApprover;
        let call = make_call(EXEC_TOOL_NAME, json!({"command": "ls"}));
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }

    #[tokio::test]
    async fn test_any_approver_cli_delegates() {
        let approver = AnyApprover::Cli(CliApprover);
        let call = make_call(EXEC_TOOL_NAME, json!({"command": "ls"}));
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }

    #[tokio::test]
    async fn test_handle_callback_allow_once() {
        let pending = PendingApprovals::new();
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut map = pending.map.lock().await;
            map.insert(
                "abc123".into(),
                PendingApproval {
                    sender: tx,
                    message_id: 42,
                },
            );
        }

        // we can't call handle_callback without a real TelegramBot,
        // but we can test the pending map logic directly
        let mut map = pending.map.lock().await;
        let entry = map.remove("abc123");
        assert!(entry.is_some());

        let approval = entry.unwrap();
        assert_eq!(approval.message_id, 42);
        let _ = approval.sender.send(ApprovalDecision::AllowOnce);

        let decision = rx.await.unwrap();
        assert_eq!(decision, ApprovalDecision::AllowOnce);
    }

    #[tokio::test]
    async fn test_handle_callback_stale_nonce() {
        let pending = PendingApprovals::new();
        // no pending approval registered — lookup returns None
        let map = pending.map.lock().await;
        let entry = map.get("nonexistent");
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn test_pending_approvals_deny() {
        let pending = PendingApprovals::new();
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut map = pending.map.lock().await;
            map.insert(
                "deny_nonce".into(),
                PendingApproval {
                    sender: tx,
                    message_id: 99,
                },
            );
        }

        let mut map = pending.map.lock().await;
        let approval = map.remove("deny_nonce").unwrap();
        let _ = approval.sender.send(ApprovalDecision::Deny);
        drop(map);

        let decision = rx.await.unwrap();
        assert_eq!(decision, ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn test_pending_approvals_allow_always() {
        let pending = PendingApprovals::new();
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut map = pending.map.lock().await;
            map.insert(
                "always_nonce".into(),
                PendingApproval {
                    sender: tx,
                    message_id: 100,
                },
            );
        }

        let mut map = pending.map.lock().await;
        let approval = map.remove("always_nonce").unwrap();
        let _ = approval.sender.send(ApprovalDecision::AllowAlways {
            pattern: "ls *".into(),
        });
        drop(map);

        let decision = rx.await.unwrap();
        assert!(matches!(decision, ApprovalDecision::AllowAlways { pattern } if pattern == "ls *"));
    }

    #[test]
    fn test_rand_u32_produces_values() {
        // just verify it doesn't panic and produces a value
        let a = rand_u32();
        let b = rand_u32();
        // they should be different (extremely unlikely to collide)
        assert_ne!(a, b);
    }

    #[test]
    fn test_references_sensitive_env_in_approval_context() {
        // verify the function used to hide "allow always" button works
        assert!(references_sensitive_env("echo $ANTHROPIC_API_KEY"));
        assert!(references_sensitive_env("echo $TELOXIDE_TOKEN"));
        assert!(!references_sensitive_env("echo hello"));
    }

    #[tokio::test]
    async fn test_telegram_approver_auto_approves_matching_rule() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo test *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        let call = make_call(EXEC_TOOL_NAME, json!({"command": "cargo test --release"}));
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }

    #[tokio::test]
    async fn test_telegram_approver_no_rule_match_times_out() {
        // with no matching rule and no callback handler, approval should time out.
        // we use a very short timeout scenario by dropping the sender side.
        let db = Arc::new(Database::open_in_memory().unwrap());
        // save a rule that won't match
        db.save_approval_rule("cargo test *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, Arc::clone(&db));

        // "rm -rf /" won't match "cargo test *" — would proceed to prompt
        // but since we have a fake bot token, the send_message_with_keyboard call will fail
        let call = make_call(EXEC_TOOL_NAME, json!({"command": "rm stuff"}));
        let result = approver.request_approval(&call).await;
        // the fake bot can't send messages, so this will error
        assert!(result.is_err());
    }
}
