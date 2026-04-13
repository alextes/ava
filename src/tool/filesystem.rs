use std::fs;
use std::path::{Component, PathBuf};

use serde::Deserialize;

use super::ToolCall;
use super::exec::truncate_output;

pub const TEXT_EDITOR_TOOL_NAME: &str = "str_replace_based_edit_tool";

#[derive(Debug, Deserialize)]
struct TextEditorInput {
    command: String,
    path: String,
    // view
    view_range: Option<[i64; 2]>,
    // str_replace
    old_str: Option<String>,
    new_str: Option<String>,
    // create
    file_text: Option<String>,
    // insert
    insert_line: Option<usize>,
    insert_text: Option<String>,
}

pub(super) async fn handle_text_editor(call: &ToolCall) -> String {
    let input: TextEditorInput = match serde_json::from_value(call.input.clone()) {
        Ok(i) => i,
        Err(err) => return format!("invalid input: {err}"),
    };

    match input.command.as_str() {
        "view" => cmd_view(&input.path, input.view_range),
        "str_replace" => cmd_str_replace(&input.path, input.old_str, input.new_str),
        "create" => cmd_create(&input.path, input.file_text),
        "insert" => cmd_insert(&input.path, input.insert_line, input.insert_text),
        other => format!("unknown command: {other}"),
    }
}

/// resolve path relative to cwd, canonicalizing `..` components.
/// the approval system (requires_approval + check_vault_deny) handles security
/// for out-of-workspace and vault paths, so we don't need to reject `..` here.
pub(super) fn validate_path(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    let resolved = if p.is_absolute() {
        p
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to get cwd: {e}"))?
            .join(&p)
    };

    // canonicalize .. components by building a normalized path.
    // we don't use std::fs::canonicalize because the path may not exist yet (create).
    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component),
        }
    }

    Ok(normalized)
}

/// resolve path and verify the file exists on disk
pub(super) fn validate_existing_path(path: &str) -> Result<PathBuf, String> {
    let resolved = validate_path(path)?;
    if !resolved.exists() {
        return Err(format!("path does not exist: {}", resolved.display()));
    }
    Ok(resolved)
}

fn cmd_view(path: &str, view_range: Option<[i64; 2]>) -> String {
    let resolved = match validate_existing_path(path) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if resolved.is_dir() {
        return view_directory(&resolved);
    }

    let content = match fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return format!("failed to read file: {e}"),
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let (start, end) = match view_range {
        Some([s, e]) => {
            let start = if s < 1 {
                0
            } else {
                (s as usize).saturating_sub(1)
            };
            let end = if e < 0 {
                total
            } else {
                (e as usize).min(total)
            };
            if start >= total {
                return format!("view_range start ({s}) is past end of file ({total} lines)");
            }
            if start >= end {
                return format!("view_range [{s}, {e}] is invalid (start must be less than end)");
            }
            (start, end)
        }
        None => (0, total),
    };

    let mut output = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        let line_num = start + i + 1;
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!("{line_num}: {line}"));
    }

    truncate_output(&output)
}

fn view_directory(path: &PathBuf) -> String {
    let mut entries = Vec::new();
    let read_dir = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => return format!("failed to read directory: {e}"),
    };
    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => return format!("failed to read directory entry: {e}"),
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            entries.push(format!("{name}/"));
        } else {
            entries.push(name);
        }
    }
    entries.sort();

    if entries.is_empty() {
        return "(empty directory)".to_string();
    }

    entries.join("\n")
}

fn cmd_str_replace(path: &str, old_str: Option<String>, new_str: Option<String>) -> String {
    let old_str = match old_str {
        Some(s) => s,
        None => return "str_replace requires old_str".to_string(),
    };
    let new_str = match new_str {
        Some(s) => s,
        None => return "str_replace requires new_str".to_string(),
    };

    let resolved = match validate_existing_path(path) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let content = match fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return format!("failed to read file: {e}"),
    };

    let count = content.matches(&*old_str).count();
    match count {
        0 => "no match found for old_str".to_string(),
        1 => {
            let new_content = content.replacen(&*old_str, &new_str, 1);
            match fs::write(&resolved, &new_content) {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("failed to write file: {e}"),
            }
        }
        n => format!("found {n} matches for old_str, expected exactly 1"),
    }
}

fn cmd_create(path: &str, file_text: Option<String>) -> String {
    let file_text = match file_text {
        Some(t) => t,
        None => return "create requires file_text".to_string(),
    };

    let resolved = match validate_path(path) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if resolved.exists() {
        return format!("file already exists: {}", resolved.display());
    }

    // create parent directories if needed
    if let Some(parent) = resolved.parent()
        && !parent.exists()
        && let Err(e) = fs::create_dir_all(parent)
    {
        return format!("failed to create parent directories: {e}");
    }

    match fs::write(&resolved, &file_text) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("failed to write file: {e}"),
    }
}

