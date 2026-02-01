mod migrations;

use std::path::Path;

use rusqlite::Connection;

use crate::config::default_db_path;
use crate::error::Error;

pub struct Database {
    conn: Connection,
}

impl Database {
    /// open database at the default location, run migrations
    pub fn open() -> Result<Self, Error> {
        let path = default_db_path()?;
        Self::open_at(&path)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_run_cleanly() {
        let db = Database::open_in_memory().unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_migrations_are_idempotent() {
        let db = Database::open_in_memory().unwrap();
        migrations::migrate(&db.conn).unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(version, 1);
    }
}
