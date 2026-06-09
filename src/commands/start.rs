use std::sync::Arc;

use crate::agent::Agent;
use crate::approver::{AnyApprover, PendingApprovals, TelegramApprover};
use crate::chat_buffer::{BufferedMessage, ChatBuffer};
use crate::cli::{
    handle_rules_command, handle_switch_command, parse_slash_command, parse_steer_command,
    provider_for_session,
};
use crate::cold_resume::{ColdResumePrompter, PendingColdResumes};
use crate::db::{Database, QueuedRecord, RuntimeEvent};
use crate::error;
use crate::message::{ChannelKind, ImageSource, InboundMessage};
use crate::queue::{ResponseSink, WakeReceiver, message_queue, send_error, send_response};
use crate::runtime::{PendingSteer, RuntimeState, SteerOrigin};
use crate::telegram::TelegramBot;

const STEER_REJECTION: &str = "no active turn to steer. send a normal message instead.";

struct BotIdentity {
    id: i64,
    username: String,
}

impl BotIdentity {
    /// check if a message is directed at the bot via @mention entities.
    fn is_mentioned_in_entities(
        &self,
        text: &str,
        entities: &[crate::telegram::MessageEntity],
    ) -> bool {
        entities.iter().any(|e| {
            if e.entity_type == "mention" {
                let start = e.offset as usize;
                let end = start.saturating_add(e.length as usize);
                text.get(start..end).is_some_and(|mention| {
                    mention.eq_ignore_ascii_case(&format!("@{}", self.username))
                })
            } else {
                e.entity_type == "text_mention" && e.user.as_ref().is_some_and(|u| u.id == self.id)
            }
        })
    }

    /// check if the bot's name appears in the message text as a word (case-insensitive).
    /// matches "ren," and "hi ren!" but not "current" or "different".
    fn is_named_in_text(&self, text: &str, display_name: &str) -> bool {
        let lower = text.to_lowercase();
        for name in [&self.username, &display_name.to_lowercase()] {
            if name.is_empty() {
                continue;
            }
            let name_lower = name.to_lowercase();
            let mut start = 0;
            while let Some(pos) = lower[start..].find(&name_lower) {
                let abs_pos = start + pos;
                let end_pos = abs_pos + name_lower.len();
                // check word boundary: char before and after must be non-alphanumeric
                let before_ok =
                    abs_pos == 0 || !lower.as_bytes()[abs_pos - 1].is_ascii_alphanumeric();
                let after_ok =
                    end_pos >= lower.len() || !lower.as_bytes()[end_pos].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    return true;
                }
                start = abs_pos + 1;
            }
        }
        false
    }

    /// strip @bot_username from the text.
    fn strip_mention<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if self.username.is_empty() {
            return std::borrow::Cow::Borrowed(text);
        }
        let pattern = format!("@{}", self.username);
        if let Some(pos) = text.to_lowercase().find(&pattern.to_lowercase()) {
            let mut result = String::with_capacity(text.len());
            result.push_str(&text[..pos]);
            result.push_str(&text[pos + pattern.len()..]);
            let trimmed = result.trim();
            std::borrow::Cow::Owned(trimmed.to_string())
        } else {
            std::borrow::Cow::Borrowed(text)
        }
    }

    /// check if a message is a reply to a message sent by this bot.
    fn is_reply_to_bot(&self, reply_to: Option<&crate::telegram::Message>) -> bool {
        reply_to
            .and_then(|m| m.from.as_ref())
            .is_some_and(|u| u.id == self.id)
    }
}

fn parse_id_list(env_var: &str) -> Vec<i64> {
    std::env::var(env_var)
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

fn startup_model_note(prior_event: Option<&RuntimeEvent>) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");
    match prior_event {
        Some(event) if event.is_restart() => format!(
            "[system: ava restarted at {now} after {}. no external restart notice was sent. \
             if recent work was interrupted, inspect the conversation state and continue \
             appropriately.]",
            event.reason
        ),
        Some(event) => format!(
            "[system: ava started at {now}. previous runtime event was {}: {}.]",
            event.source, event.reason
        ),
        None => format!("[system: ava started at {now}.]"),
    }
}

