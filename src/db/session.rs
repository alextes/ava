use serde::Serialize;

use crate::error::Error;
use crate::message::{Message, MessageContent, Role};

use super::Database;

/// a message with its timestamp, for history display
#[derive(Debug, Clone, Serialize)]
pub struct HistoryMessage {
    pub id: i64,
    pub role: Role,
    pub content: Vec<MessageContent>,
    pub created_at: String,
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
    pub fn load_messages(&self, session_id: i64) -> Result<Vec<Message>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT role, content FROM messages
             WHERE session_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;

        let messages = stmt
            .query_map([session_id], |row| {
                let role_str: String = row.get(0)?;
                let content_json: String = row.get(1)?;
                Ok((role_str, content_json))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(messages.len());
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
            "SELECT id, role, content, created_at FROM messages
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
                Ok((id, role_str, content_json, created_at))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(rows.len());
        for (id, role_str, content_json, created_at) in rows {
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
            "SELECT id, role, content, created_at FROM messages
             WHERE session_id = ?1 AND id > ?2
             ORDER BY id ASC",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![session_id, after_id], |row| {
                let id: i64 = row.get(0)?;
                let role_str: String = row.get(1)?;
                let content_json: String = row.get(2)?;
                let created_at: String = row.get(3)?;
                Ok((id, role_str, content_json, created_at))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(rows.len());
        for (id, role_str, content_json, created_at) in rows {
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
            });
        }

        Ok(result)
    }

    /// append a message to the session
    pub fn append_message(
        &self,
        session_id: i64,
        role: &str,
        content: &[MessageContent],
        channel: Option<&str>,
    ) -> Result<(), Error> {
        // guard: never persist empty text blocks — the API rejects them
        if content
            .iter()
            .any(|c| matches!(c, MessageContent::Text { text } if text.is_empty()))
        {
            tracing::warn!(role, "refusing to persist message with empty text block");
            return Ok(());
        }

        let content_json = serde_json::to_string(content)
            .map_err(|e| Error::Provider(format!("failed to serialize message: {e}")))?;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content, channel)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, role, content_json, channel],
        )?;

        // update session timestamp
        conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            [session_id],
        )?;

        Ok(())
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
    pub fn set_session_model(&self, session_id: i64, model_id: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET model = ?1 WHERE id = ?2",
            rusqlite::params![model_id, session_id],
        )?;
        Ok(())
    }

    /// clear the persisted model for a session (e.g. when it becomes invalid)
    pub fn clear_session_model(&self, session_id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET model = NULL WHERE id = ?1",
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

    /// deactivate the current session and create a fresh one.
    /// returns the new session id.
    pub fn new_session(&self) -> Result<i64, Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE sessions SET active = 0 WHERE active = 1", [])?;
        conn.execute(
            "INSERT INTO sessions (active, title) VALUES (1, 'default')",
            [],
        )?;
        Ok(conn.last_insert_rowid())
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
}
