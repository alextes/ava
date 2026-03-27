use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::message::MessageContent;

use super::{ToolCallResult, ToolDefinition};

pub const TASKS_TOOL_NAME: &str = "tasks";

#[derive(Debug, Deserialize)]
struct TasksInput {
    action: String,
    title: Option<String>,
    detail: Option<String>,
    id: Option<i64>,
    include_done: Option<bool>,
}

pub fn tasks_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: TASKS_TOOL_NAME,
        description: "manage your task scratchpad. use this to track work you can't finish this turn. action=add: save a task for later (requires title, optional detail). action=list: show tasks (pending by default, set include_done=true for all). action=get: show full detail for a task by id. action=done: mark a task complete by id.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "get", "done"],
                    "description": "action to perform"
                },
                "title": {
                    "type": "string",
                    "description": "short task title (for action=add)"
                },
                "detail": {
                    "type": "string",
                    "description": "longer description with context (for action=add)"
                },
                "id": {
                    "type": "integer",
                    "description": "task id (for action=get and action=done)"
                },
                "include_done": {
                    "type": "boolean",
                    "description": "include completed tasks in listing (for action=list, default false)"
                }
            },
            "required": ["action"]
        }),
    }
}

pub fn handle_tasks(db: &Database, call_id: &str, input: &serde_json::Value) -> ToolCallResult {
    let parsed: TasksInput = match serde_json::from_value(input.clone()) {
        Ok(v) => v,
        Err(err) => {
            return ToolCallResult {
                content: MessageContent::tool_result(call_id, format!("invalid input: {err}")),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            };
        }
    };

    match parsed.action.as_str() {
        "add" => match parsed.title {
            Some(ref title) if !title.trim().is_empty() => {
                match db.add_task(title.trim(), parsed.detail.as_deref()) {
                    Ok(id) => ToolCallResult {
                        content: MessageContent::tool_result(
                            call_id,
                            format!("task added (id={id})"),
                        ),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    },
                    Err(err) => ToolCallResult {
                        content: MessageContent::tool_result(
                            call_id,
                            format!("failed to add task: {err}"),
                        ),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    },
                }
            }
            _ => ToolCallResult {
                content: MessageContent::tool_result(call_id, "add requires a non-empty title"),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            },
        },
        "list" => {
            let include_done = parsed.include_done.unwrap_or(false);
            match db.list_tasks(include_done) {
                Ok(tasks) => {
                    if tasks.is_empty() {
                        return ToolCallResult {
                            content: MessageContent::tool_result(call_id, "no tasks"),
                            switch_provider: None,
                            complete: false,
                            compact: false,
                            voice: None,
                        };
                    }
                    let mut output = String::new();
                    for (i, task) in tasks.iter().enumerate() {
                        if i > 0 {
                            output.push('\n');
                        }
                        output
                            .push_str(&format!("id={}: [{}] {}", task.id, task.status, task.title));
                        if let Some(ref detail) = task.detail {
                            output.push_str(&format!(" | {detail}"));
                        }
                    }
                    ToolCallResult {
                        content: MessageContent::tool_result(call_id, output),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    }
                }
                Err(err) => ToolCallResult {
                    content: MessageContent::tool_result(
                        call_id,
                        format!("failed to list tasks: {err}"),
                    ),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                    voice: None,
                },
            }
        }
        "get" => match parsed.id {
            Some(id) => match db.get_task(id) {
                Ok(Some(task)) => {
                    let mut output = format!(
                        "id={}: [{}] {}\ncreated: {}",
                        task.id, task.status, task.title, task.created_at
                    );
                    if let Some(ref completed_at) = task.completed_at {
                        output.push_str(&format!("\ncompleted: {completed_at}"));
                    }
                    if let Some(ref detail) = task.detail {
                        output.push_str(&format!("\n\n{detail}"));
                    }
                    ToolCallResult {
                        content: MessageContent::tool_result(call_id, output),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                    }
                }
                Ok(None) => ToolCallResult {
                    content: MessageContent::tool_result(call_id, "task not found"),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                    voice: None,
                },
                Err(err) => ToolCallResult {
                    content: MessageContent::tool_result(
                        call_id,
                        format!("failed to get task: {err}"),
                    ),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                    voice: None,
                },
            },
            None => ToolCallResult {
                content: MessageContent::tool_result(call_id, "get requires id"),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            },
        },
        "done" => match parsed.id {
            Some(id) => match db.complete_task(id) {
                Ok(true) => ToolCallResult {
                    content: MessageContent::tool_result(call_id, "task completed"),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                    voice: None,
                },
                Ok(false) => ToolCallResult {
                    content: MessageContent::tool_result(call_id, "task not found or already done"),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                    voice: None,
                },
                Err(err) => ToolCallResult {
                    content: MessageContent::tool_result(
                        call_id,
                        format!("failed to complete task: {err}"),
                    ),
                    switch_provider: None,
                    complete: false,
                    compact: false,
                    voice: None,
                },
            },
            None => ToolCallResult {
                content: MessageContent::tool_result(call_id, "done requires id"),
                switch_provider: None,
                complete: false,
                compact: false,
                voice: None,
            },
        },
        other => ToolCallResult {
            content: MessageContent::tool_result(call_id, format!("invalid action: {other}")),
            switch_provider: None,
            complete: false,
            compact: false,
            voice: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn extract_text(content: &MessageContent) -> String {
        match content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_handle_tasks_add() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "add", "title": "fix CI"}));
        let text = extract_text(&result.content);
        assert!(text.starts_with("task added (id="));
    }

    #[test]
    fn test_handle_tasks_add_with_detail() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(
            &db,
            "t1",
            &json!({"action": "add", "title": "fix CI", "detail": "the build is red on main"}),
        );
        let text = extract_text(&result.content);
        assert!(text.starts_with("task added (id="));

        let task = db.get_task(1).unwrap().unwrap();
        assert_eq!(task.detail.as_deref(), Some("the build is red on main"));
    }

    #[test]
    fn test_handle_tasks_add_empty_title() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "add", "title": "  "}));
        assert_eq!(
            extract_text(&result.content),
            "add requires a non-empty title"
        );
    }

    #[test]
    fn test_handle_tasks_add_missing_title() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "add"}));
        assert_eq!(
            extract_text(&result.content),
            "add requires a non-empty title"
        );
    }

    #[test]
    fn test_handle_tasks_list_empty() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "list"}));
        assert_eq!(extract_text(&result.content), "no tasks");
    }

    #[test]
    fn test_handle_tasks_list() {
        let db = Database::open_in_memory().unwrap();
        db.add_task("task one", None).unwrap();
        db.add_task("task two", Some("details here")).unwrap();

        let result = handle_tasks(&db, "t1", &json!({"action": "list"}));
        let text = extract_text(&result.content);
        assert!(text.contains("task one"));
        assert!(text.contains("task two"));
        assert!(text.contains("details here"));
    }

    #[test]
    fn test_handle_tasks_get() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .add_task("review PR", Some("PR #42 needs review"))
            .unwrap();

        let result = handle_tasks(&db, "t1", &json!({"action": "get", "id": id}));
        let text = extract_text(&result.content);
        assert!(text.contains("review PR"));
        assert!(text.contains("PR #42 needs review"));
        assert!(text.contains("pending"));
    }

    #[test]
    fn test_handle_tasks_get_not_found() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "get", "id": 999}));
        assert_eq!(extract_text(&result.content), "task not found");
    }

    #[test]
    fn test_handle_tasks_get_missing_id() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "get"}));
        assert_eq!(extract_text(&result.content), "get requires id");
    }

    #[test]
    fn test_handle_tasks_done() {
        let db = Database::open_in_memory().unwrap();
        let id = db.add_task("deploy", None).unwrap();

        let result = handle_tasks(&db, "t1", &json!({"action": "done", "id": id}));
        assert_eq!(extract_text(&result.content), "task completed");

        // verify it's actually done
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, "done");
    }

    #[test]
    fn test_handle_tasks_done_not_found() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "done", "id": 999}));
        assert_eq!(
            extract_text(&result.content),
            "task not found or already done"
        );
    }

    #[test]
    fn test_handle_tasks_done_missing_id() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "done"}));
        assert_eq!(extract_text(&result.content), "done requires id");
    }

    #[test]
    fn test_handle_tasks_invalid_action() {
        let db = Database::open_in_memory().unwrap();
        let result = handle_tasks(&db, "t1", &json!({"action": "update"}));
        assert_eq!(extract_text(&result.content), "invalid action: update");
    }
}
