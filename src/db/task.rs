use crate::error::Error;

use super::Database;

pub struct Task {
    pub id: i64,
    pub title: String,
    pub detail: Option<String>,
    pub status: String,
    pub created_at: String,
    pub completed_at: Option<String>,
}

impl Database {
    pub fn add_task(&self, title: &str, detail: Option<&str>) -> Result<i64, Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks (title, detail) VALUES (?1, ?2)",
            rusqlite::params![title, detail],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_tasks(&self, include_done: bool) -> Result<Vec<Task>, Error> {
        let conn = self.conn.lock().unwrap();
        let query = if include_done {
            "SELECT id, title, detail, status, created_at, completed_at
             FROM tasks ORDER BY created_at ASC"
        } else {
            "SELECT id, title, detail, status, created_at, completed_at
             FROM tasks WHERE status = 'pending' ORDER BY created_at ASC"
        };
        let mut stmt = conn.prepare(query)?;
        let rows = stmt.query_map([], |row| {
            Ok(Task {
                id: row.get(0)?,
                title: row.get(1)?,
                detail: row.get(2)?,
                status: row.get(3)?,
                created_at: row.get(4)?,
                completed_at: row.get(5)?,
            })
        })?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }

    pub fn get_task(&self, id: i64) -> Result<Option<Task>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, detail, status, created_at, completed_at
             FROM tasks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], |row| {
            Ok(Task {
                id: row.get(0)?,
                title: row.get(1)?,
                detail: row.get(2)?,
                status: row.get(3)?,
                created_at: row.get(4)?,
                completed_at: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn complete_task(&self, id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "UPDATE tasks SET status = 'done', completed_at = datetime('now') WHERE id = ?1 AND status = 'pending'",
            [id],
        )?;
        Ok(affected > 0)
    }

    /// return pending task titles for system prompt injection
    pub fn pending_task_titles(&self) -> Result<Vec<(i64, String)>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title FROM tasks WHERE status = 'pending' ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut titles = Vec::new();
        for row in rows {
            titles.push(row?);
        }
        Ok(titles)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Database;

    #[test]
    fn test_add_and_list_tasks() {
        let db = Database::open_in_memory().unwrap();
        let id = db.add_task("fix CI", Some("the build is broken")).unwrap();
        assert!(id > 0);

        let tasks = db.list_tasks(false).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "fix CI");
        assert_eq!(tasks[0].detail.as_deref(), Some("the build is broken"));
        assert_eq!(tasks[0].status, "pending");
        assert!(tasks[0].completed_at.is_none());
    }

    #[test]
    fn test_add_task_without_detail() {
        let db = Database::open_in_memory().unwrap();
        let id = db.add_task("check logs", None).unwrap();
        assert!(id > 0);

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.title, "check logs");
        assert!(task.detail.is_none());
    }

    #[test]
    fn test_get_task() {
        let db = Database::open_in_memory().unwrap();
        let id = db.add_task("review PR", Some("PR #42")).unwrap();

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.id, id);
        assert_eq!(task.title, "review PR");
        assert_eq!(task.detail.as_deref(), Some("PR #42"));
    }

    #[test]
    fn test_get_task_not_found() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_task(999).unwrap().is_none());
    }

    #[test]
    fn test_complete_task() {
        let db = Database::open_in_memory().unwrap();
        let id = db.add_task("deploy", None).unwrap();

        assert!(db.complete_task(id).unwrap());

        // no longer in pending list
        let pending = db.list_tasks(false).unwrap();
        assert!(pending.is_empty());

        // visible in full list
        let all = db.list_tasks(true).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, "done");
        assert!(all[0].completed_at.is_some());
    }

    #[test]
    fn test_complete_task_already_done() {
        let db = Database::open_in_memory().unwrap();
        let id = db.add_task("deploy", None).unwrap();

        assert!(db.complete_task(id).unwrap());
        assert!(!db.complete_task(id).unwrap());
    }

    #[test]
    fn test_complete_task_not_found() {
        let db = Database::open_in_memory().unwrap();
        assert!(!db.complete_task(999).unwrap());
    }

    #[test]
    fn test_pending_task_titles() {
        let db = Database::open_in_memory().unwrap();
        db.add_task("task one", None).unwrap();
        let id2 = db.add_task("task two", None).unwrap();
        db.add_task("task three", None).unwrap();

        // complete one
        db.complete_task(id2).unwrap();

        let titles = db.pending_task_titles().unwrap();
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0].1, "task one");
        assert_eq!(titles[1].1, "task three");
    }

    #[test]
    fn test_pending_task_titles_empty() {
        let db = Database::open_in_memory().unwrap();
        let titles = db.pending_task_titles().unwrap();
        assert!(titles.is_empty());
    }

    #[test]
    fn test_list_tasks_include_done() {
        let db = Database::open_in_memory().unwrap();
        let id1 = db.add_task("done task", None).unwrap();
        db.add_task("pending task", None).unwrap();
        db.complete_task(id1).unwrap();

        let pending = db.list_tasks(false).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "pending task");

        let all = db.list_tasks(true).unwrap();
        assert_eq!(all.len(), 2);
    }
}
