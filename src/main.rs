mod agent;
mod channel;
mod config;
mod db;
mod error;
mod message;
mod provider;

use clap::{Parser, Subcommand};
use teloxide::prelude::*;

use crate::agent::Agent;
use crate::channel::Channel;
use crate::message::{ChannelKind, InboundMessage};
use crate::provider::AnthropicProvider;

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
    let bot = Bot::from_env();

    println!("starting telegram bot...");

    teloxide::repl(bot, |bot: Bot, msg: teloxide::types::Message| async move {
        let Some(text) = msg.text() else {
            return Ok(());
        };

        // create provider and agent for each message
        // (in the future, we'll have sessions to maintain state)
        let provider = match AnthropicProvider::from_env() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("provider error: {e}");
                bot.send_message(msg.chat.id, format!("error: {e}")).await?;
                return Ok(());
            }
        };

        let agent = Agent::new(provider);

        let inbound = InboundMessage {
            channel: ChannelKind::Telegram,
            content: text.to_string(),
        };

        match agent.process(inbound).await {
            Ok(outbound) => {
                bot.send_message(msg.chat.id, outbound.content).await?;
            }
            Err(e) => {
                eprintln!("agent error: {e}");
                bot.send_message(msg.chat.id, format!("error: {e}")).await?;
            }
        }

        Ok(())
    })
    .await;

    Ok(())
}
