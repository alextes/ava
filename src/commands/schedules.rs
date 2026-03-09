use crate::db::Database;
use crate::error;

pub(crate) fn run_schedules() -> Result<(), error::Error> {
    let db = Database::open()?;
    let schedules = db.list_schedules()?;
    if schedules.is_empty() {
        println!("no active schedules");
    } else {
        for s in schedules {
            let kind = match &s.cron_expr {
                Some(expr) => format!("recurring ({expr})"),
                None => "one-time".to_string(),
            };
            println!(
                "id={}: {} [{}] next={} | {}",
                s.id, s.description, kind, s.next_run_at, s.prompt
            );
        }
    }
    Ok(())
}
