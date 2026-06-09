use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::error::Error;
use crate::message::{Message, MessageContent, Role};
use crate::provider::{ReasoningEffort, Usage};

use super::Database;

/// a message with its timestamp, for history display
#[derive(Debug, Clone, Serialize)]
pub struct HistoryMessage {
    pub id: i64,
    pub role: Role,
    pub content: Vec<MessageContent>,
    pub created_at: String,
    pub model_id: Option<String>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct MessageUsageRecord {
    pub model_id: Option<String>,
    pub usage: Usage,
}

impl Database {
    /// get the active session id, creating one if none exists
    pub fn active_session(&self) -> Result<i64, Error> {
        let conn = self.conn.lock().unwrap();
        let result: Option<i64> = conn
            .query_row(
                "SELECT id FROM sessions WHERE active = 1 LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = result {
            return Ok(id);
        }

        // no active session — create one
        conn.execute(
            "INSERT INTO sessions (active, title) VALUES (1, 'default')",
            [],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// load all messages for a session, oldest first
    /// load messages for the LLM context. if a compaction cursor is set,
    /// skips old messages and prepends the saved summary instead.
    pub fn load_messages(&self, session_id: i64) -> Result<Vec<Message>, Error> {
        let conn = self.conn.lock().unwrap();

        // check for compaction cursor + summary
        let (cursor, summary): (Option<i64>, Option<String>) = conn.query_row(
            "SELECT compacted_before_id, summary FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let mut stmt = conn.prepare(
            "SELECT role, content FROM messages
             WHERE session_id = ?1 AND id > ?2
             ORDER BY created_at ASC, id ASC",
        )?;

        let after_id = cursor.unwrap_or(0);
        let messages = stmt
            .query_map(rusqlite::params![session_id, after_id], |row| {
                let role_str: String = row.get(0)?;
                let content_json: String = row.get(1)?;
                Ok((role_str, content_json))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(messages.len() + 1);

        // prepend summary if compaction has occurred
        if let (Some(_), Some(summary_text)) = (cursor, &summary) {
            result.push(Message::user(format!(
                "[conversation summary]\n{summary_text}"
            )));
        }

        for (role_str, content_json) in messages {
            let role = match role_str.as_str() {
                // system messages are sent to the API as user role
                "user" | "system" => Role::User,
                "assistant" => Role::Assistant,
                _ => continue, // skip unknown roles
            };
            let content: Vec<MessageContent> = serde_json::from_str(&content_json)
                .map_err(|e| Error::Provider(format!("failed to deserialize message: {e}")))?;
            if content
                .iter()
                .any(|c| matches!(c, MessageContent::Text { text } if text.is_empty()))
            {
                tracing::warn!("skipping message with empty text block (role={role_str})");
                continue;
            }
            result.push(Message { role, content });
        }

        Ok(result)
    }

    /// load the most recent messages for a session, oldest first
    pub fn load_recent_messages(
        &self,
        session_id: i64,
        limit: u32,
    ) -> Result<Vec<HistoryMessage>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, role, content, created_at, model_id, input_tokens,
                    output_tokens, reasoning_tokens, cache_creation_tokens, cache_read_tokens
             FROM messages
             WHERE session_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![session_id, limit], |row| {
                let id: i64 = row.get(0)?;
                let role_str: String = row.get(1)?;
                let content_json: String = row.get(2)?;
                let created_at: String = row.get(3)?;
                let model_id: Option<String> = row.get(4)?;
                let input_tokens: Option<u32> = row.get(5)?;
                let output_tokens: Option<u32> = row.get(6)?;
                let reasoning_tokens: Option<u32> = row.get(7)?;
                let cache_creation_tokens: Option<u32> = row.get(8)?;
                let cache_read_tokens: Option<u32> = row.get(9)?;
                Ok((
                    id,
                    role_str,
                    content_json,
                    created_at,
                    model_id,
                    input_tokens,
                    output_tokens,
                    reasoning_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(rows.len());
        for (
            id,
            role_str,
            content_json,
            created_at,
            model_id,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        ) in rows
        {
            let role = match role_str.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "system" => Role::System,
                _ => continue,
            };
            let content: Vec<MessageContent> = serde_json::from_str(&content_json)
                .map_err(|e| Error::Provider(format!("failed to deserialize message: {e}")))?;
            result.push(HistoryMessage {
                id,
                role,
                content,
                created_at,
                model_id,
                input_tokens,
                output_tokens,
                reasoning_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            });
        }

        // reverse so oldest is first
        result.reverse();
        Ok(result)
    }

    /// load messages for a session with id greater than `after_id`, oldest first.
    /// used by `history --follow` to poll for new messages.
    pub fn load_messages_after(
        &self,
        session_id: i64,
        after_id: i64,
    ) -> Result<Vec<HistoryMessage>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, role, content, created_at, model_id, input_tokens,
                    output_tokens, reasoning_tokens, cache_creation_tokens, cache_read_tokens
             FROM messages
             WHERE session_id = ?1 AND id > ?2
             ORDER BY id ASC",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![session_id, after_id], |row| {
                let id: i64 = row.get(0)?;
                let role_str: String = row.get(1)?;
                let content_json: String = row.get(2)?;
                let created_at: String = row.get(3)?;
                let model_id: Option<String> = row.get(4)?;
                let input_tokens: Option<u32> = row.get(5)?;
                let output_tokens: Option<u32> = row.get(6)?;
                let reasoning_tokens: Option<u32> = row.get(7)?;
                let cache_creation_tokens: Option<u32> = row.get(8)?;
                let cache_read_tokens: Option<u32> = row.get(9)?;
                Ok((
                    id,
                    role_str,
                    content_json,
                    created_at,
                    model_id,
                    input_tokens,
                    output_tokens,
                    reasoning_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(rows.len());
        for (
            id,
            role_str,
            content_json,
            created_at,
            model_id,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        ) in rows
        {
            let role = match role_str.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "system" => Role::System,
                _ => continue,
            };
            let content: Vec<MessageContent> = serde_json::from_str(&content_json)
                .map_err(|e| Error::Provider(format!("failed to deserialize message: {e}")))?;
            result.push(HistoryMessage {
                id,
                role,
                content,
                created_at,
                model_id,
                input_tokens,
                output_tokens,
                reasoning_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            });
        }

        Ok(result)
    }

    /// append a message to the session, returning the inserted row id.
    pub fn append_message(
        &self,
        session_id: i64,
        role: &str,
        content: &[MessageContent],
        channel: Option<&str>,
    ) -> Result<i64, Error> {
        // guard: never persist empty text blocks — the API rejects them
        if content
            .iter()
            .any(|c| matches!(c, MessageContent::Text { text } if text.is_empty()))
        {
            tracing::warn!(role, "refusing to persist message with empty text block");
            return Ok(0);
        }

        let content_json = serde_json::to_string(content)
            .map_err(|e| Error::Provider(format!("failed to serialize message: {e}")))?;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content, channel)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, role, content_json, channel],
        )?;
        let message_id = conn.last_insert_rowid();

        // update session timestamp
        conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            [session_id],
        )?;

        Ok(message_id)
    }

    /// attach provider token usage to an already-persisted message.
    pub fn set_message_usage(
        &self,
        message_id: i64,
        usage: &Usage,
        model_id: &str,
    ) -> Result<(), Error> {
        if message_id <= 0 {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages
             SET input_tokens = ?1, output_tokens = ?2, reasoning_tokens = ?3,
                 model_id = ?4, cache_creation_tokens = ?5, cache_read_tokens = ?6
             WHERE id = ?7",
            rusqlite::params![
                usage.input_tokens,
                usage.output_tokens,
                usage.reasoning_tokens,
                model_id,
                usage.cache_creation_tokens,
                usage.cache_read_tokens,
                message_id
            ],
        )?;
        Ok(())
    }

    /// load persisted provider usage records for session-level cost estimates.
    pub fn session_usage_records(&self, session_id: i64) -> Result<Vec<MessageUsageRecord>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT model_id, input_tokens, output_tokens, reasoning_tokens,
                    cache_creation_tokens, cache_read_tokens
             FROM messages
             WHERE session_id = ?1
               AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)
             ORDER BY id ASC",
        )?;

