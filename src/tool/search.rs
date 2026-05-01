use std::path::Path;
use std::time::SystemTime;

use globset::Glob;
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::json;

use super::exec::truncate_output;
use super::{ToolCall, ToolDefinition};

pub const GREP_TOOL_NAME: &str = "grep";
pub const GLOB_TOOL_NAME: &str = "glob";

const DEFAULT_MAX_RESULTS: u64 = 50;
const MAX_MAX_RESULTS: u64 = 200;
const DEFAULT_CONTEXT_LINES: u64 = 0;
const MAX_CONTEXT_LINES: u64 = 10;

// --- grep ---

#[derive(Debug, Deserialize)]
struct GrepInput {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    context_lines: Option<u64>,
    max_results: Option<u64>,
    case_insensitive: Option<bool>,
}

pub(super) fn grep_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: GREP_TOOL_NAME,
        description: "search file contents using regex. powered by ripgrep. respects .gitignore.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "file or directory to search in (default: working directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "filter files by glob pattern, e.g. '*.rs'"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "number of context lines before and after each match (default: 0, max: 10)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "maximum number of matching lines to return (default: 50, max: 200)"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "case insensitive search (default: false)"
                }
            },
            "required": ["pattern"]
        }),
    }
}

pub(super) async fn handle_grep(call: &ToolCall) -> String {
    match serde_json::from_value::<GrepInput>(call.input.clone()) {
        Ok(input) => execute_grep(&input).await,
        Err(err) => format!("invalid input: {err}"),
    }
}

async fn execute_grep(input: &GrepInput) -> String {
    let context = input
        .context_lines
        .unwrap_or(DEFAULT_CONTEXT_LINES)
        .min(MAX_CONTEXT_LINES);
    let max_results = input
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(MAX_MAX_RESULTS);

    let mut args = vec![
        "--json".to_string(),
        "--line-number".to_string(),
        format!("-m{max_results}"),
    ];

    if context > 0 {
        args.push(format!("-C{context}"));
    }

    if input.case_insensitive.unwrap_or(false) {
        args.push("-i".to_string());
    }

    if let Some(ref glob) = input.glob {
        args.push("-g".to_string());
        args.push(glob.clone());
    }

    args.push("--".to_string());
    args.push(input.pattern.clone());

    if let Some(ref path) = input.path {
        args.push(path.clone());
    }

    let result = tokio::process::Command::new("rg")
        .args(&args)
        .output()
        .await;

    match result {
        Ok(output) => {
            if !output.status.success() && output.stdout.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.is_empty() || output.status.code() == Some(1) {
                    return "no matches found".to_string();
                }
                return format!("grep error: {stderr}");
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let formatted = format_rg_json(&stdout);
            truncate_output(&formatted)
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                "rg not found — install ripgrep: cargo install ripgrep".to_string()
            } else {
                format!("failed to execute rg: {e}")
            }
        }
    }
}

/// parse ripgrep JSON lines output into a readable format
fn format_rg_json(json_output: &str) -> String {
    let mut output = String::new();
    let mut current_file: Option<String> = None;
    let mut match_count: u64 = 0;

    for line in json_output.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        let msg_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        match msg_type {
            "match" | "context" => {
                let data = match value.get("data") {
                    Some(d) => d,
                    None => continue,
                };

                let path = data
                    .get("path")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("?");

                let line_number = data
                    .get("line_number")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);

                let text = data
                    .get("lines")
                    .and_then(|l| l.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");

                // print file header when file changes
                if current_file.as_deref() != Some(path) {
                    if current_file.is_some() {
                        output.push('\n');
                    }
                    output.push_str(path);
                    output.push_str(":\n");
                    current_file = Some(path.to_string());
                }

                let text = text.trim_end_matches('\n');
                output.push_str(&format!("  {line_number}: {text}\n"));

                if msg_type == "match" {
                    match_count += 1;
                }
            }
            "summary" => {
                let stats = value.get("data").and_then(|d| d.get("stats"));
                if let Some(stats) = stats {
                    let total = stats.get("matches").and_then(|m| m.as_u64()).unwrap_or(0);
                    if total > match_count {
                        output.push_str(&format!(
                            "\n({} matches shown, {} total)\n",
                            match_count, total
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    if output.is_empty() {
        "no matches found".to_string()
    } else {
        output
    }
}

// --- glob ---

#[derive(Debug, Deserialize)]
struct GlobInput {
    pattern: String,
    path: Option<String>,
}

pub(super) fn glob_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: GLOB_TOOL_NAME,
        description: "find files by name pattern. respects .gitignore. use this to discover files before reading them.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "glob pattern, e.g. '**/*.rs' or 'src/**/*.toml'"
                },
                "path": {
                    "type": "string",
                    "description": "directory to search in (default: working directory)"
                }
            },
            "required": ["pattern"]
        }),
    }
}