fn cmd_insert(path: &str, insert_line: Option<usize>, insert_text: Option<String>) -> String {
    let insert_line = match insert_line {
        Some(l) => l,
        None => return "insert requires insert_line".to_string(),
    };
    let insert_text = match insert_text {
        Some(t) => t,
        None => return "insert requires insert_text".to_string(),
    };

    let resolved = match validate_existing_path(path) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let content = match fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return format!("failed to read file: {e}"),
    };

    let mut lines: Vec<&str> = content.lines().collect();
    let insert_lines: Vec<&str> = insert_text.lines().collect();

    let pos = insert_line.min(lines.len());
    for (i, line) in insert_lines.iter().enumerate() {
        lines.insert(pos + i, line);
    }

    let new_content = if content.ends_with('\n') {
        format!("{}\n", lines.join("\n"))
    } else {
        lines.join("\n")
    };

    match fs::write(&resolved, &new_content) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("failed to write file: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_cmd_view_file() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "hello.txt", "line one\nline two\nline three\n");

        let result = cmd_view(path.to_str().unwrap(), None);
        assert!(result.contains("1: line one"));
        assert!(result.contains("2: line two"));
        assert!(result.contains("3: line three"));
    }

    #[test]
    fn test_cmd_view_directory() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "a.txt", "");
        temp_file(&dir, "b.txt", "");
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let result = cmd_view(dir.path().to_str().unwrap(), None);
        assert!(result.contains("a.txt"));
        assert!(result.contains("b.txt"));
        assert!(result.contains("subdir/"));
    }

    #[test]
    fn test_cmd_view_range() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(
            &dir,
            "range.txt",
            "line 1\nline 2\nline 3\nline 4\nline 5\n",
        );

        let result = cmd_view(path.to_str().unwrap(), Some([2, 4]));
        assert!(!result.contains("1: line 1"));
        assert!(result.contains("2: line 2"));
        assert!(result.contains("3: line 3"));
        assert!(result.contains("4: line 4"));
        assert!(!result.contains("5: line 5"));
    }

    #[test]
    fn test_cmd_view_file_not_found() {
        let result = cmd_view("/tmp/nonexistent_ava_test_file_12345.txt", None);
        assert!(result.contains("does not exist"));
    }

    #[test]
    fn test_cmd_str_replace_single_match() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "replace.txt", "hello world\nfoo bar\n");

        let result = cmd_str_replace(
            path.to_str().unwrap(),
            Some("foo bar".into()),
            Some("baz qux".into()),
        );
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("baz qux"));
        assert!(!content.contains("foo bar"));
    }

    #[test]
    fn test_cmd_str_replace_no_match() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "nope.txt", "hello world\n");

        let result = cmd_str_replace(
            path.to_str().unwrap(),
            Some("not here".into()),
            Some("replacement".into()),
        );
        assert!(result.contains("no match found"));
    }

    #[test]
    fn test_cmd_str_replace_multiple_matches() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "multi.txt", "aaa\naaa\n");

        let result = cmd_str_replace(
            path.to_str().unwrap(),
            Some("aaa".into()),
            Some("bbb".into()),
        );
        assert!(result.contains("found 2 matches"));
    }

    #[test]
    fn test_cmd_create_new_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.txt");

        let result = cmd_create(path.to_str().unwrap(), Some("hello\n".into()));
        assert_eq!(result, "ok");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn test_cmd_create_already_exists() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "exists.txt", "content");

        let result = cmd_create(path.to_str().unwrap(), Some("new content".into()));
        assert!(result.contains("already exists"));
    }

    #[test]
    fn test_cmd_insert_middle() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "insert.txt", "line 1\nline 3\n");

        let result = cmd_insert(path.to_str().unwrap(), Some(1), Some("line 2".into()));
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines, vec!["line 1", "line 2", "line 3"]);
    }

    #[test]
    fn test_cmd_insert_beginning() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "insert0.txt", "line 2\nline 3\n");

        let result = cmd_insert(path.to_str().unwrap(), Some(0), Some("line 1".into()));
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines, vec!["line 1", "line 2", "line 3"]);
    }

    #[test]
    fn test_validate_path_resolves_traversal() {
        let result = validate_path("/tmp/../etc/passwd");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn test_validate_path_resolves_relative_traversal() {
        // ../foo from /a/b/c should resolve to /a/b/foo
        let result = validate_path("/a/b/c/../foo");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("/a/b/foo"));
    }
}
