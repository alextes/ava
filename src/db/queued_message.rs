use rusqlite::OptionalExtension;

use crate::error::Error;
use crate::message::{ChannelKind, ImageSource};

use super::Database;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedRecord {
    pub id: i64,
    pub channel: String,
    pub chat_id: i64,
    pub thread_id: Option<i64>,
    pub content: String,
    pub images: Vec<ImageSource>,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

impl QueuedRecord {
    pub fn channel_kind(&self) -> Option<ChannelKind> {
        match self.channel.as_str() {
            "telegram" => Some(ChannelKind::Telegram),
            "cli" => Some(ChannelKind::Cli),
            _ => None,
        }
    }
}

impl Database {
    pub fn enqueue_message(
        &self,
        channel: ChannelKind,
        chat_id: i64,
        thread_id: Option<i64>,
        content: &str,
        images: &[ImageSource],
    ) -> Result<i64, Error> {
        let images_json = serde_json::to_string(images)
            .map_err(|e| Error::Provider(format!("failed to serialize queued images: {e}")))?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO queued_messages (channel, chat_id, thread_id, content, images_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![channel.as_str(), chat_id, thread_id, content, images_json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn next_pending_message(&self) -> Result<Option<QueuedRecord>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel, chat_id, thread_id, content, images_json, status,
                    created_at, started_at, finished_at, error
             FROM queued_messages
             WHERE status = 'pending'
             ORDER BY id ASC
             LIMIT 1",
        )?;

        let row = stmt
            .query_row([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            })
            .optional()?;

        let Some((
            id,
            channel,
            chat_id,
            thread_id,
            content,
            images_json,
            status,
            created_at,
            started_at,
            finished_at,
            error,
        )) = row
        else {
            return Ok(None);
        };

        let images = serde_json::from_str(&images_json)
            .map_err(|e| Error::Provider(format!("failed to deserialize queued images: {e}")))?;

        Ok(Some(QueuedRecord {
            id,
            channel,
            chat_id,
            thread_id,
            content,
            images,
            status,
            created_at,
            started_at,
            finished_at,
            error,
        }))
    }

    pub fn mark_message_processing(&self, id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE queued_messages
             SET status = 'processing', started_at = datetime('now'), finished_at = NULL, error = NULL
             WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn mark_message_done(&self, id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE queued_messages
             SET status = 'done', finished_at = datetime('now'), error = NULL
             WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn mark_message_failed(&self, id: i64, error: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE queued_messages
             SET status = 'failed', finished_at = datetime('now'), error = ?2
             WHERE id = ?1",
            rusqlite::params![id, error],
        )?;
        Ok(())
    }

    pub fn reset_processing_messages(&self) -> Result<u32, Error> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE queued_messages
             SET status = 'pending', started_at = NULL, error = NULL
             WHERE status = 'processing'",
            [],
        )?;
        Ok(count as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_fifo_and_done_rows_are_skipped() {
        let db = Database::open_in_memory().unwrap();
        let first = db
            .enqueue_message(ChannelKind::Telegram, 1, None, "first", &[])
            .unwrap();
        let second = db
            .enqueue_message(ChannelKind::Telegram, 2, None, "second", &[])
            .unwrap();

        let next = db.next_pending_message().unwrap().unwrap();
        assert_eq!(next.id, first);
        assert_eq!(next.content, "first");
        assert_eq!(next.channel_kind(), Some(ChannelKind::Telegram));

        db.mark_message_done(first).unwrap();

        let next = db.next_pending_message().unwrap().unwrap();
        assert_eq!(next.id, second);
        assert_eq!(next.content, "second");
    }

    #[test]
    fn test_queue_images_and_thread_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let images = vec![ImageSource {
            source_type: "base64".into(),
            media_type: "image/jpeg".into(),
            data: "abc123".into(),
        }];

        let id = db
            .enqueue_message(ChannelKind::Telegram, -100, Some(42), "with image", &images)
            .unwrap();
        let next = db.next_pending_message().unwrap().unwrap();

        assert_eq!(next.id, id);
        assert_eq!(next.thread_id, Some(42));
        assert_eq!(next.images, images);
        assert!(!next.created_at.is_empty());
        assert_eq!(next.started_at, None);
        assert_eq!(next.finished_at, None);
    }

    #[test]
    fn test_processing_reset_retries_rows() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .enqueue_message(ChannelKind::Telegram, 1, None, "retry me", &[])
            .unwrap();

        db.mark_message_processing(id).unwrap();
        assert_eq!(db.next_pending_message().unwrap(), None);

        assert_eq!(db.reset_processing_messages().unwrap(), 1);

        let next = db.next_pending_message().unwrap().unwrap();
        assert_eq!(next.id, id);
        assert_eq!(next.status, "pending");
        assert_eq!(next.started_at, None);
    }

    #[test]
    fn test_failed_rows_are_not_retried() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .enqueue_message(ChannelKind::Telegram, 1, None, "fail me", &[])
            .unwrap();

        db.mark_message_failed(id, "provider failed").unwrap();

        assert_eq!(db.next_pending_message().unwrap(), None);
    }
}
