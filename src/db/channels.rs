use crate::error::Error;

use super::Database;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ChannelInfo {
    pub chat_id: i64,
    pub chat_type: String,
    pub title: Option<String>,
    pub last_seen_at: String,
}

impl Database {
    /// insert or update a channel's metadata and bump last_seen_at.
    pub fn upsert_channel(
        &self,
        chat_id: i64,
        chat_type: &str,
        title: Option<&str>,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channels (chat_id, chat_type, title)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(chat_id) DO UPDATE SET
                 chat_type = excluded.chat_type,
                 title = COALESCE(excluded.title, channels.title),
                 last_seen_at = datetime('now')",
            rusqlite::params![chat_id, chat_type, title],
        )?;
        Ok(())
    }

    pub fn remove_channel(&self, chat_id: i64) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM channels WHERE chat_id = ?1", [chat_id])?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn list_channels(&self) -> Result<Vec<ChannelInfo>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT chat_id, chat_type, title, last_seen_at
             FROM channels
             ORDER BY last_seen_at DESC",
        )?;
        let channels = stmt
            .query_map([], |row| {
                Ok(ChannelInfo {
                    chat_id: row.get(0)?,
                    chat_type: row.get(1)?,
                    title: row.get(2)?,
                    last_seen_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(channels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_list_channels() {
        let db = Database::open_in_memory().unwrap();

        db.upsert_channel(-100123, "supergroup", Some("engineering"))
            .unwrap();
        db.upsert_channel(789, "private", None).unwrap();

        let channels = db.list_channels().unwrap();
        assert_eq!(channels.len(), 2);

        let eng = channels.iter().find(|c| c.chat_id == -100123).unwrap();
        assert_eq!(eng.chat_type, "supergroup");
        assert_eq!(eng.title.as_deref(), Some("engineering"));

        let dm = channels.iter().find(|c| c.chat_id == 789).unwrap();
        assert_eq!(dm.chat_type, "private");
        assert!(dm.title.is_none());
    }

    #[test]
    fn test_upsert_updates_metadata() {
        let db = Database::open_in_memory().unwrap();

        db.upsert_channel(-100123, "group", Some("old title"))
            .unwrap();
        db.upsert_channel(-100123, "supergroup", Some("new title"))
            .unwrap();

        let channels = db.list_channels().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].chat_type, "supergroup");
        assert_eq!(channels[0].title.as_deref(), Some("new title"));
    }

    #[test]
    fn test_upsert_preserves_title_when_null() {
        let db = Database::open_in_memory().unwrap();

        db.upsert_channel(-100123, "supergroup", Some("engineering"))
            .unwrap();
        // upsert with no title should keep existing title
        db.upsert_channel(-100123, "supergroup", None).unwrap();

        let channels = db.list_channels().unwrap();
        assert_eq!(channels[0].title.as_deref(), Some("engineering"));
    }

    #[test]
    fn test_remove_channel() {
        let db = Database::open_in_memory().unwrap();

        db.upsert_channel(-100123, "supergroup", Some("eng"))
            .unwrap();
        db.upsert_channel(-100456, "supergroup", Some("ops"))
            .unwrap();

        db.remove_channel(-100123).unwrap();

        let channels = db.list_channels().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].chat_id, -100456);
    }

    #[test]
    fn test_remove_nonexistent_channel() {
        let db = Database::open_in_memory().unwrap();
        // should not error
        db.remove_channel(-999).unwrap();
    }
}
