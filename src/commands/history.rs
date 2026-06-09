use crate::db::{Database, session::HistoryMessage};
use crate::error;
use crate::message::{MessageContent, Role};
use crate::pricing;
use crate::provider::Usage;

/// display mode for history output
enum HistoryMode {
    Compact,
    Pretty,
    Full,
}

// ansi color codes
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const RESET: &str = "\x1b[0m";

pub(crate) fn run_history(
    limit: u32,
    json: bool,
    compact: bool,
    full: bool,
    follow: bool,
) -> Result<(), error::Error> {
    let db = Database::open()?;
    let session_id = db.active_session()?;
    let messages = db.load_recent_messages(session_id, limit)?;

    if messages.is_empty() && !follow {
        if json {
            println!("[]");
        } else {
            println!("no messages");
        }
        return Ok(());
    }

    if json && !follow {
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

    // print initial messages
    let mut is_first = true;
    for msg in &messages {
        if !is_first {
            println!("\n");
        }
        is_first = false;
        print_message(msg, &mode);
    }

    if !follow {
        return Ok(());
    }

    // follow mode: poll for new messages
    let mut last_id = messages.last().map(|m| m.id).unwrap_or(0);

    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));

        let new_messages = db.load_messages_after(session_id, last_id)?;
        for msg in &new_messages {
            if !is_first {
                println!("\n");
            }
            is_first = false;
            print_message(msg, &mode);
            last_id = msg.id;
        }
    }
}

fn print_message(msg: &HistoryMessage, mode: &HistoryMode) {
    // detect tool-result-only user messages — these aren't real user turns
    let is_tool_results = msg.role == Role::User
        && msg.content.iter().all(|b| {
            matches!(
                b,
                MessageContent::ToolResult { .. } | MessageContent::Text { .. }
            )
        })
        && msg
            .content
            .iter()
            .any(|b| matches!(b, MessageContent::ToolResult { .. }));

    let (role, role_color) = if is_tool_results {
        ("tool result", MAGENTA)
    } else {
        match msg.role {
            Role::User => ("user", CYAN),
            Role::Assistant => ("assistant", GREEN),
            Role::System => ("system", DIM),
        }
    };
    let usage = usage_label(msg);
    let label = format!("── {role} · {}{usage} ──", msg.created_at);
    let pad_len = 56usize.saturating_sub(label.len());
    let padding = "─".repeat(pad_len);
    println!(
        "{DIM}──{RESET} {role_color}{role}{RESET} {DIM}· {}{usage} ──{padding}{RESET}",
        msg.created_at
    );
    for block in &msg.content {
        match block {
            MessageContent::Text { text } => println!("{text}"),
            MessageContent::Thinking { .. } => {}
            MessageContent::Image { .. } => println!("{DIM}[image]{RESET}"),
            MessageContent::ToolUse { name, input, .. } => match mode {
                HistoryMode::Compact => {
                    let input_str = serde_json::to_string(input).unwrap_or_default();
                    println!("{YELLOW}[tool: {name}]{RESET} {DIM}{input_str}{RESET}");
                }
                HistoryMode::Pretty => {
                    let truncated = truncate_json_strings(input, 200);
                    let formatted = serde_json::to_string_pretty(&truncated).unwrap_or_default();
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
                match mode {
                    HistoryMode::Compact => {
                        let display = truncate_str(&display_str, 200);
                        println!("{MAGENTA}[result: {tool_use_id}]{RESET} {DIM}{display}{RESET}");
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

fn usage_label(msg: &HistoryMessage) -> String {
    let mut parts = Vec::new();
    if let Some(output) = msg.output_tokens {
        parts.push(format!("output {output}"));
    }
    if let Some(reasoning) = msg.reasoning_tokens {
        parts.push(format!("reasoning {reasoning}"));
    }
    if let Some(model_id) = msg.model_id.as_deref()
        && (msg.input_tokens.is_some()
            || msg.output_tokens.is_some()
            || msg.cache_creation_tokens.is_some()
            || msg.cache_read_tokens.is_some())
    {
        let usage = Usage {
            input_tokens: msg.input_tokens.unwrap_or(0),
            output_tokens: msg.output_tokens.unwrap_or(0),
            reasoning_tokens: msg.reasoning_tokens,
            cache_creation_tokens: msg.cache_creation_tokens,
            cache_read_tokens: msg.cache_read_tokens,
        };
        parts.push(format!(
            "cost {}",
            pricing::format_usage_cost(model_id, &usage)
        ));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_label_includes_cost_when_model_is_known() {
        let msg = HistoryMessage {
            id: 1,
            role: Role::Assistant,
            content: vec![MessageContent::text("hi")],
            created_at: "now".into(),
            model_id: Some("anthropic/claude-sonnet-4-6".into()),
            input_tokens: Some(100_000),
            output_tokens: Some(10_000),
            reasoning_tokens: Some(2_000),
            cache_creation_tokens: None,
            cache_read_tokens: None,
        };

        assert_eq!(
            usage_label(&msg),
            " · output 10000 · reasoning 2000 · cost ~$0.45"
        );
    }

    #[test]
    fn usage_label_marks_unknown_model_cost() {
        let msg = HistoryMessage {
            id: 1,
            role: Role::Assistant,
            content: vec![MessageContent::text("hi")],
            created_at: "now".into(),
            model_id: Some("unknown/model".into()),
            input_tokens: Some(100),
            output_tokens: Some(10),
            reasoning_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        };

        assert_eq!(usage_label(&msg), " · output 10 · cost unknown");
    }
}
