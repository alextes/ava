use clap::{Parser, Subcommand};

use crate::db::Database;
use crate::error;
use crate::provider::{AnyProvider, ReasoningEffort};

#[derive(Parser)]
#[command(name = "ava", about = "a personal ai assistant")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// show version info
    Version,
    /// show current status
    Status,
    /// send a message to the assistant
    Message {
        /// the message to send
        content: String,
    },
    /// list installed skills
    Skills,
    /// stop the running daemon
    Stop,
    /// tail the log file
    Logs {
        /// number of lines to show (default: 50)
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// follow the log file (like tail -f)
        #[arg(short, long)]
        follow: bool,
    },
    /// start all configured channels (daemonizes by default)
    Start {
        /// run in foreground instead of daemonizing
        #[arg(long)]
        foreground: bool,
    },
    /// list active schedules
    Schedules,
    /// diagnose and repair session issues
    Doctor {
        #[command(subcommand)]
        action: Option<DoctorAction>,
    },
    /// stop and restart the daemon
    Restart {
        /// run in foreground instead of daemonizing
        #[arg(long)]
        foreground: bool,
    },
    /// rebuild from source and hot-swap the running process
    Upgrade,
    /// manage approval rules
    Rules {
        #[command(subcommand)]
        action: Option<RulesAction>,
    },
    /// show recent conversation history
    History {
        /// number of messages to show (default: 20)
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
        /// output as JSON
        #[arg(long)]
        json: bool,
        /// compact tool call JSON (disables default pretty-printing)
        #[arg(long)]
        compact: bool,
        /// show full tool call content with expanded newlines
        #[arg(long)]
        full: bool,
        /// follow new messages as they arrive (like tail -f)
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum DoctorAction {
    /// repair orphaned tool_use blocks in the session history
    RepairOrphans,
}

#[derive(Subcommand)]
pub(crate) enum RulesAction {
    /// add a new approval rule (e.g. "cargo *" or "edit:src/**")
    Add {
        /// the pattern to add
        pattern: String,
    },
    /// remove a rule by its number (from `ava rules`)
    Rm {
        /// rule number to remove
        number: usize,
    },
}

/// parse a `/command args` from user input. returns `Some(("command", "args"))` or `None`.
pub(crate) fn parse_slash_command(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let without_slash = &trimmed[1..];
    let (cmd, args) = match without_slash.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (without_slash, ""),
    };
    Some((cmd, args))
}

pub(crate) fn parse_steer_command<'a>(
    input: &'a str,
    bot_username: Option<&str>,
) -> Option<&'a str> {
    let (cmd, args) = parse_slash_command(input)?;
    if cmd.eq_ignore_ascii_case("steer") {
        return Some(args);
    }

    let (base, mention) = cmd.split_once('@')?;
    if !base.eq_ignore_ascii_case("steer") {
        return None;
    }

    let bot_username = bot_username?;
    if mention.eq_ignore_ascii_case(bot_username) {
        Some(args)
    } else {
        None
    }
}

pub(crate) fn effective_reasoning_effort(
    db: &Database,
    session_id: i64,
    model_id: &str,
    explicit: Option<&str>,
) -> ReasoningEffort {
    if let Some(effort) = explicit.and_then(ReasoningEffort::from_user_input)
        && AnyProvider::supports_reasoning_effort(model_id, effort)
    {
        return effort;
    }

    match db.model_reasoning_preference(session_id, model_id) {
        Ok(Some(effort)) if AnyProvider::supports_reasoning_effort(model_id, effort) => effort,
        _ => AnyProvider::default_reasoning_effort(model_id),
    }
}

#[cfg(test)]
mod steer_tests {
    use super::*;

    #[test]
    fn test_parse_steer_command_plain() {
        assert_eq!(
            parse_steer_command("/steer keep it short", None),
            Some("keep it short")
        );
    }

    #[test]
    fn test_parse_steer_command_with_bot_mention() {
        assert_eq!(
            parse_steer_command("/steer@ren_bot use bullets", Some("ren_bot")),
            Some("use bullets")
        );
        assert_eq!(
            parse_steer_command("/steer@other_bot use bullets", Some("ren_bot")),
            None
        );
    }

    #[test]
    fn test_parse_steer_command_empty_args() {
        assert_eq!(parse_steer_command("/steer", None), Some(""));
    }
}

/// handle the `/switch` command: parse provider/model, construct the provider, persist it.
/// returns a user-facing confirmation or error message.
pub(crate) fn handle_switch_command(args: &str, client: reqwest::Client, db: &Database) -> String {
    if args.is_empty() {
        let current_line = match provider_for_session(client, db) {
            Ok(p) => format!(
                "current: {} (reasoning: {})\n",
                p.model_id(),
                p.reasoning_effort()
            ),
            Err(_) => String::new(),
        };
        return format!(
            "{current_line}usage: /switch <provider> [model] [reasoning_effort]\n\
             providers: anthropic, deepseek, gemini, openai, openrouter\n\
             reasoning_effort: none, low, medium, high, xhigh\n\
             examples:\n  /switch deepseek deepseek-v4-pro\n  /switch gemini\n  /switch anthropic claude-sonnet-4-6"
        );
    }

    let mut parts = args.split_whitespace();
    let provider_name = parts.next().unwrap();
    let model = parts.next();
    let requested_reasoning = parts.next();

    let session_id = match db.active_session() {
        Ok(id) => id,
        Err(e) => return format!("error: failed to get session: {e}"),
    };

    match AnyProvider::from_name(client, provider_name, model) {
        Ok(mut new_provider) => {
            let model_id = new_provider.model_id();
            let reasoning_effort =
                effective_reasoning_effort(db, session_id, &model_id, requested_reasoning);
            new_provider.set_reasoning_effort(reasoning_effort);
            if let Err(e) = db.set_session_model_reasoning(session_id, &model_id, reasoning_effort)
            {
                return format!("error: failed to persist model and reasoning: {e}");
            }
            format!("switched to {model_id} (reasoning: {reasoning_effort})")
        }
        Err(e) => format!("error: {e}"),
    }
}

