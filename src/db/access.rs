use crate::error::Error;
use rusqlite::params;

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnauthorizedDmAttempt {
    pub user_id: i64,
    pub username: Option<String>,
    pub chat_id: i64,
    pub attempted_at: String,
}

use super::Database;

impl Database {
    pub fn is_user_allowed(&self, user_id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM allowed_users WHERE user_id = ?1)",
            [user_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub fn is_chat_allowed(&self, chat_id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM allowed_chats WHERE chat_id = ?1)",
            [chat_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub fn add_allowed_user(&self, user_id: i64, added_by: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO allowed_users (user_id, added_by) VALUES (?1, ?2)",
            rusqlite::params![user_id, added_by],
        )?;
        Ok(())
    }

    pub fn remove_allowed_user(&self, user_id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM allowed_users WHERE user_id = ?1", [user_id])?;
        Ok(())
    }

    pub fn add_allowed_chat(&self, chat_id: i64, added_by: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO allowed_chats (chat_id, added_by) VALUES (?1, ?2)",
            rusqlite::params![chat_id, added_by],
        )?;
        Ok(())
    }

    pub fn remove_allowed_chat(&self, chat_id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM allowed_chats WHERE chat_id = ?1", [chat_id])?;
        Ok(())
    }

    pub fn list_allowed_users(&self) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT user_id FROM allowed_users ORDER BY user_id")?;
        let ids = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    }

    pub fn list_allowed_chats(&self) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT chat_id FROM allowed_chats ORDER BY chat_id")?;
        let ids = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    }

    /// seed allowed_users from a list of user_ids (idempotent).
    pub fn seed_allowed_users(&self, user_ids: &[i64]) -> Result<(), Error> {
        for &id in user_ids {
            self.add_allowed_user(id, "env")?;
        }
        Ok(())
    }

    /// seed allowed_chats from a list of chat_ids (idempotent).
    pub fn seed_allowed_chats(&self, chat_ids: &[i64]) -> Result<(), Error> {
        for &id in chat_ids {
            self.add_allowed_chat(id, "env")?;
        }
        Ok(())
    }

    pub fn record_unauthorized_dm_attempt(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO unauthorized_dm_attempts
                (user_id, username, chat_id, attempted_at)
            VALUES
                (?1, ?2, ?3, datetime('now'))
            ON CONFLICT(user_id) DO UPDATE SET
                username = excluded.username,
                chat_id = excluded.chat_id,
                attempted_at = excluded.attempted_at
            "#,
            params![user_id, username, chat_id],
        )?;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_unauthorized_dm_attempt(
        &self,
        user_id: i64,
    ) -> Result<Option<UnauthorizedDmAttempt>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT user_id, username, chat_id, attempted_at
             FROM unauthorized_dm_attempts
             WHERE user_id = ?1",
        )?;
        let mut rows = stmt.query([user_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(UnauthorizedDmAttempt {
                user_id: row.get(0)?,
                username: row.get(1)?,
                chat_id: row.get(2)?,
                attempted_at: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_whitelist_crud() {
        let db = Database::open_in_memory().unwrap();

        assert!(!db.is_user_allowed(123).unwrap());

        db.add_allowed_user(123, "env").unwrap();
        assert!(db.is_user_allowed(123).unwrap());

        // idempotent
        db.add_allowed_user(123, "env").unwrap();
        assert_eq!(db.list_allowed_users().unwrap(), vec![123]);

        db.add_allowed_user(456, "user:123").unwrap();
        assert_eq!(db.list_allowed_users().unwrap(), vec![123, 456]);

        db.remove_allowed_user(123).unwrap();
        assert!(!db.is_user_allowed(123).unwrap());
        assert_eq!(db.list_allowed_users().unwrap(), vec![456]);
    }

    #[test]
    fn test_chat_whitelist_crud() {
        let db = Database::open_in_memory().unwrap();

        assert!(!db.is_chat_allowed(-100123).unwrap());

        db.add_allowed_chat(-100123, "env").unwrap();
        assert!(db.is_chat_allowed(-100123).unwrap());

        db.add_allowed_chat(-100456, "user:789").unwrap();
        assert_eq!(db.list_allowed_chats().unwrap(), vec![-100456, -100123]);

        db.remove_allowed_chat(-100123).unwrap();
        assert!(!db.is_chat_allowed(-100123).unwrap());
        assert_eq!(db.list_allowed_chats().unwrap(), vec![-100456]);
    }

    #[test]
    fn test_record_unauthorized_dm_attempt_upserts_latest_metadata() {
        let db = Database::open_in_memory().unwrap();

        db.record_unauthorized_dm_attempt(123, Some("christian"), 123)
            .unwrap();

        let first = db.get_unauthorized_dm_attempt(123).unwrap().unwrap();
        assert_eq!(first.user_id, 123);
        assert_eq!(first.username.as_deref(), Some("christian"));
        assert_eq!(first.chat_id, 123);

        db.record_unauthorized_dm_attempt(123, Some("chris"), 999)
            .unwrap();

        let second = db.get_unauthorized_dm_attempt(123).unwrap().unwrap();
        assert_eq!(second.username.as_deref(), Some("chris"));
        assert_eq!(second.chat_id, 999);
    }

    #[test]
    fn test_seed_users_idempotent() {
        let db = Database::open_in_memory().unwrap();

        db.seed_allowed_users(&[1, 2, 3]).unwrap();
        assert_eq!(db.list_allowed_users().unwrap(), vec![1, 2, 3]);

        // second seed doesn't duplicate
        db.seed_allowed_users(&[2, 3, 4]).unwrap();
        assert_eq!(db.list_allowed_users().unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_seed_chats_idempotent() {
        let db = Database::open_in_memory().unwrap();

        db.seed_allowed_chats(&[-100, -200]).unwrap();
        assert_eq!(db.list_allowed_chats().unwrap(), vec![-200, -100]);

        db.seed_allowed_chats(&[-200, -300]).unwrap();
        assert_eq!(db.list_allowed_chats().unwrap(), vec![-300, -200, -100]);
    }
}
