//! V4A patch format parser and applier for OpenAI's native apply_patch tool.
//!
//! the patch format uses `*** Begin Patch` / `*** End Patch` delimiters with
//! three operation types: Add File, Update File, Delete File. hunks use `@@`
//! markers with context (space-prefixed), removal (`-`), and addition (`+`) lines.
//!
//! ported from openai/codex `codex-rs/apply-patch/` (MIT license).

use std::fs;

use serde::Deserialize;

use super::ToolCall;
use super::exec::truncate_output;
use super::filesystem::{validate_existing_path, validate_path};

pub const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";

#[derive(Debug, Deserialize)]
struct ApplyPatchInput {
    operation: String,
    path: String,
    diff: Option<String>,
}

pub(super) async fn handle_apply_patch(call: &ToolCall) -> String {
    let input: ApplyPatchInput = match serde_json::from_value(call.input.clone()) {
        Ok(i) => i,
        Err(err) => return format!("invalid input: {err}"),
    };

    match input.operation.as_str() {
        "create_file" => {
            let diff = match input.diff {
                Some(d) => d,
                None => return "create_file requires diff".to_string(),
            };
            apply_create(&input.path, &diff)
        }
        "update_file" => {
            let diff = match input.diff {
                Some(d) => d,
                None => return "update_file requires diff".to_string(),
            };
            apply_update(&input.path, &diff)
        }
        "delete_file" => apply_delete(&input.path),
        other => format!("unknown operation: {other}"),
    }
}

// --- patch format types ---

#[derive(Debug)]
#[allow(dead_code)]
struct UpdateChunk {
    /// context lines before the change (used to locate where to apply)
    context_before: Vec<String>,
    /// lines to remove (must match file content after context_before)
    old_lines: Vec<String>,
    /// lines to insert in place of old_lines
    new_lines: Vec<String>,
    /// context lines after the change
    context_after: Vec<String>,
}

// --- operations ---

fn apply_create(path: &str, diff: &str) -> String {
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

    // extract content from V4A diff: lines prefixed with `+` become file content
    let content = extract_added_content(diff);

    match fs::write(&resolved, &content) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("failed to write file: {e}"),
    }
}

fn apply_update(path: &str, diff: &str) -> String {
    let resolved = match validate_existing_path(path) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let content = match fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return format!("failed to read file: {e}"),
    };

    let chunks = match parse_update_chunks(diff) {
        Ok(c) => c,
        Err(e) => return format!("failed to parse patch: {e}"),
    };

    if chunks.is_empty() {
        return "patch contains no changes".to_string();
    }

    let lines: Vec<&str> = content.lines().collect();
    let new_lines = match apply_chunks(&lines, &chunks) {
        Ok(l) => l,
        Err(e) => return e,
    };

    let mut new_content = new_lines.join("\n");
    if content.ends_with('\n') && !new_content.is_empty() {
        new_content.push('\n');
    }

    match fs::write(&resolved, &new_content) {
        Ok(()) => truncate_output("ok"),
        Err(e) => format!("failed to write file: {e}"),
    }
}

fn apply_delete(path: &str) -> String {
    let resolved = match validate_existing_path(path) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match fs::remove_file(&resolved) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("failed to delete file: {e}"),
    }
}

// --- content extraction for create ---

/// extract file content from a V4A diff. lines starting with `+` have the prefix
/// stripped; the patch envelope markers are skipped.
fn extract_added_content(diff: &str) -> String {
    let mut lines = Vec::new();
    for line in diff.lines() {
        // skip envelope markers
        if line.starts_with("*** ") || line.starts_with("@@") {
            continue;
        }
        if let Some(content) = line.strip_prefix('+') {
            lines.push(content);
        }
    }
    if lines.is_empty() {
        return diff.to_string();
    }
    let mut result = lines.join("\n");
    result.push('\n');
    result
}

// --- V4A update parser ---