        let rows = stmt
            .query_map([session_id], |row| {
                let model_id: Option<String> = row.get(0)?;
                let input_tokens: Option<u32> = row.get(1)?;
                let output_tokens: Option<u32> = row.get(2)?;
                let reasoning_tokens: Option<u32> = row.get(3)?;
                let cache_creation_tokens: Option<u32> = row.get(4)?;
                let cache_read_tokens: Option<u32> = row.get(5)?;
                Ok(MessageUsageRecord {
                    model_id,
                    usage: Usage {
                        input_tokens: input_tokens.unwrap_or(0),
                        output_tokens: output_tokens.unwrap_or(0),
                        reasoning_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// load all messages for a session with their DB row IDs, oldest first.
    /// used by `doctor` to identify orphaned tool_use blocks by position.
    pub fn load_messages_with_ids(&self, session_id: i64) -> Result<Vec<(i64, Message)>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, role, content FROM messages
             WHERE session_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;

        let rows = stmt
            .query_map([session_id], |row| {
                let id: i64 = row.get(0)?;
                let role_str: String = row.get(1)?;
                let content_json: String = row.get(2)?;
                Ok((id, role_str, content_json))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(rows.len());
        for (id, role_str, content_json) in rows {
            let role = match role_str.as_str() {
                "user" | "system" => Role::User,
                "assistant" => Role::Assistant,
                _ => continue,
            };
            let content: Vec<MessageContent> = serde_json::from_str(&content_json)
                .map_err(|e| Error::Provider(format!("failed to deserialize message: {e}")))?;
            result.push((id, Message { role, content }));
        }

        Ok(result)
    }

    /// insert a message immediately after a specific message ID.
    ///
    /// since `load_messages` uses `ORDER BY created_at ASC, id ASC`, we bump
    /// the `created_at` of all later messages in this session by 1 second,
    /// then insert the new row with the original `created_at`. this guarantees
    /// correct ordering regardless of id or timestamp collisions.
    pub fn insert_message_after(
        &self,
        session_id: i64,
        after_message_id: i64,
        role: &str,
        content: &[MessageContent],
    ) -> Result<(), Error> {
        let content_json = serde_json::to_string(content)
            .map_err(|e| Error::Provider(format!("failed to serialize message: {e}")))?;

        let conn = self.conn.lock().unwrap();

        let created_at: String = conn.query_row(
            "SELECT created_at FROM messages WHERE id = ?1 AND session_id = ?2",
            rusqlite::params![after_message_id, session_id],
            |row| row.get(0),
        )?;

        // bump all messages after the target so the new row sorts between them
        conn.execute(
            "UPDATE messages
             SET created_at = datetime(created_at, '+1 second')
             WHERE session_id = ?1 AND id > ?2",
            rusqlite::params![session_id, after_message_id],
        )?;

        conn.execute(
            "INSERT INTO messages (session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, role, content_json, created_at],
        )?;

        conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            [session_id],
        )?;

        Ok(())
    }

    /// count messages in a session
    pub fn session_message_count(&self, session_id: i64) -> Result<u32, Error> {
        let conn = self.conn.lock().unwrap();
        let count: u32 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// persist the selected model for a session
    #[allow(dead_code)]
    pub fn set_session_model(&self, session_id: i64, model_id: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET model = ?1 WHERE id = ?2",
            rusqlite::params![model_id, session_id],
        )?;
        Ok(())
    }

    /// persist the selected model and effective reasoning effort for a session.
    pub fn set_session_model_reasoning(
        &self,
        session_id: i64,
        model_id: &str,
        reasoning_effort: ReasoningEffort,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET model = ?1, reasoning_effort = ?2 WHERE id = ?3",
            rusqlite::params![model_id, reasoning_effort.as_str(), session_id],
        )?;
        conn.execute(
            "INSERT INTO model_reasoning_preferences (session_id, model, reasoning_effort, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(session_id, model) DO UPDATE SET
                reasoning_effort = excluded.reasoning_effort,
                updated_at = excluded.updated_at",
            rusqlite::params![session_id, model_id, reasoning_effort.as_str()],
        )?;
        Ok(())
    }

    /// clear the persisted model for a session (e.g. when it becomes invalid)
    pub fn clear_session_model(&self, session_id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET model = NULL, reasoning_effort = NULL WHERE id = ?1",
            [session_id],
        )?;
        Ok(())
    }

    /// persist context usage snapshot for a session
    pub fn set_session_usage(
        &self,
        session_id: i64,
        input_tokens: u32,
        context_window: u32,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET last_input_tokens = ?1, last_context_window = ?2 WHERE id = ?3",
            rusqlite::params![input_tokens, context_window, session_id],
        )?;
        Ok(())
    }

    /// record when a provider completion finished, so the cold-resume detector
    /// can tell whether the prompt cache has expired since.
    pub fn set_session_last_completion_at(
        &self,
        session_id: i64,
        epoch_secs: i64,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET last_completion_at = ?1 WHERE id = ?2",
            rusqlite::params![epoch_secs, session_id],
        )?;
        Ok(())
    }

    /// load the recorded last completion timestamp (unix epoch seconds), if any.
    pub fn session_last_completion_at(&self, session_id: i64) -> Result<Option<i64>, Error> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT last_completion_at FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(result)
    }

