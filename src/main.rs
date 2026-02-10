mod agent;
mod approver;
mod channel;
mod config;
mod db;
mod error;
mod message;
mod provider;
mod queue;
mod scheduler;
mod telegram;
mod telegram_fmt;
mod tool;

use std::sync::Arc;

use clap::{Parser, Subcommand};

use crate::agent::Agent;
use crate::approver::{AnyApprover, CliApprover, PendingApprovals, TelegramApprover};
use crate::channel::Channel;
use crate::db::Database;
use crate::message::{ChannelKind, InboundMessage, Message, MessageContent, Role};
use crate::provider::AnyProvider;
use crate::queue::{
    MessageReceiver, QueuedMessage, ResponseSink, message_queue, send_error, send_response,
};
use crate::telegram::TelegramBot;

#[derive(Parser)]
#[command(name = "ava", about = "a personal ai assistant")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// show version info
    Version,
    /// show current status
    Status,
    /// send a message to the assistant
    Message {
        /// the message to send
        content: String,
    },
    /// start all configured channels
    Start,
    /// list active schedules
    Schedules,
    /// diagnose and repair session issues
    Doctor {
        #[command(subcommand)]
        action: Option<DoctorAction>,
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
    },
}

#[derive(Subcommand)]
enum DoctorAction {
    /// repair orphaned tool_use blocks in the session history
    RepairOrphans,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "starting ava");

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("ava {}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Status => {
            println!("ava {}", env!("CARGO_PKG_VERSION"));
            println!("db: {}", config::default_db_path().display());
            if let Ok(db) = Database::open()
                && let Ok(sid) = db.active_session()
            {
                let msg_count = db.session_message_count(sid).unwrap_or(0);
                println!("session: {sid} ({msg_count} messages)");
            }
        }
        Commands::Message { content } => {
            if let Err(e) = run_message(content).await {
                tracing::error!(%e, "message command failed");
                std::process::exit(1);
            }
        }
        Commands::Start => {
            if let Err(e) = run_start().await {
                tracing::error!(%e, "start failed");
                std::process::exit(1);
            }
        }
        Commands::Schedules => {
            if let Err(e) = run_schedules() {
                tracing::error!(%e, "schedules command failed");
                std::process::exit(1);
            }
        }
        Commands::Doctor { action } => match action {
            None => {
                if let Err(e) = run_doctor_diagnose() {
                    tracing::error!(%e, "doctor failed");
                    std::process::exit(1);
                }
            }
            Some(DoctorAction::RepairOrphans) => {
                if let Err(e) = run_doctor_fix() {
                    tracing::error!(%e, "doctor repair-orphans failed");
                    std::process::exit(1);
                }
            }
        },
        Commands::History {
            limit,
            json,
            compact,
            full,
        } => {
            if let Err(e) = run_history(limit, json, compact, full) {
                tracing::error!(%e, "history command failed");
                std::process::exit(1);
            }
        }
    }
}

fn run_schedules() -> Result<(), error::Error> {
    let db = Database::open()?;
    let schedules = db.list_schedules()?;
    if schedules.is_empty() {
        println!("no active schedules");
    } else {
        for s in schedules {
            let kind = match &s.cron_expr {
                Some(expr) => format!("recurring ({expr})"),
                None => "one-time".to_string(),
            };
            println!(
                "id={}: {} [{}] next={} | {}",
                s.id, s.description, kind, s.next_run_at, s.prompt
            );
        }
    }
    Ok(())
}

/// display mode for history output
enum HistoryMode {
    Compact,
    Pretty,
    Full,
}

