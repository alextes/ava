use std::path::PathBuf;
use std::sync::OnceLock;

static WORKSPACE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// initialize the workspace root from `AVA_WORKSPACE` env var or cwd.
/// must be called once at startup before any workspace checks.
pub fn init_workspace_root() {
    let raw = if let Ok(path) = std::env::var("AVA_WORKSPACE") {
        PathBuf::from(path)
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    let canonical = raw.canonicalize().unwrap_or(raw);
    WORKSPACE_ROOT.set(canonical).ok();
}

/// returns the workspace root. panics if `init_workspace_root()` was not called.
pub fn workspace_root() -> &'static PathBuf {
    WORKSPACE_ROOT
        .get()
        .expect("workspace root not initialized — call init_workspace_root() first")
}

/// returns path to the ava home directory (~/.ava/).
/// override with AVA_HOME env var.
pub fn ava_home_dir() -> PathBuf {
    if let Ok(path) = std::env::var("AVA_HOME") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ava")
}

/// returns path to the vault directory (~/.ava/vault/).
pub fn vault_dir() -> PathBuf {
    ava_home_dir().join("vault")
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

/// check whether a process with the given PID is alive.
/// uses kill(pid, 0) on unix — sends no signal, just checks existence.
#[cfg(unix)]
#[allow(dead_code)]
pub fn check_process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 doesn't actually send a signal, just checks if the process exists.
    // returns 0 on success (process exists), -1 on error.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
pub fn check_process_alive(_pid: u32) -> bool {
    todo!("process liveness check not implemented for this platform")
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

/// shared lock for tests that mutate process-global env vars.
///
/// cargo runs tests in parallel by default, and `std::env::set_var` /
/// `remove_var` are process-wide — so two tests racing on `AVA_HOME`,
/// `AVA_DB_PATH`, etc. can clobber each other's state. every test that
/// touches an env var must take this lock for the duration of the
/// set → read → unset block.
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_db_path_from_env() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();

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
        let _guard = ENV_TEST_LOCK.lock().unwrap();

        // SAFETY: we hold ENV_MUTEX to ensure no concurrent env var access
        unsafe {
            std::env::remove_var("AVA_DB_PATH");
        }

        let result = default_db_path();

        // should be ava.db in current directory
        assert_eq!(result, PathBuf::from("ava.db"));
    }
}
