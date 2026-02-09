use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use croner::Cron;

use crate::db::Database;
use crate::message::ChannelKind;
use crate::queue::{MessageSender, QueuedMessage, ResponseSink};
use crate::telegram::TelegramBot;

pub async fn run(
    db: Arc<Database>,
    tx: MessageSender,
    bot: Arc<TelegramBot>,
    default_chat_id: i64,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        interval.tick().await;

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
    }
}
