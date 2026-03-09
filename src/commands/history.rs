use crate::db::Database;
use crate::error;
use crate::message::{MessageContent, Role};

/// display mode for history output
enum HistoryMode {
    Compact,
    Pretty,
    Full,
}

pub(crate) fn run_history(
    limit: u32,
    json: bool,
    compact: bool,
    full: bool,
) -> Result<(), error::Error> {
    let db = Database::open()?;
    let session_id = db.active_session()?;
    let messages = db.load_recent_messages(session_id, limit)?;

    if messages.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("no messages");
        }
        return Ok(());
    }

    if json {
        let out = serde_json::to_string_pretty(&messages)
            .map_err(|e| error::Error::Provider(format!("failed to serialize history: {e}")))?;
        println!("{out}");
        return Ok(());
    }

    // --full wins if both are passed
    let mode = if full {
        HistoryMode::Full
    } else if compact {
        HistoryMode::Compact
    } else {
        HistoryMode::Pretty
    };

    // ansi color codes
    const DIM: &str = "\x1b[2m";
    const CYAN: &str = "\x1b[36m";
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const MAGENTA: &str = "\x1b[35m";
    const RESET: &str = "\x1b[0m";

    for (i, msg) in messages.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let (role, role_color) = match msg.role {
            Role::User => ("user", CYAN),
            Role::Assistant => ("assistant", GREEN),
        };
        let label = format!("── {role} · {} ──", msg.created_at);
        let pad_len = 56usize.saturating_sub(label.len());
        let padding = "─".repeat(pad_len);
        println!(
            "{DIM}──{RESET} {role_color}{role}{RESET} {DIM}· {} ──{padding}{RESET}",
            msg.created_at
        );
        for block in &msg.content {
            match block {
                MessageContent::Text { text } => println!("{text}"),
                MessageContent::ToolUse { name, input, .. } => match &mode {
                    HistoryMode::Compact => {
                        let input_str = serde_json::to_string(input).unwrap_or_default();
                        println!("{YELLOW}[tool: {name}]{RESET} {DIM}{input_str}{RESET}");
                    }
                    HistoryMode::Pretty => {
                        let truncated = truncate_json_strings(input, 200);
                        let formatted =
                            serde_json::to_string_pretty(&truncated).unwrap_or_default();
                        println!("{YELLOW}[tool: {name}]{RESET}");
                        println!("{DIM}{formatted}{RESET}");
                    }
                    HistoryMode::Full => {
                        println!("{YELLOW}[tool: {name}]{RESET}");
                        print_expanded_json(input);
                    }
                },
                MessageContent::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    let display_str = content.as_display_str();
                    match &mode {
                        HistoryMode::Compact => {
                            let display = truncate_str(&display_str, 200);
                            println!(
                                "{MAGENTA}[result: {tool_use_id}]{RESET} {DIM}{display}{RESET}"
                            );
                        }
                        HistoryMode::Pretty => {
                            let display = truncate_str(&display_str, 500);
                            println!("{MAGENTA}[result: {tool_use_id}]{RESET}");
                            println!("{DIM}{display}{RESET}");
                        }
                        HistoryMode::Full => {
                            println!("{MAGENTA}[result: {tool_use_id}]{RESET}");
                            println!("{display_str}");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// truncate a string to `max` chars, appending `…` if truncated
fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// recursively truncate long string values inside a JSON value
fn truncate_json_strings(value: &serde_json::Value, max: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(truncate_str(s, max)),
        serde_json::Value::Object(map) => {
            let truncated = map
                .iter()
                .map(|(k, v)| (k.clone(), truncate_json_strings(v, max)))
                .collect();
            serde_json::Value::Object(truncated)
        }
        serde_json::Value::Array(arr) => {
            let truncated = arr.iter().map(|v| truncate_json_strings(v, max)).collect();
            serde_json::Value::Array(truncated)
        }
        other => other.clone(),
    }
}

/// print a JSON value in expanded key-value format with newlines rendered
fn print_expanded_json(value: &serde_json::Value) {
    const DIM: &str = "\x1b[2m";
    const RESET: &str = "\x1b[0m";

    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                match val {
                    serde_json::Value::String(s) => {
                        if s.contains('\n') {
                            println!("  {DIM}{key}:{RESET}");
                            for line in s.lines() {
                                println!("    {line}");
                            }
                        } else {
                            println!("  {DIM}{key}:{RESET} \"{s}\"");
                        }
                    }
                    serde_json::Value::Number(n) => println!("  {DIM}{key}:{RESET} {n}"),
                    serde_json::Value::Bool(b) => println!("  {DIM}{key}:{RESET} {b}"),
                    serde_json::Value::Null => println!("  {DIM}{key}:{RESET} null"),
                    other => {
                        // nested objects/arrays: pretty-print with indent
                        let formatted = serde_json::to_string_pretty(other).unwrap_or_default();
                        println!("  {DIM}{key}:{RESET}");
                        for line in formatted.lines() {
                            println!("    {line}");
                        }
                    }
                }
            }
        }
        // non-object top-level: just pretty-print
        other => {
            let formatted = serde_json::to_string_pretty(other).unwrap_or_default();
            println!("{formatted}");
        }
    }
}
