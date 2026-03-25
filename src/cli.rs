use clap::{Parser, Subcommand};

use crate::db::Database;
use crate::error;
use crate::provider::AnyProvider;

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

/// handle the `/switch` command: parse provider/model, construct the provider, persist it.
/// returns a user-facing confirmation or error message.
pub(crate) fn handle_switch_command(args: &str, client: reqwest::Client, db: &Database) -> String {
    if args.is_empty() {
        return "usage: /switch <provider> [model]\n\
                providers: anthropic, openai\n\
                examples:\n  /switch openai\n  /switch anthropic claude-sonnet-4-6"
            .to_string();
    }

    let mut parts = args.split_whitespace();
    let provider_name = parts.next().unwrap();
    let model = parts.next();

    let session_id = match db.active_session() {
        Ok(id) => id,
        Err(e) => return format!("error: failed to get session: {e}"),
    };

    match AnyProvider::from_name(client, provider_name, model) {
        Ok(new_provider) => {
            let model_id = new_provider.model_id();
            if let Err(e) = db.set_session_model(session_id, &model_id) {
                return format!("error: failed to persist model: {e}");
            }
            format!("switched to {model_id}")
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
    if let Ok(Some(model_id)) = db.session_model(session_id) {
        if let Some((provider_name, model)) = model_id.split_once('/') {
            match AnyProvider::from_name(client.clone(), provider_name, Some(model)) {
                Ok(p) => {
                    tracing::info!(%model_id, "loaded persisted model");
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