fn run_history(limit: u32, json: bool, compact: bool, full: bool) -> Result<(), error::Error> {
    let db = Database::open()?;
    let session_id = db.active_session()?;
    let messages = db.load_recent_messages(session_id, limit)?;

    if messages.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("no messages");
        }
        return Ok(());
    }

    if json {
        let out = serde_json::to_string_pretty(&messages)
            .map_err(|e| error::Error::Provider(format!("failed to serialize history: {e}")))?;
        println!("{out}");
        return Ok(());
    }

    // --full wins if both are passed
    let mode = if full {
        HistoryMode::Full
    } else if compact {
        HistoryMode::Compact
    } else {
        HistoryMode::Pretty
    };

    // ansi color codes
    const DIM: &str = "\x1b[2m";
    const CYAN: &str = "\x1b[36m";
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const MAGENTA: &str = "\x1b[35m";
    const RESET: &str = "\x1b[0m";

    for (i, msg) in messages.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let (role, role_color) = match msg.role {
            Role::User => ("user", CYAN),
            Role::Assistant => ("assistant", GREEN),
        };
        let label = format!("── {role} · {} ──", msg.created_at);
        let pad_len = 56usize.saturating_sub(label.len());
        let padding = "─".repeat(pad_len);
        println!(
            "{DIM}──{RESET} {role_color}{role}{RESET} {DIM}· {} ──{padding}{RESET}",
            msg.created_at
        );
        for block in &msg.content {
            match block {
                MessageContent::Text { text } => println!("{text}"),
                MessageContent::ToolUse { name, input, .. } => match &mode {
                    HistoryMode::Compact => {
                        let input_str = serde_json::to_string(input).unwrap_or_default();
                        println!("{YELLOW}[tool: {name}]{RESET} {DIM}{input_str}{RESET}");
                    }
                    HistoryMode::Pretty => {
                        let truncated = truncate_json_strings(input, 200);
                        let formatted =
                            serde_json::to_string_pretty(&truncated).unwrap_or_default();
                        println!("{YELLOW}[tool: {name}]{RESET}");
                        println!("{DIM}{formatted}{RESET}");
                    }
                    HistoryMode::Full => {
                        println!("{YELLOW}[tool: {name}]{RESET}");
                        print_expanded_json(input);
                    }
                },
                MessageContent::ToolResult {
                    tool_use_id,
                    content,
                } => match &mode {
                    HistoryMode::Compact => {
                        let display = truncate_str(content, 200);
                        println!("{MAGENTA}[result: {tool_use_id}]{RESET} {DIM}{display}{RESET}");
                    }
                    HistoryMode::Pretty => {
                        let display = truncate_str(content, 500);
                        println!("{MAGENTA}[result: {tool_use_id}]{RESET}");
                        println!("{DIM}{display}{RESET}");
                    }
                    HistoryMode::Full => {
                        println!("{MAGENTA}[result: {tool_use_id}]{RESET}");
                        println!("{content}");
                    }
                },
            }
        }
    }

    Ok(())
}

/// truncate a string to `max` chars, appending `…` if truncated
fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// recursively truncate long string values inside a JSON value
fn truncate_json_strings(value: &serde_json::Value, max: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(truncate_str(s, max)),
        serde_json::Value::Object(map) => {
            let truncated = map
                .iter()
                .map(|(k, v)| (k.clone(), truncate_json_strings(v, max)))
                .collect();
            serde_json::Value::Object(truncated)
        }
        serde_json::Value::Array(arr) => {
            let truncated = arr.iter().map(|v| truncate_json_strings(v, max)).collect();
            serde_json::Value::Array(truncated)
        }
        other => other.clone(),
    }
}

/// print a JSON value in expanded key-value format with newlines rendered
fn print_expanded_json(value: &serde_json::Value) {
    const DIM: &str = "\x1b[2m";
    const RESET: &str = "\x1b[0m";

    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                match val {
                    serde_json::Value::String(s) => {
                        if s.contains('\n') {
                            println!("  {DIM}{key}:{RESET}");
                            for line in s.lines() {
                                println!("    {line}");
                            }
                        } else {
                            println!("  {DIM}{key}:{RESET} \"{s}\"");
                        }
                    }
                    serde_json::Value::Number(n) => println!("  {DIM}{key}:{RESET} {n}"),
                    serde_json::Value::Bool(b) => println!("  {DIM}{key}:{RESET} {b}"),
                    serde_json::Value::Null => println!("  {DIM}{key}:{RESET} null"),
                    other => {
                        // nested objects/arrays: pretty-print with indent
                        let formatted = serde_json::to_string_pretty(other).unwrap_or_default();
                        println!("  {DIM}{key}:{RESET}");
                        for line in formatted.lines() {
                            println!("    {line}");
                        }
                    }
                }
            }
        }
        // non-object top-level: just pretty-print
        other => {
            let formatted = serde_json::to_string_pretty(other).unwrap_or_default();
            println!("{formatted}");
        }
    }
}

