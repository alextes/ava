use serde_json::json;

use super::{ToolCallResult, ToolDefinition};
use crate::message::MessageContent;

pub const UPGRADE_TOOL_NAME: &str = "self_upgrade";

pub fn upgrade_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: UPGRADE_TOOL_NAME,
        description: "rebuild ava from source and trigger a hot-swap restart. use when the user asks you to upgrade or update yourself. only works when running from a local source checkout. if you need to verify the upgrade or continue work right after restart, call request_continuation before your final response.",
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
        voice: None,
        attachment: None,
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
            // use libc::kill directly to avoid shell "kill" printing to stderr
            // when the process doesn't exist
            let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGUSR1) };
            if ret == 0 {
                if let Err(e) = crate::db::Database::open()
                    .and_then(|db| db.record_runtime_event("self_upgrade", "self_upgrade tool"))
                {
                    tracing::warn!(%e, "failed to record self-upgrade restart event");
                }
                result.push_str(&restart_signaled_message(pid));
            } else {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ESRCH) {
                    result.push_str(&format!(" pid {pid} not running — restart ava manually."));
                } else {
                    result.push_str(&format!(" failed to signal pid {pid}: {err}."));
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

fn restart_signaled_message(pid: u32) -> String {
    format!(
        " signaled running ava (pid {pid}) to restart after this response. \
         if you need to continue or verify immediately after restart, call request_continuation \
         before your final response."
    )
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

    #[test]
    fn test_restart_signaled_message_guides_model() {
        let msg = restart_signaled_message(123);
        assert!(msg.contains("restart after this response"));
        assert!(msg.contains("request_continuation"));
        assert!(!msg.contains("tell the user"));
    }
}