    /// reset session state for a "clear context" action: move the compaction
    /// cursor to past the latest persisted message, drop any saved summary,
    /// and clear context-usage and completion timestamps. messages remain in
    /// the db for history purposes but are skipped on subsequent loads.
    ///
    /// returns the number of messages that were elided.
    #[allow(dead_code)]
    pub fn clear_session_context(&self, session_id: i64) -> Result<u32, Error> {
        self.clear_session_context_before(session_id, None)
    }

    /// reset session state while preserving a newly received message.
    ///
    /// when `before_message_id` is set, only messages older than that row are
    /// elided, so the turn that triggered the cold-resume prompt remains in
    /// the fresh context after the user chooses "clear".
    pub fn clear_session_context_before(
        &self,
        session_id: i64,
        before_message_id: Option<i64>,
    ) -> Result<u32, Error> {
        let conn = self.conn.lock().unwrap();

        // find the cursor target. with no preserved message this is the latest
        // message. with a preserved message this is the latest older message.
        let max_id: Option<i64> = match before_message_id {
            Some(before_id) => conn
                .query_row(
                    "SELECT MAX(id) FROM messages WHERE session_id = ?1 AND id < ?2",
                    rusqlite::params![session_id, before_id],
                    |row| row.get(0),
                )
                .ok()
                .flatten(),
            None => conn
                .query_row(
                    "SELECT MAX(id) FROM messages WHERE session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .ok()
                .flatten(),
        };

        let prior_cursor: Option<i64> = conn
            .query_row(
                "SELECT compacted_before_id FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let cursor = max_id.unwrap_or(0);
        conn.execute(
            "UPDATE sessions SET compacted_before_id = ?1, summary = NULL, \
             last_input_tokens = NULL, last_context_window = NULL, \
             last_completion_at = NULL WHERE id = ?2",
            rusqlite::params![cursor, session_id],
        )?;

        let cleared = (cursor - prior_cursor.unwrap_or(0)).max(0) as u32;
        Ok(cleared)
    }

    /// load the last context usage snapshot for a session
    pub fn session_usage(&self, session_id: i64) -> Result<Option<(u32, u32)>, Error> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT last_input_tokens, last_context_window FROM sessions WHERE id = ?1",
            [session_id],
            |row| {
                let input: Option<u32> = row.get(0)?;
                let window: Option<u32> = row.get(1)?;
                Ok(input.zip(window))
            },
        )?;
        Ok(result)
    }

