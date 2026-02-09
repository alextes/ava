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
use crate::message::{ChannelKind, InboundMessage};
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

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting ava");

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
