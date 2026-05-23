use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::Error;

use super::Database;

const LAST_RUNTIME_EVENT_KEY: &str = "last_runtime_event";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub source: String,
    pub reason: String,
    pub occurred_at: String,
}

impl RuntimeEvent {
    pub fn is_restart(&self) -> bool {
        matches!(self.source.as_str(), "cli_restart" | "self_upgrade")
    }
}

impl Database {
    pub fn set_app_state(&self, key: &str, value: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO app_state (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn app_state(&self, key: &str) -> Result<Option<String>, Error> {
        let conn = self.conn.lock().unwrap();
        let value = conn
            .query_row("SELECT value FROM app_state WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(value)
    }

    pub fn record_runtime_event(&self, source: &str, reason: &str) -> Result<(), Error> {
        let event = RuntimeEvent {
            source: source.to_string(),
            reason: reason.to_string(),
            occurred_at: chrono::Utc::now().to_rfc3339(),
        };
        let json = serde_json::to_string(&event)
            .map_err(|e| Error::Provider(format!("failed to serialize runtime event: {e}")))?;
        self.set_app_state(LAST_RUNTIME_EVENT_KEY, &json)
    }

    pub fn last_runtime_event(&self) -> Result<Option<RuntimeEvent>, Error> {
        let Some(json) = self.app_state(LAST_RUNTIME_EVENT_KEY)? else {
            return Ok(None);
        };
        let event = serde_json::from_str(&json)
            .map_err(|e| Error::Provider(format!("failed to deserialize runtime event: {e}")))?;
        Ok(Some(event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state_round_trip() {
        let db = Database::open_in_memory().unwrap();

        assert_eq!(db.app_state("missing").unwrap(), None);

        db.set_app_state("hello", "world").unwrap();
        assert_eq!(db.app_state("hello").unwrap().as_deref(), Some("world"));

        db.set_app_state("hello", "again").unwrap();
        assert_eq!(db.app_state("hello").unwrap().as_deref(), Some("again"));
    }

    #[test]
    fn test_runtime_event_round_trip() {
        let db = Database::open_in_memory().unwrap();

        db.record_runtime_event("cli_restart", "ava restart")
            .unwrap();
        let event = db.last_runtime_event().unwrap().unwrap();

        assert_eq!(event.source, "cli_restart");
        assert_eq!(event.reason, "ava restart");
        assert!(event.is_restart());
        assert!(!event.occurred_at.is_empty());
    }
}
