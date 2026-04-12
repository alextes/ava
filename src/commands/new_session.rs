use crate::db::Database;

pub(crate) fn run_new_session() {
    match Database::open() {
        Ok(db) => match db.new_session() {
            Ok(id) => println!("started new session (id: {id})"),
            Err(e) => {
                eprintln!("error: failed to create new session: {e}");
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("error: failed to open database: {e}");
            std::process::exit(1);
        }
    }
}
