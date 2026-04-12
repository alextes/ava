mod access;
pub(crate) mod channels;
mod memory;
mod migrations;
mod rules;
mod schedule;
pub(crate) mod session;
mod task;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::config::default_db_path;
use crate::error::Error;

pub use memory::{Memory, MemoryKind};
pub use rules::{
    contains_command_substitution, generate_edit_pattern, generate_narrow_pattern,
    generate_pattern, generate_read_pattern,
};

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_run_cleanly() {
        let db = Database::open_in_memory().unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(version, 13);
    }

    #[test]
    fn test_migrations_are_idempotent() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            migrations::migrate(&conn).unwrap();
        }
        let version = db.schema_version().unwrap();
        assert_eq!(version, 13);
    }

    #[test]
    fn test_v6_migration_preserves_facts() {
        // manually set up schema at v5 with facts data, then run migration to v6
        let conn = Connection::open_in_memory().unwrap();

        // create schema_version table and run migrations 1-5
        conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY)",
            [],
        )
        .unwrap();

        // v1: sessions + messages
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                title TEXT,
                model TEXT
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY,
                session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
            .unwrap();

        // v2: facts table
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS facts (
                id INTEGER PRIMARY KEY,
                category TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'agent',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(category, key)
            );
            "#,
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (2)", [])
            .unwrap();

        // v3-v5: other tables (simplified)
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS approval_rules (
                id INTEGER PRIMARY KEY,
                pattern TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (3)", [])
            .unwrap();

        conn.execute_batch(
            r#"
            ALTER TABLE sessions ADD COLUMN active INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE messages ADD COLUMN channel TEXT;
            INSERT INTO sessions (active, title) VALUES (1, 'default');
            "#,
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (4)", [])
            .unwrap();

        conn.execute_batch("ALTER TABLE sessions ADD COLUMN summary TEXT;")
            .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (5)", [])
            .unwrap();

        // insert test facts at v5
        conn.execute(
            "INSERT INTO facts (category, key, value, source, created_at, updated_at) VALUES (?1, ?2, ?3, 'agent', '2024-01-01 00:00:00', '2024-01-01 00:00:00')",
            ["user", "name", "alex"],
        ).unwrap();
        conn.execute(
            "INSERT INTO facts (category, key, value, source, created_at, updated_at) VALUES (?1, ?2, ?3, 'agent', '2024-01-02 00:00:00', '2024-01-02 00:00:00')",
            ["preferences", "response_style", "concise"],
        ).unwrap();

        // run migration (v6 only, since 1-5 are already applied)
        migrations::migrate(&conn).unwrap();

        // verify schema version is latest
        let version = migrations::schema_version(&conn).unwrap();
        assert_eq!(version, 13);

        // verify facts table is gone
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='facts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!table_exists);

        // verify memories table has the migrated data
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // verify fact content
        let (kind, content, category, key): (String, String, String, String) = conn
            .query_row(
                "SELECT kind, content, category, key FROM memories WHERE category = 'user'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(kind, "fact");
        assert_eq!(content, "alex");
        assert_eq!(category, "user");
        assert_eq!(key, "name");

        // verify timestamps were preserved
        let created_at: String = conn
            .query_row(
                "SELECT created_at FROM memories WHERE category = 'user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(created_at, "2024-01-01 00:00:00");

        // verify fts5 index works (migrated data is searchable)
        let fts_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories m JOIN memories_fts f ON m.id = f.rowid WHERE memories_fts MATCH 'alex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 1);
    }
}
