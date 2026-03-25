use serde_json::json;

use super::{ToolCallResult, ToolDefinition};
use crate::message::MessageContent;

pub const UPGRADE_TOOL_NAME: &str = "self_upgrade";

pub fn upgrade_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: UPGRADE_TOOL_NAME,
        description: "rebuild ava from source and trigger a hot-swap restart. use when the user asks you to upgrade or update yourself. only works when running from a local source checkout.",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    }
}

pub fn handle_upgrade(call_id: &str) -> ToolCallResult {
    let result = run_upgrade();
    tracing::info!(result, "upgrade complete");
    ToolCallResult {
        content: MessageContent::tool_result(call_id, result),
        switch_provider: None,
        complete: false,
        compact: false,
    }
}

fn run_upgrade() -> String {
    let source_dir = env!("CARGO_MANIFEST_DIR");
    let cargo_toml = std::path::Path::new(source_dir).join("Cargo.toml");

    if !cargo_toml.exists() {
        return "source directory not found — this binary wasn't built from a local checkout. \
                for installed binaries, re-run the install script to update."
            .to_string();
    }

    tracing::info!(source_dir, "building from source");

    let build_output = match std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(source_dir)
        .output()
    {
        Ok(output) => output,
        Err(e) => return format!("failed to run cargo build: {e}"),
    };

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        return format!("cargo build failed:\n{stderr}");
    }

    let mut result = "build succeeded.".to_string();

    #[cfg(unix)]
    {
        if let Some(pid) = crate::config::read_pid_file() {
            tracing::info!(pid, "signaling running ava to restart via SIGUSR1");
            let status = std::process::Command::new("kill")
                .args(["-USR1", &pid.to_string()])
                .status();
            match status {
                Ok(s) if s.success() => {
                    result.push_str(&format!(" signaled running ava (pid {pid}) to restart."));
                }
                Ok(s) => {
                    result.push_str(&format!(
                        " kill -USR1 {pid} exited with {s}. process may not be running — restart ava manually."
                    ));
                }
                Err(e) => {
                    result.push_str(&format!(" failed to signal pid {pid}: {e}."));
                }
            }
        } else {
            result.push_str(" no PID file found. restart ava manually.");
        }
    }

    #[cfg(not(unix))]
    {
        result.push_str(
            " signal-based restart not supported on this platform — restart ava manually.",
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upgrade_definition_name() {
        let def = upgrade_definition();
        assert_eq!(def.name(), UPGRADE_TOOL_NAME);
    }

    #[test]
    fn test_handle_upgrade_builds_from_source() {
        // this actually runs cargo build, so we just verify it doesn't panic
        // and returns a string (may succeed or fail depending on environment)
        let result = handle_upgrade("test_id");
        let text = match &result.content {
            MessageContent::ToolResult { content, .. } => content.as_display_str(),
            _ => panic!("expected ToolResult"),
        };
        // should either succeed or report a meaningful error
        assert!(
            text.contains("build succeeded")
                || text.contains("failed")
                || text.contains("not found"),
            "unexpected result: {text}"
        );
        assert!(result.switch_provider.is_none());
        assert!(!result.complete);
    }
}
