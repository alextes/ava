use std::path::PathBuf;

/// returns path to the sqlite database.
/// defaults to ./ava.db in the current directory.
/// override with AVA_DB_PATH env var.
pub fn default_db_path() -> PathBuf {
    if let Ok(path) = std::env::var("AVA_DB_PATH") {
        return PathBuf::from(path);
    }
    PathBuf::from("ava.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // mutex to serialize tests that modify env vars
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_default_db_path_from_env() {
        let _guard = ENV_MUTEX.lock().unwrap();

        let test_path = "/custom/path/to/db.sqlite";
        // SAFETY: we hold ENV_MUTEX to ensure no concurrent env var access
        unsafe {
            std::env::set_var("AVA_DB_PATH", test_path);
        }

        let result = default_db_path();
        assert_eq!(result, PathBuf::from(test_path));

        // SAFETY: we hold ENV_MUTEX to ensure no concurrent env var access
        unsafe {
            std::env::remove_var("AVA_DB_PATH");
        }
    }

    #[test]
    fn test_default_db_path_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap();

        // SAFETY: we hold ENV_MUTEX to ensure no concurrent env var access
        unsafe {
            std::env::remove_var("AVA_DB_PATH");
        }

        let result = default_db_path();

        // should be ava.db in current directory
        assert_eq!(result, PathBuf::from("ava.db"));
    }
}