    /// load the persisted model for a session
    #[allow(dead_code)]
    pub fn session_model(&self, session_id: i64) -> Result<Option<String>, Error> {
        let conn = self.conn.lock().unwrap();
        let model: Option<String> = conn
            .query_row(
                "SELECT model FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(Error::from)?;
        Ok(model)
    }

    /// load the persisted model and active reasoning effort for a session.
    pub fn session_model_reasoning(
        &self,
        session_id: i64,
    ) -> Result<Option<(String, Option<ReasoningEffort>)>, Error> {
        let conn = self.conn.lock().unwrap();
        let (model, effort): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT model, reasoning_effort FROM sessions WHERE id = ?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Error::from)?;

        Ok(model.map(|model| {
            (
                model,
                effort.as_deref().and_then(ReasoningEffort::from_persisted),
            )
        }))
    }

    /// load the remembered effective reasoning effort for a model in this session.
    pub fn model_reasoning_preference(
        &self,
        session_id: i64,
        model_id: &str,
    ) -> Result<Option<ReasoningEffort>, Error> {
        let conn = self.conn.lock().unwrap();
        let effort: Option<String> = conn
            .query_row(
                "SELECT reasoning_effort FROM model_reasoning_preferences
                 WHERE session_id = ?1 AND model = ?2",
                rusqlite::params![session_id, model_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(effort.as_deref().and_then(ReasoningEffort::from_persisted))
    }

    /// get the creation timestamp for a session (e.g. "2026-04-12 14:30:00")
    pub fn session_created_at(&self, session_id: i64) -> Result<String, Error> {
        let conn = self.conn.lock().unwrap();
        let created_at: String = conn.query_row(
            "SELECT created_at FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        Ok(created_at)
    }

    pub fn get_session_summary(&self, session_id: i64) -> Result<Option<String>, Error> {
        let conn = self.conn.lock().unwrap();
        let summary: Option<String> = conn.query_row(
            "SELECT summary FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        Ok(summary)
    }

    pub fn set_session_summary(&self, session_id: i64, summary: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET summary = ?1 WHERE id = ?2",
            rusqlite::params![summary, session_id],
        )?;
        Ok(())
    }

    /// set the compaction cursor — messages with id <= this are considered summarized.
    pub fn set_compaction_cursor(&self, session_id: i64, before_id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET compacted_before_id = ?1 WHERE id = ?2",
            rusqlite::params![before_id, session_id],
        )?;
        Ok(())
    }

    /// get the current compaction cursor for a session.
    pub fn get_compaction_cursor(&self, session_id: i64) -> Result<Option<i64>, Error> {
        let conn = self.conn.lock().unwrap();
        let cursor: Option<i64> = conn.query_row(
            "SELECT compacted_before_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        Ok(cursor)
    }

    /// get the id of the nth message (1-indexed) after a given cursor.
    /// used to compute the new compaction cursor after summarization.
    pub fn nth_message_id(
        &self,
        session_id: i64,
        after_id: i64,
        n: usize,
    ) -> Result<Option<i64>, Error> {
        if n == 0 {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM messages
                 WHERE session_id = ?1 AND id > ?2
                 ORDER BY created_at ASC, id ASC
                 LIMIT 1 OFFSET ?3",
                rusqlite::params![session_id, after_id, (n - 1) as i64],
                |row| row.get(0),
            )
            .ok();
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_session_returns_seeded_session() {
        let db = Database::open_in_memory().unwrap();
        let id = db.active_session().unwrap();
        assert!(id > 0);
        // calling again returns the same id
        assert_eq!(db.active_session().unwrap(), id);
    }

    #[test]
    fn test_append_and_load_messages() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        let user_content = vec![MessageContent::text("hello")];
        db.append_message(sid, "user", &user_content, Some("cli"))
            .unwrap();

        let asst_content = vec![MessageContent::text("hi there")];
        db.append_message(sid, "assistant", &asst_content, None)
            .unwrap();

        let messages = db.load_messages(sid).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);

        // verify content round-trips
        match &messages[0].content[0] {
            MessageContent::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text content"),
        }
    }

    #[test]
    fn test_load_messages_preserves_tool_blocks() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        let content = vec![
            MessageContent::text("thinking..."),
            MessageContent::tool_use("call_1", "web_search", serde_json::json!({"query": "rust"})),
        ];
        db.append_message(sid, "assistant", &content, None).unwrap();

        let messages = db.load_messages(sid).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.len(), 2);

        match &messages[0].content[1] {
            MessageContent::ToolUse { id, name, .. } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "web_search");
            }
            _ => panic!("expected tool_use content"),
        }
    }

    #[test]
    fn test_load_messages_empty_session() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();
        let messages = db.load_messages(sid).unwrap();
        assert!(messages.is_empty());
    }

    #[test]
    fn test_session_message_count() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        assert_eq!(db.session_message_count(sid).unwrap(), 0);

        db.append_message(sid, "user", &[MessageContent::text("hi")], Some("cli"))
            .unwrap();
        db.append_message(sid, "assistant", &[MessageContent::text("hey")], None)
            .unwrap();

        assert_eq!(db.session_message_count(sid).unwrap(), 2);
    }

    #[test]
    fn test_set_and_get_session_model() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        db.set_session_model(sid, "anthropic/claude-sonnet-4-6")
            .unwrap();
        assert_eq!(
            db.session_model(sid).unwrap().as_deref(),
            Some("anthropic/claude-sonnet-4-6")
        );

        // update to a different model
        db.set_session_model(sid, "openai/gpt-5.4").unwrap();
        assert_eq!(
            db.session_model(sid).unwrap().as_deref(),
            Some("openai/gpt-5.4")
        );
    }

    #[test]
    fn test_set_and_get_session_model_reasoning() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        db.set_session_model_reasoning(
            sid,
            "openrouter/deepseek/deepseek-v4-pro",
            ReasoningEffort::High,
        )
        .unwrap();

        let (model, effort) = db.session_model_reasoning(sid).unwrap().unwrap();
        assert_eq!(model, "openrouter/deepseek/deepseek-v4-pro");
        assert_eq!(effort, Some(ReasoningEffort::High));
        assert_eq!(
            db.model_reasoning_preference(sid, "openrouter/deepseek/deepseek-v4-pro")
                .unwrap(),
            Some(ReasoningEffort::High)
        );

        db.set_session_model_reasoning(
            sid,
            "openrouter/deepseek/deepseek-v4-pro",
            ReasoningEffort::XHigh,
        )
        .unwrap();
        assert_eq!(
            db.model_reasoning_preference(sid, "openrouter/deepseek/deepseek-v4-pro")
                .unwrap(),
            Some(ReasoningEffort::XHigh)
        );
    }

    #[test]
    fn test_session_model_default_is_none() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();
        assert_eq!(db.session_model(sid).unwrap(), None);
    }

    #[test]
    fn test_insert_message_after() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        // seed: user, assistant(tool_use), user("next message")
        db.append_message(sid, "user", &[MessageContent::text("hello")], Some("cli"))
            .unwrap();
        db.append_message(
            sid,
            "assistant",
            &[
                MessageContent::text("let me run that"),
                MessageContent::tool_use(
                    "call_1",
                    "exec",
                    serde_json::json!({"command": "echo hi"}),
                ),
            ],
            None,
        )
        .unwrap();
        db.append_message(
            sid,
            "user",
            &[MessageContent::text("next message")],
            Some("cli"),
        )
        .unwrap();

        // find the assistant message ID
        let msgs_with_ids = db.load_messages_with_ids(sid).unwrap();
        assert_eq!(msgs_with_ids.len(), 3);
        let assistant_id = msgs_with_ids[1].0;

        // insert a synthetic tool_result after the assistant message
        let synthetic = vec![MessageContent::tool_result("call_1", "interrupted")];
        db.insert_message_after(sid, assistant_id, "user", &synthetic)
            .unwrap();

        // reload and verify ordering
        let msgs = db.load_messages(sid).unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, Role::User); // "hello"
        assert_eq!(msgs[1].role, Role::Assistant); // tool_use
        assert_eq!(msgs[2].role, Role::User); // synthetic tool_result (inserted)
        assert_eq!(msgs[3].role, Role::User); // "next message"

        // verify the inserted message content
        assert!(matches!(
            &msgs[2].content[0],
            MessageContent::ToolResult { tool_use_id, .. } if tool_use_id == "call_1"
        ));
    }

    #[test]
    fn test_load_recent_messages() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        db.append_message(sid, "user", &[MessageContent::text("first")], Some("cli"))
            .unwrap();
        db.append_message(sid, "assistant", &[MessageContent::text("second")], None)
            .unwrap();
        db.append_message(sid, "user", &[MessageContent::text("third")], Some("cli"))
            .unwrap();

        // limit to 2 — should get the two most recent, oldest first
        let msgs = db.load_recent_messages(sid, 2).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[1].role, Role::User);
        assert!(!msgs[0].created_at.is_empty());

        match &msgs[1].content[0] {
            MessageContent::Text { text } => assert_eq!(text, "third"),
            _ => panic!("expected text content"),
        }
    }

    #[test]
    fn test_message_usage_round_trip_for_history() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();
        let message_id = db
            .append_message(sid, "assistant", &[MessageContent::text("hi")], None)
            .unwrap();

        db.set_message_usage(
            message_id,
            &Usage {
                input_tokens: 100,
                output_tokens: 25,
                reasoning_tokens: Some(10),
                cache_creation_tokens: Some(30),
                cache_read_tokens: Some(40),
                ..Default::default()
            },
            "anthropic/claude-sonnet-4-6",
        )
        .unwrap();

        let msgs = db.load_recent_messages(sid, 1).unwrap();
        assert_eq!(
            msgs[0].model_id.as_deref(),
            Some("anthropic/claude-sonnet-4-6")
        );
        assert_eq!(msgs[0].input_tokens, Some(100));
        assert_eq!(msgs[0].output_tokens, Some(25));
        assert_eq!(msgs[0].reasoning_tokens, Some(10));
        assert_eq!(msgs[0].cache_creation_tokens, Some(30));
        assert_eq!(msgs[0].cache_read_tokens, Some(40));

        let usage = db.session_usage_records(sid).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(
            usage[0].model_id.as_deref(),
            Some("anthropic/claude-sonnet-4-6")
        );
        assert_eq!(usage[0].usage.input_tokens, 100);
        assert_eq!(usage[0].usage.output_tokens, 25);
        assert_eq!(usage[0].usage.reasoning_tokens, Some(10));
        assert_eq!(usage[0].usage.cache_creation_tokens, Some(30));
        assert_eq!(usage[0].usage.cache_read_tokens, Some(40));
    }

    #[test]
    fn test_session_summary_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        // default is none
        assert_eq!(db.get_session_summary(sid).unwrap(), None);

        // set and get
        db.set_session_summary(sid, "user discussed rust").unwrap();
        assert_eq!(
            db.get_session_summary(sid).unwrap().as_deref(),
            Some("user discussed rust")
        );

        // update
        db.set_session_summary(sid, "updated summary").unwrap();
        assert_eq!(
            db.get_session_summary(sid).unwrap().as_deref(),
            Some("updated summary")
        );
    }

    #[test]
    fn test_compaction_cursor_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        // default is none
        assert_eq!(db.get_compaction_cursor(sid).unwrap(), None);

        // set and get
        db.set_compaction_cursor(sid, 42).unwrap();
        assert_eq!(db.get_compaction_cursor(sid).unwrap(), Some(42));

        // advance
        db.set_compaction_cursor(sid, 100).unwrap();
        assert_eq!(db.get_compaction_cursor(sid).unwrap(), Some(100));
    }

    #[test]
    fn test_last_completion_at_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        // default is none
        assert_eq!(db.session_last_completion_at(sid).unwrap(), None);

        db.set_session_last_completion_at(sid, 1_700_000_000)
            .unwrap();
        assert_eq!(
            db.session_last_completion_at(sid).unwrap(),
            Some(1_700_000_000)
        );
    }

    #[test]
    fn test_clear_session_context_drops_history_and_resets() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        // seed three messages and a summary + usage + completion-at
        for i in 1..=3 {
            let role = if i % 2 == 1 { "user" } else { "assistant" };
            db.append_message(sid, role, &[MessageContent::text(format!("m{i}"))], None)
                .unwrap();
        }
        db.set_session_summary(sid, "old summary").unwrap();
        db.set_session_usage(sid, 50_000, 200_000).unwrap();
        db.set_session_last_completion_at(sid, 1_700_000_000)
            .unwrap();

        let cleared = db.clear_session_context(sid).unwrap();
        assert!(cleared >= 3, "expected at least 3 elided messages");

        // summary, usage, and completion are gone
        assert_eq!(db.get_session_summary(sid).unwrap(), None);
        assert_eq!(db.session_usage(sid).unwrap(), None);
        assert_eq!(db.session_last_completion_at(sid).unwrap(), None);

        // load_messages now returns nothing because the cursor sits past every
        // existing message id.
        let msgs = db.load_messages(sid).unwrap();
        assert!(msgs.is_empty(), "expected empty after clear, got {msgs:?}");
    }

    #[test]
    fn test_clear_session_context_before_preserves_trigger_message() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        db.append_message(
            sid,
            "user",
            &[MessageContent::text("old user")],
            Some("cli"),
        )
        .unwrap();
        db.append_message(sid, "assistant", &[MessageContent::text("old reply")], None)
            .unwrap();
        db.set_session_summary(sid, "old summary").unwrap();
        db.set_session_usage(sid, 50_000, 200_000).unwrap();
        db.set_session_last_completion_at(sid, 1_700_000_000)
            .unwrap();

        let trigger_id = db
            .append_message(
                sid,
                "user",
                &[MessageContent::text("fresh question")],
                Some("cli"),
            )
            .unwrap();

        let cleared = db
            .clear_session_context_before(sid, Some(trigger_id))
            .unwrap();
        assert!(cleared >= 2, "expected old messages to be elided");

        let msgs = db.load_messages(sid).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content[0] {
            MessageContent::Text { text } => assert_eq!(text, "fresh question"),
            other => panic!("expected text content, got {other:?}"),
        }
        assert_eq!(db.get_session_summary(sid).unwrap(), None);
        assert_eq!(db.session_usage(sid).unwrap(), None);
        assert_eq!(db.session_last_completion_at(sid).unwrap(), None);
    }

    #[test]
    fn test_load_messages_respects_cursor() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        // insert 6 messages
        for i in 1..=6 {
            let role = if i % 2 == 1 { "user" } else { "assistant" };
            db.append_message(
                sid,
                role,
                &[MessageContent::text(format!("msg {i}"))],
                Some("cli"),
            )
            .unwrap();
        }

        // find the 4th message id (we'll compact everything up to it)
        let all = db.load_messages_with_ids(sid).unwrap();
        assert_eq!(all.len(), 6);
        let cursor_id = all[3].0; // 4th message

        // set cursor + summary
        db.set_compaction_cursor(sid, cursor_id).unwrap();
        db.set_session_summary(sid, "summary of msgs 1-4").unwrap();

        // load_messages should return [summary, msg5, msg6]
        let msgs = db.load_messages(sid).unwrap();
        assert_eq!(msgs.len(), 3);

        // first is the synthetic summary
        match &msgs[0].content[0] {
            MessageContent::Text { text } => {
                assert!(text.contains("[conversation summary]"));
                assert!(text.contains("summary of msgs 1-4"));
            }
            _ => panic!("expected text"),
        }

        // remaining are the real messages after the cursor
        match &msgs[1].content[0] {
            MessageContent::Text { text } => assert_eq!(text, "msg 5"),
            _ => panic!("expected text"),
        }
        match &msgs[2].content[0] {
            MessageContent::Text { text } => assert_eq!(text, "msg 6"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn test_load_messages_no_cursor_unchanged() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        db.append_message(sid, "user", &[MessageContent::text("hello")], Some("cli"))
            .unwrap();
        db.append_message(sid, "assistant", &[MessageContent::text("hi")], None)
            .unwrap();

        // no cursor set — should behave exactly as before
        let msgs = db.load_messages(sid).unwrap();
        assert_eq!(msgs.len(), 2);
        match &msgs[0].content[0] {
            MessageContent::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn test_nth_message_id() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        for i in 1..=5 {
            db.append_message(
                sid,
                "user",
                &[MessageContent::text(format!("msg {i}"))],
                Some("cli"),
            )
            .unwrap();
        }

        let all = db.load_messages_with_ids(sid).unwrap();
        let id1 = all[0].0;
        let id3 = all[2].0;
        let id5 = all[4].0;

        // 1st message after id 0
        assert_eq!(db.nth_message_id(sid, 0, 1).unwrap(), Some(id1));
        // 3rd message after id 0
        assert_eq!(db.nth_message_id(sid, 0, 3).unwrap(), Some(id3));
        // 5th message after id 0
        assert_eq!(db.nth_message_id(sid, 0, 5).unwrap(), Some(id5));
        // 6th doesn't exist
        assert_eq!(db.nth_message_id(sid, 0, 6).unwrap(), None);
        // 0th is none
        assert_eq!(db.nth_message_id(sid, 0, 0).unwrap(), None);
        // 2nd message after id1 should be id3
        assert_eq!(db.nth_message_id(sid, id1, 2).unwrap(), Some(id3));
    }

    #[test]
    fn test_load_recent_messages_unaffected_by_cursor() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();

        for i in 1..=4 {
            let role = if i % 2 == 1 { "user" } else { "assistant" };
            db.append_message(
                sid,
                role,
                &[MessageContent::text(format!("msg {i}"))],
                Some("cli"),
            )
            .unwrap();
        }

        let all = db.load_messages_with_ids(sid).unwrap();
        let cursor_id = all[1].0;
        db.set_compaction_cursor(sid, cursor_id).unwrap();
        db.set_session_summary(sid, "summary").unwrap();

        // load_recent_messages should still return all 4 messages
        let recent = db.load_recent_messages(sid, 10).unwrap();
        assert_eq!(recent.len(), 4);
    }
}