/// parse the diff content of an update_file operation into chunks.
/// handles both the full `*** Begin Patch ... *** End Patch` envelope
/// and bare hunk content.
fn parse_update_chunks(diff: &str) -> Result<Vec<UpdateChunk>, String> {
    let mut chunks = Vec::new();
    let mut in_hunk = false;
    let mut context_before: Vec<String> = Vec::new();
    let mut old_lines: Vec<String> = Vec::new();
    let mut new_lines: Vec<String> = Vec::new();
    let mut context_after: Vec<String> = Vec::new();
    let mut seen_change = false;

    for line in diff.lines() {
        // skip envelope markers
        if line == "*** Begin Patch"
            || line == "*** End Patch"
            || line.starts_with("*** Update File:")
            || line.starts_with("*** Add File:")
            || line.starts_with("*** Delete File:")
        {
            continue;
        }

        if line.starts_with("@@") {
            // flush previous chunk if we have one
            if in_hunk && seen_change {
                chunks.push(UpdateChunk {
                    context_before: std::mem::take(&mut context_before),
                    old_lines: std::mem::take(&mut old_lines),
                    new_lines: std::mem::take(&mut new_lines),
                    context_after: std::mem::take(&mut context_after),
                });
                seen_change = false;
            } else {
                context_before.clear();
                old_lines.clear();
                new_lines.clear();
                context_after.clear();
                seen_change = false;
            }
            in_hunk = true;
            continue;
        }

        if !in_hunk {
            // before first @@ marker — treat as start of an implicit hunk
            in_hunk = true;
        }

        if let Some(removed) = line.strip_prefix('-') {
            // if we were collecting context_after, that context belongs to a
            // new chunk — flush the previous chunk
            if seen_change && !context_after.is_empty() {
                chunks.push(UpdateChunk {
                    context_before: std::mem::take(&mut context_before),
                    old_lines: std::mem::take(&mut old_lines),
                    new_lines: std::mem::take(&mut new_lines),
                    context_after: Vec::new(), // context_after becomes next context_before
                });
                context_before = std::mem::take(&mut context_after);
            }
            old_lines.push(removed.to_string());
            seen_change = true;
        } else if let Some(added) = line.strip_prefix('+') {
            // if we were collecting context_after, flush for same reason
            if seen_change && !context_after.is_empty() && old_lines.is_empty() {
                // this shouldn't normally happen (+ after context_after without -)
                // but handle gracefully by continuing the current chunk
            }
            new_lines.push(added.to_string());
            seen_change = true;
        } else {
            // context line — strip the leading space if present
            let ctx = line.strip_prefix(' ').unwrap_or(line);
            if seen_change {
                context_after.push(ctx.to_string());
            } else {
                context_before.push(ctx.to_string());
            }
        }
    }

    // flush final chunk
    if seen_change {
        chunks.push(UpdateChunk {
            context_before,
            old_lines,
            new_lines,
            context_after,
        });
    }

    Ok(chunks)
}

// --- chunk application ---

/// apply all chunks to the file content, producing new lines.
fn apply_chunks(lines: &[&str], chunks: &[UpdateChunk]) -> Result<Vec<String>, String> {
    let mut result: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

    // apply chunks in reverse order so that earlier chunk offsets remain valid
    // when later chunks shift line numbers
    let mut chunk_positions: Vec<(usize, &UpdateChunk)> = Vec::new();
    let mut search_from = 0;

    for chunk in chunks {
        let pos = find_chunk_position(&result, chunk, search_from)?;
        chunk_positions.push((pos, chunk));
        // next chunk must come after this one
        search_from = pos + chunk.context_before.len() + chunk.old_lines.len();
    }

    // apply in reverse
    for (pos, chunk) in chunk_positions.into_iter().rev() {
        let start = pos + chunk.context_before.len();
        let end = start + chunk.old_lines.len();

        // verify old_lines match
        if end > result.len() {
            return Err(format!(
                "patch extends past end of file at line {}",
                start + 1
            ));
        }
        for (i, old) in chunk.old_lines.iter().enumerate() {
            if !lines_match(&result[start + i], old) {
                return Err(format!(
                    "patch mismatch at line {}: expected '{}', found '{}'",
                    start + i + 1,
                    old,
                    result[start + i]
                ));
            }
        }

        // replace old_lines with new_lines
        let new: Vec<String> = chunk.new_lines.iter().map(|l| l.to_string()).collect();
        result.splice(start..end, new);
    }

    Ok(result)
}

