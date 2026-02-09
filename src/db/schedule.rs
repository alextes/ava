use crate::error::Error;

use super::Database;

#[allow(dead_code)]
pub struct Schedule {
    pub id: i64,
    pub description: String,
    pub prompt: String,
    pub cron_expr: Option<String>,
    pub next_run_at: String,
    pub last_run_at: Option<String>,
    pub active: bool,
    pub created_at: String,
}

impl Database {
    pub fn create_schedule(
        &self,
        description: &str,
        prompt: &str,
        next_run_at: &str,
        cron_expr: Option<&str>,
    ) -> Result<i64, Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO schedules (description, prompt, next_run_at, cron_expr) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![description, prompt, next_run_at, cron_expr],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_schedules(&self) -> Result<Vec<Schedule>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, description, prompt, cron_expr, next_run_at, last_run_at, active, created_at
             FROM schedules WHERE active = 1 ORDER BY next_run_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Schedule {
                id: row.get(0)?,
                description: row.get(1)?,
                prompt: row.get(2)?,
                cron_expr: row.get(3)?,
                next_run_at: row.get(4)?,
                last_run_at: row.get(5)?,
                active: row.get::<_, i32>(6)? != 0,
                created_at: row.get(7)?,
            })
        })?;
        let mut schedules = Vec::new();
        for row in rows {
            schedules.push(row?);
        }
        Ok(schedules)
    }

    pub fn cancel_schedule(&self, id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "UPDATE schedules SET active = 0 WHERE id = ?1 AND active = 1",
            [id],
        )?;
        Ok(affected > 0)
    }

    pub fn due_schedules(&self) -> Result<Vec<Schedule>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, description, prompt, cron_expr, next_run_at, last_run_at, active, created_at
             FROM schedules WHERE active = 1 AND next_run_at <= datetime('now')
             ORDER BY next_run_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Schedule {
                id: row.get(0)?,
                description: row.get(1)?,
                prompt: row.get(2)?,
                cron_expr: row.get(3)?,
                next_run_at: row.get(4)?,
                last_run_at: row.get(5)?,
                active: row.get::<_, i32>(6)? != 0,
                created_at: row.get(7)?,
            })
        })?;
        let mut schedules = Vec::new();
        for row in rows {
            schedules.push(row?);
        }
        Ok(schedules)
    }

    pub fn advance_schedule(&self, id: i64, next_run_at: Option<&str>) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        match next_run_at {
            Some(next) => {
                conn.execute(
                    "UPDATE schedules SET last_run_at = datetime('now'), next_run_at = ?1 WHERE id = ?2",
                    rusqlite::params![next, id],
                )?;
            }
            None => {
                conn.execute(
                    "UPDATE schedules SET last_run_at = datetime('now'), active = 0 WHERE id = ?1",
                    [id],
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Database;

    #[test]
    fn test_create_and_list_schedule() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .create_schedule(
                "morning check",
                "good morning!",
                "2099-01-01 07:30:00",
                None,
            )
            .unwrap();
        assert!(id > 0);

        let schedules = db.list_schedules().unwrap();
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].description, "morning check");
        assert_eq!(schedules[0].prompt, "good morning!");
        assert_eq!(schedules[0].next_run_at, "2099-01-01 07:30:00");
        assert!(schedules[0].cron_expr.is_none());
        assert!(schedules[0].active);
    }

    #[test]
    fn test_cancel_schedule() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .create_schedule("test", "prompt", "2099-01-01 00:00:00", None)
            .unwrap();

        assert!(db.cancel_schedule(id).unwrap());
        assert!(db.list_schedules().unwrap().is_empty());

        // canceling again returns false
        assert!(!db.cancel_schedule(id).unwrap());
    }

    #[test]
    fn test_due_schedules() {
        let db = Database::open_in_memory().unwrap();
        // create schedule with past next_run_at
        db.create_schedule("overdue", "do it", "2000-01-01 00:00:00", None)
            .unwrap();

        let due = db.due_schedules().unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].description, "overdue");
    }

    #[test]
    fn test_due_schedules_skips_future() {
        let db = Database::open_in_memory().unwrap();
        db.create_schedule("future", "later", "2099-12-31 23:59:59", None)
            .unwrap();

        let due = db.due_schedules().unwrap();
        assert!(due.is_empty());
    }

    #[test]
    fn test_advance_schedule_recurring() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .create_schedule(
                "recurring",
                "do it",
                "2000-01-01 00:00:00",
                Some("30 7 * * *"),
            )
            .unwrap();

        db.advance_schedule(id, Some("2099-01-02 07:30:00"))
            .unwrap();

        let schedules = db.list_schedules().unwrap();
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].next_run_at, "2099-01-02 07:30:00");
        assert!(schedules[0].last_run_at.is_some());
        assert!(schedules[0].active);
    }

    #[test]
    fn test_advance_schedule_one_time() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .create_schedule("one-time", "do it once", "2000-01-01 00:00:00", None)
            .unwrap();

        db.advance_schedule(id, None).unwrap();

        // should no longer appear in active list
        assert!(db.list_schedules().unwrap().is_empty());
    }
}
