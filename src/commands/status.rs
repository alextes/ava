use crate::config;
use crate::db::Database;

pub(crate) fn run_status() {
    println!("ava {}", env!("CARGO_PKG_VERSION"));
    println!("db: {}", config::default_db_path().display());
    if let Ok(db) = Database::open()
        && let Ok(sid) = db.active_session()
    {
        let msg_count = db.session_message_count(sid).unwrap_or(0);
        println!("session: {sid} ({msg_count} messages)");
        match db.session_usage(sid) {
            Ok(Some((input_tokens, context_window))) if context_window > 0 => {
                let pct = input_tokens as f64 / context_window as f64 * 100.0;
                println!("context: {pct:.0}% ({input_tokens}/{context_window} tokens)");
            }
            _ => {
                println!("context: unknown");
            }
        }
        if let Ok(Some((model, reasoning))) = db.session_model_reasoning(sid) {
            match reasoning {
                Some(effort) => println!("model: {model} (reasoning: {effort})"),
                None => println!("model: {model}"),
            }
        }
    }
}