/// find where a chunk should be applied by matching its context_before lines.
fn find_chunk_position(
    lines: &[String],
    chunk: &UpdateChunk,
    search_from: usize,
) -> Result<usize, String> {
    if chunk.context_before.is_empty() {
        // no context — if we have old_lines, search for those directly
        if !chunk.old_lines.is_empty() {
            return find_sequence(lines, &chunk.old_lines, search_from);
        }
        // no context and no old_lines (pure insertion) — apply at search_from
        return Ok(search_from);
    }

    find_sequence(lines, &chunk.context_before, search_from)
}

/// find a sequence of lines within the file, using progressive matching.
fn find_sequence(
    lines: &[String],
    pattern: &[String],
    search_from: usize,
) -> Result<usize, String> {
    if pattern.is_empty() {
        return Ok(search_from);
    }
    if pattern.len() > lines.len() {
        return Err("pattern is larger than file".to_string());
    }

    let max_start = lines.len() - pattern.len();

    // pass 1: exact match
    for i in search_from..=max_start {
        if pattern.iter().enumerate().all(|(j, p)| lines[i + j] == *p) {
            return Ok(i);
        }
    }

    // pass 2: trimmed whitespace match
    for i in search_from..=max_start {
        if pattern
            .iter()
            .enumerate()
            .all(|(j, p)| lines[i + j].trim() == p.trim())
        {
            return Ok(i);
        }
    }

    // pass 3: normalized match (unicode normalization)
    for i in search_from..=max_start {
        if pattern
            .iter()
            .enumerate()
            .all(|(j, p)| normalize(lines[i + j].trim()) == normalize(p.trim()))
        {
            return Ok(i);
        }
    }

    Err(format!(
        "could not find matching location for context: '{}'",
        pattern.first().unwrap_or(&String::new())
    ))
}

/// compare two lines with trimmed whitespace.
fn lines_match(a: &str, b: &str) -> bool {
    a == b || a.trim() == b.trim() || normalize(a.trim()) == normalize(b.trim())
}

/// normalize unicode characters to ASCII equivalents for fuzzy matching.
fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2013}' | '\u{2014}' => '-',              // en-dash, em-dash
            '\u{2018}' | '\u{2019}' => '\'',             // smart single quotes
            '\u{201C}' | '\u{201D}' => '"',              // smart double quotes
            '\u{00A0}' | '\u{2000}'..='\u{200B}' => ' ', // non-breaking and various spaces
            '\u{2026}' => '.',                           // ellipsis (approximate)
            other => other,
        })
        .collect()
}

