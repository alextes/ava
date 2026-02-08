use crate::error::Error;

use super::Database;

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

impl Database {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
