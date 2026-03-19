use std::sync::Arc;

use crate::agent::Agent;
use crate::approver::{AnyApprover, PendingApprovals, TelegramApprover};
use crate::cli::{
    handle_rules_command, handle_switch_command, parse_slash_command, provider_for_session,
};
use crate::db::Database;
use crate::error;
use crate::message::{ChannelKind, InboundMessage};
use crate::queue::{
    MessageReceiver, QueuedMessage, ResponseSink, message_queue, send_error, send_response,
};
use crate::telegram::TelegramBot;

fn allowed_telegram_ids() -> Vec<i64> {
    std::env::var("TELEGRAM_ALLOWED_IDS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

pub(crate) async fn run_start() -> Result<(), error::Error> {
    let client = reqwest::Client::new();
    let db = Arc::new(Database::open()?);
    let (tx, rx) = message_queue(64);
    let pending = Arc::new(PendingApprovals::new());

    crate::config::write_pid_file();
    tracing::info!(pid = std::process::id(), "wrote PID file");

    crate::signal::install_signal_handler();

    // start MCP servers (if configured)
    let mcp = match crate::mcp::manager::McpManager::start().await {
        Ok(m) => {
            if m.has_servers().await {
                Some(m)
            } else {
                None
            }
        }
        Err(e) => {
            tracing::warn!(%e, "failed to start MCP manager, continuing without MCP");
            None
        }
    };

    // load skills from ~/.ava/skills/
    let skills = Arc::new(crate::skill::load_skills());
    if !skills.is_empty() {
        tracing::info!(count = skills.len(), "loaded skills");
    }

    // start agent loop
    let db_clone = Arc::clone(&db);
    let pending_clone = Arc::clone(&pending);
    let client_clone = client.clone();
    let mcp_clone = mcp.clone();
    let skills_clone = Arc::clone(&skills);
    let agent_handle = tokio::spawn(async move {
        agent_loop(
            rx,
            db_clone,
            pending_clone,
            client_clone,
            mcp_clone,
            skills_clone,
        )
        .await;
    });

    // start telegram if configured
    if std::env::var("TELEGRAM_BOT_TOKEN").is_ok() {
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
            tokio::spawn(crate::scheduler::run(
                db_sched, tx_sched, bot_sched, chat_id,
            ));
        }

        tracing::info!("starting telegram channel");
        tokio::spawn(telegram_producer(
            bot,
            tx.clone(),
            Arc::clone(&pending),
            allowed_ids,
        ));
    } else {
        tracing::info!("TELEGRAM_BOT_TOKEN not set, skipping telegram");
    }

    // drop our copy of tx so agent_loop can exit when all producers stop
    drop(tx);

    agent_handle
        .await
        .map_err(|e| error::Error::Provider(format!("agent loop panicked: {e}")))?;

    // shut down MCP servers
    if let Some(ref mcp) = mcp {
        mcp.shutdown().await;
    }

    crate::config::remove_pid_file();
    Ok(())
}

async fn agent_loop(
    mut rx: MessageReceiver,
    db: Arc<Database>,
    pending: Arc<PendingApprovals>,
    client: reqwest::Client,
    mcp: Option<Arc<crate::mcp::manager::McpManager>>,
    skills: Arc<Vec<crate::skill::Skill>>,
) {
    while let Some(queued) = rx.recv().await {
        if let Some(("switch", args)) = parse_slash_command(&queued.content) {
            let msg = handle_switch_command(args, client.clone(), &db);
            send_response(
                queued.sink,
                crate::message::OutboundMessage { content: msg },
            )
            .await;
            continue;
        }

        if let Some(("rules", args)) = parse_slash_command(&queued.content) {
            let msg = handle_rules_command(args, &db);
            send_response(
                queued.sink,
                crate::message::OutboundMessage { content: msg },
            )
            .await;
            continue;
        }

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

        let mut agent = Agent::new(provider, approver, Arc::clone(&db), client.clone())
            .with_skills(Arc::clone(&skills));
        if let Some(ref mcp) = mcp {
            agent = agent.with_mcp(Arc::clone(mcp));
        }
        let inbound = InboundMessage {
            channel: queued.channel,
            content: queued.content,
        };

        match agent.process(&inbound).await {
            Ok(Some(outbound)) => send_response(queued.sink, outbound).await,
            Ok(None) => tracing::debug!("agent completed silently"),
            Err(error::Error::RateLimited(ref msg)) => {
                tracing::warn!(%msg, "rate limited");
                send_error(queued.sink, &format!("rate limited: {msg}")).await;
            }
            Err(error::Error::BudgetExhausted(ref msg)) => {
                tracing::error!(%msg, "budget exhausted, fallback also failed");
                let help = format!(
                    "budget exhausted: {msg}\n\n\
                     automatic fallback failed. use `/switch <provider>` to \
                     switch manually (e.g. `/switch openai` or `/switch anthropic`)."
                );
                send_error(queued.sink, &help).await;
            }
            Err(e) => {
                tracing::error!(%e, "agent processing failed");
                send_error(queued.sink, &format!("error: {e}")).await;
            }
        }

        if crate::signal::restart_requested() {
            #[cfg(unix)]
            match crate::signal::do_exec_restart() {
                // do_exec_restart never returns Ok — exec replaces the process
                Err(e) => {
                    tracing::error!(%e, "exec restart failed, continuing normally");
                }
            }
        }
    }
}

async fn telegram_producer(
    bot: Arc<TelegramBot>,
    tx: crate::queue::MessageSender,
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

                    let message_id = callback.message.as_ref().map(|m| m.message_id);
                    TelegramApprover::handle_callback(
                        &pending,
                        &bot,
                        &callback.id,
                        data,
                        chat_id,
                        message_id,
                    )
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