/// JSON schema for the text editor tool, used by providers that need to send it
/// as a regular function tool (e.g. OpenRouter).
pub fn text_editor_function_schema() -> (&'static str, &'static str, serde_json::Value) {
    (
        super::filesystem::TEXT_EDITOR_TOOL_NAME,
        "a text editor tool that can view, create, and edit files. commands: \
         view (read a file or directory), str_replace (replace exact text in a file), \
         create (create a new file), insert (insert text at a line number).",
        serde_json::json!({
            "type": "object",
            "required": ["command", "path"],
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["view", "str_replace", "create", "insert"],
                    "description": "the operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "absolute or relative file/directory path"
                },
                "view_range": {
                    "type": "array",
                    "items": {"type": "integer"},
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "[start_line, end_line] for view command (1-indexed, inclusive)"
                },
                "old_str": {
                    "type": "string",
                    "description": "the exact text to replace (for str_replace). must match exactly once."
                },
                "new_str": {
                    "type": "string",
                    "description": "the replacement text (for str_replace)"
                },
                "file_text": {
                    "type": "string",
                    "description": "the full file content (for create)"
                },
                "insert_line": {
                    "type": "integer",
                    "description": "line number to insert after (for insert, 0-indexed)"
                },
                "insert_text": {
                    "type": "string",
                    "description": "the text to insert (for insert)"
                }
            },
            "additionalProperties": false
        }),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use tempfile::TempDir;

    fn temp_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path
    }

    // --- extract_added_content ---

    #[test]
    fn test_extract_added_content_basic() {
        let diff = "\
*** Begin Patch
*** Add File: test.txt
+line one
+line two
+line three
*** End Patch";
        let result = extract_added_content(diff);
        assert_eq!(result, "line one\nline two\nline three\n");
    }

    #[test]
    fn test_extract_added_content_bare() {
        let diff = "+hello\n+world";
        let result = extract_added_content(diff);
        assert_eq!(result, "hello\nworld\n");
    }

    // --- parse_update_chunks ---

    #[test]
    fn test_parse_single_hunk() {
        let diff = "\
@@
 context before
-old line
+new line
 context after";
        let chunks = parse_update_chunks(diff).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].context_before, vec!["context before"]);
        assert_eq!(chunks[0].old_lines, vec!["old line"]);
        assert_eq!(chunks[0].new_lines, vec!["new line"]);
    }

    #[test]
    fn test_parse_multiple_hunks() {
        let diff = "\
@@
 first context
-remove1
+add1
@@
 second context
-remove2
+add2";
        let chunks = parse_update_chunks(diff).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].old_lines, vec!["remove1"]);
        assert_eq!(chunks[1].old_lines, vec!["remove2"]);
    }

    #[test]
    fn test_parse_with_envelope() {
        let diff = "\
*** Begin Patch
*** Update File: src/main.rs
@@
 fn main() {
-    println!(\"hello\");
+    println!(\"world\");
 }
*** End Patch";
        let chunks = parse_update_chunks(diff).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].old_lines, vec!["    println!(\"hello\");"]);
        assert_eq!(chunks[0].new_lines, vec!["    println!(\"world\");"]);
    }

    #[test]
    fn test_parse_addition_only() {
        let diff = "\
@@
 line before
+inserted line
 line after";
        let chunks = parse_update_chunks(diff).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].old_lines.is_empty());
        assert_eq!(chunks[0].new_lines, vec!["inserted line"]);
    }

    #[test]
    fn test_parse_deletion_only() {
        let diff = "\
@@
 line before
-removed line
 line after";
        let chunks = parse_update_chunks(diff).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].old_lines, vec!["removed line"]);
        assert!(chunks[0].new_lines.is_empty());
    }

    // --- apply operations ---

    #[test]
    fn test_apply_create() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.txt");

        let diff = "+line one\n+line two\n";
        let result = apply_create(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "line one\nline two\n");
    }

    #[test]
    fn test_apply_create_already_exists() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "exists.txt", "content");

        let result = apply_create(path.to_str().unwrap(), "+new content");
        assert!(result.contains("already exists"));
    }

    #[test]
    fn test_apply_create_nested_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a/b/c/new.txt");

        let result = apply_create(path.to_str().unwrap(), "+hello");
        assert_eq!(result, "ok");
        assert!(path.exists());
    }

    #[test]
    fn test_apply_update_simple() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "update.txt", "line 1\nline 2\nline 3\n");

        let diff = "\
@@
 line 1
-line 2
+line 2 modified
 line 3";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "line 1\nline 2 modified\nline 3\n");
    }

    #[test]
    fn test_apply_update_multiple_hunks() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "multi.txt", "a\nb\nc\nd\ne\nf\n");

        let diff = "\
@@
 a
-b
+B
 c
@@
 d
