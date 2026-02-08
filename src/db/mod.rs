mod migrations;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::config::default_db_path;
use crate::error::Error;
use crate::message::{Message, MessageContent, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    Fact,
    Episode,
    Character,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Fact => "fact",
            MemoryKind::Episode => "episode",
            MemoryKind::Character => "character",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fact" => Some(MemoryKind::Fact),
            "episode" => Some(MemoryKind::Episode),
            "character" => Some(MemoryKind::Character),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    pub id: i64,
    pub kind: MemoryKind,
    pub content: String,
    pub category: Option<String>,
    pub key: Option<String>,
    pub created_at: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRule {
    pub id: i64,
    pub pattern: String,
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// open database at the default location, run migrations
    pub fn open() -> Result<Self, Error> {
        Self::open_at(default_db_path())
    }

    /// open database at a specific path
    pub fn open_at(path: impl AsRef<Path>) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        migrations::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// in-memory database for testing
    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        migrations::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[allow(dead_code)]
    pub fn schema_version(&self) -> Result<i32, Error> {
        let conn = self.conn.lock().unwrap();
        migrations::schema_version(&conn)
    }

    pub fn remember(
        &self,
        kind: MemoryKind,
        content: &str,
        category: Option<&str>,
        key: Option<&str>,
    ) -> Result<i64, Error> {
        tracing::debug!(?kind, ?category, ?key, "remembering");
        let conn = self.conn.lock().unwrap();
        let kind_str = kind.as_str();

        match kind {
            MemoryKind::Fact => {
                let cat = category.unwrap_or("");
                let k = key.unwrap_or("");
                // try update first, insert if 0 rows
                let updated = conn.execute(
                    "UPDATE memories SET content = ?1, updated_at = datetime('now')
                     WHERE kind = 'fact' AND category = ?2 AND key = ?3",
                    rusqlite::params![content, cat, k],
                )?;
                if updated > 0 {
                    let id: i64 = conn.query_row(
                        "SELECT id FROM memories WHERE kind = 'fact' AND category = ?1 AND key = ?2",
                        rusqlite::params![cat, k],
                        |row| row.get(0),
                    )?;
                    return Ok(id);
                }
                conn.execute(
                    "INSERT INTO memories (kind, content, category, key, source)
                     VALUES (?1, ?2, ?3, ?4, 'agent')",
                    rusqlite::params![kind_str, content, cat, k],
                )?;
                Ok(conn.last_insert_rowid())
            }
            MemoryKind::Character => {
                let k = key.unwrap_or("");
                let updated = conn.execute(
                    "UPDATE memories SET content = ?1, updated_at = datetime('now')
                     WHERE kind = 'character' AND key = ?2",
                    rusqlite::params![content, k],
                )?;
                if updated > 0 {
                    let id: i64 = conn.query_row(
                        "SELECT id FROM memories WHERE kind = 'character' AND key = ?1",
                        rusqlite::params![k],
                        |row| row.get(0),
                    )?;
                    return Ok(id);
                }
                conn.execute(
                    "INSERT INTO memories (kind, content, key, source)
                     VALUES (?1, ?2, ?3, 'agent')",
                    rusqlite::params![kind_str, content, k],
                )?;
                Ok(conn.last_insert_rowid())
            }
            MemoryKind::Episode => {
                conn.execute(
                    "INSERT INTO memories (kind, content, source) VALUES (?1, ?2, 'agent')",
                    rusqlite::params![kind_str, content],
                )?;
                Ok(conn.last_insert_rowid())
            }
        }
    }

    pub fn forget_fact(&self, category: &str, key: &str) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM memories WHERE kind = 'fact' AND category = ?1 AND key = ?2",
            rusqlite::params![category, key],
        )?;
        Ok(rows > 0)
    }

    pub fn forget_character(&self, key: &str) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM memories WHERE kind = 'character' AND key = ?1",
            rusqlite::params![key],
        )?;
        Ok(rows > 0)
    }

    pub fn forget_memory(&self, id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])?;
        Ok(rows > 0)
    }

    pub fn character_traits(&self) -> Result<Vec<Memory>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, content, category, key, created_at
             FROM memories WHERE kind = 'character'
             ORDER BY updated_at DESC LIMIT 20",
        )?;
        let memories = stmt
            .query_map([], |row| {
                let kind_str: String = row.get(1)?;
                Ok(Memory {
                    id: row.get(0)?,
                    kind: MemoryKind::from_str(&kind_str).unwrap_or(MemoryKind::Character),
                    content: row.get(2)?,
                    category: row.get(3)?,
                    key: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(memories)
    }

    pub fn recent_episodes(&self) -> Result<Vec<Memory>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, content, category, key, created_at
             FROM memories WHERE kind = 'episode'
             ORDER BY created_at DESC LIMIT 20",
        )?;
        let memories = stmt
            .query_map([], |row| {
                let kind_str: String = row.get(1)?;
                Ok(Memory {
                    id: row.get(0)?,
                    kind: MemoryKind::from_str(&kind_str).unwrap_or(MemoryKind::Episode),
                    content: row.get(2)?,
                    category: row.get(3)?,
                    key: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(memories)
    }

    pub fn search_memories(&self, query: &str, limit: u32) -> Result<Vec<Memory>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.kind, m.content, m.category, m.key, m.created_at
             FROM memories m
             JOIN memories_fts f ON m.id = f.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY f.rank
             LIMIT ?2",
        )?;
        let memories = stmt
            .query_map(rusqlite::params![query, limit], |row| {
                let kind_str: String = row.get(1)?;
                Ok(Memory {
                    id: row.get(0)?,
                    kind: MemoryKind::from_str(&kind_str).unwrap_or(MemoryKind::Fact),
                    content: row.get(2)?,
                    category: row.get(3)?,
                    key: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(memories)
    }

    pub fn save_approval_rule(&self, pattern: &str) -> Result<(), Error> {
        tracing::debug!(pattern, "saving approval rule");
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO approval_rules (pattern) VALUES (?1)",
            [pattern],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn find_matching_rule(&self, command: &str) -> Result<Option<i64>, Error> {
        let rules = self.list_approval_rules()?;
        for rule in rules {
            if matches_rule(&rule.pattern, command) {
                return Ok(Some(rule.id));
            }
        }
        Ok(None)
    }

    #[allow(dead_code)]
    pub fn list_approval_rules(&self) -> Result<Vec<ApprovalRule>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, pattern FROM approval_rules ORDER BY id")?;

        let rules = stmt
            .query_map([], |row| {
                Ok(ApprovalRule {
                    id: row.get(0)?,
                    pattern: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rules)
    }

    #[allow(dead_code)]
    pub fn delete_approval_rule(&self, id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM approval_rules WHERE id = ?1", [id])?;
        Ok(rows > 0)
    }

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

    pub fn recent_facts(&self) -> Result<Vec<Memory>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, content, category, key, created_at
             FROM memories WHERE kind = 'fact'
             ORDER BY updated_at DESC LIMIT 50",
        )?;

        let facts = stmt
            .query_map([], |row| {
                let kind_str: String = row.get(1)?;
                Ok(Memory {
                    id: row.get(0)?,
                    kind: MemoryKind::from_str(&kind_str).unwrap_or(MemoryKind::Fact),
                    content: row.get(2)?,
                    category: row.get(3)?,
                    key: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(facts)
    }
}

/// matches a command against a rule pattern.
/// tokens are space-separated. `*` as trailing wildcard matches any remaining args.
/// `*` in a middle position matches exactly one token.
/// for commands with pipes/chains (|, &&, ||, ;), each sub-command must match.
#[allow(dead_code)]
fn matches_rule(pattern: &str, command: &str) -> bool {
    let sub_commands = split_subcommands(command);

    // every sub-command must match the pattern
    sub_commands
        .iter()
        .all(|sub| matches_single(pattern, sub.trim()))
}

#[allow(dead_code)]
fn split_subcommands(command: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = command.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] {
            b'|' => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    // ||
                    parts.push(&command[start..i]);
                    i += 2;
                    start = i;
                } else {
                    // |
                    parts.push(&command[start..i]);
                    i += 1;
                    start = i;
                }
            }
            b'&' if i + 1 < len && bytes[i + 1] == b'&' => {
                // &&
                parts.push(&command[start..i]);
                i += 2;
                start = i;
            }
            b';' => {
                parts.push(&command[start..i]);
                i += 1;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    if start < len {
        parts.push(&command[start..]);
    }

    parts
}

#[allow(dead_code)]
fn matches_single(pattern: &str, command: &str) -> bool {
    let pattern_tokens: Vec<&str> = pattern.split_whitespace().collect();
    let command_tokens: Vec<&str> = command.split_whitespace().collect();

    if pattern_tokens.is_empty() {
        return command_tokens.is_empty();
    }

    for (i, pat) in pattern_tokens.iter().enumerate() {
        let is_last = i == pattern_tokens.len() - 1;

        if *pat == "*" {
            if is_last {
                // trailing * matches everything remaining
                return true;
            }
            // middle * matches exactly one token
            if i >= command_tokens.len() {
                return false;
            }
            // any single token matches, continue
            continue;
        }

        if i >= command_tokens.len() {
            return false;
        }

        if *pat != command_tokens[i] {
            return false;
        }
    }

    // pattern fully consumed — command must be exactly the same length
    command_tokens.len() == pattern_tokens.len()
}

/// generates an "allow always" pattern from a command:
/// first token (executable) + `*`
pub fn generate_pattern(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or(command);
    format!("{first} *")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_run_cleanly() {
        let db = Database::open_in_memory().unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(version, 6);
    }

    #[test]
    fn test_migrations_are_idempotent() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            migrations::migrate(&conn).unwrap();
        }
        let version = db.schema_version().unwrap();
        assert_eq!(version, 6);
    }

    #[test]
    fn test_remember_fact_upserts() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();
        db.remember(MemoryKind::Fact, "alex2", Some("user"), Some("name"))
            .unwrap();

        let facts = db.recent_facts().unwrap();
        let fact = facts
            .iter()
            .find(|f| f.key.as_deref() == Some("name"))
            .unwrap();
        assert_eq!(fact.content, "alex2");
    }

    #[test]
    fn test_remember_character_upserts() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Character, "formal", None, Some("tone"))
            .unwrap();
        db.remember(MemoryKind::Character, "casual", None, Some("tone"))
            .unwrap();

        let traits = db.character_traits().unwrap();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].content, "casual");
    }

    #[test]
    fn test_remember_episode_appends() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Episode, "discussed migration plan", None, None)
            .unwrap();
        db.remember(MemoryKind::Episode, "chose option B", None, None)
            .unwrap();

        let episodes = db.recent_episodes().unwrap();
        assert_eq!(episodes.len(), 2);
    }

    #[test]
    fn test_forget_fact() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();
        assert!(db.forget_fact("user", "name").unwrap());
        assert!(!db.forget_fact("user", "name").unwrap());
        assert!(db.recent_facts().unwrap().is_empty());
    }

    #[test]
    fn test_forget_character() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Character, "formal", None, Some("tone"))
            .unwrap();
        assert!(db.forget_character("tone").unwrap());
        assert!(!db.forget_character("tone").unwrap());
        assert!(db.character_traits().unwrap().is_empty());
    }

    #[test]
    fn test_forget_memory_by_id() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .remember(MemoryKind::Episode, "some event", None, None)
            .unwrap();
        assert!(db.forget_memory(id).unwrap());
        assert!(!db.forget_memory(id).unwrap());
    }

    #[test]
    fn test_search_memories() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "loves rust programming",
            Some("user"),
            Some("hobby"),
        )
        .unwrap();
        db.remember(
            MemoryKind::Episode,
            "discussed python migration",
            None,
            None,
        )
        .unwrap();
        db.remember(MemoryKind::Fact, "amsterdam", Some("user"), Some("city"))
            .unwrap();

        let results = db.search_memories("rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("rust"));

        let results = db.search_memories("migration", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, MemoryKind::Episode);
    }

    #[test]
    fn test_recent_facts_limit_and_order() {
        let db = Database::open_in_memory().unwrap();

        {
            let conn = db.conn.lock().unwrap();
            for i in 0..55 {
                let key = format!("k{i:02}");
                let content = format!("v{i:02}");
                let updated_at = format!("2024-01-01 00:00:{i:02}");
                conn.execute(
                    "INSERT INTO memories (kind, content, category, key, updated_at)
                    VALUES ('fact', ?1, 'user', ?2, ?3)",
                    rusqlite::params![content, key, updated_at],
                )
                .unwrap();
            }
        }

        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 50);
        assert_eq!(facts.first().unwrap().key.as_deref(), Some("k54"));
        assert_eq!(facts.last().unwrap().key.as_deref(), Some("k05"));
    }

    #[test]
    fn test_save_and_list_approval_rules() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        db.save_approval_rule("cargo *").unwrap();

        let rules = db.list_approval_rules().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].pattern, "ls *");
        assert_eq!(rules[1].pattern, "cargo *");
    }

    #[test]
    fn test_save_approval_rule_ignores_duplicate() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        db.save_approval_rule("ls *").unwrap();

        let rules = db.list_approval_rules().unwrap();
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_delete_approval_rule() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();

        let rules = db.list_approval_rules().unwrap();
        assert!(db.delete_approval_rule(rules[0].id).unwrap());
        assert_eq!(db.list_approval_rules().unwrap().len(), 0);
    }

    #[test]
    fn test_find_matching_rule() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();

        assert!(db.find_matching_rule("ls -la").unwrap().is_some());
        assert!(db.find_matching_rule("ls").unwrap().is_some());
        assert!(db.find_matching_rule("rm -rf /").unwrap().is_none());
    }

    #[test]
    fn test_matches_rule_trailing_wildcard() {
        assert!(matches_rule("ls *", "ls"));
        assert!(matches_rule("ls *", "ls -la"));
        assert!(matches_rule("ls *", "ls -la /tmp"));
        assert!(!matches_rule("ls *", "rm foo"));
    }

    #[test]
    fn test_matches_rule_exact() {
        assert!(matches_rule("git status", "git status"));
        assert!(!matches_rule("git status", "git status -v"));
        assert!(!matches_rule("git status", "git"));
    }

    #[test]
    fn test_matches_rule_cargo_test() {
        assert!(matches_rule("cargo test *", "cargo test"));
        assert!(matches_rule("cargo test *", "cargo test -- --nocapture"));
    }

    #[test]
    fn test_matches_rule_pipe() {
        // both sub-commands must match
        assert!(matches_rule("ls *", "ls -la | ls /tmp"));
        assert!(!matches_rule("ls *", "ls -la | rm foo"));
    }

    #[test]
    fn test_matches_rule_chain() {
        assert!(matches_rule("cargo *", "cargo fmt && cargo test"));
        assert!(!matches_rule("cargo *", "cargo fmt && rm foo"));
    }

    #[test]
    fn test_generate_pattern() {
        assert_eq!(generate_pattern("ls -la /tmp"), "ls *");
        assert_eq!(generate_pattern("cargo test -- --nocapture"), "cargo *");
    }

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
