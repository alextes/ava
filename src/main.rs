mod agent;
mod channel;
mod config;
mod db;
mod error;
mod message;
mod provider;

use clap::{Parser, Subcommand};

use crate::agent::Agent;
use crate::channel::CliChannel;
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
    }
}

async fn run_message(content: String) -> Result<(), error::Error> {
    let provider = AnthropicProvider::from_env()?;
    let agent = Agent::new(provider);
    let channel = CliChannel;

    let inbound = InboundMessage {
        channel: ChannelKind::Cli,
        content,
    };

    agent.process(inbound, &channel).await
}
