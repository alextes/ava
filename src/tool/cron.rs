use std::str::FromStr;

use chrono::Utc;
use croner::Cron;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::message::MessageContent;

use super::{ToolCallResult, ToolDefinition};

pub const CRON_TOOL_NAME: &str = "cron";

pub fn cron_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: CRON_TOOL_NAME,
        description: "schedule one-time or recurring tasks. action=schedule: create a new schedule. action=list: show active schedules. action=cancel: cancel a schedule by id.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["schedule", "list", "cancel"],
                    "description": "action to perform"
                },
                "description": {
                    "type": "string",
                    "description": "human-readable description of the schedule (for action=schedule)"
                },
                "prompt": {
                    "type": "string",
                    "description": "the message to send when the schedule fires (for action=schedule)"
                },
                "run_at": {
                    "type": "string",
                    "description": "ISO 8601 datetime for one-time schedules, e.g. 2025-01-15T09:00:00Z (for action=schedule, ignored if cron_expr is set)"
                },
                "cron_expr": {
                    "type": "string",
                    "description": "5-field cron expression for recurring schedules, e.g. '30 7 * * *' for daily at 7:30 (for action=schedule)"
                },
                "id": {
                    "type": "integer",
                    "description": "schedule id (for action=cancel)"
                }
            },
            "required": ["action"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CronInput {
    action: String,
    description: Option<String>,
    prompt: Option<String>,
    run_at: Option<String>,
    cron_expr: Option<String>,
    id: Option<i64>,
}

pub fn handle_cron(db: &Database, call_id: &str, input: &serde_json::Value) -> ToolCallResult {
    let input: CronInput = match serde_json::from_value(input.clone()) {
        Ok(v) => v,
        Err(e) => {
            return ToolCallResult {
                content: MessageContent::tool_result(call_id, format!("invalid input: {e}")),
                switch_provider: None,
                complete: false,
                compact: false,
            };
        }
    };

    match input.action.as_str() {
        "schedule" => handle_schedule(db, call_id, &input),
        "list" => handle_list(db, call_id),
        "cancel" => handle_cancel(db, call_id, &input),
        other => ToolCallResult {
            content: MessageContent::tool_result(call_id, format!("invalid action: {other}")),
            switch_provider: None,
            complete: false,
            compact: false,
        },
    }
}

fn handle_schedule(db: &Database, call_id: &str, input: &CronInput) -> ToolCallResult {
    let description = match &input.description {
        Some(d) if !d.trim().is_empty() => d.trim(),
        _ => {
            return ToolCallResult {
                content: MessageContent::tool_result(call_id, "schedule requires description"),
                switch_provider: None,
                complete: false,
                compact: false,
            };
        }
    };

    let prompt = match &input.prompt {
        Some(p) if !p.trim().is_empty() => p.trim(),
        _ => {
            return ToolCallResult {
                content: MessageContent::tool_result(call_id, "schedule requires prompt"),
                switch_provider: None,
                complete: false,
                compact: false,
            };
        }
    };

    let (next_run_at, cron_expr) = if let Some(ref expr) = input.cron_expr {
        // recurring schedule: validate cron expression and compute next occurrence
        let cron = match Cron::from_str(expr) {
            Ok(c) => c,
            Err(e) => {
                return ToolCallResult {
                    content: MessageContent::tool_result(
                        call_id,
                        format!("invalid cron expression: {e}"),
                    ),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                };
            }
        };

        let next = match cron.find_next_occurrence(&Utc::now(), false) {
            Ok(t) => t,
            Err(e) => {
                return ToolCallResult {
                    content: MessageContent::tool_result(
                        call_id,
                        format!("could not compute next occurrence: {e}"),
                    ),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                };
            }
        };

        (
            next.format("%Y-%m-%d %H:%M:%S").to_string(),
            Some(expr.as_str()),
        )
    } else if let Some(ref run_at) = input.run_at {
        // one-time schedule: parse the provided datetime
        let parsed = chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M:%SZ")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M:%S"))
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%d %H:%M:%S"));

        match parsed {
            Ok(dt) => (dt.format("%Y-%m-%d %H:%M:%S").to_string(), None),
            Err(e) => {
                return ToolCallResult {
                    content: MessageContent::tool_result(
                        call_id,
                        format!("invalid run_at datetime: {e}"),
                    ),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                };
            }
        }
    } else {
        return ToolCallResult {
            content: MessageContent::tool_result(
                call_id,
                "schedule requires either cron_expr or run_at",
            ),
            switch_provider: None,
            complete: false,
            compact: false,
        };
    };

    match db.create_schedule(description, prompt, &next_run_at, cron_expr) {
        Ok(id) => {
            let kind = if input.cron_expr.is_some() {
                "recurring"
            } else {
                "one-time"
            };
            ToolCallResult {
                content: MessageContent::tool_result(
                    call_id,
                    format!("scheduled ({kind}, id={id}, next_run_at={next_run_at})"),
                ),
                switch_provider: None,
                complete: false,
                compact: false,
            }
        }
        Err(e) => ToolCallResult {
            content: MessageContent::tool_result(call_id, format!("error: {e}")),
            switch_provider: None,
            complete: false,
            compact: false,
        },
    }
}

