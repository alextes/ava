use crate::error::Error;
use crate::message::{Message, MessageContent, Role};

use super::Database;

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
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => continue, // skip unknown roles
            };
            let content: Vec<MessageContent> = serde_json::from_str(&content_json)
                .map_err(|e| Error::Provider(format!("failed to deserialize message: {e}")))?;
            result.push(Message { role, content });
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

        db.set_session_model(sid, "anthropic/claude-sonnet-4-5")
            .unwrap();
        assert_eq!(
            db.session_model(sid).unwrap().as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );

        // update to a different model
        db.set_session_model(sid, "openai/gpt-5.2").unwrap();
        assert_eq!(
            db.session_model(sid).unwrap().as_deref(),
            Some("openai/gpt-5.2")
        );
    }

    #[test]
    fn test_session_model_default_is_none() {
        let db = Database::open_in_memory().unwrap();
        let sid = db.active_session().unwrap();
        assert_eq!(db.session_model(sid).unwrap(), None);
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
