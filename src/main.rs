mod agent;
mod channel;
mod config;
mod db;
mod error;
mod message;
mod provider;
mod telegram;
mod tool;

use clap::{Parser, Subcommand};

use crate::agent::Agent;
use crate::channel::Channel;
use crate::db::Database;
use crate::message::{ChannelKind, InboundMessage};
use crate::provider::AnthropicProvider;
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
    /// start the telegram bot
    Telegram,
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

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("ava {}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Status => {
            println!("ava {}", env!("CARGO_PKG_VERSION"));
            println!("db: {}", config::default_db_path().display());
        }
        Commands::Message { content } => {
            if let Err(e) = run_message(content).await {
                tracing::error!(%e, "message command failed");
                std::process::exit(1);
            }
        }
        Commands::Telegram => {
            if let Err(e) = run_telegram().await {
                tracing::error!(%e, "telegram bot failed");
                std::process::exit(1);
            }
        }
    }
}

async fn run_message(content: String) -> Result<(), error::Error> {
    let provider = AnthropicProvider::from_env()?;
    let db = Database::open()?;
    let agent = Agent::new(provider, db);

    let inbound = InboundMessage {
        channel: ChannelKind::Cli,
        content,
    };

    let outbound = agent.process(inbound).await?;
    channel::CliChannel.send(outbound)?;
    Ok(())
}

fn allowed_telegram_ids() -> Vec<i64> {
    std::env::var("TELEGRAM_ALLOWED_IDS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

async fn run_telegram() -> Result<(), error::Error> {
    let bot = TelegramBot::from_env()?;
    let allowed_ids = allowed_telegram_ids();

    if allowed_ids.is_empty() {
        tracing::warn!("TELEGRAM_ALLOWED_IDS not set, bot will ignore all messages");
    } else {
        tracing::info!(?allowed_ids, "loaded user whitelist");
    }

    tracing::info!("starting telegram bot");

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

            // create provider and agent for each message
            // (in the future, we'll have sessions to maintain state)
            let provider = match AnthropicProvider::from_env() {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!(%e, "provider init failed");
                    let _ = bot.send_message(chat_id, &format!("error: {e}")).await;
                    continue;
                }
            };

            let db = match Database::open() {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(%e, "database open failed");
                    let _ = bot.send_message(chat_id, &format!("error: {e}")).await;
                    continue;
                }
            };

            let agent = Agent::new(provider, db);

            let inbound = InboundMessage {
                channel: ChannelKind::Telegram,
                content: text,
            };

            match agent.process(inbound).await {
                Ok(outbound) => {
                    if let Err(e) = bot.send_message(chat_id, &outbound.content).await {
                        tracing::error!(%e, chat_id, "failed to send telegram message");
                    }
                }
                Err(e) => {
                    tracing::error!(%e, chat_id, "agent processing failed");
                    let _ = bot.send_message(chat_id, &format!("error: {e}")).await;
                }
            }
        }
    }
}
