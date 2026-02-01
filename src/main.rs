mod config;
mod db;
mod error;

use clap::{Parser, Subcommand};

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
}

fn main() {
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
    }
}
