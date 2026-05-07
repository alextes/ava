use rusqlite::OptionalExtension;

use crate::error::Error;

use super::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    Fact,
    Episode,
    Identity,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Fact => "fact",
            MemoryKind::Episode => "episode",
            MemoryKind::Identity => "identity",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fact" => Some(MemoryKind::Fact),
            "episode" => Some(MemoryKind::Episode),
            "identity" => Some(MemoryKind::Identity),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySearchMode {
    AllTerms,
    AnyTerms,
    ExactPhrase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySearchOptions<'a> {
    pub query: &'a str,
    pub limit: u32,
    pub match_mode: MemorySearchMode,
    pub kind: Option<MemoryKind>,
}

impl<'a> MemorySearchOptions<'a> {
    pub fn searchable_term_count(&self) -> usize {
        searchable_terms(self.query).len()
    }
}

fn searchable_terms(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect()
}

fn quote_fts5(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn fts5_query(options: &MemorySearchOptions<'_>) -> Option<String> {
    let terms = searchable_terms(options.query);
    if terms.is_empty() {
        return None;
    }

    match options.match_mode {
        MemorySearchMode::AllTerms => Some(
            terms
                .iter()
                .map(|term| quote_fts5(term))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        MemorySearchMode::AnyTerms => Some(
            terms
                .iter()
                .map(|term| quote_fts5(term))
                .collect::<Vec<_>>()
                .join(" OR "),
        ),
        MemorySearchMode::ExactPhrase => Some(quote_fts5(options.query.trim())),
    }
}

impl Database {
    pub fn identity_name(&self) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT content FROM memories WHERE kind = ?1 AND key = ?2 ORDER BY id DESC LIMIT 1",
            rusqlite::params![MemoryKind::Identity.as_str(), "name"],
            |row| row.get(0),
        )
        .optional()
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
            MemoryKind::Identity => {
                let k = key.unwrap_or("");
                let updated = conn.execute(
                    "UPDATE memories SET content = ?1, updated_at = datetime('now')
                     WHERE kind = 'identity' AND key = ?2",
                    rusqlite::params![content, k],
                )?;
                if updated > 0 {
                    let id: i64 = conn.query_row(
                        "SELECT id FROM memories WHERE kind = 'identity' AND key = ?1",
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

    pub fn forget_identity(&self, key: &str) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM memories WHERE kind = 'identity' AND key = ?1",
            rusqlite::params![key],
        )?;
        Ok(rows > 0)
    }

    pub fn forget_memory(&self, id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])?;
        Ok(rows > 0)
    }

    pub fn identity_traits(&self) -> Result<Vec<Memory>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, content, category, key, created_at
             FROM memories WHERE kind = 'identity'
             ORDER BY updated_at DESC LIMIT 20",
        )?;
        let memories = stmt
            .query_map([], |row| {
                let kind_str: String = row.get(1)?;
                Ok(Memory {
                    id: row.get(0)?,
                    kind: MemoryKind::from_str(&kind_str).unwrap_or(MemoryKind::Identity),
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

    pub fn search_memories(&self, options: MemorySearchOptions<'_>) -> Result<Vec<Memory>, Error> {
        let Some(query) = fts5_query(&options) else {
            return Ok(Vec::new());
        };
        let kind = options.kind.map(|kind| kind.as_str());
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.kind, m.content, m.category, m.key, m.created_at
             FROM memories m
             JOIN memories_fts f ON m.id = f.rowid
             WHERE memories_fts MATCH ?1
               AND (?3 IS NULL OR m.kind = ?3)
             ORDER BY f.rank
             LIMIT ?2",
        )?;
        let memories = stmt
            .query_map(rusqlite::params![query, options.limit, kind], |row| {
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

    /// mark initial setup as completed.
    pub fn mark_setup_complete(&self) -> Result<(), Error> {
        self.remember(
            MemoryKind::Fact,
            "true",
            Some("system"),
            Some("setup_completed"),
        )?;
        Ok(())
    }

    /// check if the initial setup flow has been completed.
    pub fn is_setup_complete(&self) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories
             WHERE kind = 'fact' AND category = 'system' AND key = 'setup_completed'",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
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

    fn all_terms(query: &str, limit: u32) -> MemorySearchOptions<'_> {
        MemorySearchOptions {
            query,
            limit,
            match_mode: MemorySearchMode::AllTerms,
            kind: None,
        }
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
    fn test_remember_identity_upserts() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Identity, "formal", None, Some("tone"))
            .unwrap();
        db.remember(MemoryKind::Identity, "casual", None, Some("tone"))
            .unwrap();

        let traits = db.identity_traits().unwrap();
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
    fn test_forget_identity() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Identity, "formal", None, Some("tone"))
            .unwrap();
        assert!(db.forget_identity("tone").unwrap());
        assert!(!db.forget_identity("tone").unwrap());
        assert!(db.identity_traits().unwrap().is_empty());
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

        let results = db.search_memories(all_terms("rust", 10)).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("rust"));

        let results = db.search_memories(all_terms("migration", 10)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, MemoryKind::Episode);
    }

    #[test]
    fn test_search_memories_match_modes() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "loves rust programming",
            Some("user"),
            Some("hobby"),
        )
        .unwrap();
        db.remember(MemoryKind::Episode, "discussed rust migration", None, None)
            .unwrap();
        db.remember(MemoryKind::Episode, "discussed migration rust", None, None)
            .unwrap();
        db.remember(
            MemoryKind::Episode,
            "discussed python migration",
            None,
            None,
        )
        .unwrap();

        let all_terms = db
            .search_memories(MemorySearchOptions {
                query: "rust migration",
                limit: 10,
                match_mode: MemorySearchMode::AllTerms,
                kind: None,
            })
            .unwrap();
        assert_eq!(all_terms.len(), 2);
        assert!(
            all_terms
                .iter()
                .all(|memory| memory.content.contains("rust")
                    && memory.content.contains("migration"))
        );

        let any_terms = db
            .search_memories(MemorySearchOptions {
                query: "rust migration",
                limit: 10,
                match_mode: MemorySearchMode::AnyTerms,
                kind: None,
            })
            .unwrap();
        assert_eq!(any_terms.len(), 4);

        let exact_phrase = db
            .search_memories(MemorySearchOptions {
                query: "rust migration",
                limit: 10,
                match_mode: MemorySearchMode::ExactPhrase,
                kind: None,
            })
            .unwrap();
        assert_eq!(exact_phrase.len(), 1);
        assert_eq!(exact_phrase[0].content, "discussed rust migration");
    }

    #[test]
    fn test_search_memories_escapes_special_characters() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "project turbo-relay supports quote \" handling or near syntax",
            Some("project"),
            Some("turbo-relay"),
        )
        .unwrap();

        for query in [
            "turbo-relay",
            "\"turbo-relay\"",
            "(turbo-relay)",
            "project:turbo-relay",
            "turbo OR NEAR",
        ] {
            let results = db.search_memories(all_terms(query, 10)).unwrap();
            assert_eq!(results.len(), 1, "query should be safe: {query}");
        }

        for query in ["", " - : ( ) \" "] {
            let results = db.search_memories(all_terms(query, 10)).unwrap();
            assert!(
                results.is_empty(),
                "query should return no matches: {query}"
            );
        }
    }

    #[test]
    fn test_search_memories_filters_by_kind() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "rust developer",
            Some("user"),
            Some("role"),
        )
        .unwrap();
        db.remember(MemoryKind::Episode, "discussed rust", None, None)
            .unwrap();

        let results = db
            .search_memories(MemorySearchOptions {
                query: "rust",
                limit: 10,
                match_mode: MemorySearchMode::AllTerms,
                kind: Some(MemoryKind::Episode),
            })
            .unwrap();
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