fn persist_startup_model_note(db: &Database, prior_event: Option<&RuntimeEvent>) {
    let note = startup_model_note(prior_event);
    let content = vec![crate::message::MessageContent::text(note)];
    match db.active_session() {
        Ok(session_id) => {
            if let Err(e) = db.append_message(session_id, "system", &content, None) {
                tracing::warn!(%e, "failed to persist startup model note");
            }
        }
        Err(e) => tracing::warn!(%e, "failed to load active session for startup note"),
    }
}

pub(crate) async fn run_start() -> Result<(), error::Error> {
    let client = reqwest::Client::new();
    let db = Arc::new(Database::open()?);
    let prior_runtime_event = match db.last_runtime_event() {
        Ok(event) => event,
        Err(e) => {
            tracing::warn!(%e, "failed to load prior runtime event");
            None
        }
    };
    persist_startup_model_note(&db, prior_runtime_event.as_ref());
    if let Err(e) = db.record_runtime_event("startup", "daemon started") {
        tracing::warn!(%e, "failed to record startup event");
    }
    match db.reset_processing_messages() {
        Ok(count) if count > 0 => {
            tracing::info!(count, "reset interrupted queued messages to pending")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "failed to reset interrupted queued messages"),
    }
    let (tx, rx) = message_queue(64);
    let pending = Arc::new(PendingApprovals::new());
    let pending_cold = Arc::new(PendingColdResumes::new());

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

    // shared message buffer for cross-channel context
    let chat_buffer = Arc::new(ChatBuffer::new());
    let runtime = Arc::new(RuntimeState::new(String::new()));

    // track background tasks so we can abort them on shutdown
    let mut bg_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut telegram_bot_for_agent: Option<Arc<TelegramBot>> = None;

    // start telegram if configured
    if std::env::var("TELEGRAM_BOT_TOKEN").is_ok() {
        let bot = Arc::new(TelegramBot::from_env()?);

        // fetch bot identity for mention detection
        let bot_identity = bot.get_me().await?;
        let bot_id = bot_identity.id;
        let bot_username = bot_identity.username.clone().unwrap_or_default();
        let db_identity_name = db.identity_name().unwrap_or(None);
        let env_bot_name = std::env::var("TELEGRAM_BOT_NAME").unwrap_or_default();
        let bot_name = db_identity_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| env_bot_name.clone());
        runtime.set_telegram_display_name(bot_name.clone());
        tracing::info!(
            bot_id,
            %bot_username,
            db_identity_name = ?db_identity_name,
            env_bot_name = %env_bot_name,
            resolved_display_name = %bot_name,
            "fetched bot identity"
        );
        telegram_bot_for_agent = Some(Arc::clone(&bot));

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
            bg_tasks.push(tokio::spawn(crate::scheduler::run(
                db_sched, tx_sched, chat_id,
            )));
        }

        tracing::info!("starting telegram channel");
        bg_tasks.push(tokio::spawn(telegram_producer(
            bot,
            tx.clone(),
            Arc::clone(&pending),
            Arc::clone(&pending_cold),
            Arc::clone(&db),
            Arc::clone(&chat_buffer),
            Arc::clone(&runtime),
            BotIdentity {
                id: bot_id,
                username: bot_username,
            },
        )));
    } else {
        tracing::info!("TELEGRAM_BOT_TOKEN not set, skipping telegram");
    }

    // start agent loop after channels are initialized so queued rows can be
    // converted into live response sinks.
    let db_clone = Arc::clone(&db);
    let pending_clone = Arc::clone(&pending);
    let pending_cold_clone = Arc::clone(&pending_cold);
    let client_clone = client.clone();
    let mcp_clone = mcp.clone();
    let skills_clone = Arc::clone(&skills);
    let chat_buffer_clone = Arc::clone(&chat_buffer);
    let runtime_clone = Arc::clone(&runtime);
    let agent_handle = tokio::spawn(async move {
        agent_loop(
            rx,
            db_clone,
            pending_clone,
            pending_cold_clone,
            client_clone,
            mcp_clone,
            skills_clone,
            chat_buffer_clone,
            runtime_clone,
            telegram_bot_for_agent,
        )
        .await;
    });
    if let Err(e) = tx.send(()).await {
        tracing::warn!(%e, "failed to send startup queue wake");
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

#[allow(clippy::too_many_arguments)]
async fn agent_loop(
    mut rx: WakeReceiver,
    db: Arc<Database>,
    pending: Arc<PendingApprovals>,
    pending_cold: Arc<PendingColdResumes>,
    client: reqwest::Client,
    mcp: Option<Arc<crate::mcp::manager::McpManager>>,
    skills: Arc<Vec<crate::skill::Skill>>,
    chat_buffer: Arc<ChatBuffer>,
    runtime: Arc<RuntimeState>,
    telegram_bot: Option<Arc<TelegramBot>>,
) {
    while rx.recv().await.is_some() {
        loop {
            let record = match db.next_pending_message() {
                Ok(Some(record)) => record,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(%e, "failed to load next queued message");
                    break;
                }
            };

            let Some(channel) = record.channel_kind() else {
                tracing::warn!(
                    queue_id = record.id,
                    channel = %record.channel,
                    "queued message has unsupported channel"
                );
                if let Err(e) = db.mark_message_failed(record.id, "unsupported channel") {
                    tracing::warn!(%e, queue_id = record.id, "failed to mark queue row failed");
                }
                continue;
            };

            let sink = match response_sink_for_record(&record, telegram_bot.as_ref()) {
                Some(sink) => sink,
                None => {
                    tracing::warn!(
                        queue_id = record.id,
                        channel = %record.channel,
                        "queued message cannot be routed yet"
                    );
                    break;
                }
            };

            let records = match queue_batch_for_record(&db, record.clone()) {
                Ok(records) => records,
                Err(e) => {
                    tracing::error!(%e, queue_id = record.id, "failed to load queue batch");
                    break;
                }
            };

            let queue_ids: Vec<i64> = records.iter().map(|r| r.id).collect();
            if let Err(e) = db.mark_messages_processing(&queue_ids) {
                tracing::error!(%e, ?queue_ids, "failed to mark queue rows processing");
                break;
            }

            process_queued_record(
                records,
                channel,
                sink,
                Arc::clone(&db),
                Arc::clone(&pending),
                Arc::clone(&pending_cold),
                client.clone(),
                mcp.clone(),
                Arc::clone(&skills),
                Arc::clone(&chat_buffer),
                Arc::clone(&runtime),
            )
            .await;

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
}

fn queue_batch_for_record(
    db: &Database,
    record: QueuedRecord,
) -> Result<Vec<QueuedRecord>, error::Error> {
    if is_control_command(&record.content) {
        return Ok(vec![record]);
    }

    let mut batch = Vec::new();
    for next in db.pending_messages_from(record.id)? {
        if !same_queue_sink(&record, &next) || is_control_command(&next.content) {
            break;
        }
        batch.push(next);
    }

    if batch.is_empty() {
        Ok(vec![record])
    } else {
        Ok(batch)
    }
}

fn same_queue_sink(a: &QueuedRecord, b: &QueuedRecord) -> bool {
    a.channel == b.channel && a.chat_id == b.chat_id && a.thread_id == b.thread_id
}

fn is_control_command(content: &str) -> bool {
    matches!(
        parse_slash_command(content),
        Some(("switch", _)) | Some(("rules", _))
    )
}

fn response_sink_for_record(
    record: &QueuedRecord,
    telegram_bot: Option<&Arc<TelegramBot>>,
) -> Option<ResponseSink> {
    match record.channel_kind()? {
        ChannelKind::Telegram => {
            let bot = telegram_bot?;
            Some(ResponseSink::Telegram {
                chat_id: record.chat_id,
                thread_id: record.thread_id,
                bot: Arc::clone(bot),
            })
        }
        ChannelKind::Cli => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_queued_record(
    records: Vec<QueuedRecord>,
    channel: ChannelKind,
    sink: ResponseSink,
    db: Arc<Database>,
    pending: Arc<PendingApprovals>,
    pending_cold: Arc<PendingColdResumes>,
    client: reqwest::Client,
    mcp: Option<Arc<crate::mcp::manager::McpManager>>,
    skills: Arc<Vec<crate::skill::Skill>>,
    chat_buffer: Arc<ChatBuffer>,
    runtime: Arc<RuntimeState>,
) {
    let Some(record) = records.first().cloned() else {
        return;
    };
    let queue_ids: Vec<i64> = records.iter().map(|r| r.id).collect();
    let chat_id = sink.chat_id();
    let thread_id = sink.thread_id();

    // helper: buffer a bot response so it appears in chat history context
    let buffer_bot_reply = |text: &str| {
        if text.is_empty() {
            return;
        }
        let bot_name = runtime.telegram_display_name();
        let label = if bot_name.is_empty() {
            "bot".to_string()
        } else {
            bot_name
        };
        // truncate long responses to avoid dominating the ring buffer
        let truncated: String = text.chars().take(500).collect();
        chat_buffer.push(
            chat_id,
            thread_id,
            BufferedMessage {
                user_name: label,
                user_id: None,
                text: truncated,
                images: vec![],
                received_at: std::time::Instant::now(),
            },
        );
    };

    if let Some(("switch", args)) = parse_slash_command(&record.content) {
        let msg = handle_switch_command(args, client.clone(), &db);
        buffer_bot_reply(&msg);
        send_response(
            sink,
            crate::message::OutboundMessage {
                content: msg,
                voice: None,
                attachments: vec![],
            },
        )
        .await;
        mark_queue_done_many(&db, &queue_ids);
        return;
    }

    if let Some(("rules", args)) = parse_slash_command(&record.content) {
        let msg = handle_rules_command(args, &db);
        buffer_bot_reply(&msg);
        send_response(
            sink,
            crate::message::OutboundMessage {
                content: msg,
                voice: None,
                attachments: vec![],
            },
        )
        .await;
        mark_queue_done_many(&db, &queue_ids);
        return;
    }

    let provider = match provider_for_session(client.clone(), &db) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(%e, "provider init failed");
            send_error(sink, &format!("error: {e}")).await;
            mark_queue_done_many(&db, &queue_ids);
            return;
        }
    };

    let (approver, cold_prompter) = match &sink {
        ResponseSink::Telegram {
            chat_id,
            thread_id,
            bot,
        } => {
            let approver = AnyApprover::Telegram(TelegramApprover::new(
                Arc::clone(bot),
                *chat_id,
                *thread_id,
                Arc::clone(&pending),
                Arc::clone(&db),
            ));
            let cold = ColdResumePrompter::new(
                Arc::clone(bot),
                *chat_id,
                *thread_id,
                Arc::clone(&pending_cold),
            );
            (approver, Some(cold))
        }
    };

    let mut agent = Agent::new(provider, approver, Arc::clone(&db), client.clone())
        .with_skills(Arc::clone(&skills))
        .with_chat_buffer(Arc::clone(&chat_buffer))
        .with_runtime(Arc::clone(&runtime))
        .with_continuation_target(crate::tool::ContinuationTarget {
            channel,
            chat_id,
            thread_id,
        });
    if let Some(cold) = cold_prompter {
        agent = agent.with_cold_resume_prompter(cold);
    }
    if let Some(ref mcp) = mcp {
        agent = agent.with_mcp(Arc::clone(mcp));
    }
    let content = records
        .iter()
        .map(|r| r.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let images = records
        .iter()
        .flat_map(|r| r.images.iter().cloned())
        .collect();
    let inbound = InboundMessage {
        channel,
        content,
        images,
    };

    // send "typing" indicator while the agent processes.
    // telegram's indicator expires after ~5s, so we re-send every 4s.
    let typing_bot = match &sink {
        ResponseSink::Telegram { chat_id, bot, .. } => Some((*chat_id, Arc::clone(bot))),
    };
    let typing_handle = typing_bot.map(|(chat_id, bot)| {
        tokio::spawn(async move {
            loop {
                let _ = bot.send_chat_action(chat_id, "typing").await;
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            }
        })
    });
    let rejection_bot = match &sink {
        ResponseSink::Telegram { bot, .. } => Arc::clone(bot),
    };

    runtime.begin_turn();
    let result = agent.process(&inbound).await;
    let rejected_steers = runtime.close_turn();

    // stop typing indicator
    if let Some(handle) = typing_handle {
        handle.abort();
    }

    match result {
        Ok(Some(outbound)) => {
            buffer_bot_reply(&outbound.content);
            send_response(sink, outbound).await;
        }
        Ok(None) => tracing::debug!("agent completed silently"),
        Err(error::Error::RateLimited(ref msg)) => {
            tracing::warn!(%msg, "rate limited");
            send_error(sink, &format!("rate limited: {msg}")).await;
        }
        Err(error::Error::BudgetExhausted(ref msg)) => {
            tracing::error!(%msg, "budget exhausted, fallback also failed");
            let help = format!(
                "budget exhausted: {msg}\n\n\
                 automatic fallback failed. use `/switch <provider>` to \
                 switch manually (e.g. `/switch gemini`, `/switch openai`, \
                 or `/switch anthropic`)."
            );
            send_error(sink, &help).await;
        }
        Err(e) => {
            tracing::error!(%e, "agent processing failed");
            send_error(sink, &format!("error: {e}")).await;
        }
    }

    send_steer_rejections(&rejection_bot, rejected_steers).await;
    mark_queue_done_many(&db, &queue_ids);
}

async fn send_steer_rejections(bot: &Arc<TelegramBot>, steers: Vec<PendingSteer>) {
    for steer in steers {
        if let Err(e) = bot
            .send_message(
                steer.origin.chat_id,
                STEER_REJECTION,
                steer.origin.thread_id,
            )
            .await
        {
            tracing::warn!(
                %e,
                chat_id = steer.origin.chat_id,
                thread_id = ?steer.origin.thread_id,
                "failed to send steer rejection"
            );
        }
    }
}

fn mark_queue_done_many(db: &Database, queue_ids: &[i64]) {
    for queue_id in queue_ids {
        if let Err(e) = db.mark_message_done(*queue_id) {
            tracing::warn!(%e, queue_id, "failed to mark queue row done");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn telegram_producer(
    bot: Arc<TelegramBot>,
    tx: crate::queue::WakeSender,
    pending: Arc<PendingApprovals>,
    pending_cold: Arc<PendingColdResumes>,
    db: Arc<Database>,
    chat_buffer: Arc<ChatBuffer>,
    runtime: Arc<RuntimeState>,
    bot_identity: BotIdentity,
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
                    // try cold-resume routing first (cheap prefix check). if it
                    // wasn't a cold-resume callback, fall through to approval.
                    let handled = crate::cold_resume::handle_callback(
                        &pending_cold,
                        &bot,
                        &callback.id,
                        data,
                        chat_id,
                        message_id,
                    )
                    .await;
                    if !handled {
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
                }
                continue;
            }

            // handle messages (text and/or photo)
            let Some(msg) = update.message else {
                continue;
            };

            let has_text = msg.text.is_some();
            let has_photo = msg.photo.is_some();

            // skip messages with neither text nor photo
            if !has_text && !has_photo {
                continue;
            }

            // extract text: prefer msg.text, fall back to caption for photo messages
            let text = if let Some(ref t) = msg.text {
                t.clone()
            } else {
                msg.caption.clone().unwrap_or_default()
            };

            // download photo if present
            let mut images = Vec::new();
            if let Some(ref photos) = msg.photo {
                // telegram sends multiple sizes; pick the largest (last in array)
                if let Some(largest) = photos.last() {
                    match bot.get_file(&largest.file_id).await {
                        Ok(file_info) => {
                            if let Some(ref file_path) = file_info.file_path {
                                match bot.download_file(file_path).await {
                                    Ok(bytes) => {
                                        use base64::Engine;
                                        let data = base64::engine::general_purpose::STANDARD
                                            .encode(&bytes);
                                        // telegram compresses photos to jpeg
                                        images.push(ImageSource {
                                            source_type: "base64".into(),
                                            media_type: "image/jpeg".into(),
                                            data,
                                        });
                                    }
                                    Err(e) => {
                                        tracing::error!(%e, "failed to download telegram photo");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(%e, "failed to get telegram file info");
                        }
                    }
                }
            }

            let chat_id = msg.chat.id;
            let user_id = msg.from.as_ref().map(|u| u.id);
            let username = msg.from.as_ref().and_then(|u| u.username.clone());
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
                    if let Some(id) = user_id
                        && let Err(err) =
                            db.record_unauthorized_dm_attempt(id, username.as_deref(), chat_id)
                    {
                        tracing::warn!(
                            ?err,
                            user_id = id,
                            "failed to record unauthorized DM attempt"
                        );
                    }
                    tracing::warn!(
                        ?user_id,
                        username = ?username,
                        chat_id,
                        "rejecting DM from non-whitelisted user"
                    );
                    let _ = bot
                        .send_message(chat_id, "DM not available for this user.", None)
                        .await;
                    continue;
                }
            }

            // track channel activity
            let _ = db.upsert_channel(chat_id, chat_type, msg.chat.title.as_deref());

            if let Some(steer) = parse_steer_command(&text, Some(&bot_identity.username)) {
                let steer = steer.trim();
                if steer.is_empty() {
                    continue;
                }
                let origin = SteerOrigin {
                    chat_id,
                    thread_id: msg.message_thread_id,
                };
                if runtime.try_push_steer(steer, origin) {
                    tracing::info!(
                        chat_id,
                        thread_id = ?msg.message_thread_id,
                        "accepted telegram steer for active turn"
                    );
                    continue;
                }

                if let Err(e) = bot
                    .send_message(chat_id, STEER_REJECTION, msg.message_thread_id)
                    .await
                {
                    tracing::warn!(
                        %e,
                        chat_id,
                        thread_id = ?msg.message_thread_id,
                        "failed to send inactive steer rejection"
                    );
                }
                continue;
            }

            // buffer message for context (all authorized messages, not just those that trigger the agent)
            let buffer_text = if text.is_empty() {
                "[photo]".to_string()
            } else {
                text.clone()
            };
            let user_name = username.clone().unwrap_or_else(|| {
                user_id
                    .map(|id| format!("user_{id}"))
                    .unwrap_or_else(|| "unknown".into())
            });
            let thread_id = msg.message_thread_id;
            chat_buffer.push(
                chat_id,
                thread_id,
                BufferedMessage {
                    user_name,
                    user_id,
                    text: buffer_text,
                    images: images.clone(),
                    received_at: std::time::Instant::now(),
                },
            );

            // build sender identity prefix
            let sender_name = username
                .as_deref()
                .unwrap_or_else(|| user_id.map(|_| "unknown").unwrap_or("unknown"));
            let chat_title = msg.chat.title.as_deref();

            // mention-only filter for group chats
            let content = if is_group {
                let entities = msg.entities.as_deref().unwrap_or_default();
                let mentioned = bot_identity.is_mentioned_in_entities(&text, entities);
                let replied_to = bot_identity.is_reply_to_bot(msg.reply_to_message.as_deref());
                let display_name = runtime.telegram_display_name();
                let named = bot_identity.is_named_in_text(&text, &display_name);

                tracing::info!(
                    chat_id,
                    thread_id,
                    display_name = %display_name,
                    mentioned,
                    replied_to,
                    named,
                    text = %text,
                    "evaluated group message trigger"
                );

                // photos sent as replies to the bot count as addressed
                let photo_reply = has_photo && replied_to;
                if !mentioned && !replied_to && !named && !photo_reply {
                    tracing::debug!(
                        chat_id,
                        thread_id,
                        text = %text,
                        "group message not addressed to bot, skipping"
                    );
                    continue;
                }

                // build identity + context prefix
                let cleaned = bot_identity.strip_mention(&text);
                let group_name = chat_title.unwrap_or("group");
                let from_line = match thread_id {
                    Some(_tid) => format!("[from: {sender_name} in #{group_name} (topic)]"),
                    None => format!("[from: {sender_name} in #{group_name}]"),
                };
                let context_header = match thread_id {
                    Some(_) => format!("[recent messages in #{group_name} > topic]"),
                    None => format!("[recent messages in #{group_name}]"),
                };
                match chat_buffer.drain_context(chat_id, thread_id) {
                    Some((ctx, buffer_images)) => {
                        // include images from recent buffer messages so the agent
                        // can see photos that were sent before it was mentioned
                        images.extend(buffer_images);
                        format!("{context_header}\n{ctx}\n\n{from_line}\n{cleaned}")
                    }
                    None => format!("{from_line}\n{cleaned}"),
                }
            } else {
                // DMs: include sender identity
                format!("[from: {sender_name} (DM)]\n{text}")
            };

            match db.enqueue_message(
                ChannelKind::Telegram,
                chat_id,
                msg.message_thread_id,
                &content,
                &images,
            ) {
                Ok(queue_id) => tracing::info!(
                    queue_id,
                    chat_id,
                    thread_id = ?msg.message_thread_id,
                    "queued telegram message"
                ),
                Err(e) => {
                    tracing::error!(%e, chat_id, "failed to persist telegram message");
                    continue;
                }
            }

            if tx.send(()).await.is_err() {
                tracing::error!("agent loop stopped, exiting telegram producer");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::MessageEntity;

    fn bot() -> BotIdentity {
        BotIdentity {
            id: 123,
            username: "ren_bot".into(),
        }
    }

    #[test]
    fn test_queue_batch_combines_contiguous_same_sink_normals() {
        let db = Database::open_in_memory().unwrap();
        let first = db
            .enqueue_message(ChannelKind::Telegram, 1, Some(7), "first", &[])
            .unwrap();
        let second = db
            .enqueue_message(ChannelKind::Telegram, 1, Some(7), "second", &[])
            .unwrap();

        let record = db.next_pending_message().unwrap().unwrap();
        let batch = queue_batch_for_record(&db, record).unwrap();
        let ids: Vec<i64> = batch.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![first, second]);
    }

    #[test]
    fn test_queue_batch_stops_at_different_thread() {
        let db = Database::open_in_memory().unwrap();
        let first = db
            .enqueue_message(ChannelKind::Telegram, 1, Some(7), "first", &[])
            .unwrap();
        db.enqueue_message(ChannelKind::Telegram, 1, Some(8), "second", &[])
            .unwrap();

        let record = db.next_pending_message().unwrap().unwrap();
        let batch = queue_batch_for_record(&db, record).unwrap();
        let ids: Vec<i64> = batch.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![first]);
    }

    #[test]
    fn test_queue_batch_stops_before_control_command() {
        let db = Database::open_in_memory().unwrap();
        let first = db
            .enqueue_message(ChannelKind::Telegram, 1, None, "first", &[])
            .unwrap();
        db.enqueue_message(ChannelKind::Telegram, 1, None, "/switch openai", &[])
            .unwrap();

        let record = db.next_pending_message().unwrap().unwrap();
        let batch = queue_batch_for_record(&db, record).unwrap();
        let ids: Vec<i64> = batch.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![first]);
    }

    #[test]
    fn test_is_mentioned_in_entities() {
        let b = bot();
        let entities = vec![MessageEntity {
            entity_type: "mention".into(),
            offset: 0,
            length: 8,
            user: None,
        }];
        assert!(b.is_mentioned_in_entities("@ren_bot hi", &entities));
        assert!(!b.is_mentioned_in_entities("@someone hi", &entities));

        let text_mention = vec![MessageEntity {
            entity_type: "text_mention".into(),
            offset: 0,
            length: 3,
            user: Some(crate::telegram::User {
                id: 123,
                username: None,
                is_bot: Some(true),
            }),
        }];
        assert!(b.is_mentioned_in_entities("ren", &text_mention));

        // wrong user id
        let wrong_user = vec![MessageEntity {
            entity_type: "text_mention".into(),
            offset: 0,
            length: 3,
            user: Some(crate::telegram::User {
                id: 999,
                username: None,
                is_bot: None,
            }),
        }];
        assert!(!b.is_mentioned_in_entities("ren", &wrong_user));

        assert!(!b.is_mentioned_in_entities("hello", &[]));
    }

    #[test]
    fn test_is_named_in_text() {
        let b = bot();
        // word boundary matches
        assert!(b.is_named_in_text("hey ren, what's up?", "ren"));
        assert!(b.is_named_in_text("Hey Ren, what's up?", "ren"));
        assert!(b.is_named_in_text("ren.", "ren"));
        assert!(b.is_named_in_text("ren!", "ren"));
        assert!(b.is_named_in_text("hi ren", "ren"));
        assert!(b.is_named_in_text("ren can you help", "ren"));
        assert!(b.is_named_in_text("that's it ren old boy", "ren"));
        // username match
        assert!(b.is_named_in_text("@ren_bot do something", "ren"));
        // should NOT match substrings inside other words
        assert!(!b.is_named_in_text("the current status", "ren"));
        assert!(!b.is_named_in_text("apparently not", "ren"));
        assert!(!b.is_named_in_text("different approach", "ren"));
        assert!(!b.is_named_in_text("rendering complete", "ren"));
        assert!(!b.is_named_in_text("hello everyone", "ren"));
    }

    #[test]
    fn test_strip_mention() {
        let b = bot();
        assert_eq!(b.strip_mention("@ren_bot what's up").as_ref(), "what's up");
        assert_eq!(
            b.strip_mention("hey @ren_bot do this").as_ref(),
            "hey  do this"
        );
        assert_eq!(
            b.strip_mention("no mention here").as_ref(),
            "no mention here"
        );
        assert_eq!(b.strip_mention("@REN_BOT caps").as_ref(), "caps");
    }

    #[test]
    fn test_is_reply_to_bot() {
        let b = bot();
        let bot_msg = crate::telegram::Message {
            message_id: 1,
            from: Some(crate::telegram::User {
                id: 123,
                username: None,
                is_bot: Some(true),
            }),
            chat: crate::telegram::Chat {
                id: 1,
                chat_type: None,
                title: None,
            },
            text: Some("hi".into()),
            photo: None,
            caption: None,
            reply_to_message: None,
            entities: None,
            message_thread_id: None,
        };
        assert!(b.is_reply_to_bot(Some(&bot_msg)));

        let other_msg = crate::telegram::Message {
            message_id: 2,
            from: Some(crate::telegram::User {
                id: 999,
                username: None,
                is_bot: None,
            }),
            chat: crate::telegram::Chat {
                id: 1,
                chat_type: None,
                title: None,
            },
            text: Some("hi".into()),
            photo: None,
            caption: None,
            reply_to_message: None,
            entities: None,
            message_thread_id: None,
        };
        assert!(!b.is_reply_to_bot(Some(&other_msg)));
        assert!(!b.is_reply_to_bot(None));
    }

    #[test]
    fn test_startup_model_note_records_restart_silently() {
        let event = RuntimeEvent {
            source: "cli_restart".into(),
            reason: "ava restart".into(),
            occurred_at: "2026-05-08T12:00:00Z".into(),
        };

        let note = startup_model_note(Some(&event));

        assert!(note.contains("ava restarted"));
        assert!(note.contains("ava restart"));
        assert!(note.contains("no external restart notice was sent"));
    }

    #[test]
    fn test_startup_model_note_records_plain_start() {
        let note = startup_model_note(None);

        assert!(note.contains("ava started"));
        assert!(!note.contains("no external restart notice was sent"));
    }
}
