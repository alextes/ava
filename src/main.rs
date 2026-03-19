mod agent;
mod approver;
mod channel;
mod cli;
mod commands;
mod config;
mod daemon;
mod db;
mod error;
mod mcp;
mod message;
mod provider;
mod queue;
mod scheduler;
mod signal;
mod telegram;
mod telegram_fmt;
mod tool;

use clap::Parser;

use crate::cli::{Cli, Commands, DoctorAction};
use crate::commands::{
    run_doctor_diagnose, run_doctor_fix, run_history, run_logs, run_message, run_restart,
    run_rules, run_schedules, run_start, run_status, run_stop, run_upgrade,
};

fn main() {
    dotenvy::dotenv().ok();
    config::init_workspace_root();

    let cli = Cli::parse();

    // daemonize before tokio runtime and tracing are initialized.
    // fork() must happen while the process is single-threaded.
    let daemonized = matches!(cli.command, Commands::Start { foreground: false });

    if daemonized {
        if let Some(pid) = daemon::is_already_running() {
            eprintln!("ava is already running (pid {pid})");
            std::process::exit(1);
        }

        if let Err(e) = daemon::daemonize() {
            eprintln!("failed to daemonize: {e}");
            std::process::exit(1);
        }
    }

    init_tracing(daemonized);

    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "starting ava");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(async_main(cli));
}

fn init_tracing(daemonized: bool) {
    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive(tracing::Level::INFO.into());

    let log_dir = config::ava_home_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        if daemonized {
            // can't write to stdout in daemon mode, just exit
            std::process::exit(1);
        }
        eprintln!(
            "warning: could not create log directory {}: {e}",
            log_dir.display()
        );
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        return;
    }

    let file_appender = tracing_appender::rolling::daily(&log_dir, "ava.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);

    // in foreground mode, also log to stdout with colors.
    // Option<Layer> is itself a Layer — None is a no-op.
    let stdout_layer = if daemonized {
        None
    } else {
        Some(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    // keep the guard alive for the entire program — leak it so the
    // non-blocking writer flushes on drop at process exit.
    std::mem::forget(_guard);
}

async fn async_main(cli: Cli) {
    match cli.command {
        Commands::Version => {
            println!("ava {}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Status => {
            run_status();
        }
        Commands::Stop => {
            run_stop();
        }
        Commands::Restart { foreground } => {
            run_restart(foreground);
        }
        Commands::Logs { lines, follow } => {
            run_logs(lines, follow);
        }
        Commands::Message { content } => {
            if let Err(e) = run_message(content).await {
                tracing::error!(%e, "message command failed");
                std::process::exit(1);
            }
        }
        Commands::Start { .. } => {
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
        Commands::Upgrade => {
            if let Err(e) = run_upgrade() {
                tracing::error!(%e, "upgrade failed");
                std::process::exit(1);
            }
        }
        Commands::Rules { action } => {
            if let Err(e) = run_rules(action) {
                tracing::error!(%e, "rules command failed");
                std::process::exit(1);
            }
        }
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