pub(super) fn handle_glob(call: &ToolCall) -> String {
    match serde_json::from_value::<GlobInput>(call.input.clone()) {
        Ok(input) => execute_glob(&input),
        Err(err) => format!("invalid input: {err}"),
    }
}

fn execute_glob(input: &GlobInput) -> String {
    let matcher = match Glob::new(&input.pattern) {
        Ok(g) => g.compile_matcher(),
        Err(e) => return format!("invalid glob pattern: {e}"),
    };

    let root = input.path.as_deref().unwrap_or(".");
    if !Path::new(root).is_dir() {
        return format!("directory not found: {root}");
    }

    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .build();

    let mut entries: Vec<(String, SystemTime)> = Vec::new();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        // skip directories
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            continue;
        }

        let path = entry.path();

        // match against the relative path from root
        let display_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if !matcher.is_match(&display_path) && !matcher.is_match(path) {
            continue;
        }

        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        entries.push((display_path, mtime));
    }

    if entries.is_empty() {
        return "no files found".to_string();
    }

    // sort newest first
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.1));

    let mut output = String::new();
    for (path, _) in &entries {
        output.push_str(path);
        output.push('\n');
    }

    truncate_output(&output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // --- glob tests ---

    #[test]
    fn test_glob_finds_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("foo.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("bar.rs"), "fn bar() {}").unwrap();
        fs::write(dir.path().join("baz.txt"), "hello").unwrap();

        let input = GlobInput {
            pattern: "*.rs".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
        };
        let result = execute_glob(&input);
        assert!(result.contains("foo.rs"), "expected foo.rs in: {result}");
        assert!(result.contains("bar.rs"), "expected bar.rs in: {result}");
        assert!(
            !result.contains("baz.txt"),
            "unexpected baz.txt in: {result}"
        );
    }

    #[test]
    fn test_glob_recursive() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(dir.path().join("top.rs"), "").unwrap();
        fs::write(sub.join("nested.rs"), "").unwrap();

        let input = GlobInput {
            pattern: "**/*.rs".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
        };
        let result = execute_glob(&input);
        assert!(result.contains("top.rs"), "expected top.rs in: {result}");
        assert!(
            result.contains("nested.rs"),
            "expected nested.rs in: {result}"
        );
    }

    #[test]
    fn test_glob_no_matches() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("foo.txt"), "hello").unwrap();

        let input = GlobInput {
            pattern: "*.rs".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
        };
        let result = execute_glob(&input);
        assert_eq!(result, "no files found");
    }

    #[test]
    fn test_glob_invalid_pattern() {
        let input = GlobInput {
            pattern: "[invalid".to_string(),
            path: None,
        };
        let result = execute_glob(&input);
        assert!(
            result.contains("invalid glob pattern"),
            "expected error but got: {result}"
        );
    }

    #[test]
    fn test_glob_nonexistent_directory() {
        let input = GlobInput {
            pattern: "*.rs".to_string(),
            path: Some("/nonexistent_dir_12345".to_string()),
        };
        let result = execute_glob(&input);
        assert!(
            result.contains("directory not found"),
            "expected error but got: {result}"
        );
    }

    #[test]
    fn test_glob_respects_gitignore() {
        let dir = TempDir::new().unwrap();

        // init a git repo so .gitignore is respected
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        fs::write(dir.path().join(".gitignore"), "ignored.rs\n").unwrap();
        fs::write(dir.path().join("kept.rs"), "").unwrap();
        fs::write(dir.path().join("ignored.rs"), "").unwrap();

        let input = GlobInput {
            pattern: "*.rs".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
        };
        let result = execute_glob(&input);
        assert!(result.contains("kept.rs"), "expected kept.rs in: {result}");
        assert!(
            !result.contains("ignored.rs"),
            "unexpected ignored.rs in: {result}"
        );
    }

    // --- grep tests ---

    #[test]
    fn test_format_rg_json_matches() {
        let json = r#"{"type":"begin","data":{"path":{"text":"src/main.rs"}}}
{"type":"match","data":{"path":{"text":"src/main.rs"},"lines":{"text":"fn main() {\n"},"line_number":1,"absolute_offset":0,"submatches":[{"match":{"text":"main"},"start":3,"end":7}]}}
{"type":"end","data":{"path":{"text":"src/main.rs"},"binary_offset":null,"stats":{"elapsed":{"secs":0,"nanos":100},"searches":1,"searches_with_match":1,"bytes_searched":100,"bytes_printed":200,"matched_lines":1,"matches":1}}}
{"type":"summary","data":{"elapsed_total":{"human":"0.001s","nanos":1000000},"stats":{"elapsed":{"secs":0,"nanos":100},"searches":1,"searches_with_match":1,"bytes_searched":100,"bytes_printed":200,"matched_lines":1,"matches":1}}}"#;

        let result = format_rg_json(json);
        assert!(
            result.contains("src/main.rs:"),
            "expected file header in: {result}"
        );
        assert!(
            result.contains("1: fn main() {"),
            "expected match line in: {result}"
        );
    }

    #[test]
    fn test_format_rg_json_multiple_files() {
        let json = r#"{"type":"match","data":{"path":{"text":"a.rs"},"lines":{"text":"fn foo()\n"},"line_number":1,"absolute_offset":0,"submatches":[]}}
{"type":"match","data":{"path":{"text":"b.rs"},"lines":{"text":"fn bar()\n"},"line_number":5,"absolute_offset":0,"submatches":[]}}"#;

        let result = format_rg_json(json);
        assert!(result.contains("a.rs:\n  1: fn foo()"));
        assert!(result.contains("b.rs:\n  5: fn bar()"));
    }

    #[test]
    fn test_format_rg_json_empty() {
        let result = format_rg_json("");
        assert_eq!(result, "no matches found");
    }

    #[tokio::test]
    async fn test_grep_integration() {
        // this test requires rg to be installed
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("test.rs"),
            "fn hello_world() {}\nfn goodbye() {}\n",
        )
        .unwrap();

        let input = GrepInput {
            pattern: "hello".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
            glob: None,
            context_lines: None,
            max_results: None,
            case_insensitive: None,
        };
        let result = execute_grep(&input).await;
        assert!(
            result.contains("hello_world"),
            "expected hello_world in: {result}"
        );
        assert!(
            !result.contains("goodbye"),
            "unexpected goodbye in: {result}"
        );
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.txt"), "nothing here\n").unwrap();

        let input = GrepInput {
            pattern: "nonexistent_pattern_xyz".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
            glob: None,
            context_lines: None,
            max_results: None,
            case_insensitive: None,
        };
        let result = execute_grep(&input).await;
        assert_eq!(result, "no matches found");
    }

    #[tokio::test]
    async fn test_grep_case_insensitive() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.txt"), "Hello World\n").unwrap();

        let input = GrepInput {
            pattern: "hello".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
            glob: None,
            context_lines: None,
            max_results: None,
            case_insensitive: Some(true),
        };
        let result = execute_grep(&input).await;
        assert!(
            result.contains("Hello World"),
            "expected case-insensitive match in: {result}"
        );
    }

    #[tokio::test]
    async fn test_grep_with_glob_filter() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("match.rs"), "fn target() {}\n").unwrap();
        fs::write(dir.path().join("skip.txt"), "fn target() {}\n").unwrap();

        let input = GrepInput {
            pattern: "target".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
            glob: Some("*.rs".to_string()),
            context_lines: None,
            max_results: None,
            case_insensitive: None,
        };
        let result = execute_grep(&input).await;
        assert!(
            result.contains("match.rs"),
            "expected match.rs in: {result}"
        );
        assert!(
            !result.contains("skip.txt"),
            "unexpected skip.txt in: {result}"
        );
    }

    #[tokio::test]
    async fn test_grep_with_context() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("test.rs"),
            "line1\nline2\ntarget\nline4\nline5\n",
        )
        .unwrap();

        let input = GrepInput {
            pattern: "target".to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
            glob: None,
            context_lines: Some(1),
            max_results: None,
            case_insensitive: None,
        };
        let result = execute_grep(&input).await;
        assert!(
            result.contains("line2"),
            "expected context line2 in: {result}"
        );
        assert!(result.contains("target"), "expected target in: {result}");
        assert!(
            result.contains("line4"),
            "expected context line4 in: {result}"
        );
    }
}