fn handle_list(db: &Database, call_id: &str) -> ToolCallResult {
    match db.list_schedules() {
        Ok(schedules) => {
            if schedules.is_empty() {
                return ToolCallResult {
                    content: MessageContent::tool_result(call_id, "no active schedules"),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                };
            }
            let mut output = String::new();
            for (i, s) in schedules.iter().enumerate() {
                if i > 0 {
                    output.push('\n');
                }
                let kind = if let Some(expr) = &s.cron_expr {
                    format!("recurring ({expr})")
                } else {
                    "one-time".to_string()
                };
                output.push_str(&format!(
                    "id={}: {} [{}] next_run_at={} | prompt: {}",
                    s.id, s.description, kind, s.next_run_at, s.prompt
                ));
            }
            ToolCallResult {
                content: MessageContent::tool_result(call_id, output),
                switch_provider: None,
                complete: false,
                compact: false,
            }
        }
        Err(e) => ToolCallResult {
            content: MessageContent::tool_result(call_id, format!("error: {e}")),
            switch_provider: None,
            complete: false,
            compact: false,
        },
    }
}

fn handle_cancel(db: &Database, call_id: &str, input: &CronInput) -> ToolCallResult {
    let id = match input.id {
        Some(id) => id,
        None => {
            return ToolCallResult {
                content: MessageContent::tool_result(call_id, "cancel requires id"),
                switch_provider: None,
                complete: false,
                compact: false,
            };
        }
    };

    match db.cancel_schedule(id) {
        Ok(true) => ToolCallResult {
            content: MessageContent::tool_result(call_id, "cancelled"),
            switch_provider: None,
            complete: false,
            compact: false,
        },
        Ok(false) => ToolCallResult {
            content: MessageContent::tool_result(call_id, "not found"),
            switch_provider: None,
            complete: false,
            compact: false,
        },
        Err(e) => ToolCallResult {
            content: MessageContent::tool_result(call_id, format!("error: {e}")),
            switch_provider: None,
            complete: false,
            compact: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Database;
    use crate::message::MessageContent;
    use serde_json::json;

    use super::handle_cron;

    fn extract_text(content: &MessageContent) -> String {
        match content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_handle_cron_schedule_one_time() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({
            "action": "schedule",
            "description": "remind me",
            "prompt": "hey, time to do the thing",
            "run_at": "2099-06-15T14:00:00Z"
        });
        let result = handle_cron(&db, "t1", &input);
        let text = extract_text(&result.content);
        assert!(text.contains("one-time"), "got: {text}");
        assert!(text.contains("2099-06-15 14:00:00"));

        let schedules = db.list_schedules().unwrap();
        assert_eq!(schedules.len(), 1);
        assert!(schedules[0].cron_expr.is_none());
    }

    #[test]
    fn test_handle_cron_schedule_recurring() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({
            "action": "schedule",
            "description": "daily standup",
            "prompt": "time for standup",
            "cron_expr": "30 9 * * *"
        });
        let result = handle_cron(&db, "t1", &input);
        let text = extract_text(&result.content);
        assert!(text.contains("recurring"), "got: {text}");
        assert!(text.contains("id="));

        let schedules = db.list_schedules().unwrap();
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].cron_expr.as_deref(), Some("30 9 * * *"));
    }

    #[test]
    fn test_handle_cron_schedule_invalid_cron() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({
            "action": "schedule",
            "description": "bad cron",
            "prompt": "won't work",
            "cron_expr": "not a cron"
        });
        let result = handle_cron(&db, "t1", &input);
        let text = extract_text(&result.content);
        assert!(text.contains("invalid cron expression"), "got: {text}");
    }

    #[test]
    fn test_handle_cron_list_empty() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({"action": "list"});
        let result = handle_cron(&db, "t1", &input);
        assert_eq!(extract_text(&result.content), "no active schedules");
    }

    #[test]
    fn test_handle_cron_list() {
        let db = Database::open_in_memory().unwrap();
        db.create_schedule(
            "daily check",
            "check in",
            "2099-01-01 07:30:00",
            Some("30 7 * * *"),
        )
        .unwrap();

        let input = json!({"action": "list"});
        let result = handle_cron(&db, "t1", &input);
        let text = extract_text(&result.content);
        assert!(text.contains("daily check"), "got: {text}");
        assert!(text.contains("30 7 * * *"), "got: {text}");
    }

    #[test]
    fn test_handle_cron_cancel() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .create_schedule("test", "prompt", "2099-01-01 00:00:00", None)
            .unwrap();

        let input = json!({"action": "cancel", "id": id});
        let result = handle_cron(&db, "t1", &input);
        assert_eq!(extract_text(&result.content), "cancelled");
    }

    #[test]
    fn test_handle_cron_cancel_not_found() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({"action": "cancel", "id": 999});
        let result = handle_cron(&db, "t1", &input);
        assert_eq!(extract_text(&result.content), "not found");
    }

    #[test]
    fn test_handle_cron_invalid_action() {
        let db = Database::open_in_memory().unwrap();
        let input = json!({"action": "update"});
        let result = handle_cron(&db, "t1", &input);
        assert_eq!(extract_text(&result.content), "invalid action: update");
    }
}