/// handle the `/rules` command: list or delete approval rules.
/// returns a user-facing message.
pub(crate) fn handle_rules_command(args: &str, db: &Database) -> String {
    let args = args.trim();

    if args.is_empty() {
        return match db.list_approval_rules() {
            Ok(rules) if rules.is_empty() => "no approval rules saved.".to_string(),
            Ok(rules) => {
                let mut out = "approval rules:".to_string();
                for (i, rule) in rules.iter().enumerate() {
                    out.push_str(&format!("\n{}. {}", i + 1, rule.pattern));
                }
                out
            }
            Err(e) => format!("error: {e}"),
        };
    }

    let mut parts = args.split_whitespace();
    let sub = parts.next().unwrap();

    if sub != "delete" {
        return format!("unknown subcommand: {sub}\nusage: /rules [delete <number>]");
    }

    let Some(num_str) = parts.next() else {
        return "usage: /rules delete <number>".to_string();
    };

    let num: usize = match num_str.parse() {
        Ok(n) if n >= 1 => n,
        _ => return format!("invalid rule number: {num_str}"),
    };

    let rules = match db.list_approval_rules() {
        Ok(r) => r,
        Err(e) => return format!("error: {e}"),
    };

    if num > rules.len() {
        return format!("rule {num} not found (have {} rules)", rules.len());
    }

    let rule = &rules[num - 1];
    match db.delete_approval_rule(rule.id) {
        Ok(true) => format!("deleted rule: {}", rule.pattern),
        Ok(false) => format!("rule {num} not found"),
        Err(e) => format!("error: {e}"),
    }
}

/// load the persisted provider/model for the active session, falling back to default
pub(crate) fn provider_for_session(
    client: reqwest::Client,
    db: &Database,
) -> Result<AnyProvider, error::Error> {
    let session_id = db.active_session()?;
    if let Ok(Some((model_id, persisted_reasoning))) = db.session_model_reasoning(session_id) {
        if let Some((provider_name, model)) = model_id.split_once('/') {
            let reasoning_effort = persisted_reasoning
                .or_else(|| {
                    db.model_reasoning_preference(session_id, &model_id)
                        .ok()
                        .flatten()
                })
                .unwrap_or_else(|| AnyProvider::default_reasoning_effort(&model_id));
            match AnyProvider::from_name_with_reasoning(
                client.clone(),
                provider_name,
                Some(model),
                reasoning_effort,
            ) {
                Ok(p) => {
                    let _ = db.set_session_model_reasoning(session_id, &model_id, reasoning_effort);
                    tracing::info!(%model_id, reasoning = %reasoning_effort, "loaded persisted model");
                    return Ok(p);
                }
                Err(e) => {
                    tracing::warn!(%e, %model_id, "failed to load persisted model, clearing and using default");
                    let _ = db.clear_session_model(session_id);
                }
            }
        } else {
            tracing::warn!(%model_id, "invalid model id format, clearing and using default");
            let _ = db.clear_session_model(session_id);
        }
    }
    AnyProvider::default_from_env(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effective_reasoning_prefers_valid_explicit_value() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();
        db.set_session_model_reasoning(
            sid,
            "openrouter/deepseek/deepseek-v4-pro",
            ReasoningEffort::High,
        )
        .unwrap();

        assert_eq!(
            effective_reasoning_effort(
                &db,
                sid,
                "openrouter/deepseek/deepseek-v4-pro",
                Some("xhigh")
            ),
            ReasoningEffort::XHigh
        );
    }

    #[test]
    fn test_effective_reasoning_ignores_invalid_value_and_uses_memory() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();
        db.set_session_model_reasoning(
            sid,
            "openrouter/deepseek/deepseek-v4-pro",
            ReasoningEffort::High,
        )
        .unwrap();

        assert_eq!(
            effective_reasoning_effort(
                &db,
                sid,
                "openrouter/deepseek/deepseek-v4-pro",
                Some("minimal")
            ),
            ReasoningEffort::High
        );
    }

    #[test]
    fn test_effective_reasoning_ignores_unsupported_xhigh() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        assert_eq!(
            effective_reasoning_effort(&db, sid, "openai/gpt-5.4", Some("xhigh")),
            ReasoningEffort::Medium
        );
    }

    #[test]
    fn test_effective_reasoning_uses_default_without_memory() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        assert_eq!(
            effective_reasoning_effort(&db, sid, "openai/gpt-5.4", None),
            ReasoningEffort::Medium
        );
        assert_eq!(
            effective_reasoning_effort(&db, sid, "openrouter/deepseek/deepseek-chat", None),
            ReasoningEffort::None
        );
    }
}