-e
+E
 f";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nB\nc\nd\nE\nf\n");
    }

    #[test]
    fn test_apply_update_trimmed_match() {
        let dir = TempDir::new().unwrap();
        // file has trailing spaces
        let path = temp_file(&dir, "trim.txt", "  line 1  \n  line 2  \n  line 3  \n");

        // patch doesn't have trailing spaces
        let diff = "\
@@
 line 1
-  line 2
+  line 2 modified
 line 3";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_apply_update_no_match() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "nomatch.txt", "line 1\nline 2\nline 3\n");

        let diff = "\
@@
 nonexistent context
-line 2
+line 2 modified";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert!(result.contains("could not find matching location"));
    }

    #[test]
    fn test_apply_delete() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "delete.txt", "content");

        let result = apply_delete(path.to_str().unwrap());
        assert_eq!(result, "ok");
        assert!(!path.exists());
    }

    #[test]
    fn test_apply_delete_nonexistent() {
        let result = apply_delete("/tmp/nonexistent_ava_test_file_99999.txt");
        assert!(result.contains("does not exist"));
    }

    // --- find_sequence ---

    #[test]
    fn test_find_sequence_exact() {
        let lines: Vec<String> = vec!["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect();
        let pattern = vec!["b".to_string(), "c".to_string()];
        assert_eq!(find_sequence(&lines, &pattern, 0).unwrap(), 1);
    }

    #[test]
    fn test_find_sequence_trimmed() {
        let lines: Vec<String> = vec!["  a  ", "  b  ", "  c  "]
            .into_iter()
            .map(String::from)
            .collect();
        let pattern = vec!["b".to_string()];
        assert_eq!(find_sequence(&lines, &pattern, 0).unwrap(), 1);
    }

    #[test]
    fn test_normalize_unicode() {
        assert_eq!(normalize("hello \u{2014} world"), "hello - world");
        assert_eq!(normalize("\u{201C}quoted\u{201D}"), "\"quoted\"");
    }

    // --- multi-line changes ---

    #[test]
    fn test_apply_update_multi_line_replacement() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(
            &dir,
            "multi_line.txt",
            "fn main() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n",
        );

        let diff = "\
@@
 fn main() {
-    let x = 1;
-    let y = 2;
-    let z = 3;
+    let x = 10;
+    let y = 20;
+    let z = 30;
+    let w = 40;
 }";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(
            content,
            "fn main() {\n    let x = 10;\n    let y = 20;\n    let z = 30;\n    let w = 40;\n}\n"
        );
    }

    // --- no context patch ---

    #[test]
    fn test_apply_update_no_context() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "nocontext.txt", "aaa\nbbb\nccc\n");

        // patch with no context lines, just old/new
        let diff = "\
@@
-bbb
+BBB";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "aaa\nBBB\nccc\n");
    }

    // --- pure insertion (no removal) applied end-to-end ---

    #[test]
    fn test_apply_update_pure_insertion() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "insert.txt", "line 1\nline 2\nline 3\n");

        let diff = "\
@@
 line 1
+inserted after line 1
 line 2";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "line 1\ninserted after line 1\nline 2\nline 3\n");
    }

    // --- pure deletion (no insertion) applied end-to-end ---

    #[test]
    fn test_apply_update_pure_deletion() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "delete_line.txt", "keep\nremove me\nkeep too\n");

        let diff = "\
@@
 keep
-remove me
 keep too";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "keep\nkeep too\n");
    }

    // --- file without trailing newline ---

    #[test]
    fn test_apply_update_no_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "no_newline.txt", "line 1\nline 2\nline 3");

        let diff = "\
@@
 line 1
-line 2
+line 2 changed
 line 3";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        // should preserve lack of trailing newline
        assert_eq!(content, "line 1\nline 2 changed\nline 3");
    }

    // --- unicode normalized matching end-to-end ---

    #[test]
    fn test_apply_update_unicode_normalized_match() {
        let dir = TempDir::new().unwrap();
        // file uses smart quotes
        let path = temp_file(
            &dir,
            "unicode.txt",
            "say \u{201C}hello\u{201D}\nkeep this\n",
        );

        // patch uses regular quotes
        let diff = "\
@@
 say \"hello\"
-keep this
+changed this";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");
    }

    // --- adjacent hunks sharing context ---

    #[test]
    fn test_apply_update_adjacent_changes() {
        let dir = TempDir::new().unwrap();
        let path = temp_file(&dir, "adjacent.txt", "a\nb\nc\nd\ne\n");

        // two changes with only one line of context between them
        let diff = "\
@@
 a
-b
+B
 c
-d
+D
 e";

        let result = apply_update(path.to_str().unwrap(), diff);
        assert_eq!(result, "ok");

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nB\nc\nD\ne\n");
    }

    // --- text_editor_function_schema ---

    #[test]
    fn test_text_editor_schema_has_required_fields() {
        let (name, desc, schema) = text_editor_function_schema();
        assert_eq!(name, "str_replace_based_edit_tool");
        assert!(!desc.is_empty());
        assert!(schema["properties"]["command"].is_object());
        assert!(schema["properties"]["path"].is_object());
    }
}
