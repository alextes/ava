mod migrations;

use std::path::Path;

use rusqlite::Connection;

use crate::config::default_db_path;
use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub category: String,
    pub key: String,
    pub value: String,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    /// open database at the default location, run migrations
    pub fn open() -> Result<Self, Error> {
        Self::open_at(default_db_path())
    }

    /// open database at a specific path
    pub fn open_at(path: impl AsRef<Path>) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        migrations::migrate(&db.conn)?;
        Ok(db)
    }

    /// in-memory database for testing
    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        migrations::migrate(&db.conn)?;
        Ok(db)
    }

    pub fn schema_version(&self) -> Result<i32, Error> {
        migrations::schema_version(&self.conn)
    }

    pub fn remember_fact(&self, category: &str, key: &str, value: &str) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO facts (category, key, value, source)
            VALUES (?1, ?2, ?3, 'agent')
            ON CONFLICT(category, key) DO UPDATE SET
                value = excluded.value,
                source = excluded.source,
                updated_at = datetime('now')",
            [category, key, value],
        )?;
        Ok(())
    }

    pub fn recent_facts(&self) -> Result<Vec<Fact>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT category, key, value
            FROM facts
            ORDER BY updated_at DESC
            LIMIT 50",
        )?;

        let facts = stmt
            .query_map([], |row| {
                Ok(Fact {
                    category: row.get(0)?,
                    key: row.get(1)?,
                    value: row.get(2)?,
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
    fn test_migrations_run_cleanly() {
        let db = Database::open_in_memory().unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn test_migrations_are_idempotent() {
        let db = Database::open_in_memory().unwrap();
        migrations::migrate(&db.conn).unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn test_remember_fact_upserts() {
        let db = Database::open_in_memory().unwrap();
        db.remember_fact("user", "name", "alex").unwrap();
        db.remember_fact("user", "name", "alex2").unwrap();

        let value: String = db
            .conn
            .query_row(
                "SELECT value FROM facts WHERE category = ?1 AND key = ?2",
                ["user", "name"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(value, "alex2");
    }

    #[test]
    fn test_recent_facts_limit_and_order() {
        let db = Database::open_in_memory().unwrap();

        for i in 0..55 {
            let key = format!("k{i:02}");
            let value = format!("v{i:02}");
            let updated_at = format!("2024-01-01 00:00:{i:02}");
            db.conn
                .execute(
                    "INSERT INTO facts (category, key, value, updated_at)
                    VALUES (?1, ?2, ?3, ?4)",
                    ["user", &key, &value, &updated_at],
                )
                .unwrap();
        }

        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 50);
        assert_eq!(facts.first().unwrap().key, "k54");
        assert_eq!(facts.last().unwrap().key, "k05");
    }
}
