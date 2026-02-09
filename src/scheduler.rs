use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use croner::Cron;
use tokio::time::Instant;

use crate::db::Database;
use crate::message::ChannelKind;
use crate::queue::{MessageSender, QueuedMessage, ResponseSink};
use crate::telegram::TelegramBot;

/// default interval between task board nudges (30 minutes)
const DEFAULT_TASK_CHECK_INTERVAL_SECS: u64 = 1800;

fn task_check_interval() -> Duration {
    let secs = std::env::var("AVA_TASK_CHECK_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TASK_CHECK_INTERVAL_SECS);
    Duration::from_secs(secs)
}

pub async fn run(
    db: Arc<Database>,
    tx: MessageSender,
    bot: Arc<TelegramBot>,
    default_chat_id: i64,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    let task_check_interval = task_check_interval();
    // start with last_nudge far enough in the past to allow an immediate first check
    let mut last_task_nudge = Instant::now() - task_check_interval;

    loop {
        interval.tick().await;

        // --- cron schedules ---

        let due = match db.due_schedules() {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(%e, "failed to query due schedules");
                continue;
            }
        };

        for schedule in due {
            tracing::info!(
                schedule_id = schedule.id,
                description = %schedule.description,
                "firing schedule"
            );

            let queued = QueuedMessage {
                channel: ChannelKind::Telegram,
                content: schedule.prompt.clone(),
                sink: ResponseSink::Telegram {
                    chat_id: default_chat_id,
                    bot: Arc::clone(&bot),
                },
            };

            if tx.send(queued).await.is_err() {
                tracing::error!("agent loop stopped, exiting scheduler");
                return;
            }

            // advance the schedule
            let next = schedule.cron_expr.as_ref().and_then(|expr| {
                Cron::from_str(expr)
                    .ok()
                    .and_then(|c| c.find_next_occurrence(&Utc::now(), false).ok())
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            });

            if let Err(e) = db.advance_schedule(schedule.id, next.as_deref()) {
                tracing::error!(%e, schedule_id = schedule.id, "failed to advance schedule");
            }
        }

        // --- task board check ---

        if last_task_nudge.elapsed() < task_check_interval {
            continue;
        }

        let pending = match db.pending_task_titles() {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(%e, "failed to query pending tasks");
                continue;
            }
        };

        if pending.is_empty() {
            continue;
        }

        let count = pending.len();
        tracing::info!(count, "nudging agent about pending tasks");

        let queued = QueuedMessage {
            channel: ChannelKind::Telegram,
            content: format!(
                "you have {count} pending task{}. review your task list and make progress where possible.",
                if count == 1 { "" } else { "s" }
            ),
            sink: ResponseSink::Telegram {
                chat_id: default_chat_id,
                bot: Arc::clone(&bot),
            },
        };

        if tx.send(queued).await.is_err() {
            tracing::error!("agent loop stopped, exiting scheduler");
            return;
        }

        last_task_nudge = Instant::now();
    }
}
