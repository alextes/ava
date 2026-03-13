use std::path::PathBuf;

/// returns path to the ava home directory (~/.ava/).
/// override with AVA_HOME env var.
pub fn ava_home_dir() -> PathBuf {
    if let Ok(path) = std::env::var("AVA_HOME") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ava")
}

/// returns path to the PID file (~/.ava/ava.pid).
pub fn pid_file_path() -> PathBuf {
    ava_home_dir().join("ava.pid")
}

/// write the current process PID to the PID file.
pub fn write_pid_file() {
    let path = pid_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, std::process::id().to_string()) {
        tracing::warn!("failed to write PID file {}: {e}", path.display());
    }
}

/// read the PID from the PID file, if it exists.
pub fn read_pid_file() -> Option<u32> {
    let path = pid_file_path();
    let contents = std::fs::read_to_string(&path).ok()?;
    contents.trim().parse().ok()
}

/// remove the PID file.
pub fn remove_pid_file() {
    let _ = std::fs::remove_file(pid_file_path());
}

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
