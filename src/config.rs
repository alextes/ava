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