/// load the persisted provider/model for the active session, falling back to default
fn provider_for_session(
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
                    tracing::warn!(%e, %model_id, "failed to load persisted model, using default");
                }
            }
        } else {
            tracing::warn!(%model_id, "invalid model id format, using default");
        }
    }
    AnyProvider::default_from_env(client)
}

async fn run_message(content: String) -> Result<(), error::Error> {
    let client = reqwest::Client::new();
    let db = Arc::new(Database::open()?);
    let provider = provider_for_session(client.clone(), &db)?;
    let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, client);

    let inbound = InboundMessage {
        channel: ChannelKind::Cli,
        content,
    };

    if let Some(outbound) = agent.process(&inbound).await? {
        channel::CliChannel.send(outbound)?;
    }
    Ok(())
}

/// find orphaned tool_use blocks: assistant messages with tool_use not
/// followed by matching tool_results.
fn find_orphaned_tool_calls(messages: &[(i64, Message)]) -> Vec<(usize, i64, Vec<String>)> {
    let mut orphans = Vec::new();

    for i in 0..messages.len() {
        let (msg_id, ref msg) = messages[i];

        if msg.role != Role::Assistant {
            continue;
        }

        let tool_use_ids: Vec<String> = msg
            .content
            .iter()
            .filter_map(|c| match c {
                MessageContent::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        if tool_use_ids.is_empty() {
            continue;
        }

        let has_results = messages.get(i + 1).is_some_and(|(_, next)| {
            next.role == Role::User
                && tool_use_ids.iter().all(|id| {
                    next.content.iter().any(|c| {
                        matches!(c, MessageContent::ToolResult { tool_use_id, .. } if tool_use_id == id)
                    })
                })
        });

        if !has_results {
            orphans.push((i, msg_id, tool_use_ids));
        }
    }

    orphans
}

fn is_ava_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", "ava start"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_doctor_diagnose() -> Result<(), error::Error> {
    let mut issues = 0u32;

    // check 1: is ava running?
    if is_ava_running() {
        println!("  ok: ava process is running");
    } else {
        println!("  warning: ava is not running (start with `ava start`)");
        issues += 1;
    }

    // check 2: orphaned tool_use blocks
    let db = Database::open()?;
    let session_id = db.active_session()?;
    let messages = db.load_messages_with_ids(session_id)?;
    let orphans = find_orphaned_tool_calls(&messages);

    if orphans.is_empty() {
        println!("  ok: no orphaned tool calls");
    } else {
        let total_blocks: usize = orphans.iter().map(|(_, _, ids)| ids.len()).sum();
        println!(
            "  error: {} orphaned tool_use block(s) across {} message(s)",
            total_blocks,
            orphans.len()
        );
        println!("         fix with `ava doctor repair-orphans`");
        issues += 1;
    }

    if issues == 0 {
        println!("\nsession is healthy");
    } else {
        println!("\nfound {issues} issue(s)");
    }

    Ok(())
}

fn run_doctor_fix() -> Result<(), error::Error> {
    let db = Database::open()?;
    let session_id = db.active_session()?;
    let messages = db.load_messages_with_ids(session_id)?;
    let orphans = find_orphaned_tool_calls(&messages);

    if orphans.is_empty() {
        println!("nothing to fix");
        return Ok(());
    }

    let mut repaired = 0usize;
    for (_, msg_id, tool_use_ids) in &orphans {
        let synthetic: Vec<MessageContent> = tool_use_ids
            .iter()
            .map(|id| {
                MessageContent::tool_result(
                    id,
                    "the session was interrupted and it is unknown whether this tool call completed.",
                )
            })
            .collect();

        db.insert_message_after(session_id, *msg_id, "user", &synthetic)?;
        repaired += 1;
        println!(
            "  repaired orphaned tool_use at message id {msg_id} ({} blocks)",
            tool_use_ids.len()
        );
    }

    println!("repaired {repaired} orphaned tool_use block(s)");
    Ok(())
}

fn allowed_telegram_ids() -> Vec<i64> {
    std::env::var("TELEGRAM_ALLOWED_IDS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

async fn run_start() -> Result<(), error::Error> {
    let client = reqwest::Client::new();
    let db = Arc::new(Database::open()?);
    let (tx, rx) = message_queue(64);
    let pending = Arc::new(PendingApprovals::new());

    // start agent loop
    let db_clone = Arc::clone(&db);
    let pending_clone = Arc::clone(&pending);
    let client_clone = client.clone();
    let agent_handle = tokio::spawn(async move {
        agent_loop(rx, db_clone, pending_clone, client_clone).await;
    });

    // start telegram if configured
    if std::env::var("TELOXIDE_TOKEN").is_ok() {
        let bot = Arc::new(TelegramBot::from_env()?);
        let allowed_ids = allowed_telegram_ids();

        if allowed_ids.is_empty() {
            tracing::warn!("TELEGRAM_ALLOWED_IDS not set, bot will ignore all messages");
        } else {
            tracing::info!(?allowed_ids, "loaded user whitelist");
        }

        // spawn scheduler if we have a default chat_id
        if let Some(&chat_id) = allowed_ids.first() {
            let db_sched = Arc::clone(&db);
            let tx_sched = tx.clone();
            let bot_sched = Arc::clone(&bot);
            tokio::spawn(scheduler::run(db_sched, tx_sched, bot_sched, chat_id));
        }

        tracing::info!("starting telegram channel");
        tokio::spawn(telegram_producer(
            bot,
            tx.clone(),
            Arc::clone(&pending),
            allowed_ids,
        ));
    } else {
        tracing::info!("TELOXIDE_TOKEN not set, skipping telegram");
    }

    // drop our copy of tx so agent_loop can exit when all producers stop
    drop(tx);

    agent_handle
        .await
        .map_err(|e| error::Error::Provider(format!("agent loop panicked: {e}")))?;
    Ok(())
}

async fn agent_loop(
    mut rx: MessageReceiver,
    db: Arc<Database>,
    pending: Arc<PendingApprovals>,
    client: reqwest::Client,
) {
    while let Some(queued) = rx.recv().await {
        let provider = match provider_for_session(client.clone(), &db) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(%e, "provider init failed");
                send_error(queued.sink, &format!("error: {e}")).await;
                continue;
            }
        };

        let approver = match &queued.sink {
            ResponseSink::Telegram { chat_id, bot } => {
                AnyApprover::Telegram(TelegramApprover::new(
                    Arc::clone(bot),
                    *chat_id,
                    Arc::clone(&pending),
                    Arc::clone(&db),
                ))
            }
        };

        let agent = Agent::new(provider, approver, Arc::clone(&db), client.clone());
        let inbound = InboundMessage {
            channel: queued.channel,
            content: queued.content,
        };

        match agent.process(&inbound).await {
            Ok(Some(outbound)) => send_response(queued.sink, outbound).await,
            Ok(None) => tracing::debug!("agent completed silently"),
            Err(e) => {
                tracing::error!(%e, "agent processing failed");
                send_error(queued.sink, &format!("error: {e}")).await;
            }
        }
    }
}

async fn telegram_producer(
    bot: Arc<TelegramBot>,
    tx: queue::MessageSender,
    pending: Arc<PendingApprovals>,
    allowed_ids: Vec<i64>,
) {
    let mut offset: Option<i64> = None;

    loop {
        let updates = match bot.get_updates(offset).await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!(%e, "failed to fetch updates");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        for update in updates {
            offset = Some(update.update_id + 1);

            // handle callback queries (approval button presses)
            if let Some(callback) = update.callback_query {
                if let Some(data) = &callback.data {
                    let chat_id = callback
                        .message
                        .as_ref()
                        .map(|m| m.chat.id)
                        .unwrap_or_default();

                    TelegramApprover::handle_callback(&pending, &bot, &callback.id, data, chat_id)
                        .await;
                }
                continue;
            }

            // handle text messages
            let Some(msg) = update.message else {
                continue;
            };

            let Some(text) = msg.text else {
                continue;
            };

            let chat_id = msg.chat.id;
            let user_id = msg.from.map(|u| u.id);

            // check whitelist
            let is_allowed = user_id.map(|id| allowed_ids.contains(&id)).unwrap_or(false);
            if !is_allowed {
                tracing::warn!(?user_id, "ignoring message from unauthorized user");
                continue;
            }

            // push to queue instead of spawning a task
            let queued = QueuedMessage {
                channel: ChannelKind::Telegram,
                content: text,
                sink: ResponseSink::Telegram {
                    chat_id,
                    bot: Arc::clone(&bot),
                },
            };

            if tx.send(queued).await.is_err() {
                tracing::error!("agent loop stopped, exiting telegram producer");
                return;
            }
        }
    }
}
