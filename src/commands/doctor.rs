use crate::db::Database;
use crate::error;
use crate::message::{Message, MessageContent, Role};

/// find orphaned tool_use blocks: assistant messages with tool_use not
/// followed by matching tool_results.
fn find_orphaned_tool_calls(messages: &[(i64, Message)]) -> Vec<(usize, i64, Vec<String>)> {
    let mut orphans = Vec::new();

    for i in 0..messages.len() {
        let (msg_id, ref msg) = messages[i];

        if msg.role != Role::Assistant {
            continue;
        }

        let tool_use_ids: Vec<String> = msg
            .content
            .iter()
            .filter_map(|c| match c {
                MessageContent::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        if tool_use_ids.is_empty() {
            continue;
        }

        let has_results = messages.get(i + 1).is_some_and(|(_, next)| {
            next.role == Role::User
                && tool_use_ids.iter().all(|id| {
                    next.content.iter().any(|c| {
                        matches!(c, MessageContent::ToolResult { tool_use_id, .. } if tool_use_id == id)
                    })
                })
        });

        if !has_results {
            orphans.push((i, msg_id, tool_use_ids));
        }
    }

    orphans
}

fn is_ava_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", "ava start"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub(crate) fn run_doctor_diagnose() -> Result<(), error::Error> {
    let mut issues = 0u32;

    // check 1: is ava running?
    if is_ava_running() {
        println!("  ok: ava process is running");
    } else {
        println!("  warning: ava is not running (start with `ava start`)");
        issues += 1;
    }

    // check 2: orphaned tool_use blocks
    let db = Database::open()?;
    let session_id = db.active_session()?;
    let messages = db.load_messages_with_ids(session_id)?;
    let orphans = find_orphaned_tool_calls(&messages);

    if orphans.is_empty() {
        println!("  ok: no orphaned tool calls");
    } else {
        let total_blocks: usize = orphans.iter().map(|(_, _, ids)| ids.len()).sum();
        println!(
            "  error: {} orphaned tool_use block(s) across {} message(s)",
            total_blocks,
            orphans.len()
        );
        println!("         fix with `ava doctor repair-orphans`");
        issues += 1;
    }

    if issues == 0 {
        println!("\nsession is healthy");
    } else {
        println!("\nfound {issues} issue(s)");
    }

    Ok(())
}

pub(crate) fn run_doctor_fix() -> Result<(), error::Error> {
    let db = Database::open()?;
    let session_id = db.active_session()?;
    let messages = db.load_messages_with_ids(session_id)?;
    let orphans = find_orphaned_tool_calls(&messages);

    if orphans.is_empty() {
        println!("nothing to fix");
        return Ok(());
    }

    let mut repaired = 0usize;
    for (_, msg_id, tool_use_ids) in &orphans {
        let synthetic: Vec<MessageContent> = tool_use_ids
            .iter()
            .map(|id| {
                MessageContent::tool_result(
                    id,
                    "the session was interrupted and it is unknown whether this tool call completed.",
                )
            })
            .collect();

        db.insert_message_after(session_id, *msg_id, "user", &synthetic)?;
        repaired += 1;
        println!(
            "  repaired orphaned tool_use at message id {msg_id} ({} blocks)",
            tool_use_ids.len()
        );
    }

    println!("repaired {repaired} orphaned tool_use block(s)");
    Ok(())
}
