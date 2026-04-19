//! resolve secrets from skill .env.op files at startup.
//!
//! scans ~/.ava/skills/*/.env.op for 1Password secret references,
//! resolves them via `op run`, and sets the values as env vars on the
//! current process. this must run before daemonization so Touch ID
//! can prompt the user.

use std::path::PathBuf;

use crate::config;

/// find all .env.op files in skill directories.
fn find_env_op_files() -> Vec<PathBuf> {
    let skills_dir = config::ava_home_dir().join("skills");
    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let env_op = path.join(".env.op");
            if env_op.is_file() {
                files.push(env_op);
            }
        }
    }
    files
}

/// parse a .env.op file and return (key, op_reference) pairs.
/// only includes lines with op:// references.
fn parse_env_op(path: &PathBuf) -> Vec<(String, String)> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("warning: failed to read {}: {e}", path.display());
            return Vec::new();
        }
    };

    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            let value = value.trim();
            if value.starts_with("op://") {
                Some((key.to_string(), value.to_string()))
            } else {
                None
            }
        })
        .collect()
}

/// resolve all skill secrets from .env.op files via `op run`.
/// sets resolved values as env vars on the current process.
/// returns the number of secrets resolved, or an error message.
///
/// must be called before daemonization (needs terminal for Touch ID).
pub fn inject_secrets() -> Result<usize, String> {
    let env_op_files = find_env_op_files();
    if env_op_files.is_empty() {
        return Ok(0);
    }

    // merge all .env.op files into one temporary file for a single op run invocation
    let mut all_entries = Vec::new();
    for file in &env_op_files {
        all_entries.extend(parse_env_op(file));
    }

    if all_entries.is_empty() {
        return Ok(0);
    }

    // skip if all secrets are already in the environment (e.g. inherited from parent
    // process after an exec restart). avoids a failing `op run` when there's no terminal.
    if all_entries.iter().all(|(k, _)| std::env::var(k).is_ok()) {
        return Ok(0);
    }

    // write a merged .env.op to a temp file
    let tmp_dir = std::env::temp_dir();
    let merged_path = tmp_dir.join("ava-secrets.env.op");
    let merged_content: String = all_entries
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    std::fs::write(&merged_path, &merged_content)
        .map_err(|e| format!("failed to write temp env file: {e}"))?;

    // run `op run --env-file=<merged> -- env` to resolve all secrets at once
    let output = std::process::Command::new("op")
        .args([
            "run",
            &format!("--env-file={}", merged_path.display()),
            "--",
            "env",
        ])
        .output()
        .map_err(|e| format!("failed to run `op`: {e}. is the 1Password CLI installed?"))?;

    // clean up temp file
    let _ = std::fs::remove_file(&merged_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("op run failed: {stderr}"));
    }

    // parse the env output and set matching vars
    let env_output = String::from_utf8_lossy(&output.stdout);
    let keys: std::collections::HashSet<&str> =
        all_entries.iter().map(|(k, _)| k.as_str()).collect();
    let mut count = 0;

    for line in env_output.lines() {
        if let Some((key, value)) = line.split_once('=')
            && keys.contains(key)
        {
            // SAFETY: single-threaded at this point (before tokio runtime)
            unsafe { std::env::set_var(key, value) };
            count += 1;
        }
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_env_op() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".env.op");
        std::fs::write(
            &file,
            "# comment\nGMAIL_CLIENT_ID=op://Personal/abc/client_id\nPLAIN_VAR=just_a_value\n",
        )
        .unwrap();

        let entries = parse_env_op(&file);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "GMAIL_CLIENT_ID");
        assert_eq!(entries[0].1, "op://Personal/abc/client_id");
    }

    #[test]
    fn test_parse_env_op_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".env.op");
        std::fs::write(&file, "# only comments\n\n").unwrap();

        let entries = parse_env_op(&file);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_find_env_op_files() {
        let _guard = crate::config::ENV_TEST_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        std::fs::create_dir_all(skills.join("gmail")).unwrap();
        std::fs::write(skills.join("gmail/.env.op"), "KEY=op://x").unwrap();
        std::fs::create_dir_all(skills.join("other")).unwrap();
        // no .env.op in "other"

        // temporarily override AVA_HOME
        unsafe { std::env::set_var("AVA_HOME", dir.path().to_str().unwrap()) };
        let files = find_env_op_files();
        unsafe { std::env::remove_var("AVA_HOME") };

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("gmail/.env.op"));
    }
}
