mod agent;
mod channel;
mod config;
mod db;
mod error;
mod message;
mod provider;
mod telegram;

use clap::{Parser, Subcommand};

use crate::agent::Agent;
use crate::channel::Channel;
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

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("ava {}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Status => {
            println!("ava {}", env!("CARGO_PKG_VERSION"));

            match config::default_db_path() {
                Ok(path) => println!("db: {}", path.display()),
                Err(e) => println!("db: error: {e}"),
            }
        }
        Commands::Message { content } => {
            if let Err(e) = run_message(content).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Telegram => {
            if let Err(e) = run_telegram().await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

async fn run_message(content: String) -> Result<(), error::Error> {
    let provider = AnthropicProvider::from_env()?;
    let agent = Agent::new(provider);

    let inbound = InboundMessage {
        channel: ChannelKind::Cli,
        content,
    };

    let outbound = agent.process(inbound).await?;
    channel::CliChannel.send(outbound)?;
    Ok(())
}

async fn run_telegram() -> Result<(), error::Error> {
    let bot = TelegramBot::from_env()?;

    println!("starting telegram bot...");

    let mut offset: Option<i64> = None;

    loop {
        let updates = match bot.get_updates(offset).await {
            Ok(u) => u,
            Err(e) => {
                eprintln!("error fetching updates: {e}");
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

            // create provider and agent for each message
            // (in the future, we'll have sessions to maintain state)
            let provider = match AnthropicProvider::from_env() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("provider error: {e}");
                    let _ = bot.send_message(chat_id, &format!("error: {e}")).await;
                    continue;
                }
            };

            let agent = Agent::new(provider);

            let inbound = InboundMessage {
                channel: ChannelKind::Telegram,
                content: text,
            };

            match agent.process(inbound).await {
                Ok(outbound) => {
                    if let Err(e) = bot.send_message(chat_id, &outbound.content).await {
                        eprintln!("error sending message: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("agent error: {e}");
                    let _ = bot.send_message(chat_id, &format!("error: {e}")).await;
                }
            }
        }
    }
}
