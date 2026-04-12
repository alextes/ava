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

fn parse_id_list(env_var: &str) -> Vec<i64> {
    std::env::var(env_var)
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

    // seed whitelists from env vars
    let allowed_user_ids = parse_id_list("TELEGRAM_ALLOWED_IDS");
    let allowed_chat_ids = parse_id_list("TELEGRAM_ALLOWED_CHATS");
    if !allowed_user_ids.is_empty() {
        db.seed_allowed_users(&allowed_user_ids)?;
        tracing::info!(
            count = allowed_user_ids.len(),
            "seeded allowed users from env"
        );
    }
    if !allowed_chat_ids.is_empty() {
        db.seed_allowed_chats(&allowed_chat_ids)?;
        tracing::info!(
            count = allowed_chat_ids.len(),
            "seeded allowed chats from env"
        );
    }

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

    // load skills from ~/.ava/skills/ and ~/.claude/skills/
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

    // track background tasks so we can abort them on shutdown
    let mut bg_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // start telegram if configured
    if std::env::var("TELEGRAM_BOT_TOKEN").is_ok() {
        let bot = Arc::new(TelegramBot::from_env()?);
        let allowed_users = db.list_allowed_users().unwrap_or_default();

        if allowed_users.is_empty() {
            tracing::warn!("no allowed users configured, bot will ignore all DMs");
        } else {
            tracing::info!(?allowed_users, "loaded user whitelist from DB");
        }

        let allowed_chats = db.list_allowed_chats().unwrap_or_default();
        if !allowed_chats.is_empty() {
            tracing::info!(?allowed_chats, "loaded chat whitelist from DB");
        }

        // spawn scheduler if we have a default chat_id
        if let Some(&chat_id) = allowed_users.first() {
            let db_sched = Arc::clone(&db);
            let tx_sched = tx.clone();
            let bot_sched = Arc::clone(&bot);
            bg_tasks.push(tokio::spawn(crate::scheduler::run(
                db_sched, tx_sched, bot_sched, chat_id,
            )));
        }

        tracing::info!("starting telegram channel");
        bg_tasks.push(tokio::spawn(telegram_producer(
            bot,
            tx.clone(),
            Arc::clone(&pending),
            Arc::clone(&db),
        )));
    } else {
        tracing::info!("TELEGRAM_BOT_TOKEN not set, skipping telegram");
    }

    // drop our copy so the channel closes once bg tasks are aborted
    drop(tx);

    // wait for either Ctrl+C or the agent loop to finish naturally.
    tokio::pin!(agent_handle);
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received SIGINT, shutting down");
            // abort background producers so their tx clones are dropped,
            // which closes the channel and lets the agent loop exit.
            for task in &bg_tasks {
                task.abort();
            }
            // give the agent loop a moment to finish its current work
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                agent_handle,
            ).await;
        }
        result = &mut agent_handle => {
            for task in &bg_tasks {
                task.abort();
            }
            result.map_err(|e| error::Error::Provider(format!("agent loop panicked: {e}")))?;
        }
    }

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
                crate::message::OutboundMessage {
                    content: msg,
                    voice: None,
                },
            )
            .await;
            continue;
        }

        if let Some(("rules", args)) = parse_slash_command(&queued.content) {
            let msg = handle_rules_command(args, &db);
            send_response(
                queued.sink,
                crate::message::OutboundMessage {
                    content: msg,
                    voice: None,
                },
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
    db: Arc<Database>,
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

            // handle bot membership changes (added/removed from chats)
            if let Some(membership) = update.my_chat_member {
                let chat = &membership.chat;
                let status = &membership.new_chat_member.status;
                match status.as_str() {
                    "member" | "administrator" => {
                        // bot was added — fetch full metadata via getChat
                        match bot.get_chat(chat.id).await {
                            Ok(full_chat) => {
                                let chat_type = full_chat.chat_type.as_deref().unwrap_or("unknown");
                                let _ = db.upsert_channel(
                                    chat.id,
                                    chat_type,
                                    full_chat.title.as_deref(),
                                );
                                tracing::info!(
                                    chat_id = chat.id,
                                    chat_type,
                                    title = full_chat.title.as_deref().unwrap_or("(none)"),
                                    "bot added to chat"
                                );
                            }
                            Err(e) => {
                                // fall back to whatever we have from the update
                                let chat_type = chat.chat_type.as_deref().unwrap_or("unknown");
                                let _ =
                                    db.upsert_channel(chat.id, chat_type, chat.title.as_deref());
                                tracing::warn!(
                                    chat_id = chat.id,
                                    %e,
                                    "bot added to chat but getChat failed, using partial metadata"
                                );
                            }
                        }
                    }
                    "left" | "kicked" => {
                        let _ = db.remove_channel(chat.id);
                        tracing::info!(chat_id = chat.id, status, "bot removed from chat");
                    }
                    _ => {
                        tracing::debug!(
                            chat_id = chat.id,
                            status,
                            "ignoring unhandled membership status"
                        );
                    }
                }
                continue;
            }

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
            let chat_type = msg.chat.chat_type.as_deref().unwrap_or("private");
            let is_group = chat_type == "group" || chat_type == "supergroup";

            // pre-agent authorization — no LLM sees rejected messages
            if is_group {
                // group chats: check chat_id whitelist
                let chat_allowed = db.is_chat_allowed(chat_id).unwrap_or(false);
                if !chat_allowed {
                    tracing::debug!(chat_id, "ignoring message from non-whitelisted chat");
                    continue;
                }
            } else {
                // DMs: check user_id whitelist
                let user_allowed = user_id
                    .map(|id| db.is_user_allowed(id).unwrap_or(false))
                    .unwrap_or(false);
                if !user_allowed {
                    tracing::warn!(?user_id, "rejecting DM from non-whitelisted user");
                    let _ = bot
                        .send_message(chat_id, "DM not available for this user.")
                        .await;
                    continue;
                }
            }

            // track channel activity
            let _ = db.upsert_channel(chat_id, chat_type, msg.chat.title.as_deref());

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
