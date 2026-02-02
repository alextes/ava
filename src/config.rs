use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::Error;

pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "ava")
}

pub fn data_dir() -> Result<PathBuf, Error> {
    let dirs = project_dirs().ok_or(Error::NoHomeDirectory)?;
    Ok(dirs.data_dir().to_path_buf())
}

pub fn default_db_path() -> Result<PathBuf, Error> {
    if let Ok(path) = std::env::var("AVA_DB_PATH") {
        return Ok(PathBuf::from(path));
    }
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("ava.db"))
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

        let result = default_db_path().unwrap();
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

        let result = default_db_path().unwrap();

        // should end with ava.db in the data directory
        assert!(result.ends_with("ava.db"));
        // should be an absolute path
        assert!(result.is_absolute());
    }
}
